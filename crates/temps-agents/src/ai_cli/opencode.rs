use async_trait::async_trait;
use std::process::Stdio;
use tokio::process::Command;

use super::{AiCliProvider, AiCliStatus, AiRunConfig, AiRunResult};
use crate::error::AgentError;

pub struct OpenCodeCliProvider;

#[async_trait]
impl AiCliProvider for OpenCodeCliProvider {
    fn name(&self) -> &str {
        "opencode"
    }

    async fn check_installed(&self) -> bool {
        Command::new("opencode")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .map(|s| s.success())
            .unwrap_or(false)
    }

    async fn get_status(&self) -> AiCliStatus {
        let version_output = Command::new("opencode")
            .arg("--version")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await;

        let (installed, version) = match version_output {
            Ok(output) if output.status.success() => {
                let ver = String::from_utf8_lossy(&output.stdout).trim().to_string();
                (true, if ver.is_empty() { None } else { Some(ver) })
            }
            _ => {
                return AiCliStatus {
                    provider: "opencode".into(),
                    installed: false,
                    version: None,
                    authenticated: false,
                    auth_method: None,
                    email: None,
                    subscription_type: None,
                    setup_hint: Some(
                        "Install OpenCode: curl -fsSL https://opencode.ai/install | bash".into(),
                    ),
                };
            }
        };

        // OpenCode uses API keys configured via `opencode auth`
        // Check if any provider is configured by running `opencode models`
        let auth_output = Command::new("opencode")
            .args(["auth", "status"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await;

        let (authenticated, setup_hint) = match auth_output {
            Ok(output) if output.status.success() => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if stdout.contains("No credentials") || stdout.trim().is_empty() {
                    (
                        false,
                        Some("Run 'opencode auth add' to configure an AI provider API key.".into()),
                    )
                } else {
                    (true, None)
                }
            }
            _ => (
                false,
                Some("Run 'opencode auth add' to configure an AI provider API key.".into()),
            ),
        };

        AiCliStatus {
            provider: "opencode".into(),
            installed,
            version,
            authenticated,
            auth_method: None,
            email: None,
            subscription_type: None,
            setup_hint,
        }
    }

    async fn run(&self, config: AiRunConfig) -> Result<AiRunResult, AgentError> {
        use tokio::io::{AsyncBufReadExt, BufReader};

        let mut cmd = Command::new("opencode");
        cmd.arg("run")
            .arg(&config.prompt)
            .arg("--format")
            .arg("json")
            .current_dir(&config.work_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        if !config.api_key.is_empty() {
            cmd.env("ANTHROPIC_API_KEY", &config.api_key);
        }

        let mut child = cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                AgentError::AiCliNotInstalled {
                    provider: self.name().to_string(),
                }
            } else {
                AgentError::Io(e)
            }
        })?;

        let stdout_handle = child.stdout.take().expect("stdout was piped");
        let stderr_handle = child.stderr.take().expect("stderr was piped");
        let on_event = config.on_event.clone();

        let stream_task = tokio::spawn(async move {
            let reader = BufReader::new(stdout_handle);
            let mut lines = reader.lines();
            let mut all_output = String::new();

            while let Ok(Some(line)) = lines.next_line().await {
                all_output.push_str(&line);
                all_output.push('\n');

                if let Some(ref cb) = on_event {
                    cb(line).await;
                }
            }

            all_output
        });

        let stderr_task = tokio::spawn(async move {
            let reader = BufReader::new(stderr_handle);
            let mut lines = reader.lines();
            let mut all_stderr = String::new();

            while let Ok(Some(line)) = lines.next_line().await {
                all_stderr.push_str(&line);
                all_stderr.push('\n');
            }

            all_stderr
        });

        let wait_result = tokio::time::timeout(config.timeout, child.wait()).await;

        let status = match wait_result {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => return Err(AgentError::Io(e)),
            Err(_) => {
                let _ = child.kill().await;
                return Err(AgentError::AiCliTimeout {
                    provider: self.name().to_string(),
                    timeout_secs: config.timeout.as_secs(),
                });
            }
        };

        let stdout = stream_task.await.unwrap_or_default();
        let stderr = stderr_task.await.unwrap_or_default();
        let exit_code = status.code().unwrap_or(-1);

        if !status.success() {
            let error_output = if stderr.trim().is_empty() {
                stdout.clone()
            } else {
                stderr
            };
            return Err(AgentError::AiCliFailed {
                provider: self.name().to_string(),
                exit_code,
                stderr: error_output,
            });
        }

        // Parse OpenCode JSON output for token usage
        let (tokens_input, tokens_output, model) = parse_opencode_output(&stdout);

        Ok(AiRunResult {
            output: stdout,
            exit_code,
            tokens_input,
            tokens_output,
            model,
            changed_files: None,
        })
    }

    async fn continue_conversation(&self, config: AiRunConfig) -> Result<AiRunResult, AgentError> {
        use tokio::io::{AsyncBufReadExt, BufReader};

        let mut cmd = Command::new("opencode");
        cmd.arg("run")
            .arg("--continue")
            .arg(&config.prompt)
            .arg("--format")
            .arg("json")
            .current_dir(&config.work_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        if !config.api_key.is_empty() {
            cmd.env("ANTHROPIC_API_KEY", &config.api_key);
        }

        let mut child = cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                AgentError::AiCliNotInstalled {
                    provider: self.name().to_string(),
                }
            } else {
                AgentError::Io(e)
            }
        })?;

        let stdout_handle = child.stdout.take().expect("stdout was piped");
        let stderr_handle = child.stderr.take().expect("stderr was piped");
        let on_event = config.on_event.clone();

        let stream_task = tokio::spawn(async move {
            let reader = BufReader::new(stdout_handle);
            let mut lines = reader.lines();
            let mut all_output = String::new();

            while let Ok(Some(line)) = lines.next_line().await {
                all_output.push_str(&line);
                all_output.push('\n');

                if let Some(ref cb) = on_event {
                    cb(line).await;
                }
            }

            all_output
        });

        let stderr_task = tokio::spawn(async move {
            let reader = BufReader::new(stderr_handle);
            let mut lines = reader.lines();
            let mut all_stderr = String::new();

            while let Ok(Some(line)) = lines.next_line().await {
                all_stderr.push_str(&line);
                all_stderr.push('\n');
            }

            all_stderr
        });

        let wait_result = tokio::time::timeout(config.timeout, child.wait()).await;

        let status = match wait_result {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => return Err(AgentError::Io(e)),
            Err(_) => {
                let _ = child.kill().await;
                return Err(AgentError::AiCliTimeout {
                    provider: self.name().to_string(),
                    timeout_secs: config.timeout.as_secs(),
                });
            }
        };

        let stdout = stream_task.await.unwrap_or_default();
        let stderr = stderr_task.await.unwrap_or_default();
        let exit_code = status.code().unwrap_or(-1);

        if !status.success() {
            let error_output = if stderr.trim().is_empty() {
                stdout.clone()
            } else {
                stderr
            };
            return Err(AgentError::AiCliFailed {
                provider: self.name().to_string(),
                exit_code,
                stderr: error_output,
            });
        }

        let (tokens_input, tokens_output, model) = parse_opencode_output(&stdout);

        Ok(AiRunResult {
            output: stdout,
            exit_code,
            tokens_input,
            tokens_output,
            model,
            changed_files: None,
        })
    }
}

/// Parse OpenCode JSON output for token usage and model info.
pub fn parse_opencode_output(output: &str) -> (Option<i32>, Option<i32>, Option<String>) {
    let mut tokens_input = None;
    let mut tokens_output = None;
    let mut model = None;

    for line in output.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with('{') {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
            // Look for result/summary event with token usage
            if let Some(input) = v
                .get("tokens_input")
                .or_else(|| v.get("input_tokens"))
                .and_then(|v| v.as_i64())
            {
                tokens_input = Some(input as i32);
            }
            if let Some(output) = v
                .get("tokens_output")
                .or_else(|| v.get("output_tokens"))
                .and_then(|v| v.as_i64())
            {
                tokens_output = Some(output as i32);
            }
            if model.is_none() {
                model = v.get("model").and_then(|v| v.as_str()).map(String::from);
            }
        }
    }

    (tokens_input, tokens_output, model)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_opencode_provider_name() {
        let provider = OpenCodeCliProvider;
        assert_eq!(provider.name(), "opencode");
    }

    #[test]
    fn test_parse_opencode_output_empty() {
        let (input, output, model) = parse_opencode_output("");
        assert!(input.is_none());
        assert!(output.is_none());
        assert!(model.is_none());
    }

    #[test]
    fn test_parse_opencode_output_with_usage() {
        let output = r#"{"type":"message","content":"hello"}
{"type":"result","tokens_input":1000,"tokens_output":500,"model":"anthropic/claude-sonnet-4-20250514"}"#;
        let (input, output_tokens, model) = parse_opencode_output(output);
        assert_eq!(input, Some(1000));
        assert_eq!(output_tokens, Some(500));
        assert_eq!(model.as_deref(), Some("anthropic/claude-sonnet-4-20250514"));
    }
}
