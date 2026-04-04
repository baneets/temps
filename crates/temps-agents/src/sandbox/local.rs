use async_trait::async_trait;
use std::collections::HashMap;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use super::{SandboxCreateConfig, SandboxExecResult, SandboxHandle, SandboxProvider};
use crate::ai_cli::OnEventCallback;
use crate::error::AgentError;

/// Local (no-op) sandbox provider. Runs commands directly on the host with no
/// container isolation. Used as a fallback when Docker is unavailable (development).
#[derive(Default)]
pub struct LocalSandboxProvider;

impl LocalSandboxProvider {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SandboxProvider for LocalSandboxProvider {
    async fn create(&self, config: SandboxCreateConfig) -> Result<SandboxHandle, AgentError> {
        let sandbox_name = format!("local-sandbox-{}", config.run_id);
        tracing::debug!(
            "LocalSandboxProvider: creating sandbox {} at {:?}",
            sandbox_name,
            config.host_work_dir
        );

        Ok(SandboxHandle {
            sandbox_id: sandbox_name.clone(),
            sandbox_name,
            work_dir: config.host_work_dir,
        })
    }

    async fn exec(
        &self,
        handle: &SandboxHandle,
        cmd: Vec<String>,
        env: HashMap<String, String>,
        on_output: Option<OnEventCallback>,
    ) -> Result<SandboxExecResult, AgentError> {
        if cmd.is_empty() {
            return Err(AgentError::SandboxExecFailed {
                run_id: 0,
                sandbox_id: handle.sandbox_id.clone(),
                reason: "Empty command".to_string(),
            });
        }

        let mut command = Command::new(&cmd[0]);
        command
            .args(&cmd[1..])
            .current_dir(&handle.work_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        for (key, value) in &env {
            command.env(key, value);
        }

        let mut child = command.spawn().map_err(|e| AgentError::SandboxExecFailed {
            run_id: 0,
            sandbox_id: handle.sandbox_id.clone(),
            reason: format!("Failed to spawn process: {}", e),
        })?;

        let stdout_handle = child.stdout.take().expect("stdout was piped");

        let stream_task = tokio::spawn(async move {
            let reader = BufReader::new(stdout_handle);
            let mut lines = reader.lines();
            let mut all_output = String::new();

            while let Ok(Some(line)) = lines.next_line().await {
                all_output.push_str(&line);
                all_output.push('\n');

                if let Some(ref cb) = on_output {
                    cb(line).await;
                }
            }

            all_output
        });

        let status = child
            .wait()
            .await
            .map_err(|e| AgentError::SandboxExecFailed {
                run_id: 0,
                sandbox_id: handle.sandbox_id.clone(),
                reason: format!("Process wait failed: {}", e),
            })?;

        let stdout = stream_task.await.unwrap_or_default();
        let exit_code = status.code().unwrap_or(-1);

        Ok(SandboxExecResult { exit_code, stdout })
    }

    async fn is_alive(&self, handle: &SandboxHandle) -> Result<bool, AgentError> {
        Ok(handle.work_dir.exists())
    }

    async fn destroy(&self, handle: &SandboxHandle) -> Result<(), AgentError> {
        tracing::debug!(
            "LocalSandboxProvider: destroying sandbox {} at {:?}",
            handle.sandbox_name,
            handle.work_dir
        );
        // Local provider doesn't own the work_dir lifecycle — the executor/autofixer
        // handles cleanup of the temp directory separately.
        Ok(())
    }

    async fn recover(&self, run_id: i32) -> Result<Option<SandboxHandle>, AgentError> {
        // Check if a work directory exists for common patterns
        let autopilot_dir = std::env::temp_dir().join(format!("autopilot-run-{}", run_id));
        if autopilot_dir.exists() {
            return Ok(Some(SandboxHandle {
                sandbox_id: format!("local-sandbox-{}", run_id),
                sandbox_name: format!("local-sandbox-{}", run_id),
                work_dir: autopilot_dir,
            }));
        }

        let autofixer_dir = std::env::temp_dir().join(format!("autofixer-{}", run_id));
        if autofixer_dir.exists() {
            return Ok(Some(SandboxHandle {
                sandbox_id: format!("local-sandbox-{}", run_id),
                sandbox_name: format!("local-sandbox-{}", run_id),
                work_dir: autofixer_dir,
            }));
        }

        Ok(None)
    }

    fn name(&self) -> &str {
        "local"
    }

    async fn is_available(&self) -> bool {
        true // Local provider is always available
    }

    async fn image_status(&self) -> Result<(bool, String), AgentError> {
        Ok((true, "local (no container)".to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Duration;

    fn test_config(run_id: i32, work_dir: PathBuf) -> SandboxCreateConfig {
        SandboxCreateConfig {
            run_id,
            host_work_dir: work_dir,
            image: None,
            cpu_limit: None,
            memory_limit_mb: None,
            network_mode: None,
            env_vars: HashMap::new(),
            idle_timeout: Duration::from_secs(3600),
        }
    }

    #[tokio::test]
    async fn test_local_provider_create_returns_handle() {
        let provider = LocalSandboxProvider::new();
        let work_dir = std::env::temp_dir().join("test-local-sandbox-create");
        tokio::fs::create_dir_all(&work_dir).await.unwrap();

        let handle = provider
            .create(test_config(1, work_dir.clone()))
            .await
            .unwrap();

        assert_eq!(handle.sandbox_name, "local-sandbox-1");
        assert_eq!(handle.work_dir, work_dir);

        let _ = tokio::fs::remove_dir_all(&work_dir).await;
    }

    #[tokio::test]
    async fn test_local_provider_exec_runs_command() {
        let provider = LocalSandboxProvider::new();
        let work_dir = std::env::temp_dir().join("test-local-sandbox-exec");
        tokio::fs::create_dir_all(&work_dir).await.unwrap();

        let handle = provider
            .create(test_config(2, work_dir.clone()))
            .await
            .unwrap();

        let result = provider
            .exec(
                &handle,
                vec!["echo".to_string(), "hello sandbox".to_string()],
                HashMap::new(),
                None,
            )
            .await
            .unwrap();

        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.contains("hello sandbox"));

        let _ = tokio::fs::remove_dir_all(&work_dir).await;
    }

    #[tokio::test]
    async fn test_local_provider_exec_empty_command_fails() {
        let provider = LocalSandboxProvider::new();
        let handle = SandboxHandle {
            sandbox_id: "test".to_string(),
            sandbox_name: "test".to_string(),
            work_dir: std::env::temp_dir(),
        };

        let result = provider.exec(&handle, vec![], HashMap::new(), None).await;
        assert!(matches!(result, Err(AgentError::SandboxExecFailed { .. })));
    }

    #[tokio::test]
    async fn test_local_provider_is_alive_checks_dir() {
        let provider = LocalSandboxProvider::new();
        let work_dir = std::env::temp_dir().join("test-local-sandbox-alive");
        tokio::fs::create_dir_all(&work_dir).await.unwrap();

        let handle = SandboxHandle {
            sandbox_id: "test".to_string(),
            sandbox_name: "test".to_string(),
            work_dir: work_dir.clone(),
        };

        assert!(provider.is_alive(&handle).await.unwrap());

        let _ = tokio::fs::remove_dir_all(&work_dir).await;
        assert!(!provider.is_alive(&handle).await.unwrap());
    }

    #[tokio::test]
    async fn test_local_provider_name() {
        let provider = LocalSandboxProvider::new();
        assert_eq!(provider.name(), "local");
    }

    #[tokio::test]
    async fn test_local_provider_recover_no_dir_returns_none() {
        let provider = LocalSandboxProvider::new();
        let result = provider.recover(999999).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_local_provider_exec_with_env_vars() {
        let provider = LocalSandboxProvider::new();
        let work_dir = std::env::temp_dir().join("test-local-sandbox-env");
        tokio::fs::create_dir_all(&work_dir).await.unwrap();

        let handle = provider
            .create(test_config(3, work_dir.clone()))
            .await
            .unwrap();

        let mut env = HashMap::new();
        env.insert("TEST_VAR".to_string(), "sandbox_value".to_string());

        let result = provider
            .exec(
                &handle,
                vec![
                    "sh".to_string(),
                    "-c".to_string(),
                    "echo $TEST_VAR".to_string(),
                ],
                env,
                None,
            )
            .await
            .unwrap();

        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.contains("sandbox_value"));

        let _ = tokio::fs::remove_dir_all(&work_dir).await;
    }
}
