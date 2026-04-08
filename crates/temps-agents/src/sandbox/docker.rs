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

/// Single-quote a string for safe embedding in a `sh -c` command line.
/// Handles embedded single quotes via the `'\''` idiom.
fn shell_quote(s: &str) -> String {
    let escaped = s.replace('\'', "'\\''");
    format!("'{}'", escaped)
}

/// Shared Docker network for all workspace/agent sandboxes AND the preview
/// gateway. The gateway resolves `temps-sandbox-<sid>:<port>` via Docker's
/// embedded DNS — both sides must share this user-defined network for that
/// to work. Keep in sync with
/// `temps-cli/src/commands/serve/preview_gateway.rs::PREVIEW_GATEWAY_NETWORK`.
const SANDBOX_NETWORK: &str = "temps-sandbox-net";

/// Path inside the container where the repository is mounted.
const CONTAINER_WORK_DIR: &str = "/workspace";

/// Generate a Dockerfile for a given runtime preset.
///
/// Every image gets git, curl, jq, sudo, tmux, and the Claude CLI installed
/// on top of the base. A non-root `temps` user is created (Claude CLI refuses
/// `--dangerously-skip-permissions` as root).
///
/// Claude CLI is installed via the **native installer** (`claude.ai/install.sh`),
/// not npm. The npm package `@anthropic-ai/claude-code` is deprecated; the
/// native installer drops a prebuilt binary into `~/.local/bin/claude` and
/// removes the Node.js runtime requirement, which means runtimes like
/// python/rust/go/full no longer need `nodejs npm` purely to host Claude.
///
/// Important: the native installer must run as the **target user**, not root,
/// because it installs to `$HOME/.local/bin`. We `su - temps` after creating
/// the user (see the trailing block) instead of running as root.
pub fn dockerfile_for_runtime(runtime: &str) -> String {
    // `jq` is required by the workspace memory script (/workspace/.temps/bin/memory)
    // — it's used to build/parse JSON for the API calls. Always installed.
    //
    // `extra_run` is reserved for runtime-specific extras the base image
    // doesn't provide (e.g. `uv` for python). Claude itself is installed in
    // the unified per-user install step at the bottom, not here.
    // `dtach` is the per-tab PTY supervisor: each workspace terminal tab runs
    // its CLI (claude/codex/opencode/bash) under `dtach -A /run/temps-pty/{tab}.sock`
    // so the PTY owner is decoupled from the `docker exec` lifecycle. When the
    // websocket drops, the dtach client exits but the dtach master keeps the
    // child alive — reconnects just re-attach. This is how we guarantee
    // "claude is launched exactly once per sandbox lifetime" across arbitrary
    // browser refreshes, without losing background-shell state the CLI is
    // tracking internally. See handlers/sessions.rs::handle_session_terminal.
    let (base, extra_packages, extra_run) = match runtime {
        "bun" => (
            "oven/bun:latest",
            "git ca-certificates curl jq sudo unzip dtach",
            "true",
        ),
        "python" => (
            "python:3.12-slim",
            "git ca-certificates curl jq sudo unzip dtach",
            "curl -LsSf https://astral.sh/uv/install.sh | sh",
        ),
        "rust" => (
            "rust:1-slim",
            "git ca-certificates curl jq sudo unzip dtach",
            "true",
        ),
        "go" => (
            // `golang:1.23-slim` was pruned from Docker Hub — use the
            // debian-based tag which is still published. (Slim variants
            // for golang don't exist for 1.23+.)
            "golang:1.23-bookworm",
            "git ca-certificates curl jq sudo unzip dtach",
            "true",
        ),
        "full" => (
            "ubuntu:24.04",
            "git ca-certificates curl jq nodejs npm python3 python3-pip golang-go sudo unzip dtach",
            "curl -LsSf https://astral.sh/uv/install.sh | sh",
        ),
        // "node" or anything else — Ubuntu-based with Node 20 from NodeSource
        // so users still have npm/npx for their own work. Claude itself no
        // longer rides on top of npm.
        _ => (
            "ubuntu:24.04",
            "git ca-certificates curl jq sudo unzip gnupg dtach",
            "curl -fsSL https://deb.nodesource.com/setup_20.x | bash - \
                && apt-get install -y --no-install-recommends nodejs",
        ),
    };

    // Install Bun on every non-bun runtime so `bunx @temps-sdk/cli` Just Works
    // regardless of which base image the user picked.
    let bun_install = if runtime == "bun" {
        ""
    } else {
        // Install to /usr/local/bun so it's on PATH for all users (including the
        // non-root `temps` user we create later).
        r#"RUN curl -fsSL https://bun.sh/install | BUN_INSTALL=/usr/local/bun bash \
    && ln -s /usr/local/bun/bin/bun /usr/local/bin/bun \
    && ln -s /usr/local/bun/bin/bunx /usr/local/bin/bunx
"#
    };

    // Install tools as root, then create non-root user with sudo for package installs.
    // Claude CLI refuses --dangerously-skip-permissions when running as root,
    // and the native installer drops the binary in $HOME/.local/bin — both
    // reasons we install Claude as the `temps` user, not as root.
    //
    // GitHub CLI (gh) and GitLab CLI (glab) are installed from their official
    // releases so the workspace AI can interact with PRs/MRs, issues, and CI.
    //
    // PATH includes /home/temps/.local/bin globally so the binary is visible
    // from `docker exec`, tmux panes, and login shells alike — without this,
    // the bare-bash and tmux-wrapped paths would silently miss the installer
    // location and fall back to "claude not found".
    format!(
        r#"FROM {base}
ENV DEBIAN_FRONTEND=noninteractive
ENV PATH=/home/temps/.local/bin:/usr/local/bun/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
RUN apt-get update && apt-get install -y --no-install-recommends {extra_packages} wget tmux && rm -rf /var/lib/apt/lists/*
RUN {extra_run}
{bun_install}# Install GitHub CLI from official apt repo
RUN curl -fsSL https://cli.github.com/packages/githubcli-archive-keyring.gpg | tee /usr/share/keyrings/githubcli-archive-keyring.gpg > /dev/null \
    && chmod go+r /usr/share/keyrings/githubcli-archive-keyring.gpg \
    && echo "deb [arch=$(dpkg --print-architecture) signed-by=/usr/share/keyrings/githubcli-archive-keyring.gpg] https://cli.github.com/packages stable main" > /etc/apt/sources.list.d/github-cli.list \
    && apt-get update && apt-get install -y --no-install-recommends gh && rm -rf /var/lib/apt/lists/*
# Install GitLab CLI (glab) from official release tarball
RUN ARCH=$(dpkg --print-architecture) \
    && case "$ARCH" in amd64) GLAB_ARCH=x86_64 ;; arm64) GLAB_ARCH=arm64 ;; *) GLAB_ARCH=$ARCH ;; esac \
    && GLAB_VERSION=1.51.0 \
    && curl -fsSL "https://gitlab.com/gitlab-org/cli/-/releases/v${{GLAB_VERSION}}/downloads/glab_${{GLAB_VERSION}}_linux_${{GLAB_ARCH}}.tar.gz" -o /tmp/glab.tar.gz \
    && tar -xzf /tmp/glab.tar.gz -C /tmp \
    && mv /tmp/bin/glab /usr/local/bin/glab \
    && chmod +x /usr/local/bin/glab \
    && rm -rf /tmp/glab.tar.gz /tmp/bin
RUN EXISTING_USER=$(getent passwd 1000 | cut -d: -f1) \
    && if [ -n "$EXISTING_USER" ] && [ "$EXISTING_USER" != "temps" ]; then \
         (userdel -r "$EXISTING_USER" 2>/dev/null || userdel "$EXISTING_USER" 2>/dev/null || true); \
         (groupdel "$EXISTING_USER" 2>/dev/null || true); \
       fi \
    && useradd -m -s /bin/bash -u 1000 temps \
    && echo '# temps sandbox: scoped sudo for package install only.' > /etc/sudoers.d/temps \
    && echo 'Cmnd_Alias TEMPS_PKG = /usr/bin/apt, /usr/bin/apt-get, /usr/bin/dpkg, /usr/bin/pip, /usr/bin/pip3, /usr/local/bin/uv, /usr/bin/npm, /usr/local/bin/bun' >> /etc/sudoers.d/temps \
    && echo 'temps ALL=(ALL) NOPASSWD: TEMPS_PKG' >> /etc/sudoers.d/temps \
    && echo 'Defaults:temps !requiretty, !log_input, !log_output' >> /etc/sudoers.d/temps \
    && chmod 0440 /etc/sudoers.d/temps \
    && visudo -c -f /etc/sudoers.d/temps
RUN mkdir -p /workspace && chown temps:temps /workspace
# /run/temps-pty holds one Unix socket per terminal tab (one per {{kind,tab}}
# pair). dtach creates these sockets on first attach; subsequent reconnects
# find the existing socket and re-attach instead of respawning the CLI. The
# directory lives in the container's tmpfs, so it's wiped on container
# restart — which is exactly the "launch once per sandbox lifetime" boundary
# we want.
RUN mkdir -p /run/temps-pty && chown temps:temps /run/temps-pty && chmod 0700 /run/temps-pty
# Install Claude Code via the official native installer, as the temps user.
# Must run as the target user — the installer drops files in $HOME/.local/bin
# and refuses to install system-wide. We also seed PATH in ~/.bashrc so
# interactive shells (e.g. the tmux-wrapped terminal) find the binary even
# if the parent env wasn't propagated.
USER temps
ENV HOME=/home/temps
RUN curl -fsSL https://claude.ai/install.sh | bash \
    && /home/temps/.local/bin/claude --version \
    && echo 'export PATH=/home/temps/.local/bin:$PATH' >> /home/temps/.bashrc
WORKDIR /workspace
"#
    )
}

/// Image name for a runtime preset.
pub fn image_name_for_runtime(runtime: &str) -> String {
    match runtime {
        "node" | "" => "temps-sandbox-node:latest".to_string(),
        other => format!("temps-sandbox-{other}:latest"),
    }
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
            default_cpu_limit: 4.0,
            default_memory_limit_mb: 8192,
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
        // Map user-friendly names to Docker network modes.
        //
        // IMPORTANT: "full" used to map to docker `host` mode, which bypassed
        // container network isolation entirely and prevented sandboxes from
        // joining the shared `temps-sandbox-net` user-defined network. That
        // broke workspace preview routing because the preview gateway resolves
        // sandbox containers via Docker's embedded DNS, which only works on
        // user-defined networks. We now route "full" through the shared bridge
        // network (`SANDBOX_NETWORK`) — sandboxes still get full outbound
        // internet (the network is created with `internal: false`) but they
        // also get a real container IP and DNS name that the gateway can hit.
        //
        // `host` is still accepted as an explicit opt-out for callers that
        // really need the host stack (e.g. legacy autofixer flows).
        let docker_network = match network {
            "none" => "none".to_string(),
            "host" => "host".to_string(),
            "full" | "restricted" => SANDBOX_NETWORK.to_string(),
            other => other.to_string(),
        };

        let host_work_dir = config.host_work_dir.to_string_lossy().to_string();

        // Build environment variables
        let env_vars: Vec<String> = config
            .env_vars
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect();

        // Bind mount: only the work directory. Auth is handled via env vars
        // (CLAUDE_CODE_OAUTH_TOKEN, ANTHROPIC_API_KEY) — no host config mounting.
        //
        // Named volume for `/home/temps`: persists the sandbox user's home
        // directory (claude session jsonl, shell history, ~/.claude/projects,
        // ~/.config/...) across container recreation. Without this, killing
        // and recreating the sandbox would lose all conversation continuity
        // even though the work_dir survives via the bind mount above.
        //
        // The volume name is keyed on run_id so each session keeps its own
        // home isolated, and the volume is auto-created on first mount.
        let home_volume_name = format!("temps-sandbox-home-{}", config.run_id);
        let binds = vec![
            format!("{}:{}", host_work_dir, CONTAINER_WORK_DIR),
            format!("{}:/home/temps", home_volume_name),
        ];

        // tmpfs mount for secrets — in-memory only, never written to disk
        let mut tmpfs = HashMap::new();
        tmpfs.insert("/run/secrets".to_string(), "size=1m,mode=0700".to_string());

        let host_config = bollard::models::HostConfig {
            binds: Some(binds),
            network_mode: Some(docker_network),
            tmpfs: Some(tmpfs),
            // Resource limits.
            //
            // `memory_swap == memory` disables swap usage for the container.
            // Without this, Docker's default is `memory_swap = 2 * memory`,
            // meaning each sandbox can silently page an *additional* full
            // memory-limit's worth to host swap. With N sandboxes running
            // dev servers + claude that's a fast path to host swap
            // exhaustion, terrible latency, and no OOM feedback to the
            // sandbox itself. Disabling swap turns over-limit into a clean
            // OOM-kill of the offending process (which is the correct
            // signal — the user sees `next dev` die instead of the whole
            // host dragging).
            nano_cpus: Some((cpu_limit * 1_000_000_000.0) as i64),
            memory: Some(memory_limit_mb as i64 * 1024 * 1024),
            memory_swap: Some(memory_limit_mb as i64 * 1024 * 1024),
            // Security hardening
            cap_drop: Some(vec!["ALL".to_string()]),
            // CHOWN/FOWNER are needed so the post-start chown of /home/temps
            // (fixing stale named-volume ownership) can run as root.
            cap_add: Some(vec!["CHOWN".to_string(), "FOWNER".to_string()]),
            security_opt: Some(vec!["no-new-privileges:true".to_string()]),
            pids_limit: Some(config.pids_limit.unwrap_or(512)),
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

        // Fix /home/temps ownership: the named volume inherits the host's
        // anonymous-volume root uid on first mount, and stale volumes from
        // earlier image builds may be owned by a different uid entirely.
        // Running chown as root (not USER temps) normalizes it every start.
        {
            let exec = self
                .docker
                .create_exec(
                    &container.id,
                    bollard::models::ExecConfig {
                        user: Some("0:0".to_string()),
                        cmd: Some(vec![
                            "chown".to_string(),
                            "-R".to_string(),
                            "temps:temps".to_string(),
                            "/home/temps".to_string(),
                        ]),
                        attach_stdout: Some(true),
                        attach_stderr: Some(true),
                        ..Default::default()
                    },
                )
                .await
                .map_err(|e| AgentError::SandboxCreationFailed {
                    run_id: config.run_id,
                    provider: "docker".to_string(),
                    reason: format!("Failed to create chown exec: {}", e),
                })?;
            let _ = self
                .docker
                .start_exec(
                    &exec.id,
                    Some(bollard::exec::StartExecOptions {
                        detach: false,
                        ..Default::default()
                    }),
                )
                .await;
        }

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
                // Bollard's exec stream has a known failure mode: if the
                // underlying process exits without producing output (or the
                // docker daemon drops the trailing close frame), `output.next()`
                // parks forever instead of returning `None`. To avoid hanging
                // the entire workspace executor on a phantom stream, we poll
                // with an idle timeout: if no chunk arrives within the window
                // AND the exec is no longer running per `inspect_exec`, we
                // break out and return whatever we collected.
                const IDLE_POLL: std::time::Duration = std::time::Duration::from_secs(15);
                loop {
                    match tokio::time::timeout(IDLE_POLL, output.next()).await {
                        Ok(Some(Ok(LogOutput::StdOut { message }))) => {
                            let text = String::from_utf8_lossy(&message);
                            for line in text.lines() {
                                all_output.push_str(line);
                                all_output.push('\n');
                                if let Some(ref cb) = on_output {
                                    cb(line.to_string()).await;
                                }
                            }
                        }
                        Ok(Some(Ok(LogOutput::StdErr { message }))) => {
                            let text = String::from_utf8_lossy(&message);
                            all_output.push_str(&text);
                        }
                        Ok(Some(Ok(_))) => {}
                        Ok(Some(Err(e))) => {
                            tracing::warn!(
                                "Sandbox {} exec stream error: {}",
                                handle.sandbox_name,
                                e
                            );
                            break;
                        }
                        Ok(None) => {
                            // Stream closed cleanly — exec is done.
                            break;
                        }
                        Err(_) => {
                            // Idle timeout: check whether the exec is still
                            // running. If it is, keep waiting (long-running
                            // claude calls are normal). If it isn't, the
                            // stream is phantom — bail out.
                            match self.docker.inspect_exec(&exec.id).await {
                                Ok(info) if info.running == Some(true) => {
                                    tracing::debug!(
                                        "Sandbox {} exec idle (still running), continuing to wait",
                                        handle.sandbox_name
                                    );
                                    continue;
                                }
                                Ok(_) => {
                                    tracing::warn!(
                                        "Sandbox {} exec stream went idle and the exec is no longer running — bailing out to avoid permanent hang",
                                        handle.sandbox_name
                                    );
                                    break;
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        "Sandbox {} inspect_exec failed during idle check: {} — bailing out",
                                        handle.sandbox_name,
                                        e
                                    );
                                    break;
                                }
                            }
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

    async fn destroy(&self, handle: &SandboxHandle, purge_volumes: bool) -> Result<(), AgentError> {
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

        // Only remove the named home volume when the caller asks for a
        // full purge (session *delete*, or ephemeral agent runs). On a
        // plain session *close* the volume must survive so claude auth,
        // shell history, and ~/.claude/projects are preserved when the
        // session is reopened.
        if purge_volumes {
            if let Some(run_id_str) = handle.sandbox_name.strip_prefix(SANDBOX_NAME_PREFIX) {
                let home_volume_name = format!("temps-sandbox-home-{}", run_id_str);
                if let Err(e) = self
                    .docker
                    .remove_volume(
                        &home_volume_name,
                        None::<bollard::query_parameters::RemoveVolumeOptions>,
                    )
                    .await
                {
                    tracing::warn!(
                        "Failed to remove sandbox home volume {} (may not exist): {}",
                        home_volume_name,
                        e
                    );
                }
            }
        }

        Ok(())
    }

    async fn stop(&self, handle: &SandboxHandle) -> Result<(), AgentError> {
        tracing::info!("Stopping sandbox container {}", handle.sandbox_name);
        self.docker
            .stop_container(
                &handle.sandbox_id,
                Some(bollard::query_parameters::StopContainerOptions {
                    t: Some(10),
                    signal: None,
                }),
            )
            .await
            .map_err(|e| AgentError::SandboxExecFailed {
                run_id: 0,
                sandbox_id: handle.sandbox_id.clone(),
                reason: format!("Failed to stop container: {}", e),
            })?;
        Ok(())
    }

    async fn start(&self, handle: &SandboxHandle) -> Result<(), AgentError> {
        tracing::info!("Starting sandbox container {}", handle.sandbox_name);
        self.docker
            .start_container(
                &handle.sandbox_id,
                None::<bollard::query_parameters::StartContainerOptions>,
            )
            .await
            .map_err(|e| AgentError::SandboxExecFailed {
                run_id: 0,
                sandbox_id: handle.sandbox_id.clone(),
                reason: format!("Failed to start container: {}", e),
            })?;
        Ok(())
    }

    async fn restart(&self, handle: &SandboxHandle) -> Result<(), AgentError> {
        tracing::info!("Restarting sandbox container {}", handle.sandbox_name);
        self.docker
            .restart_container(
                &handle.sandbox_id,
                None::<bollard::query_parameters::RestartContainerOptions>,
            )
            .await
            .map_err(|e| AgentError::SandboxExecFailed {
                run_id: 0,
                sandbox_id: handle.sandbox_id.clone(),
                reason: format!("Failed to restart container: {}", e),
            })?;
        Ok(())
    }

    async fn write_file(
        &self,
        handle: &SandboxHandle,
        path: &str,
        contents: &[u8],
        mode: u32,
    ) -> Result<(), AgentError> {
        // Split the absolute path into the parent dir (extraction target) and
        // the file basename (entry name inside the tar). Docker's
        // upload_to_container extracts the tar at the given `path`.
        let (parent_dir, file_name) = match path.rsplit_once('/') {
            Some((p, f)) if !f.is_empty() => {
                let parent = if p.is_empty() { "/" } else { p };
                (parent.to_string(), f.to_string())
            }
            _ => {
                return Err(AgentError::SandboxExecFailed {
                    run_id: 0,
                    sandbox_id: handle.sandbox_id.clone(),
                    reason: format!(
                        "write_file: path '{}' must be absolute with a filename",
                        path
                    ),
                });
            }
        };

        // Build an in-memory tar with a single file entry. tar crate is sync,
        // so we do this on the current thread (cheap for skill/CLAUDE/.env sized files).
        let tar_bytes = {
            let mut header = tar::Header::new_gnu();
            header.set_size(contents.len() as u64);
            header.set_mode(mode);
            // Files under /home/temps must be owned by the `temps` user (uid 1000)
            // created in the sandbox Dockerfile, otherwise tight modes like 0600
            // become unreadable by the container's runtime user.
            if path.starts_with("/home/temps") {
                header.set_uid(1000);
                header.set_gid(1000);
            }
            header.set_mtime(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
            );
            header.set_cksum();

            let mut buf: Vec<u8> = Vec::with_capacity(contents.len() + 1024);
            {
                let mut builder = tar::Builder::new(&mut buf);
                builder
                    .append_data(&mut header, &file_name, contents)
                    .map_err(|e| AgentError::SandboxExecFailed {
                        run_id: 0,
                        sandbox_id: handle.sandbox_id.clone(),
                        reason: format!("write_file: tar build failed for {}: {}", path, e),
                    })?;
                builder
                    .finish()
                    .map_err(|e| AgentError::SandboxExecFailed {
                        run_id: 0,
                        sandbox_id: handle.sandbox_id.clone(),
                        reason: format!("write_file: tar finish failed for {}: {}", path, e),
                    })?;
            }
            buf
        };

        // Ensure parent directory exists. upload_to_container won't create
        // intermediate dirs — extraction fails if `parent_dir` is missing.
        // Use a short, well-bounded exec for mkdir (it produces no output but
        // returns quickly; the polling exec loop handles the phantom case).
        let mkdir = vec!["mkdir".to_string(), "-p".to_string(), parent_dir.clone()];
        // Best-effort: if mkdir hangs, the upload below will fail loudly with
        // a clear "no such directory" error rather than hanging silently.
        let _ = self.exec(handle, mkdir, HashMap::new(), None).await;

        let options = bollard::query_parameters::UploadToContainerOptionsBuilder::default()
            .path(&parent_dir)
            .build();

        let body = bollard::body_full(tar_bytes.into());

        // Hard timeout so we never replicate the phantom-stream hang.
        let upload = self
            .docker
            .upload_to_container(&handle.sandbox_id, Some(options), body);

        match tokio::time::timeout(std::time::Duration::from_secs(30), upload).await {
            Ok(Ok(())) => {
                tracing::debug!(
                    "write_file: uploaded {} bytes to {} in container {}",
                    contents.len(),
                    path,
                    handle.sandbox_name
                );
                Ok(())
            }
            Ok(Err(e)) => Err(AgentError::SandboxExecFailed {
                run_id: 0,
                sandbox_id: handle.sandbox_id.clone(),
                reason: format!("write_file: upload to {} failed: {}", path, e),
            }),
            Err(_) => Err(AgentError::SandboxExecFailed {
                run_id: 0,
                sandbox_id: handle.sandbox_id.clone(),
                reason: format!("write_file: upload to {} timed out after 30s", path),
            }),
        }
    }

    async fn read_file(&self, handle: &SandboxHandle, path: &str) -> Result<Vec<u8>, AgentError> {
        use futures::StreamExt;
        use std::io::Read;

        let options = bollard::query_parameters::DownloadFromContainerOptionsBuilder::default()
            .path(path)
            .build();

        let stream = self
            .docker
            .download_from_container(&handle.sandbox_id, Some(options));

        // Collect tar stream into memory with a hard 30s cap so we never hang.
        let collect = async {
            let mut buf: Vec<u8> = Vec::new();
            let mut s = stream;
            while let Some(chunk) = s.next().await {
                match chunk {
                    Ok(bytes) => buf.extend_from_slice(&bytes),
                    Err(e) => {
                        return Err(AgentError::SandboxExecFailed {
                            run_id: 0,
                            sandbox_id: handle.sandbox_id.clone(),
                            reason: format!("read_file: download {} failed: {}", path, e),
                        });
                    }
                }
            }
            Ok(buf)
        };

        let tar_bytes =
            match tokio::time::timeout(std::time::Duration::from_secs(30), collect).await {
                Ok(Ok(b)) => b,
                Ok(Err(e)) => return Err(e),
                Err(_) => {
                    return Err(AgentError::SandboxExecFailed {
                        run_id: 0,
                        sandbox_id: handle.sandbox_id.clone(),
                        reason: format!("read_file: download {} timed out after 30s", path),
                    });
                }
            };

        // Extract the single file from the tar. Docker's archive endpoint
        // returns a tar whose top-level entry is the basename of `path`.
        let mut archive = tar::Archive::new(std::io::Cursor::new(tar_bytes));
        let mut entries = archive
            .entries()
            .map_err(|e| AgentError::SandboxExecFailed {
                run_id: 0,
                sandbox_id: handle.sandbox_id.clone(),
                reason: format!("read_file: tar open for {} failed: {}", path, e),
            })?;

        for entry in entries.by_ref() {
            let mut entry = entry.map_err(|e| AgentError::SandboxExecFailed {
                run_id: 0,
                sandbox_id: handle.sandbox_id.clone(),
                reason: format!("read_file: tar entry for {} failed: {}", path, e),
            })?;
            // Skip directories, symlinks, etc. — we want the regular file.
            if entry.header().entry_type().is_file() {
                let mut contents = Vec::new();
                entry
                    .read_to_end(&mut contents)
                    .map_err(|e| AgentError::SandboxExecFailed {
                        run_id: 0,
                        sandbox_id: handle.sandbox_id.clone(),
                        reason: format!("read_file: read entry for {} failed: {}", path, e),
                    })?;
                return Ok(contents);
            }
        }

        Err(AgentError::SandboxExecFailed {
            run_id: 0,
            sandbox_id: handle.sandbox_id.clone(),
            reason: format!("read_file: no regular file entry in tar for {}", path),
        })
    }

    async fn kill_processes(
        &self,
        handle: &SandboxHandle,
        pattern: &str,
        signal: super::KillSignal,
    ) -> Result<(), AgentError> {
        // Fresh exec running pkill. pkill exits 0 if something was killed,
        // 1 if nothing matched — both are success from our POV. Bounded by
        // a 10s timeout so we never replicate the phantom-stream hang.
        //
        // Use `pgrep` + `kill` instead of `pkill -f` to handle both busybox
        // and util-linux pkill variants uniformly.
        let sig_num = signal.as_number();
        let cmd = vec![
            "sh".to_string(),
            "-c".to_string(),
            format!(
                "pgrep -f {pattern_q} 2>/dev/null | xargs -r kill -{sig} 2>/dev/null; exit 0",
                pattern_q = shell_quote(pattern),
                sig = sig_num,
            ),
        ];

        let exec = self.exec(handle, cmd, HashMap::new(), None);
        match tokio::time::timeout(std::time::Duration::from_secs(10), exec).await {
            Ok(Ok(_)) => {
                tracing::debug!(
                    "kill_processes: sent signal {} to '{}' in {}",
                    sig_num,
                    pattern,
                    handle.sandbox_name
                );
                Ok(())
            }
            Ok(Err(e)) => {
                // Don't propagate — kill is best-effort. Log and move on.
                tracing::warn!(
                    "kill_processes: exec failed for '{}' in {}: {}",
                    pattern,
                    handle.sandbox_name,
                    e
                );
                Ok(())
            }
            Err(_) => {
                tracing::warn!(
                    "kill_processes: timed out killing '{}' in {}",
                    pattern,
                    handle.sandbox_name
                );
                Ok(())
            }
        }
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

    async fn rebuild_image(&self) -> Result<String, AgentError> {
        let image_name = self.config.resolved_image();

        // Remove existing image (force, in case containers reference it)
        if self.docker.inspect_image(&image_name).await.is_ok() {
            let opts = bollard::query_parameters::RemoveImageOptionsBuilder::new()
                .force(true)
                .build();
            let _ = self
                .docker
                .remove_image(&image_name, Some(opts), None)
                .await;
            tracing::info!("Removed old sandbox image {}", image_name);
        }

        // Rebuild
        self.ensure_image().await?;

        Ok(image_name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

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
        assert_eq!(config.default_cpu_limit, 4.0);
        assert_eq!(config.default_memory_limit_mb, 8192);
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
        assert!(df.contains("jq"), "jq must be installed for memory script");
    }

    #[test]
    fn test_all_runtimes_install_jq() {
        // The memory script requires jq. Every runtime preset must install it.
        for runtime in &["node", "bun", "python", "rust", "go", "full"] {
            let df = dockerfile_for_runtime(runtime);
            assert!(
                df.contains("jq"),
                "runtime {} dockerfile must install jq",
                runtime
            );
        }
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
            pids_limit: None,
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
        provider.destroy(&handle, true).await.unwrap();

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
