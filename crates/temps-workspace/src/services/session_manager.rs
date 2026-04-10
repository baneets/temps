use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

use temps_agents::ai_cli::OnEventCallback;
use temps_agents::sandbox::{
    SandboxCreateConfig, SandboxExecResult, SandboxHandle, SandboxProvider,
};
use temps_core::EncryptionService;

use crate::error::WorkspaceError;

/// Temps platform skill file content.
/// Auto-injected into workspace sandboxes to teach the AI how to use the Temps CLI.
/// Canonical Temps CLI skill, embedded at compile time from
/// `temps/skills/temps-cli/SKILL.md`. This is the same skill markdown the
/// rest of the platform ships, so workspace sessions, autofixer runs, and
/// human users all see one consistent CLI reference.
///
/// We deliberately do NOT pre-install `@temps-sdk/cli` in the sandbox image —
/// the skill instructs Claude to call it via `bunx @temps-sdk/cli@latest`
/// (or `npx @temps-sdk/cli@latest`), so each session always picks up the
/// latest published version without rebuilding container images.
pub const TEMPS_PLATFORM_SKILL: &str = include_str!("../../../../skills/temps-cli/SKILL.md");

/// Lightweight project descriptor used when injecting the global CLAUDE.md
/// so the agent knows exactly which Temps project the sandbox belongs to.
#[derive(Debug, Clone, Copy)]
pub struct ProjectContext<'a> {
    pub id: i32,
    pub slug: &'a str,
    pub name: &'a str,
    pub repo_owner: &'a str,
    pub repo_name: &'a str,
    pub branch: &'a str,
}

/// Tracks a live workspace session's sandbox state.
#[derive(Debug, Clone)]
pub struct LiveSession {
    pub session_id: i32,
    pub project_id: i32,
    pub handle: SandboxHandle,
    /// Container-side work dir (e.g. `/workspace`). Same as `handle.work_dir`.
    pub work_dir: PathBuf,
    /// Host-side work dir bind-mounted into the container at `work_dir`.
    /// Kept here (rather than on SandboxHandle) to avoid touching every
    /// provider constructor. Used by the paste handler to write files
    /// directly via the bind mount. `None` for sessions recovered after a
    /// server restart — those paths can still be rehydrated lazily from the
    /// Docker container's Mounts if ever needed.
    pub host_work_dir: Option<PathBuf>,
    pub is_first_message: bool,
}

/// Manages sandbox containers for workspace sessions.
///
/// Wraps the existing SandboxProvider with workspace-specific concerns:
/// - Long-lived containers (persist across chat turns)
/// - Session tracking (maps session_id → SandboxHandle)
/// - Credential injection (Temps API token, AI provider keys)
/// - Idle timeout management
pub struct WorkspaceSessionManager {
    provider: Arc<dyn SandboxProvider>,
    #[allow(dead_code)]
    encryption_service: Arc<EncryptionService>,
    sessions: RwLock<HashMap<i32, LiveSession>>,
    idle_timeout: Duration,
}

impl WorkspaceSessionManager {
    pub fn new(
        provider: Arc<dyn SandboxProvider>,
        encryption_service: Arc<EncryptionService>,
        idle_timeout: Duration,
    ) -> Self {
        Self {
            provider,
            encryption_service,
            sessions: RwLock::new(HashMap::new()),
            idle_timeout,
        }
    }

    /// Create a sandbox for a workspace session.
    ///
    /// The sandbox is a long-lived Docker container with:
    /// - The project repo cloned at /workspace
    /// - Temps CLI + AI provider credentials injected
    /// - Skill file auto-generated
    #[allow(clippy::too_many_arguments)]
    pub async fn create_sandbox(
        &self,
        session_id: i32,
        project_id: i32,
        host_work_dir: PathBuf,
        env_vars: HashMap<String, String>,
        cpu_limit: Option<f32>,
        memory_limit_mb: Option<i32>,
        pids_limit: Option<i32>,
    ) -> Result<SandboxHandle, WorkspaceError> {
        let host_work_dir_for_session = host_work_dir.clone();
        let config = SandboxCreateConfig {
            run_id: session_id, // Reuse run_id field for session_id
            host_work_dir,
            image: None,
            cpu_limit: cpu_limit.map(|v| v as f64),
            memory_limit_mb: memory_limit_mb.map(|v| v as u64),
            pids_limit: pids_limit.map(|v| v as i64),
            network_mode: None,
            env_vars,
            idle_timeout: self.idle_timeout,
        };

        let handle = self.provider.create(config).await.map_err(|e| {
            WorkspaceError::SandboxCreationFailed {
                session_id,
                reason: e.to_string(),
            }
        })?;

        let work_dir = handle.work_dir.clone();

        let live = LiveSession {
            session_id,
            project_id,
            handle: handle.clone(),
            work_dir,
            host_work_dir: Some(host_work_dir_for_session),
            is_first_message: true,
        };

        self.sessions.write().await.insert(session_id, live);

        Ok(handle)
    }

    /// Execute a command inside the session's sandbox container.
    pub async fn exec(
        &self,
        session_id: i32,
        cmd: Vec<String>,
        env: HashMap<String, String>,
        on_output: Option<OnEventCallback>,
    ) -> Result<SandboxExecResult, WorkspaceError> {
        let sessions = self.sessions.read().await;
        let live = sessions
            .get(&session_id)
            .ok_or(WorkspaceError::SandboxNotAvailable { session_id })?;

        self.provider
            .exec(&live.handle, cmd, env, on_output)
            .await
            .map_err(|e| WorkspaceError::AiCliFailed {
                session_id,
                reason: e.to_string(),
            })
    }

    /// Write a file directly into the session's sandbox via the provider's
    /// native file-write API (tar streaming for Docker, fs::write for local).
    /// This avoids the bollard exec phantom-stream hang on silent processes
    /// (e.g. `bash -c "cat > ... << EOF"` heredoc writes).
    pub async fn write_file(
        &self,
        session_id: i32,
        path: &str,
        contents: &[u8],
        mode: u32,
    ) -> Result<(), WorkspaceError> {
        let sessions = self.sessions.read().await;
        let live = sessions
            .get(&session_id)
            .ok_or(WorkspaceError::SandboxNotAvailable { session_id })?;

        self.provider
            .write_file(&live.handle, path, contents, mode)
            .await
            .map_err(|e| WorkspaceError::AiCliFailed {
                session_id,
                reason: format!("Failed to write {}: {}", path, e),
            })?;

        // Verify the write actually landed. If something silently swallowed
        // it (or extracted into the wrong place), surface a loud error so the
        // chat shows it instead of running with a half-initialized sandbox.
        let verify = vec!["test".to_string(), "-s".to_string(), path.to_string()];
        let result = self
            .provider
            .exec(&live.handle, verify, HashMap::new(), None)
            .await
            .map_err(|e| WorkspaceError::AiCliFailed {
                session_id,
                reason: format!("Failed to verify {} after write: {}", path, e),
            })?;

        if result.exit_code != 0 {
            return Err(WorkspaceError::AiCliFailed {
                session_id,
                reason: format!(
                    "Sandbox setup failed: file '{}' is missing or empty after write (exit {}). \
                     The agent will not have access to required configuration.",
                    path, result.exit_code
                ),
            });
        }

        Ok(())
    }

    /// Read a file from the session's sandbox via the provider's native
    /// download API. Returns the raw bytes.
    pub async fn read_file(&self, session_id: i32, path: &str) -> Result<Vec<u8>, WorkspaceError> {
        let sessions = self.sessions.read().await;
        let live = sessions
            .get(&session_id)
            .ok_or(WorkspaceError::SandboxNotAvailable { session_id })?;
        self.provider
            .read_file(&live.handle, path)
            .await
            .map_err(|e| WorkspaceError::AiCliFailed {
                session_id,
                reason: format!("Failed to read {}: {}", path, e),
            })
    }

    /// Kill processes inside the session's sandbox matching a pattern.
    /// Best-effort: never returns a hard error, just logs.
    pub async fn kill_session_processes(
        &self,
        session_id: i32,
        pattern: &str,
        signal: temps_agents::sandbox::KillSignal,
    ) {
        let sessions = self.sessions.read().await;
        if let Some(live) = sessions.get(&session_id) {
            let _ = self
                .provider
                .kill_processes(&live.handle, pattern, signal)
                .await;
        }
    }

    /// Delete a file inside the session's sandbox. Best-effort.
    pub async fn delete_file(&self, session_id: i32, path: &str) {
        let sessions = self.sessions.read().await;
        if let Some(live) = sessions.get(&session_id) {
            let _ = self
                .provider
                .exec(
                    &live.handle,
                    vec!["rm".to_string(), "-f".to_string(), path.to_string()],
                    HashMap::new(),
                    None,
                )
                .await;
        }
    }

    /// Check whether a file exists inside the session's sandbox.
    pub async fn file_exists(&self, session_id: i32, path: &str) -> bool {
        let sessions = self.sessions.read().await;
        let Some(live) = sessions.get(&session_id) else {
            return false;
        };
        match self
            .provider
            .exec(
                &live.handle,
                vec!["test".to_string(), "-f".to_string(), path.to_string()],
                HashMap::new(),
                None,
            )
            .await
        {
            Ok(r) => r.exit_code == 0,
            Err(_) => false,
        }
    }

    /// Return true if the session's sandbox currently has a running AI CLI
    /// process (claude / codex / opencode) — either attached to a tmux
    /// client or running detached in the background. Used by the idle
    /// sweeper to avoid reaping sessions that are doing autonomous work
    /// between user keystrokes.
    ///
    /// Checks via `pgrep` inside the container. A missing `pgrep` (or any
    /// exec failure) returns false, which is the safe default — the
    /// sweeper will only *skip* reaping when we can positively confirm
    /// work is happening.
    pub async fn has_ai_cli_running(&self, session_id: i32) -> bool {
        let sessions = self.sessions.read().await;
        let Some(live) = sessions.get(&session_id) else {
            return false;
        };
        match self
            .provider
            .exec(
                &live.handle,
                vec![
                    "sh".to_string(),
                    "-c".to_string(),
                    // -f matches the full command line, so `claude` running
                    // as `node .../claude` still matches.
                    "pgrep -f '(^|/)claude( |$)|(^|/)codex( |$)|(^|/)opencode( |$)' >/dev/null"
                        .to_string(),
                ],
                HashMap::new(),
                None,
            )
            .await
        {
            Ok(r) => r.exit_code == 0,
            Err(_) => false,
        }
    }

    /// Attempt to adopt an existing sandbox container for a session after
    /// server restart. Returns true if adoption succeeded (container was
    /// found alive and registered into the in-memory map).
    pub async fn adopt_existing(
        &self,
        session_id: i32,
        project_id: i32,
    ) -> Result<bool, WorkspaceError> {
        // Already tracked? Nothing to do.
        if self.sessions.read().await.contains_key(&session_id) {
            return Ok(true);
        }

        match self.provider.recover(session_id).await {
            Ok(Some(handle)) => {
                let work_dir = handle.work_dir.clone();
                // Adopted sessions are NOT first-message — there's prior
                // conversation in the sandbox's claude jsonl.
                // Reconstruct the deterministic host work dir used at create
                // time (see message_executor.rs: `temp_dir/workspace-{id}`).
                // This is the same bind-mount source the sandbox was created
                // with, so paste-image can write through it directly.
                let host_work_dir_guess =
                    std::env::temp_dir().join(format!("workspace-{}", session_id));
                let host_work_dir = if host_work_dir_guess.exists() {
                    Some(host_work_dir_guess)
                } else {
                    None
                };
                let live = LiveSession {
                    session_id,
                    project_id,
                    handle,
                    work_dir,
                    host_work_dir,
                    is_first_message: false,
                };
                self.sessions.write().await.insert(session_id, live);
                tracing::info!(
                    "Adopted existing sandbox container for session {}",
                    session_id
                );
                Ok(true)
            }
            Ok(None) => Ok(false),
            Err(e) => Err(WorkspaceError::SandboxCreationFailed {
                session_id,
                reason: format!("adopt_existing failed: {}", e),
            }),
        }
    }

    /// Build the Claude CLI command for a workspace message.
    pub fn build_chat_cmd(
        &self,
        prompt: &str,
        max_turns: i32,
        continue_conversation: bool,
        provider: &str,
    ) -> Vec<String> {
        // Reuse the existing build_claude_cmd from temps-agents executor
        match provider {
            "claude_cli" | "" => {
                let mut cmd = vec!["claude".to_string(), "--print".to_string()];
                if continue_conversation {
                    cmd.push("--continue".to_string());
                }
                cmd.push(prompt.to_string());
                cmd.extend([
                    "--output-format".to_string(),
                    "stream-json".to_string(),
                    "--max-turns".to_string(),
                    max_turns.to_string(),
                    "--dangerously-skip-permissions".to_string(),
                    "--verbose".to_string(),
                ]);
                cmd
            }
            "codex_cli" => {
                vec![
                    "codex".to_string(),
                    "--approval-mode".to_string(),
                    "full-auto".to_string(),
                    "--quiet".to_string(),
                    prompt.to_string(),
                ]
            }
            "opencode" => {
                let mut cmd = vec!["opencode".to_string(), "run".to_string()];
                if continue_conversation {
                    cmd.push("--continue".to_string());
                }
                cmd.push(prompt.to_string());
                cmd.push("--format".to_string());
                cmd.push("json".to_string());
                cmd
            }
            other => {
                vec![other.to_string(), prompt.to_string()]
            }
        }
    }

    /// Mark a session's first message as sent (subsequent messages use --continue).
    pub async fn mark_first_message_sent(&self, session_id: i32) {
        let mut sessions = self.sessions.write().await;
        if let Some(live) = sessions.get_mut(&session_id) {
            live.is_first_message = false;
        }
    }

    /// Check if this is the first message in a session (determines --continue flag).
    pub async fn is_first_message(&self, session_id: i32) -> bool {
        let sessions = self.sessions.read().await;
        sessions
            .get(&session_id)
            .map(|s| s.is_first_message)
            .unwrap_or(true)
    }

    /// Check if a session's sandbox is alive.
    ///
    /// Returns true only if BOTH:
    ///   1. The session is tracked in the in-memory map, AND
    ///   2. The provider confirms the underlying container is actually running.
    ///
    /// This avoids the "stale in-memory handle" failure where we think a
    /// sandbox is alive but its container was stopped/removed out of band.
    pub async fn is_alive(&self, session_id: i32) -> bool {
        let sessions = self.sessions.read().await;
        if let Some(live) = sessions.get(&session_id) {
            self.provider.is_alive(&live.handle).await.unwrap_or(false)
        } else {
            false
        }
    }

    /// Stop a session's sandbox container without removing it.
    /// The session remains tracked so it can be started again later.
    pub async fn stop_sandbox(&self, session_id: i32) -> Result<(), WorkspaceError> {
        let sessions = self.sessions.read().await;
        let live = sessions
            .get(&session_id)
            .ok_or(WorkspaceError::SandboxNotAvailable { session_id })?;
        self.provider
            .stop(&live.handle)
            .await
            .map_err(|e| WorkspaceError::AiCliFailed {
                session_id,
                reason: format!("Failed to stop sandbox: {}", e),
            })
    }

    /// Start a previously stopped sandbox container.
    pub async fn start_sandbox(&self, session_id: i32) -> Result<(), WorkspaceError> {
        let sessions = self.sessions.read().await;
        let live = sessions
            .get(&session_id)
            .ok_or(WorkspaceError::SandboxNotAvailable { session_id })?;
        self.provider
            .start(&live.handle)
            .await
            .map_err(|e| WorkspaceError::AiCliFailed {
                session_id,
                reason: format!("Failed to start sandbox: {}", e),
            })
    }

    /// Restart a session's sandbox in place. The container ID is preserved
    /// so any inbound preview requests keep working as soon as the dev
    /// server is back up inside the container.
    pub async fn restart_sandbox(&self, session_id: i32) -> Result<(), WorkspaceError> {
        let sessions = self.sessions.read().await;
        let live = sessions
            .get(&session_id)
            .ok_or(WorkspaceError::SandboxNotAvailable { session_id })?;
        self.provider
            .restart(&live.handle)
            .await
            .map_err(|e| WorkspaceError::AiCliFailed {
                session_id,
                reason: format!("Failed to restart sandbox: {}", e),
            })
    }

    /// Release (destroy) a session's sandbox and remove from tracking.
    ///
    /// `purge_volumes` controls whether the per-session `/home/temps`
    /// named volume is also deleted. Pass `false` on session *close* so
    /// the user's claude auth, shell history, and tmux state survive the
    /// next reopen; pass `true` on session *delete* so nothing leaks.
    pub async fn release(
        &self,
        session_id: i32,
        purge_volumes: bool,
    ) -> Result<(), WorkspaceError> {
        let live = self.sessions.write().await.remove(&session_id);

        if let Some(live) = live {
            self.provider
                .destroy(&live.handle, purge_volumes)
                .await
                .map_err(|e| WorkspaceError::AiCliFailed {
                    session_id,
                    reason: format!("Failed to destroy sandbox: {}", e),
                })?;
            tracing::info!(
                "Released sandbox for workspace session {} (purge_volumes={})",
                session_id,
                purge_volumes
            );
        }

        Ok(())
    }

    /// Get the sandbox handle for a session (if active).
    pub async fn get_handle(&self, session_id: i32) -> Option<SandboxHandle> {
        self.sessions
            .read()
            .await
            .get(&session_id)
            .map(|s| s.handle.clone())
    }

    /// Get the host-side work dir bind-mounted into this session's sandbox.
    /// Returns None for sessions recovered from disk (pre-restart), since the
    /// path wasn't persisted. Callers can fall back to Docker `inspect` on
    /// the container to rehydrate it in that case.
    pub async fn get_host_work_dir(&self, session_id: i32) -> Option<PathBuf> {
        self.sessions
            .read()
            .await
            .get(&session_id)
            .and_then(|s| s.host_work_dir.clone())
    }

    /// Get all active session IDs (for idle timeout checking).
    pub async fn active_session_ids(&self) -> Vec<i32> {
        self.sessions.read().await.keys().copied().collect()
    }

    /// Build environment variables for a workspace sandbox.
    ///
    /// Static credentials (injected at container creation):
    /// - TEMPS_API_TOKEN — project-scoped deployment token
    /// - TEMPS_API_URL — Temps instance URL
    /// - TEMPS_PROJECT_ID — for the memory script and any other scoped CLI calls
    /// - TEMPS_WORKFLOW_SLUG — for the memory script
    /// - AI provider key (ANTHROPIC_API_KEY for api_key auth; subscription
    ///   auth uses ~/.claude/.credentials.json written post-creation)
    /// - PATH — extended with /workspace/.temps/bin so `memory` is on PATH
    ///
    /// Service credentials (DATABASE_URL, REDIS_URL) are NOT baked in.
    /// They are fetched at runtime via `temps services connect <name>`.
    pub fn build_env_vars(
        temps_api_url: &str,
        temps_api_token: &str,
        ai_provider_key: Option<&str>,
        ai_auth_type: &str,
    ) -> HashMap<String, String> {
        Self::build_env_vars_with_workflow(
            temps_api_url,
            temps_api_token,
            ai_provider_key,
            ai_auth_type,
            None,
            None,
        )
    }

    /// Like `build_env_vars` but also injects `TEMPS_PROJECT_ID` and
    /// `TEMPS_WORKFLOW_SLUG` so the memory script knows which workflow to
    /// scope its writes to.
    pub fn build_env_vars_with_workflow(
        temps_api_url: &str,
        temps_api_token: &str,
        ai_provider_key: Option<&str>,
        ai_auth_type: &str,
        project_id: Option<i32>,
        workflow_slug: Option<&str>,
    ) -> HashMap<String, String> {
        let mut env = HashMap::new();

        env.insert("TEMPS_API_URL".to_string(), temps_api_url.to_string());
        env.insert("TEMPS_API_TOKEN".to_string(), temps_api_token.to_string());

        if let Some(pid) = project_id {
            env.insert("TEMPS_PROJECT_ID".to_string(), pid.to_string());
        }
        if let Some(slug) = workflow_slug {
            env.insert("TEMPS_WORKFLOW_SLUG".to_string(), slug.to_string());
        }

        // For API key auth, set ANTHROPIC_API_KEY as container env var.
        // For subscription auth, credentials are injected via
        // ~/.claude/.credentials.json after container creation — not as
        // an env var — so the CLI gets full OAuth context.
        if let Some(key) = ai_provider_key {
            if ai_auth_type != "subscription" {
                env.insert("ANTHROPIC_API_KEY".to_string(), key.to_string());
            }
        }

        // Tell Claude CLI to accept non-interactive mode
        env.insert("CLAUDE_CODE_ENTRYPOINT".to_string(), "cli".to_string());

        // Put /workspace/.temps/bin on PATH so the memory script is callable
        // as `memory` from anywhere (instead of by full path).
        env.insert(
            "PATH".to_string(),
            "/workspace/.temps/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
                .to_string(),
        );

        env
    }

    /// Write the Temps platform skill file into the sandbox's workspace.
    /// This teaches the AI how to use the Temps CLI.
    pub async fn inject_skill_file(&self, session_id: i32) -> Result<(), WorkspaceError> {
        // Claude's skill discovery requires the directory (or filename) to match
        // the frontmatter `name:` field. The canonical skill declares
        // `name: temps-cli`, so we install it as
        // `/workspace/.claude/skills/temps-cli/SKILL.md`.
        let mkdir_cmd = vec![
            "mkdir".to_string(),
            "-p".to_string(),
            "/workspace/.claude/skills/temps-cli".to_string(),
        ];
        self.exec(session_id, mkdir_cmd, HashMap::new(), None)
            .await?;

        // Remove any stale flat-file version from older sandbox builds.
        let cleanup_cmd = vec![
            "rm".to_string(),
            "-f".to_string(),
            "/workspace/.claude/skills/temps-platform.md".to_string(),
        ];
        let _ = self
            .exec(session_id, cleanup_cmd, HashMap::new(), None)
            .await;

        // Write the skill file content via native tar upload (avoids the
        // bollard exec phantom-stream hang on silent heredoc writes).
        self.write_file(
            session_id,
            "/workspace/.claude/skills/temps-cli/SKILL.md",
            TEMPS_PLATFORM_SKILL.as_bytes(),
            0o644,
        )
        .await?;

        tracing::debug!("Injected Temps platform skill into session {}", session_id);
        Ok(())
    }

    /// Seed `~/.claude.json` inside the sandbox so Claude CLI skips the
    /// one-time onboarding flow (theme picker, "Let's get started" screen)
    /// on first launch. Each workspace session gets its own `/home/temps`
    /// named volume, so without this seed every new session's first
    /// `claude` invocation in the terminal would block on the theme picker
    /// — even though the user is already authenticated via
    /// `~/.claude/.credentials.json`.
    ///
    /// Best-effort: if the file already exists (e.g. the user completed
    /// onboarding once and the home volume persisted it across a container
    /// restart), we leave it alone.
    pub async fn seed_claude_config(&self, session_id: i32) -> Result<(), WorkspaceError> {
        // Always overwrite — ensures the latest config fields are present.
        // auto-updates / interactive prompts inside the sandbox.
        let body = serde_json::json!({
            "numStartups": 3,
            "installMethod": "native",
            "autoUpdates": false,
            "tipsHistory": { "new-user-warmup": 2 },
            "autoUpdatesProtectedForNative": true,
            "hasCompletedOnboarding": true,
            "lastOnboardingVersion": "2.1.96",
            "projects": {},
            "voiceNoticeSeenCount": 2,
            "cachedExtraUsageDisabledReason": null,
            "officialMarketplaceAutoInstallAttempted": true,
            "officialMarketplaceAutoInstalled": true,
            "theme": "dark",
            "bypassPermissionsModeAccepted": true,
            "hasSeenWelcome": true,
        });
        let bytes = serde_json::to_vec_pretty(&body).map_err(|e| WorkspaceError::AiCliFailed {
            session_id,
            reason: format!("seed_claude_config: serialize failed: {}", e),
        })?;

        self.write_file(session_id, "/home/temps/.claude.json", &bytes, 0o600)
            .await?;

        tracing::debug!(
            "Seeded /home/temps/.claude.json for session {} (skips onboarding)",
            session_id
        );

        // Also seed ~/.claude/settings.json with theme preference.
        // mkdir -p first — credentials file may not have been written yet.
        self.exec(
            session_id,
            vec!["mkdir".into(), "-p".into(), "/home/temps/.claude".into()],
            std::collections::HashMap::new(),
            None,
        )
        .await?;

        let settings = serde_json::json!({ "theme": "dark" });
        let settings_bytes =
            serde_json::to_vec_pretty(&settings).map_err(|e| WorkspaceError::AiCliFailed {
                session_id,
                reason: format!("seed_claude_config: settings serialize failed: {}", e),
            })?;
        self.write_file(
            session_id,
            "/home/temps/.claude/settings.json",
            &settings_bytes,
            0o600,
        )
        .await?;

        Ok(())
    }

    /// Write `/home/temps/.claude/.credentials.json` with the OAuth token so
    /// Claude CLI authenticates without needing `CLAUDE_CODE_OAUTH_TOKEN` env
    /// var. This gives the CLI the full auth context (subscription type, scopes)
    /// rather than just a bare token.
    ///
    /// For `api_key` auth type, does nothing — `ANTHROPIC_API_KEY` in `~/.env`
    /// is the correct mechanism for API key auth.
    pub async fn seed_claude_credentials(
        &self,
        session_id: i32,
        access_token: &str,
        auth_type: &str,
    ) -> Result<(), WorkspaceError> {
        if auth_type != "subscription" {
            return Ok(());
        }

        // Ensure ~/.claude/ directory exists
        self.exec(
            session_id,
            vec!["mkdir".into(), "-p".into(), "/home/temps/.claude".into()],
            std::collections::HashMap::new(),
            None,
        )
        .await?;

        let body = serde_json::json!({
            "claudeAiOauth": {
                "accessToken": access_token,
                "expiresAt": 1772120060006_i64,
                "scopes": [
                    "user:inference",
                    "user:mcp_servers",
                    "user:profile",
                    "user:sessions:claude_code"
                ],
                "subscriptionType": "max",
                "rateLimitTier": "default_claude_max_20x"
            }
        });
        let bytes = serde_json::to_vec_pretty(&body).map_err(|e| WorkspaceError::AiCliFailed {
            session_id,
            reason: format!("seed_claude_credentials: serialize failed: {}", e),
        })?;

        self.write_file(
            session_id,
            "/home/temps/.claude/.credentials.json",
            &bytes,
            0o600,
        )
        .await?;

        tracing::debug!(
            "Seeded /home/temps/.claude/.credentials.json for session {}",
            session_id
        );
        Ok(())
    }

    /// Write `/root/.env` inside the sandbox containing the given key/value
    /// pairs and install a global `~/.claude/CLAUDE.md` that instructs Claude
    /// to source it before running commands. Tokens (git providers, linked
    /// services, etc.) are stored here so they can be refreshed by simply
    /// rewriting the file — no container restart required.
    ///
    /// Values are written using a single-quoted shell-safe encoding so that
    /// special characters in tokens don't break sourcing.
    pub async fn inject_env_file(
        &self,
        session_id: i32,
        env: &HashMap<String, String>,
        project_context: Option<&ProjectContext<'_>>,
    ) -> Result<(), WorkspaceError> {
        // Build the .env body. Single-quote each value, escaping embedded
        // single quotes via the `'\''` shell idiom.
        let mut body = String::new();
        body.push_str("# Managed by Temps. Refreshed automatically — do not edit by hand.\n");
        let mut keys: Vec<&String> = env.keys().collect();
        keys.sort();
        for key in keys {
            let value = env.get(key).map(|v| v.as_str()).unwrap_or("");
            let escaped = value.replace('\'', "'\\''");
            body.push_str(&format!("export {}='{}'\n", key, escaped));
        }

        // Native tar upload (mode 0o600 — secrets). HOME is /home/temps for
        // the non-root sandbox user defined in the Dockerfile.
        self.write_file(session_id, "/home/temps/.env", body.as_bytes(), 0o600)
            .await?;

        // Global CLAUDE.md telling the agent (a) which Temps project this
        // sandbox belongs to, and (b) how to use the managed env file. This
        // lives in `~/.claude/CLAUDE.md` so it loads for every session,
        // independent of the project's own CLAUDE.md.
        let project_section = match project_context {
            Some(ctx) => format!(
                r#"# Current Temps project

This sandbox belongs to a single Temps project. Use these values for any
`temps-cli` / `bunx @temps-sdk/cli` command — DO NOT ask the user which
project to operate on:

- **Project ID:** `{id}`
- **Slug:** `{slug}`
- **Name:** {name}
- **Repository:** `{repo_owner}/{repo_name}`
- **Default branch:** `{branch}`

When invoking the Temps CLI, always pass `--project {slug}` (or the
equivalent project flag) so commands are scoped to this project.

"#,
                id = ctx.id,
                slug = ctx.slug,
                name = ctx.name,
                repo_owner = ctx.repo_owner,
                repo_name = ctx.repo_name,
                branch = ctx.branch,
            ),
            None => String::new(),
        };

        let claude_md = format!(
            r#"{project_section}# Sandbox environment

This sandbox is managed by Temps. Credentials for linked services and git
providers are stored in `~/.env` and refreshed in-place by the platform — they
may rotate at any time.

**Before running any shell command that needs credentials**, source the env
file in the same command:

```bash
. ~/.env && <your command>
```

Examples:

```bash
. ~/.env && gh pr list
. ~/.env && glab mr list
. ~/.env && psql "$DATABASE_URL" -c '\dt'
```

Do not copy values out of `~/.env` into other files, scripts, or commit
messages — they are short-lived and may rotate. Always re-read from `~/.env`.
"#,
            project_section = project_section
        );
        let claude_md = claude_md.as_str();

        self.write_file(
            session_id,
            "/home/temps/.claude/CLAUDE.md",
            claude_md.as_bytes(),
            0o644,
        )
        .await?;

        tracing::debug!(
            "Injected ~/.env ({} keys) and global CLAUDE.md into session {}",
            env.len(),
            session_id
        );
        Ok(())
    }

    /// Install the memory CLI script (`/workspace/.temps/bin/memory`) in the
    /// sandbox so the AI can read/write workflow memory via simple shell commands.
    /// The script uses curl to call the Temps API; no CLI binary required.
    pub async fn inject_memory_script(&self, session_id: i32) -> Result<(), WorkspaceError> {
        let cmd = crate::services::memory_script::install_command();
        self.exec(session_id, cmd, HashMap::new(), None).await?;
        tracing::debug!("Installed memory script in session {}", session_id);
        Ok(())
    }

    /// Attempt to recover a sandbox after server restart.
    pub async fn recover_session(
        &self,
        session_id: i32,
        project_id: i32,
    ) -> Result<bool, WorkspaceError> {
        match self.provider.recover(session_id).await {
            Ok(Some(handle)) => {
                // Reconstruct the deterministic host bind-mount source so
                // paste-image works without needing a new message first.
                let host_work_dir_guess =
                    std::env::temp_dir().join(format!("workspace-{}", session_id));
                let host_work_dir = if host_work_dir_guess.exists() {
                    Some(host_work_dir_guess)
                } else {
                    None
                };
                let live = LiveSession {
                    session_id,
                    project_id,
                    handle,
                    work_dir: PathBuf::from("/workspace"),
                    host_work_dir,
                    is_first_message: false, // Recovered sessions have prior context
                };
                self.sessions.write().await.insert(session_id, live);
                tracing::info!("Recovered sandbox for workspace session {}", session_id);
                Ok(true)
            }
            Ok(None) => Ok(false),
            Err(e) => {
                tracing::warn!(
                    "Failed to recover sandbox for session {}: {}",
                    session_id,
                    e
                );
                Ok(false)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use temps_agents::error::AgentError;

    /// A fake sandbox provider for unit testing.
    struct FakeSandboxProvider {
        should_fail: bool,
    }

    #[async_trait::async_trait]
    impl SandboxProvider for FakeSandboxProvider {
        async fn create(&self, config: SandboxCreateConfig) -> Result<SandboxHandle, AgentError> {
            if self.should_fail {
                return Err(AgentError::SandboxCreationFailed {
                    run_id: config.run_id,
                    provider: "fake".to_string(),
                    reason: "forced failure".to_string(),
                });
            }
            Ok(SandboxHandle {
                sandbox_id: format!("fake-{}", config.run_id),
                sandbox_name: format!("temps-sandbox-{}", config.run_id),
                work_dir: PathBuf::from("/workspace"),
            })
        }

        async fn exec(
            &self,
            _handle: &SandboxHandle,
            cmd: Vec<String>,
            _env: HashMap<String, String>,
            _on_output: Option<OnEventCallback>,
        ) -> Result<SandboxExecResult, AgentError> {
            Ok(SandboxExecResult {
                exit_code: 0,
                stdout: format!("executed: {:?}", cmd),
            })
        }

        async fn is_alive(&self, _handle: &SandboxHandle) -> Result<bool, AgentError> {
            Ok(true)
        }

        async fn write_file(
            &self,
            _handle: &SandboxHandle,
            _path: &str,
            _contents: &[u8],
            _mode: u32,
        ) -> Result<(), AgentError> {
            Ok(())
        }

        async fn write_directory(
            &self,
            _handle: &SandboxHandle,
            _local_dir: &std::path::Path,
            _target_path: &str,
        ) -> Result<(), AgentError> {
            Ok(())
        }

        async fn read_file(
            &self,
            _handle: &SandboxHandle,
            _path: &str,
        ) -> Result<Vec<u8>, AgentError> {
            Ok(Vec::new())
        }

        async fn kill_processes(
            &self,
            _handle: &SandboxHandle,
            _pattern: &str,
            _signal: temps_agents::sandbox::KillSignal,
        ) -> Result<(), AgentError> {
            Ok(())
        }

        async fn destroy(
            &self,
            _handle: &SandboxHandle,
            _purge_volumes: bool,
        ) -> Result<(), AgentError> {
            Ok(())
        }

        async fn recover(&self, run_id: i32) -> Result<Option<SandboxHandle>, AgentError> {
            Ok(Some(SandboxHandle {
                sandbox_id: format!("recovered-{}", run_id),
                sandbox_name: format!("temps-sandbox-{}", run_id),
                work_dir: PathBuf::from("/workspace"),
            }))
        }

        fn name(&self) -> &str {
            "fake"
        }

        async fn is_available(&self) -> bool {
            true
        }

        async fn image_status(&self) -> Result<(bool, String), AgentError> {
            Ok((true, "fake:latest".to_string()))
        }

        async fn rebuild_image(&self) -> Result<String, AgentError> {
            Ok("fake:latest".to_string())
        }
    }

    fn make_manager(should_fail: bool) -> WorkspaceSessionManager {
        let provider = Arc::new(FakeSandboxProvider { should_fail });
        let encryption =
            Arc::new(EncryptionService::new("test-key-32-bytes-long-padding!!").unwrap());
        WorkspaceSessionManager::new(provider, encryption, Duration::from_secs(1800))
    }

    #[tokio::test]
    async fn test_create_sandbox_success() {
        let manager = make_manager(false);
        let result = manager
            .create_sandbox(
                1,
                10,
                PathBuf::from("/tmp/test"),
                HashMap::new(),
                None,
                None,
                None,
            )
            .await;

        assert!(result.is_ok());
        let handle = result.unwrap();
        assert_eq!(handle.sandbox_id, "fake-1");
    }

    #[tokio::test]
    async fn test_create_sandbox_failure() {
        let manager = make_manager(true);
        let result = manager
            .create_sandbox(
                1,
                10,
                PathBuf::from("/tmp/test"),
                HashMap::new(),
                None,
                None,
                None,
            )
            .await;

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            WorkspaceError::SandboxCreationFailed { session_id: 1, .. }
        ));
    }

    #[tokio::test]
    async fn test_exec_success() {
        let manager = make_manager(false);
        manager
            .create_sandbox(
                1,
                10,
                PathBuf::from("/tmp/test"),
                HashMap::new(),
                None,
                None,
                None,
            )
            .await
            .unwrap();

        let result = manager
            .exec(
                1,
                vec!["echo".to_string(), "hello".to_string()],
                HashMap::new(),
                None,
            )
            .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap().exit_code, 0);
    }

    #[tokio::test]
    async fn test_exec_no_sandbox_fails() {
        let manager = make_manager(false);

        let result = manager
            .exec(999, vec!["echo".to_string()], HashMap::new(), None)
            .await;

        assert!(result.is_err());
        let err = result.err().unwrap();
        assert!(matches!(
            err,
            WorkspaceError::SandboxNotAvailable { session_id: 999 }
        ));
    }

    #[tokio::test]
    async fn test_first_message_tracking() {
        let manager = make_manager(false);
        manager
            .create_sandbox(
                1,
                10,
                PathBuf::from("/tmp/test"),
                HashMap::new(),
                None,
                None,
                None,
            )
            .await
            .unwrap();

        assert!(manager.is_first_message(1).await);
        manager.mark_first_message_sent(1).await;
        assert!(!manager.is_first_message(1).await);
    }

    #[tokio::test]
    async fn test_release_sandbox() {
        let manager = make_manager(false);
        manager
            .create_sandbox(
                1,
                10,
                PathBuf::from("/tmp/test"),
                HashMap::new(),
                None,
                None,
                None,
            )
            .await
            .unwrap();

        assert!(manager.is_alive(1).await);
        manager.release(1, true).await.unwrap();
        assert!(!manager.is_alive(1).await);
    }

    #[tokio::test]
    async fn test_release_nonexistent_is_ok() {
        let manager = make_manager(false);
        let result = manager.release(999, true).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_active_session_ids() {
        let manager = make_manager(false);
        manager
            .create_sandbox(
                1,
                10,
                PathBuf::from("/tmp/t1"),
                HashMap::new(),
                None,
                None,
                None,
            )
            .await
            .unwrap();
        manager
            .create_sandbox(
                2,
                10,
                PathBuf::from("/tmp/t2"),
                HashMap::new(),
                None,
                None,
                None,
            )
            .await
            .unwrap();

        let mut ids = manager.active_session_ids().await;
        ids.sort();
        assert_eq!(ids, vec![1, 2]);
    }

    #[tokio::test]
    async fn test_recover_session() {
        let manager = make_manager(false);
        let recovered = manager.recover_session(42, 10).await.unwrap();
        assert!(recovered);
        assert!(manager.is_alive(42).await);
    }

    #[test]
    fn test_build_env_vars_api_key() {
        let env = WorkspaceSessionManager::build_env_vars(
            "http://localhost:3000",
            "test-token",
            Some("sk-ant-123"),
            "api_key",
        );

        assert_eq!(env.get("TEMPS_API_URL").unwrap(), "http://localhost:3000");
        assert_eq!(env.get("TEMPS_API_TOKEN").unwrap(), "test-token");
        assert_eq!(env.get("ANTHROPIC_API_KEY").unwrap(), "sk-ant-123");
        assert!(!env.contains_key("CLAUDE_CODE_OAUTH_TOKEN"));
    }

    #[test]
    fn test_build_env_vars_subscription() {
        // Subscription auth uses ~/.claude/.credentials.json (written after
        // container creation), NOT an env var. So neither ANTHROPIC_API_KEY
        // nor CLAUDE_CODE_OAUTH_TOKEN should be in the container env.
        let env = WorkspaceSessionManager::build_env_vars(
            "http://localhost:3000",
            "test-token",
            Some("oauth-token-123"),
            "subscription",
        );

        assert!(!env.contains_key("CLAUDE_CODE_OAUTH_TOKEN"));
        assert!(!env.contains_key("ANTHROPIC_API_KEY"));
    }

    #[test]
    fn test_build_env_vars_no_ai_key() {
        let env = WorkspaceSessionManager::build_env_vars(
            "http://localhost:3000",
            "test-token",
            None,
            "api_key",
        );

        assert!(!env.contains_key("ANTHROPIC_API_KEY"));
        assert!(!env.contains_key("CLAUDE_CODE_OAUTH_TOKEN"));
        assert!(env.contains_key("TEMPS_API_URL"));
    }

    #[test]
    fn test_build_env_vars_includes_path() {
        let env = WorkspaceSessionManager::build_env_vars(
            "http://localhost:3000",
            "test-token",
            None,
            "api_key",
        );
        let path = env.get("PATH").expect("PATH should be set");
        assert!(
            path.starts_with("/workspace/.temps/bin:"),
            "memory script dir must come first in PATH (got: {})",
            path
        );
    }

    #[test]
    fn test_build_env_vars_with_workflow_includes_scope() {
        let env = WorkspaceSessionManager::build_env_vars_with_workflow(
            "http://localhost:3000",
            "test-token",
            Some("sk-ant-xxx"),
            "api_key",
            Some(42),
            Some("error-autofix"),
        );

        assert_eq!(env.get("TEMPS_PROJECT_ID").unwrap(), "42");
        assert_eq!(env.get("TEMPS_WORKFLOW_SLUG").unwrap(), "error-autofix");
        assert_eq!(env.get("ANTHROPIC_API_KEY").unwrap(), "sk-ant-xxx");
    }

    #[test]
    fn test_build_env_vars_with_workflow_omits_scope_when_none() {
        let env = WorkspaceSessionManager::build_env_vars_with_workflow(
            "http://localhost:3000",
            "test-token",
            None,
            "api_key",
            None,
            None,
        );

        assert!(!env.contains_key("TEMPS_PROJECT_ID"));
        assert!(!env.contains_key("TEMPS_WORKFLOW_SLUG"));
    }

    #[tokio::test]
    async fn test_inject_memory_script() {
        let manager = make_manager(false);
        manager
            .create_sandbox(
                1,
                10,
                PathBuf::from("/tmp/test"),
                HashMap::new(),
                None,
                None,
                None,
            )
            .await
            .unwrap();

        // FakeSandboxProvider returns success for any exec, so we just verify
        // the call doesn't error out.
        let result = manager.inject_memory_script(1).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_inject_skill_file() {
        let manager = make_manager(false);
        manager
            .create_sandbox(
                1,
                10,
                PathBuf::from("/tmp/test"),
                HashMap::new(),
                None,
                None,
                None,
            )
            .await
            .unwrap();

        // This should succeed — FakeSandboxProvider just returns success
        let result = manager.inject_skill_file(1).await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_skill_file_content() {
        // Verify the skill file has key sections
        assert!(TEMPS_PLATFORM_SKILL.contains("temps analytics"));
        assert!(TEMPS_PLATFORM_SKILL.contains("temps errors"));
        assert!(TEMPS_PLATFORM_SKILL.contains("temps services connect"));
        assert!(TEMPS_PLATFORM_SKILL.contains("temps deployments"));
        assert!(TEMPS_PLATFORM_SKILL.contains("temps monitoring"));
        assert!(TEMPS_PLATFORM_SKILL.contains("read-only"));
    }

    #[test]
    fn test_skill_file_has_memory_section() {
        // The memory section is critical — without it, the AI doesn't know
        // workflow memory exists and never uses it.
        assert!(TEMPS_PLATFORM_SKILL.contains("## Memory"));
        assert!(TEMPS_PLATFORM_SKILL.contains("memory write"));
        assert!(TEMPS_PLATFORM_SKILL.contains("memory search"));
        assert!(TEMPS_PLATFORM_SKILL.contains("memory list"));
        assert!(TEMPS_PLATFORM_SKILL.contains("memory supersede"));
        assert!(TEMPS_PLATFORM_SKILL.contains("Tags matter"));
    }

    #[test]
    fn test_build_chat_cmd_claude_first_message() {
        let manager = make_manager(false);
        let cmd = manager.build_chat_cmd("fix the bug", 25, false, "claude_cli");

        assert_eq!(cmd[0], "claude");
        assert_eq!(cmd[1], "--print");
        assert_eq!(cmd[2], "fix the bug");
        assert!(!cmd.contains(&"--continue".to_string()));
        assert!(cmd.contains(&"--dangerously-skip-permissions".to_string()));
    }

    #[test]
    fn test_build_chat_cmd_claude_continue() {
        let manager = make_manager(false);
        let cmd = manager.build_chat_cmd("follow up", 25, true, "claude_cli");

        assert_eq!(cmd[0], "claude");
        assert_eq!(cmd[1], "--print");
        assert_eq!(cmd[2], "--continue");
        assert_eq!(cmd[3], "follow up");
    }

    #[test]
    fn test_build_chat_cmd_codex() {
        let manager = make_manager(false);
        let cmd = manager.build_chat_cmd("do stuff", 25, false, "codex_cli");

        assert_eq!(cmd[0], "codex");
        assert!(cmd.contains(&"full-auto".to_string()));
    }

    #[test]
    fn test_build_chat_cmd_opencode() {
        let manager = make_manager(false);
        let cmd = manager.build_chat_cmd("help", 25, true, "opencode");

        assert_eq!(cmd[0], "opencode");
        assert!(cmd.contains(&"--continue".to_string()));
    }
}
