use async_trait::async_trait;
use bollard::container::LogOutput;
use bollard::exec::StartExecResults;
use bollard::Docker;
use futures::StreamExt;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use super::{SandboxCreateConfig, SandboxExecResult, SandboxHandle, SandboxProvider};
use crate::ai_cli::OnEventCallback;
use crate::error::AgentError;

/// Container naming prefix — used for recovery after server restarts.
const SANDBOX_NAME_PREFIX: &str = "temps-sandbox-";

/// Default sandbox image name.
const DEFAULT_SANDBOX_IMAGE: &str = "temps-agent-sandbox:latest";

/// Network name for agent sandboxes (isolated from deployment network).
const SANDBOX_NETWORK: &str = "temps-agent-sandbox";

/// Path inside the container where the repository is mounted.
const CONTAINER_WORK_DIR: &str = "/workspace";

/// Dockerfile content for building the sandbox image.
const SANDBOX_DOCKERFILE: &str = r#"FROM node:20-slim
RUN apt-get update && apt-get install -y --no-install-recommends git ca-certificates && rm -rf /var/lib/apt/lists/*
RUN npm install -g @anthropic-ai/claude-code
WORKDIR /workspace
"#;

/// Host path to the Claude CLI config directory (auth tokens, session state).
/// Bind-mounted read-only into the container so Claude CLI can authenticate
/// using the host's credentials without exposing them as env vars.
fn claude_config_dir() -> Option<PathBuf> {
    std::env::var("HOME")
        .ok()
        .map(|h| PathBuf::from(h).join(".claude"))
}

/// Configuration for the Docker sandbox provider.
#[derive(Debug, Clone)]
pub struct DockerSandboxConfig {
    /// Docker image to use for sandboxes
    pub image: String,
    /// Default CPU limit in cores
    pub default_cpu_limit: f64,
    /// Default memory limit in MB
    pub default_memory_limit_mb: u64,
    /// Network mode: "none" for full isolation, or a bridge name
    pub network_mode: String,
}

impl Default for DockerSandboxConfig {
    fn default() -> Self {
        Self {
            image: std::env::var("TEMPS_AGENT_SANDBOX_IMAGE")
                .unwrap_or_else(|_| DEFAULT_SANDBOX_IMAGE.to_string()),
            default_cpu_limit: 2.0,
            default_memory_limit_mb: 2048,
            network_mode: SANDBOX_NETWORK.to_string(),
        }
    }
}

/// Docker-based sandbox provider. Each agent run gets its own container with
/// bind-mounted work directory, resource limits, and security hardening.
pub struct DockerSandboxProvider {
    docker: Arc<Docker>,
    config: DockerSandboxConfig,
}

impl DockerSandboxProvider {
    pub fn new(docker: Arc<Docker>, config: DockerSandboxConfig) -> Self {
        Self { docker, config }
    }

    /// Build the sandbox image if it doesn't exist.
    pub async fn ensure_image(&self) -> Result<(), AgentError> {
        // Check if image already exists
        if self.docker.inspect_image(&self.config.image).await.is_ok() {
            tracing::debug!("Sandbox image {} already exists", self.config.image);
            return Ok(());
        }

        tracing::info!("Building sandbox image {}...", self.config.image);

        // Create tar archive with Dockerfile
        let mut header = tar::Header::new_gnu();
        let dockerfile_bytes = SANDBOX_DOCKERFILE.as_bytes();
        header.set_size(dockerfile_bytes.len() as u64);
        header
            .set_path("Dockerfile")
            .map_err(|e| AgentError::SandboxProviderUnavailable {
                provider: "docker".to_string(),
                reason: format!("Failed to create tar header: {}", e),
            })?;
        header.set_mode(0o644);
        header.set_cksum();

        let mut tar_buf = Vec::new();
        {
            let mut tar_builder = tar::Builder::new(&mut tar_buf);
            tar_builder.append(&header, dockerfile_bytes).map_err(|e| {
                AgentError::SandboxProviderUnavailable {
                    provider: "docker".to_string(),
                    reason: format!("Failed to build tar: {}", e),
                }
            })?;
            tar_builder
                .finish()
                .map_err(|e| AgentError::SandboxProviderUnavailable {
                    provider: "docker".to_string(),
                    reason: format!("Failed to finish tar: {}", e),
                })?;
        }

        let options = bollard::query_parameters::BuildImageOptionsBuilder::new()
            .t(&self.config.image)
            .build();

        let body = http_body_util::Full::new(bytes::Bytes::from(tar_buf));
        let mut stream =
            self.docker
                .build_image(options, None, Some(http_body_util::Either::Left(body)));

        while let Some(result) = stream.next().await {
            match result {
                Ok(info) => {
                    if let Some(ref error_detail) = info.error_detail {
                        let msg = error_detail
                            .message
                            .as_deref()
                            .unwrap_or("unknown build error");
                        return Err(AgentError::SandboxProviderUnavailable {
                            provider: "docker".to_string(),
                            reason: format!("Image build error: {}", msg),
                        });
                    }
                }
                Err(e) => {
                    return Err(AgentError::SandboxProviderUnavailable {
                        provider: "docker".to_string(),
                        reason: format!("Image build failed: {}", e),
                    });
                }
            }
        }

        tracing::info!("Sandbox image {} built successfully", self.config.image);
        Ok(())
    }

    /// Ensure the sandbox network exists.
    async fn ensure_network(&self) -> Result<(), AgentError> {
        let networks = self
            .docker
            .list_networks(None::<bollard::query_parameters::ListNetworksOptions>)
            .await
            .map_err(|e| AgentError::SandboxProviderUnavailable {
                provider: "docker".to_string(),
                reason: format!("Failed to list networks: {}", e),
            })?;

        let exists = networks
            .iter()
            .any(|n| n.name.as_ref() == Some(&self.config.network_mode));

        if !exists && self.config.network_mode != "none" && self.config.network_mode != "host" {
            tracing::info!("Creating sandbox network: {}", self.config.network_mode);
            let create_opts = bollard::models::NetworkCreateRequest {
                name: self.config.network_mode.clone(),
                driver: Some("bridge".to_string()),
                internal: Some(false), // Allow outbound (Claude CLI needs API access)
                ..Default::default()
            };
            self.docker.create_network(create_opts).await.map_err(|e| {
                AgentError::SandboxProviderUnavailable {
                    provider: "docker".to_string(),
                    reason: format!("Failed to create network: {}", e),
                }
            })?;
        }

        Ok(())
    }

    fn container_name(run_id: i32) -> String {
        format!("{}{}", SANDBOX_NAME_PREFIX, run_id)
    }
}

#[async_trait]
impl SandboxProvider for DockerSandboxProvider {
    async fn create(&self, config: SandboxCreateConfig) -> Result<SandboxHandle, AgentError> {
        self.ensure_network().await?;

        let container_name = Self::container_name(config.run_id);

        // Remove existing container with the same name if any (leftover from crash)
        let _ = self
            .docker
            .remove_container(
                &container_name,
                Some(bollard::query_parameters::RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                }),
            )
            .await;

        let cpu_limit = config.cpu_limit.unwrap_or(self.config.default_cpu_limit);
        let memory_limit_mb = config
            .memory_limit_mb
            .unwrap_or(self.config.default_memory_limit_mb);

        let host_work_dir = config.host_work_dir.to_string_lossy().to_string();

        // Build environment variables
        let env_vars: Vec<String> = config
            .env_vars
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect();

        // Bind mounts: work dir + Claude config (for auth)
        let mut binds = vec![format!("{}:{}", host_work_dir, CONTAINER_WORK_DIR)];
        if let Some(claude_dir) = claude_config_dir() {
            if claude_dir.exists() {
                binds.push(format!("{}:/root/.claude:ro", claude_dir.to_string_lossy()));
            }
        }

        let host_config = bollard::models::HostConfig {
            binds: Some(binds),
            network_mode: Some(self.config.network_mode.clone()),
            // Resource limits
            nano_cpus: Some((cpu_limit * 1_000_000_000.0) as i64),
            memory: Some(memory_limit_mb as i64 * 1024 * 1024),
            // Security hardening
            cap_drop: Some(vec!["ALL".to_string()]),
            security_opt: Some(vec!["no-new-privileges:true".to_string()]),
            pids_limit: Some(512),
            init: Some(true),
            ..Default::default()
        };

        let mut labels = HashMap::new();
        labels.insert("sh.temps.sandbox".to_string(), "true".to_string());
        labels.insert(
            "sh.temps.sandbox.run_id".to_string(),
            config.run_id.to_string(),
        );

        let container_config = bollard::models::ContainerCreateBody {
            image: Some(self.config.image.clone()),
            // Keep the container alive — exec calls run commands inside it
            cmd: Some(vec!["sleep".to_string(), "infinity".to_string()]),
            env: if env_vars.is_empty() {
                None
            } else {
                Some(env_vars)
            },
            working_dir: Some(CONTAINER_WORK_DIR.to_string()),
            host_config: Some(host_config),
            labels: Some(labels),
            ..Default::default()
        };

        let container = self
            .docker
            .create_container(
                Some(
                    bollard::query_parameters::CreateContainerOptionsBuilder::new()
                        .name(&container_name)
                        .build(),
                ),
                container_config,
            )
            .await
            .map_err(|e| AgentError::SandboxCreationFailed {
                run_id: config.run_id,
                provider: "docker".to_string(),
                reason: format!("Failed to create container: {}", e),
            })?;

        self.docker
            .start_container(
                &container.id,
                None::<bollard::query_parameters::StartContainerOptions>,
            )
            .await
            .map_err(|e| AgentError::SandboxCreationFailed {
                run_id: config.run_id,
                provider: "docker".to_string(),
                reason: format!("Failed to start container: {}", e),
            })?;

        tracing::info!(
            "Sandbox container {} ({}) created for run {}",
            container_name,
            &container.id[..12],
            config.run_id
        );

        Ok(SandboxHandle {
            sandbox_id: container.id,
            sandbox_name: container_name,
            work_dir: PathBuf::from(CONTAINER_WORK_DIR),
        })
    }

    async fn exec(
        &self,
        handle: &SandboxHandle,
        cmd: Vec<String>,
        env: HashMap<String, String>,
        on_output: Option<OnEventCallback>,
    ) -> Result<SandboxExecResult, AgentError> {
        let env_vars: Vec<String> = env.iter().map(|(k, v)| format!("{}={}", k, v)).collect();

        let exec_config = bollard::models::ExecConfig {
            attach_stdout: Some(true),
            attach_stderr: Some(true),
            cmd: Some(cmd.clone()),
            working_dir: Some(handle.work_dir.to_string_lossy().to_string()),
            env: if env_vars.is_empty() {
                None
            } else {
                Some(env_vars)
            },
            ..Default::default()
        };

        let exec = self
            .docker
            .create_exec(&handle.sandbox_id, exec_config)
            .await
            .map_err(|e| AgentError::SandboxExecFailed {
                run_id: 0,
                sandbox_id: handle.sandbox_id.clone(),
                reason: format!("Failed to create exec: {}", e),
            })?;

        let start_config = bollard::exec::StartExecOptions {
            detach: false,
            ..Default::default()
        };

        let output = self
            .docker
            .start_exec(&exec.id, Some(start_config))
            .await
            .map_err(|e| AgentError::SandboxExecFailed {
                run_id: 0,
                sandbox_id: handle.sandbox_id.clone(),
                reason: format!("Failed to start exec: {}", e),
            })?;

        let mut all_output = String::new();

        match output {
            StartExecResults::Attached { mut output, .. } => {
                while let Some(chunk) = output.next().await {
                    match chunk {
                        Ok(LogOutput::StdOut { message }) => {
                            let text = String::from_utf8_lossy(&message);
                            // Stream line by line for the callback
                            for line in text.lines() {
                                all_output.push_str(line);
                                all_output.push('\n');

                                if let Some(ref cb) = on_output {
                                    cb(line.to_string()).await;
                                }
                            }
                        }
                        Ok(LogOutput::StdErr { message }) => {
                            let text = String::from_utf8_lossy(&message);
                            all_output.push_str(&text);
                        }
                        Ok(_) => {}
                        Err(e) => {
                            tracing::warn!(
                                "Sandbox {} exec stream error: {}",
                                handle.sandbox_name,
                                e
                            );
                            break;
                        }
                    }
                }
            }
            StartExecResults::Detached => {
                return Err(AgentError::SandboxExecFailed {
                    run_id: 0,
                    sandbox_id: handle.sandbox_id.clone(),
                    reason: "Exec started in detached mode unexpectedly".to_string(),
                });
            }
        }

        // Get exit code
        let exit_code = self
            .docker
            .inspect_exec(&exec.id)
            .await
            .ok()
            .and_then(|i| i.exit_code)
            .unwrap_or(-1) as i32;

        Ok(SandboxExecResult {
            exit_code,
            stdout: all_output,
        })
    }

    async fn is_alive(&self, handle: &SandboxHandle) -> Result<bool, AgentError> {
        match self
            .docker
            .inspect_container(
                &handle.sandbox_id,
                None::<bollard::query_parameters::InspectContainerOptions>,
            )
            .await
        {
            Ok(info) => {
                let running = info.state.and_then(|s| s.running).unwrap_or(false);
                Ok(running)
            }
            Err(_) => Ok(false),
        }
    }

    async fn destroy(&self, handle: &SandboxHandle) -> Result<(), AgentError> {
        tracing::info!(
            "Destroying sandbox container {} ({})",
            handle.sandbox_name,
            &handle.sandbox_id[..std::cmp::min(12, handle.sandbox_id.len())]
        );

        // Stop gracefully (5s timeout), then force remove
        let _ = self
            .docker
            .stop_container(
                &handle.sandbox_id,
                Some(bollard::query_parameters::StopContainerOptions {
                    t: Some(5),
                    signal: None,
                }),
            )
            .await;

        self.docker
            .remove_container(
                &handle.sandbox_id,
                Some(bollard::query_parameters::RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                }),
            )
            .await
            .map_err(|e| AgentError::SandboxExecFailed {
                run_id: 0,
                sandbox_id: handle.sandbox_id.clone(),
                reason: format!("Failed to remove container: {}", e),
            })?;

        Ok(())
    }

    async fn recover(&self, run_id: i32) -> Result<Option<SandboxHandle>, AgentError> {
        let container_name = Self::container_name(run_id);

        match self
            .docker
            .inspect_container(
                &container_name,
                None::<bollard::query_parameters::InspectContainerOptions>,
            )
            .await
        {
            Ok(info) => {
                let running = info.state.as_ref().and_then(|s| s.running).unwrap_or(false);

                let container_id = info.id.unwrap_or_default();

                if running {
                    tracing::info!(
                        "Recovered running sandbox {} for run {}",
                        container_name,
                        run_id
                    );
                    Ok(Some(SandboxHandle {
                        sandbox_id: container_id,
                        sandbox_name: container_name,
                        work_dir: PathBuf::from(CONTAINER_WORK_DIR),
                    }))
                } else {
                    // Container exists but is stopped — clean it up
                    tracing::info!(
                        "Found stopped sandbox {} for run {}, removing",
                        container_name,
                        run_id
                    );
                    let _ = self
                        .docker
                        .remove_container(
                            &container_name,
                            Some(bollard::query_parameters::RemoveContainerOptions {
                                force: true,
                                ..Default::default()
                            }),
                        )
                        .await;
                    Ok(None)
                }
            }
            Err(_) => Ok(None),
        }
    }

    fn name(&self) -> &str {
        "docker"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_container_name_format() {
        assert_eq!(
            DockerSandboxProvider::container_name(42),
            "temps-sandbox-42"
        );
    }

    #[test]
    fn test_default_config() {
        let config = DockerSandboxConfig::default();
        assert_eq!(config.default_cpu_limit, 2.0);
        assert_eq!(config.default_memory_limit_mb, 2048);
        assert_eq!(config.network_mode, SANDBOX_NETWORK);
    }

    #[tokio::test]
    async fn test_docker_provider_recover_no_docker() {
        // If Docker isn't available, connect will fail — we test gracefully
        let docker = match Docker::connect_with_local_defaults() {
            Ok(d) => d,
            Err(_) => {
                println!("Docker not available, skipping test");
                return;
            }
        };

        if docker.ping().await.is_err() {
            println!("Docker not responding, skipping test");
            return;
        }

        let provider = DockerSandboxProvider::new(Arc::new(docker), DockerSandboxConfig::default());

        // Recover a run that doesn't exist
        let result = provider.recover(999999).await.unwrap();
        assert!(result.is_none());
    }
}
