use async_trait::async_trait;
use bollard::container::LogOutput;
use bollard::exec::StartExecResults;
use bollard::Docker;
use futures::StreamExt;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use super::{SandboxCreateConfig, SandboxExecResult, SandboxHandle, SandboxProvider};
use crate::ai_cli::OnEventCallback;
use crate::error::AgentError;

/// Container naming prefix — used for recovery after server restarts.
const SANDBOX_NAME_PREFIX: &str = "temps-sandbox-";

/// Network name for agent sandboxes (isolated from deployment network).
const SANDBOX_NETWORK: &str = "temps-agent-sandbox";

/// Path inside the container where the repository is mounted.
const CONTAINER_WORK_DIR: &str = "/workspace";

/// Generate a Dockerfile for a given runtime preset.
/// Every image gets git, curl, and Claude CLI installed on top of the base.
/// A non-root `temps` user is created so Claude CLI accepts --dangerously-skip-permissions.
fn dockerfile_for_runtime(runtime: &str) -> String {
    let (base, extra_packages, extra_run) = match runtime {
        "bun" => (
            "oven/bun:latest",
            "git ca-certificates curl sudo",
            "npm install -g @anthropic-ai/claude-code",
        ),
        "python" => (
            "python:3.12-slim",
            "git ca-certificates curl nodejs npm sudo",
            "npm install -g @anthropic-ai/claude-code && curl -LsSf https://astral.sh/uv/install.sh | sh",
        ),
        "rust" => (
            "rust:1-slim",
            "git ca-certificates curl nodejs npm sudo",
            "npm install -g @anthropic-ai/claude-code",
        ),
        "go" => (
            "golang:1.23-slim",
            "git ca-certificates curl nodejs npm sudo",
            "npm install -g @anthropic-ai/claude-code",
        ),
        "full" => (
            "ubuntu:24.04",
            "git ca-certificates curl nodejs npm python3 python3-pip golang-go sudo",
            "npm install -g @anthropic-ai/claude-code && curl -LsSf https://astral.sh/uv/install.sh | sh",
        ),
        // "node" or anything else
        _ => (
            "node:20-slim",
            "git ca-certificates curl sudo",
            "npm install -g @anthropic-ai/claude-code",
        ),
    };

    // Install tools as root, then create non-root user with sudo for package installs.
    // Claude CLI refuses --dangerously-skip-permissions when running as root.
    format!(
        r#"FROM {base}
RUN apt-get update && apt-get install -y --no-install-recommends {extra_packages} && rm -rf /var/lib/apt/lists/*
RUN {extra_run}
RUN useradd -m -s /bin/bash temps && echo "temps ALL=(ALL) NOPASSWD: ALL" >> /etc/sudoers.d/temps
RUN mkdir -p /workspace && chown temps:temps /workspace
USER temps
WORKDIR /workspace
"#
    )
}

/// Image name for a runtime preset.
fn image_name_for_runtime(runtime: &str) -> String {
    match runtime {
        "node" | "" => "temps-sandbox-node:latest".to_string(),
        other => format!("temps-sandbox-{other}:latest"),
    }
}

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
    /// Runtime preset: "node", "bun", "python", "rust", "go", "full", or "custom"
    pub runtime: String,
    /// Custom Docker image (only used when runtime is "custom")
    pub custom_image: String,
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
            runtime: "node".to_string(),
            custom_image: String::new(),
            default_cpu_limit: 2.0,
            default_memory_limit_mb: 2048,
            network_mode: SANDBOX_NETWORK.to_string(),
        }
    }
}

impl DockerSandboxConfig {
    /// Resolve the image name for the current configuration.
    /// For presets, returns `temps-sandbox-{runtime}:latest`.
    /// For custom, returns the user-provided image.
    pub fn resolved_image(&self) -> String {
        if self.runtime == "custom" && !self.custom_image.is_empty() {
            self.custom_image.clone()
        } else {
            image_name_for_runtime(&self.runtime)
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
    /// For preset runtimes, generates a Dockerfile dynamically.
    /// For custom images, assumes the image is already available (pull or pre-built).
    pub async fn ensure_image(&self) -> Result<(), AgentError> {
        self.ensure_image_for_runtime(&self.config.runtime).await
    }

    /// Build a sandbox image for a specific runtime preset.
    async fn ensure_image_for_runtime(&self, runtime: &str) -> Result<(), AgentError> {
        // Custom images: just check if they exist (user must pull/build them)
        if runtime == "custom" {
            let img = &self.config.custom_image;
            if img.is_empty() {
                return Err(AgentError::SandboxProviderUnavailable {
                    provider: "docker".to_string(),
                    reason: "Custom runtime selected but no image specified".to_string(),
                });
            }
            // Try to pull if not present locally
            if self.docker.inspect_image(img).await.is_err() {
                tracing::info!("Pulling custom sandbox image {}...", img);
                let options = bollard::query_parameters::CreateImageOptionsBuilder::new()
                    .from_image(img.as_str())
                    .build();
                let mut stream = self.docker.create_image(Some(options), None, None);
                while let Some(result) = stream.next().await {
                    if let Err(e) = result {
                        return Err(AgentError::SandboxProviderUnavailable {
                            provider: "docker".to_string(),
                            reason: format!("Failed to pull custom image {}: {}", img, e),
                        });
                    }
                }
            }
            return Ok(());
        }

        let image_name = image_name_for_runtime(runtime);

        // Check if image already exists
        if self.docker.inspect_image(&image_name).await.is_ok() {
            tracing::debug!("Sandbox image {} already exists", image_name);
            return Ok(());
        }

        tracing::info!(
            "Building sandbox image {} (runtime: {})...",
            image_name,
            runtime
        );

        let dockerfile_content = dockerfile_for_runtime(runtime);

        // Create tar archive with Dockerfile
        let mut header = tar::Header::new_gnu();
        let dockerfile_bytes = dockerfile_content.as_bytes();
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
            .t(&image_name)
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

        tracing::info!("Sandbox image {} built successfully", image_name);
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

        // Resolve image: per-run override > provider config
        let default_image = self.config.resolved_image();
        let image = config
            .image
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or(&default_image);

        // Ensure the image exists (build for presets, pull for custom)
        if self.docker.inspect_image(image).await.is_err() {
            // If this is a preset image, build it
            if image.starts_with("temps-sandbox-") {
                let runtime = image
                    .strip_prefix("temps-sandbox-")
                    .and_then(|s| s.strip_suffix(":latest"))
                    .unwrap_or("node");
                self.ensure_image_for_runtime(runtime).await?;
            }
            // Otherwise it's a custom image — try to pull
            else {
                tracing::info!("Pulling sandbox image {}...", image);
                let options = bollard::query_parameters::CreateImageOptionsBuilder::new()
                    .from_image(image)
                    .build();
                let mut stream = self.docker.create_image(Some(options), None, None);
                while let Some(result) = stream.next().await {
                    if let Err(e) = result {
                        return Err(AgentError::SandboxCreationFailed {
                            run_id: config.run_id,
                            provider: "docker".to_string(),
                            reason: format!("Failed to pull image {}: {}", image, e),
                        });
                    }
                }
            }
        }
        let cpu_limit = config.cpu_limit.unwrap_or(self.config.default_cpu_limit);
        let memory_limit_mb = config
            .memory_limit_mb
            .unwrap_or(self.config.default_memory_limit_mb);
        let network = config
            .network_mode
            .as_deref()
            .unwrap_or(&self.config.network_mode);
        // Map user-friendly names to Docker network modes
        let docker_network = match network {
            "none" => "none".to_string(),
            "full" | "host" => "host".to_string(),
            other => other.to_string(), // "restricted" uses the default bridge network
        };

        let host_work_dir = config.host_work_dir.to_string_lossy().to_string();

        // Build environment variables
        let env_vars: Vec<String> = config
            .env_vars
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect();

        // Bind mounts: work dir + Claude config (read-only at staging path)
        // The host's ~/.claude is mounted read-only at /home/temps/.claude-host.
        // On first exec, we copy it to /home/temps/.claude so Claude CLI can write
        // session state, MCP server cache, etc. while preserving auth tokens.
        let mut binds = vec![format!("{}:{}", host_work_dir, CONTAINER_WORK_DIR)];
        let has_claude_config = if let Some(claude_dir) = claude_config_dir() {
            if claude_dir.exists() {
                binds.push(format!(
                    "{}:/home/temps/.claude-host:ro",
                    claude_dir.to_string_lossy()
                ));
                true
            } else {
                false
            }
        } else {
            false
        };

        let host_config = bollard::models::HostConfig {
            binds: Some(binds),
            network_mode: Some(docker_network),
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
            image: Some(image.to_string()),
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

        // Copy Claude config from read-only mount to writable location.
        // This allows Claude CLI to write session state and MCP server cache
        // while preserving the host's auth tokens and settings.json (MCP config).
        if has_claude_config {
            let init_cmd = bollard::models::ExecConfig {
                attach_stdout: Some(true),
                attach_stderr: Some(true),
                cmd: Some(vec![
                    "sh".to_string(),
                    "-c".to_string(),
                    "cp -a /home/temps/.claude-host/. /home/temps/.claude/ 2>/dev/null; true"
                        .to_string(),
                ]),
                ..Default::default()
            };
            if let Ok(exec) = self.docker.create_exec(&container.id, init_cmd).await {
                let _ = self
                    .docker
                    .start_exec(&exec.id, None::<bollard::exec::StartExecOptions>)
                    .await;
                // Brief wait for copy to complete
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            tracing::debug!(
                "Copied Claude config to writable /home/temps/.claude in container {}",
                &container.id[..12]
            );
        }

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

    async fn is_available(&self) -> bool {
        self.docker.ping().await.is_ok()
    }

    async fn image_status(&self) -> Result<(bool, String), AgentError> {
        let image_name = self.config.resolved_image();
        let ready = self.docker.inspect_image(&image_name).await.is_ok();
        Ok((ready, image_name))
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
        assert_eq!(config.runtime, "node");
        assert_eq!(config.custom_image, "");
        assert_eq!(config.default_cpu_limit, 2.0);
        assert_eq!(config.default_memory_limit_mb, 2048);
        assert_eq!(config.network_mode, SANDBOX_NETWORK);
    }

    #[test]
    fn test_resolved_image_for_presets() {
        for (runtime, expected) in [
            ("node", "temps-sandbox-node:latest"),
            ("python", "temps-sandbox-python:latest"),
            ("rust", "temps-sandbox-rust:latest"),
            ("bun", "temps-sandbox-bun:latest"),
            ("go", "temps-sandbox-go:latest"),
            ("full", "temps-sandbox-full:latest"),
        ] {
            let config = DockerSandboxConfig {
                runtime: runtime.to_string(),
                ..Default::default()
            };
            assert_eq!(config.resolved_image(), expected, "runtime={}", runtime);
        }
    }

    #[test]
    fn test_resolved_image_custom() {
        let config = DockerSandboxConfig {
            runtime: "custom".to_string(),
            custom_image: "my-registry/my-agent:v2".to_string(),
            ..Default::default()
        };
        assert_eq!(config.resolved_image(), "my-registry/my-agent:v2");
    }

    #[test]
    fn test_resolved_image_custom_empty_falls_back() {
        let config = DockerSandboxConfig {
            runtime: "custom".to_string(),
            custom_image: String::new(),
            ..Default::default()
        };
        // Falls back to node since custom_image is empty
        assert_eq!(config.resolved_image(), "temps-sandbox-custom:latest");
    }

    #[test]
    fn test_dockerfile_for_runtime_node() {
        let df = dockerfile_for_runtime("node");
        assert!(df.contains("FROM node:20-slim"));
        assert!(df.contains("claude-code"));
        assert!(df.contains("git"));
    }

    #[test]
    fn test_dockerfile_for_runtime_python() {
        let df = dockerfile_for_runtime("python");
        assert!(df.contains("FROM python:3.12-slim"));
        assert!(df.contains("claude-code"));
        assert!(df.contains("uv"));
    }

    #[test]
    fn test_dockerfile_for_runtime_rust() {
        let df = dockerfile_for_runtime("rust");
        assert!(df.contains("FROM rust:1-slim"));
        assert!(df.contains("claude-code"));
    }

    #[test]
    fn test_dockerfile_for_runtime_bun() {
        let df = dockerfile_for_runtime("bun");
        assert!(df.contains("FROM oven/bun:latest"));
        assert!(df.contains("claude-code"));
    }

    #[test]
    fn test_dockerfile_for_runtime_go() {
        let df = dockerfile_for_runtime("go");
        assert!(df.contains("FROM golang:1.23-slim"));
        assert!(df.contains("claude-code"));
    }

    #[test]
    fn test_dockerfile_for_runtime_full() {
        let df = dockerfile_for_runtime("full");
        assert!(df.contains("FROM ubuntu:24.04"));
        assert!(df.contains("claude-code"));
        assert!(df.contains("python3"));
        assert!(df.contains("golang-go"));
        assert!(df.contains("nodejs"));
        assert!(df.contains("uv"));
    }

    #[test]
    fn test_dockerfile_for_unknown_runtime_defaults_to_node() {
        let df = dockerfile_for_runtime("unknown");
        assert!(df.contains("FROM node:20-slim"));
    }

    #[test]
    fn test_image_name_for_runtime() {
        assert_eq!(image_name_for_runtime("node"), "temps-sandbox-node:latest");
        assert_eq!(image_name_for_runtime(""), "temps-sandbox-node:latest");
        assert_eq!(
            image_name_for_runtime("python"),
            "temps-sandbox-python:latest"
        );
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

    #[tokio::test]
    async fn test_docker_sandbox_e2e_lifecycle() {
        // Full lifecycle: create → exec → is_alive → recover → destroy
        let docker = match Docker::connect_with_local_defaults() {
            Ok(d) => d,
            Err(_) => {
                println!("Docker not available, skipping e2e test");
                return;
            }
        };
        let docker = Arc::new(docker);

        if docker.ping().await.is_err() {
            println!("Docker not responding, skipping e2e test");
            return;
        }

        let config = DockerSandboxConfig::default();
        let provider = DockerSandboxProvider::new(docker.clone(), config);

        // Ensure the default image is built
        if let Err(e) = provider.ensure_image().await {
            println!("Cannot build sandbox image, skipping e2e test: {}", e);
            return;
        }

        let run_id = 99990; // Unlikely to conflict
        let work_dir = std::env::temp_dir().join(format!("sandbox-e2e-test-{}", run_id));
        let _ = std::fs::create_dir_all(&work_dir);
        std::fs::write(work_dir.join("test.txt"), "hello from test").unwrap();

        // 1. Create sandbox
        let create_config = SandboxCreateConfig {
            run_id,
            host_work_dir: work_dir.clone(),
            image: None,
            cpu_limit: Some(1.0),
            memory_limit_mb: Some(512),
            network_mode: Some("none".to_string()),
            env_vars: HashMap::from([("TEST_VAR".to_string(), "test_value".to_string())]),
            idle_timeout: Duration::from_secs(120),
        };

        let handle = provider.create(create_config).await.unwrap();
        assert!(handle.sandbox_name.contains("temps-sandbox-"));
        assert!(!handle.sandbox_id.is_empty());

        // 2. Verify it's alive
        assert!(provider.is_alive(&handle).await.unwrap());

        // 3. Execute a command — check the work dir is mounted
        let result = provider
            .exec(
                &handle,
                vec!["cat".to_string(), "/workspace/test.txt".to_string()],
                HashMap::new(),
                None,
            )
            .await
            .unwrap();
        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.contains("hello from test"));

        // 4. Execute with env vars
        let result = provider
            .exec(
                &handle,
                vec![
                    "sh".to_string(),
                    "-c".to_string(),
                    "echo $MY_VAR".to_string(),
                ],
                HashMap::from([("MY_VAR".to_string(), "injected".to_string())]),
                None,
            )
            .await
            .unwrap();
        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.contains("injected"));

        // 5. Verify recovery — simulate finding existing container
        let recovered = provider.recover(run_id).await.unwrap();
        assert!(recovered.is_some());
        let recovered_handle = recovered.unwrap();
        assert_eq!(recovered_handle.sandbox_name, handle.sandbox_name);

        // 6. Destroy
        provider.destroy(&handle).await.unwrap();

        // 7. Verify it's gone
        assert!(!provider.is_alive(&handle).await.unwrap_or(false));
        let after_destroy = provider.recover(run_id).await.unwrap();
        assert!(after_destroy.is_none());

        // Cleanup
        let _ = std::fs::remove_dir_all(&work_dir);
    }

    #[tokio::test]
    async fn test_docker_sandbox_image_status() {
        let docker = match Docker::connect_with_local_defaults() {
            Ok(d) => d,
            Err(_) => {
                println!("Docker not available, skipping test");
                return;
            }
        };
        let docker = Arc::new(docker);

        if docker.ping().await.is_err() {
            println!("Docker not responding, skipping test");
            return;
        }

        let provider = DockerSandboxProvider::new(docker, DockerSandboxConfig::default());
        assert!(provider.is_available().await);

        let (_, image_name) = provider.image_status().await.unwrap();
        assert!(image_name.starts_with("temps-sandbox-"));
    }

    #[tokio::test]
    async fn test_docker_sandbox_custom_runtime() {
        let docker = match Docker::connect_with_local_defaults() {
            Ok(d) => d,
            Err(_) => {
                println!("Docker not available, skipping test");
                return;
            }
        };
        let docker = Arc::new(docker);

        if docker.ping().await.is_err() {
            println!("Docker not responding, skipping test");
            return;
        }

        // Test that different runtimes produce different images
        let config = DockerSandboxConfig {
            runtime: "python".to_string(),
            ..Default::default()
        };
        let provider = DockerSandboxProvider::new(docker, config);

        let (_, image_name) = provider.image_status().await.unwrap();
        assert_eq!(image_name, "temps-sandbox-python:latest");
    }
}
