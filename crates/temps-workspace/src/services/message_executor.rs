use sea_orm::{DatabaseConnection, EntityTrait};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

use temps_agents::ai_cli::OnEventCallback;
use temps_core::EncryptionService;
use temps_deployments::services::deployment_token_service::{
    CreateDeploymentTokenRequest, DeploymentTokenService,
};
use temps_entities::{projects, settings, users};
use temps_git::services::git_provider_manager_trait::GitProviderManagerTrait;
use temps_providers::ExternalServiceManager;

use crate::error::WorkspaceError;
use crate::services::memory_service::WorkflowMemoryService;
use crate::services::session_manager::WorkspaceSessionManager;
use crate::services::workspace_service::{
    SendMessageRequest, UpdateSessionFields, WorkspaceService,
};

/// Executes chat messages within a workspace session.
///
/// Responsibilities:
/// - On first message: clone the project repo, create sandbox, inject skill
/// - Run the AI CLI with the user's prompt
/// - Stream AI output as assistant messages into workspace_messages
/// - Update the session's token/cost accounting
pub struct MessageExecutor {
    db: Arc<DatabaseConnection>,
    workspace_service: Arc<WorkspaceService>,
    session_manager: Arc<WorkspaceSessionManager>,
    git_provider_manager: Arc<dyn GitProviderManagerTrait>,
    encryption_service: Arc<EncryptionService>,
    deployment_token_service: Arc<DeploymentTokenService>,
    external_service_manager: Arc<ExternalServiceManager>,
    /// Optional memory service. When set, the executor pre-loads relevant
    /// workflow memory into the prompt before spawning the harness.
    /// Workspace chat sessions don't currently use this (they have no
    /// associated workflow), but the field is here so that future workflow-run
    /// executors can be wired up the same way.
    memory_service: Option<Arc<WorkflowMemoryService>>,
    /// Per-session execution locks. Ensures only one Claude CLI run is in
    /// flight per session at a time — concurrent `--continue` invocations
    /// race on the on-disk session state file and silently hang. Holding a
    /// per-session async Mutex serializes them so the second user message
    /// waits politely for the first to finish.
    session_locks: Arc<RwLock<HashMap<i32, Arc<Mutex<()>>>>>,
    /// Cancellation tokens for in-flight runs. Populated at the start of
    /// `execute_message`, removed at the end. The `cancel_run` handler
    /// fires the token to tell the exec loop to bail out early.
    active_runs: Arc<RwLock<HashMap<i32, CancellationToken>>>,
    /// Sessions whose claude jsonl may be in a dirty state (prior run was
    /// cancelled or timed out mid-turn). On the next message, we run a
    /// repair step before invoking `claude --continue`. Cleared on success.
    dirty_sessions: Arc<RwLock<HashSet<i32>>>,
    /// Sessions that currently have a drain loop running. Used to deduplicate
    /// `enqueue_run` calls — the second send_message just queues the message
    /// (already persisted by the handler) and returns; the running loop picks
    /// it up on its next iteration. Distinct from `active_runs` which holds
    /// the per-turn cancellation token.
    draining_sessions: Arc<RwLock<HashSet<i32>>>,
    /// Sessions whose drain loop should bail out at the next turn boundary.
    /// Set by `cancel`. The loop clears it when it exits.
    drain_cancel: Arc<RwLock<HashSet<i32>>>,
}

impl MessageExecutor {
    pub fn new(
        db: Arc<DatabaseConnection>,
        workspace_service: Arc<WorkspaceService>,
        session_manager: Arc<WorkspaceSessionManager>,
        git_provider_manager: Arc<dyn GitProviderManagerTrait>,
        encryption_service: Arc<EncryptionService>,
        deployment_token_service: Arc<DeploymentTokenService>,
        external_service_manager: Arc<ExternalServiceManager>,
    ) -> Self {
        Self {
            db,
            workspace_service,
            session_manager,
            git_provider_manager,
            encryption_service,
            deployment_token_service,
            external_service_manager,
            memory_service: None,
            session_locks: Arc::new(RwLock::new(HashMap::new())),
            active_runs: Arc::new(RwLock::new(HashMap::new())),
            dirty_sessions: Arc::new(RwLock::new(HashSet::new())),
            draining_sessions: Arc::new(RwLock::new(HashSet::new())),
            drain_cancel: Arc::new(RwLock::new(HashSet::new())),
        }
    }

    /// Cancel an in-flight run for this session. Called from the cancel
    /// handler. Fires the cancellation token (exec loop bails out on its
    /// next poll) and kicks off a best-effort process-tree kill in the
    /// sandbox. Also marks the session dirty so the next run repairs the
    /// jsonl before invoking --continue.
    pub async fn cancel(&self, session_id: i32) {
        // Tell the drain loop to bail out at the next turn boundary, so any
        // queued user messages are NOT processed. Cancel = stop everything.
        self.drain_cancel.write().await.insert(session_id);
        if let Some(token) = self.active_runs.read().await.get(&session_id).cloned() {
            token.cancel();
        }
        // Mark dirty so next message runs the jsonl repair step.
        self.dirty_sessions.write().await.insert(session_id);
        // SIGTERM first — give claude ~2s to flush the current turn to jsonl.
        self.session_manager
            .kill_session_processes(session_id, "^claude ", 15)
            .await;
        // Spawn the escalation-to-SIGKILL so we don't block the handler.
        let sm = self.session_manager.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            sm.kill_session_processes(session_id, "^claude ", 9).await;
        });
    }

    /// Mark a session's claude jsonl as dirty (pending repair on next run).
    /// Used by startup reconciliation for orphaned runs.
    pub async fn mark_dirty(&self, session_id: i32) {
        self.dirty_sessions.write().await.insert(session_id);
    }

    /// Find the most-recently-modified Claude CLI session jsonl inside the
    /// sandbox and repair it if needed. Claude stores session state at
    /// `~/.claude/projects/<encoded-workdir>/<session-id>.jsonl` — we list
    /// the project dir, pick the newest file, and run `repair_claude_jsonl`
    /// on its bytes. Writes back only if the repair changed anything.
    async fn repair_session_jsonl(&self, session_id: i32) -> Result<(), WorkspaceError> {
        // Claude encodes the workdir as a filename by replacing '/' with '-'.
        // Our sandbox workdir is /workspace, so the encoded path is "-workspace".
        const CLAUDE_PROJECTS_DIR: &str = "/home/temps/.claude/projects/-workspace";

        // List the directory and find the newest .jsonl. We run `ls -t` via
        // exec — small, bounded, no phantom hang risk.
        let list_cmd = vec![
            "sh".to_string(),
            "-c".to_string(),
            format!(
                "ls -t {}/*.jsonl 2>/dev/null | head -1",
                CLAUDE_PROJECTS_DIR
            ),
        ];
        let listing = self
            .session_manager
            .exec(session_id, list_cmd, HashMap::new(), None)
            .await?;
        let jsonl_path = listing.stdout.trim().to_string();
        if jsonl_path.is_empty() {
            // No session file yet — nothing to repair. This is normal for
            // a fresh session that was marked dirty before its first run.
            tracing::debug!(
                "repair_session_jsonl: no claude jsonl found for session {}",
                session_id
            );
            return Ok(());
        }

        let raw = self
            .session_manager
            .read_file(session_id, &jsonl_path)
            .await?;

        let (repaired, changed) = repair_claude_jsonl(&raw);
        if !changed {
            tracing::debug!(
                "repair_session_jsonl: session {} jsonl already clean",
                session_id
            );
            return Ok(());
        }

        self.session_manager
            .write_file(session_id, &jsonl_path, &repaired, 0o644)
            .await?;

        tracing::info!(
            "repair_session_jsonl: repaired {} for session {} ({} -> {} bytes)",
            jsonl_path,
            session_id,
            raw.len(),
            repaired.len()
        );
        Ok(())
    }

    /// Read the current git branch from inside the sandbox's `/workspace`.
    /// Returns `Ok(None)` if the dir is not a git repo or HEAD is detached.
    /// Best-effort: errors are logged and converted to `Ok(None)` so callers
    /// can use `.unwrap_or(None)` semantics without aborting their flow.
    pub async fn read_current_branch(&self, session_id: i32) -> Option<String> {
        if !self.session_manager.is_alive(session_id).await {
            return None;
        }
        let cmd = vec![
            "sh".to_string(),
            "-c".to_string(),
            "git -C /workspace rev-parse --abbrev-ref HEAD 2>/dev/null".to_string(),
        ];
        match self
            .session_manager
            .exec(session_id, cmd, HashMap::new(), None)
            .await
        {
            Ok(r) if r.exit_code == 0 => {
                let branch = r.stdout.trim().to_string();
                if branch.is_empty() || branch == "HEAD" {
                    None
                } else {
                    Some(branch)
                }
            }
            Ok(_) => None,
            Err(e) => {
                tracing::debug!(
                    "read_current_branch: exec failed for session {}: {}",
                    session_id,
                    e
                );
                None
            }
        }
    }

    /// Sync the cached `branch_name` on the session row to whatever
    /// `/workspace` HEAD currently points at. No-op if unchanged or unreadable.
    async fn sync_current_branch(&self, session_id: i32, cached: Option<&str>) {
        let current = self.read_current_branch(session_id).await;
        if current.as_deref() == cached {
            return;
        }
        if let Some(branch) = current {
            let _ = self
                .workspace_service
                .update_session(
                    session_id,
                    UpdateSessionFields {
                        branch_name: Some(branch),
                        ..Default::default()
                    },
                )
                .await;
        }
    }

    /// Get-or-create the per-session execution lock.
    async fn lock_for(&self, session_id: i32) -> Arc<Mutex<()>> {
        if let Some(lock) = self.session_locks.read().await.get(&session_id) {
            return lock.clone();
        }
        let mut w = self.session_locks.write().await;
        w.entry(session_id)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    /// Attach a workflow memory service so future runs can pre-load relevant
    /// memory into the prompt before spawning the AI harness.
    pub fn with_memory_service(mut self, memory: Arc<WorkflowMemoryService>) -> Self {
        self.memory_service = Some(memory);
        self
    }

    /// Build the full prompt for a chat turn, including any pre-loaded
    /// workflow memory. For workspace chat sessions (no agent_id), memory
    /// rendering is a no-op and the user message is returned as-is.
    pub(crate) async fn build_chat_prompt(
        &self,
        user_content: &str,
        is_first_message: bool,
        workflow_agent_id: Option<i32>,
        project_id: i32,
        relevant_tags: Vec<String>,
    ) -> String {
        build_chat_prompt_with_memory(
            self.memory_service.as_deref(),
            user_content,
            is_first_message,
            workflow_agent_id,
            project_id,
            relevant_tags,
        )
        .await
    }
}

/// Free-function variant of `build_chat_prompt` that takes the memory service
/// as a parameter. Easier to unit-test in isolation.
pub(crate) async fn build_chat_prompt_with_memory(
    memory_service: Option<&WorkflowMemoryService>,
    user_content: &str,
    is_first_message: bool,
    workflow_agent_id: Option<i32>,
    project_id: i32,
    relevant_tags: Vec<String>,
) -> String {
    // Memory is only injected on the first message of a session.
    // Subsequent messages use --continue and inherit the prior context.
    if !is_first_message {
        return user_content.to_string();
    }

    let memory_section = match (memory_service, workflow_agent_id) {
        (Some(svc), Some(agent_id)) => {
            let ctx = crate::services::memory_service::TriggerContext {
                project_id,
                agent_id,
                relevant_tags,
                limit: None,
            };
            match svc.render_for_prompt(&ctx).await {
                Ok(text) => text,
                Err(e) => {
                    tracing::warn!(
                        "Failed to render memory for prompt (agent={}): {}. Continuing without memory.",
                        agent_id,
                        e
                    );
                    String::new()
                }
            }
        }
        _ => String::new(),
    };

    if memory_section.is_empty() {
        user_content.to_string()
    } else {
        format!("{}\n## Current request\n{}", memory_section, user_content)
    }
}

impl MessageExecutor {
    /// Execute a user message end-to-end:
    /// 1. If sandbox not created yet → clone repo + create sandbox + inject skill
    /// 2. Run the AI CLI via session_manager
    /// 3. Stream output as assistant messages
    /// 4. Update token counts
    ///
    /// Drain all pending user messages for a session, one turn at a time.
    ///
    /// Called by the send_message handler after persisting a new user message.
    /// If a drain loop is already running for this session, this is a no-op
    /// (the running loop will pick up the new message on its next iteration).
    /// Otherwise, this spawns a loop that:
    ///   1. Reads all unprocessed user messages on the session
    ///   2. Concatenates them into a single prompt
    ///   3. Runs the AI CLI for one turn
    ///   4. Repeats until no more user messages are pending
    ///
    /// This is the queue-and-drain pattern used by every Claude wrapper —
    /// the input stays enabled while the assistant is thinking, queued
    /// messages stack on the session, and they get merged into the next turn.
    pub async fn enqueue_run(self: &Arc<Self>, session_id: i32) -> Result<(), WorkspaceError> {
        // Atomically check whether a drain loop is already running. If so,
        // the running loop will see the new message in its next DB query —
        // we have nothing to do. Otherwise install the sentinel and spawn
        // the loop. The sentinel is removed when the loop exits.
        {
            let mut draining = self.draining_sessions.write().await;
            if draining.contains(&session_id) {
                tracing::debug!(
                    "Drain loop already running for session {} — message queued",
                    session_id
                );
                return Ok(());
            }
            draining.insert(session_id);
        }
        // Clear any prior drain-cancel flag from a previous run.
        self.drain_cancel.write().await.remove(&session_id);

        let executor = self.clone();
        tokio::spawn(async move {
            if let Err(e) = executor.drain_loop(session_id).await {
                // Swallow errors that are really just user-initiated cancels.
                // The HTTP cancel_run handler already wrote a terminal
                // system+assistant turn for the user, so writing another pair
                // here would produce the duplicate "Run failed: ... cancelled
                // by user" cascade the UI showed.
                let was_cancelled = executor.drain_cancel.read().await.contains(&session_id);
                if was_cancelled {
                    tracing::debug!(
                        "Drain loop for session {} ended via cancel — suppressing error messages",
                        session_id
                    );
                } else {
                    tracing::error!(
                        "Workspace drain loop failed for session {}: {}",
                        session_id,
                        e
                    );
                    let detail = e.to_string();
                    let _ = executor
                        .workspace_service
                        .append_message(SendMessageRequest {
                            session_id,
                            role: "system".to_string(),
                            content: format!("Run failed: {}", detail),
                            metadata: None,
                        })
                        .await;
                    let _ = executor
                        .workspace_service
                        .append_message(SendMessageRequest {
                            session_id,
                            role: "assistant".to_string(),
                            content: format!("Run failed: {}", detail),
                            metadata: Some(serde_json::json!({
                                "error": true,
                                "error_kind": "execution_failed",
                                "detail": detail,
                            })),
                        })
                        .await;
                }
            }
            executor.draining_sessions.write().await.remove(&session_id);
            executor.drain_cancel.write().await.remove(&session_id);
        });

        Ok(())
    }

    /// The actual drain loop. Runs turns until no pending user messages
    /// remain. Each iteration concatenates all currently-pending user
    /// messages into one prompt — fewer turns, lower cost, matches user
    /// mental model of "I'm adding to my thought".
    async fn drain_loop(&self, session_id: i32) -> Result<(), WorkspaceError> {
        let mut last_processed_user_id: i64 = 0;
        loop {
            // Pull all user messages on this session newer than the last one
            // we processed. Filter out non-user roles to avoid re-running on
            // assistant/system messages we wrote ourselves.
            let pending = self
                .workspace_service
                .get_messages_after(session_id, last_processed_user_id)
                .await?;
            let pending_user: Vec<_> = pending.into_iter().filter(|m| m.role == "user").collect();
            if pending_user.is_empty() {
                return Ok(());
            }
            let max_id = pending_user.last().map(|m| m.id).unwrap_or(0);
            // Concatenate queued user messages with blank-line separators.
            let combined = pending_user
                .iter()
                .map(|m| m.content.as_str())
                .collect::<Vec<_>>()
                .join("\n\n");

            // Check for cancellation before each turn. `cancel` sets this
            // flag and the running execute_message will also bail via its
            // own per-turn cancellation token.
            if self.drain_cancel.read().await.contains(&session_id) {
                tracing::debug!("Drain loop cancelled for session {}", session_id);
                return Ok(());
            }

            self.execute_message(session_id, combined).await?;
            last_processed_user_id = max_id;
        }
    }

    /// Refresh a live sandbox in place: re-issues the deployment token,
    /// rewrites `~/.env` (linked services + git tokens + new TEMPS_API_TOKEN),
    /// re-injects the latest `temps-platform.md` skill, and re-installs git
    /// credentials. Does NOT recreate the container — the bind-mounted
    /// work_dir, the home volume, and any in-flight Claude conversation
    /// state are preserved.
    ///
    /// Use this when:
    ///   - The Temps binary has been upgraded and the embedded skill changed
    ///   - The deployment token is about to expire or has been rotated
    ///   - A linked service / git provider token has changed
    ///   - A new email domain was verified and the agent needs to know
    ///
    /// The container's *process env* (`TEMPS_API_TOKEN` set at create time)
    /// stays stale, but the rewritten `~/.env` overrides it whenever the
    /// agent runs `. ~/.env && <cmd>` per the documented procedure in
    /// `~/.claude/CLAUDE.md`. Application code that reads from process env
    /// directly will keep seeing the old token until the sandbox is
    /// recreated — that's a known limitation of in-place refresh.
    pub async fn refresh_sandbox(&self, session_id: i32) -> Result<(), WorkspaceError> {
        // Take the per-session lock so a refresh can't race with an
        // execute_message turn rewriting the same files.
        let lock = self.lock_for(session_id).await;
        let _guard = lock.lock().await;

        let session = self.workspace_service.get_session(session_id).await?;
        if session.status == "closed" {
            return Err(WorkspaceError::SessionNotActive {
                session_id,
                status: "closed".to_string(),
            });
        }
        if !self.session_manager.is_alive(session_id).await {
            return Err(WorkspaceError::SandboxNotAvailable { session_id });
        }

        let project = projects::Entity::find_by_id(session.project_id)
            .one(self.db.as_ref())
            .await?
            .ok_or(WorkspaceError::ProjectNotFound {
                project_id: session.project_id,
            })?;

        // Re-issue the deployment token. Old token is left to expire on its
        // own — we don't have a revoke path here.
        let session_token = self
            .issue_session_token(session.project_id, session.id)
            .await?;

        // Rebuild the managed env: linked services + git tokens + new
        // TEMPS_API_TOKEN + TEMPS_API_URL. Mirrors the build in
        // initialize_sandbox so the agent's `. ~/.env` view stays consistent.
        let mut managed_env: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();

        managed_env.insert("TEMPS_API_URL".to_string(), self.get_temps_api_url());
        managed_env.insert("TEMPS_API_TOKEN".to_string(), session_token);
        managed_env.insert(
            "TEMPS_PROJECT_ID".to_string(),
            session.project_id.to_string(),
        );

        match self
            .external_service_manager
            .get_project_service_environment_variables(session.project_id)
            .await
        {
            Ok(by_service) => {
                for (_service_id, vars) in by_service {
                    for (k, v) in vars {
                        managed_env.insert(k, v);
                    }
                }
            }
            Err(e) => {
                tracing::warn!(
                    "refresh_sandbox: failed to load linked-service env for project {}: {}",
                    session.project_id,
                    e
                );
            }
        }

        let mut git_creds: Option<(String, String)> = None;
        if let Some(connection_id) = project.git_provider_connection_id {
            match self
                .git_provider_manager
                .get_connection_access_token(connection_id)
                .await
            {
                Ok((token, provider_type)) => match provider_type.as_str() {
                    "github" => {
                        managed_env.insert("GH_TOKEN".to_string(), token.clone());
                        managed_env.insert("GITHUB_TOKEN".to_string(), token.clone());
                        git_creds = Some((token, "github".to_string()));
                    }
                    "gitlab" => {
                        managed_env.insert("GITLAB_TOKEN".to_string(), token.clone());
                        managed_env.insert("GL_TOKEN".to_string(), token.clone());
                        git_creds = Some((token, "gitlab".to_string()));
                    }
                    _ => {}
                },
                Err(e) => {
                    tracing::warn!(
                        "refresh_sandbox: failed to fetch git provider token for connection {}: {}",
                        connection_id,
                        e
                    );
                }
            }
        }

        let project_ctx = crate::services::session_manager::ProjectContext {
            id: project.id,
            slug: &project.slug,
            name: &project.name,
            repo_owner: &project.repo_owner,
            repo_name: &project.repo_name,
            branch: session
                .branch_name
                .as_deref()
                .or(session.base_branch_name.as_deref())
                .unwrap_or(project.main_branch.as_str()),
        };

        // Rewrite ~/.env + ~/.claude/CLAUDE.md, re-inject the platform skill,
        // re-install git credentials. All best-effort with logged warnings —
        // a single failed step shouldn't abort the others.
        if let Err(e) = self
            .session_manager
            .inject_env_file(session.id, &managed_env, Some(&project_ctx))
            .await
        {
            tracing::warn!("refresh_sandbox: inject_env_file failed: {}", e);
        }
        if let Err(e) = self.session_manager.inject_skill_file(session.id).await {
            tracing::warn!("refresh_sandbox: inject_skill_file failed: {}", e);
        }
        if let Err(e) = self
            .setup_git_credentials(session.id, session.user_id, git_creds.as_ref())
            .await
        {
            tracing::warn!("refresh_sandbox: setup_git_credentials failed: {}", e);
        }

        // Sync cached branch with actual /workspace HEAD.
        self.sync_current_branch(session.id, session.branch_name.as_deref())
            .await;

        // Surface a system message so the user sees the refresh happened.
        let _ = self
            .workspace_service
            .append_message(SendMessageRequest {
                session_id,
                role: "system".to_string(),
                content: "Sandbox refreshed: skill, env, and deployment token reloaded."
                    .to_string(),
                metadata: None,
            })
            .await;

        Ok(())
    }

    pub async fn execute_message(
        &self,
        session_id: i32,
        user_message_content: String,
    ) -> Result<(), WorkspaceError> {
        // Serialize per session — concurrent `claude --continue` invocations
        // race on the CLI's on-disk session state and silently hang. The
        // second sender waits here until the first run finishes.
        let lock = self.lock_for(session_id).await;
        let _guard = lock.lock().await;

        let session = self.workspace_service.get_session(session_id).await?;

        if session.status == "closed" {
            return Err(WorkspaceError::SessionNotActive {
                session_id,
                status: "closed".to_string(),
            });
        }

        // Check if sandbox exists for this session
        let sandbox_ready = self.session_manager.is_alive(session_id).await;

        // Defensive backfill: if the sandbox is already in-memory but the DB
        // row is missing the container id (e.g. an earlier run errored after
        // create_container but before update_session, or a server restart
        // adopted an existing container), persist it now so the UI stops
        // showing "not started".
        if sandbox_ready && session.sandbox_container_id.is_none() {
            if let Some(handle) = self.session_manager.get_handle(session_id).await {
                let _ = self
                    .workspace_service
                    .update_session(
                        session_id,
                        UpdateSessionFields {
                            sandbox_container_id: Some(handle.sandbox_id.clone()),
                            ..Default::default()
                        },
                    )
                    .await;
            }
        }

        if !sandbox_ready {
            // Surface a heartbeat event so the UI's "Thinking…" label tells
            // the user we're provisioning, not just sitting silent. Hard
            // wall-clock timeout on the whole setup pipeline so we never
            // hang the chat on a stuck git clone or stuck setup exec.
            let _ = self
                .workspace_service
                .append_message(SendMessageRequest {
                    session_id,
                    role: "ai_event".to_string(),
                    content: r#"{"type":"system","subtype":"setup","message":"Provisioning sandbox (clone repo, build container, inject skill files)…"}"#.to_string(),
                    metadata: None,
                })
                .await;

            const SETUP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);
            match tokio::time::timeout(SETUP_TIMEOUT, self.initialize_sandbox(&session)).await {
                Ok(r) => r?,
                Err(_) => {
                    return Err(WorkspaceError::SandboxCreationFailed {
                        session_id,
                        reason: format!(
                            "Sandbox provisioning exceeded {}s timeout",
                            SETUP_TIMEOUT.as_secs()
                        ),
                    });
                }
            }
        }

        // --- Pre-run hygiene ---
        //
        // 1. Zombie sweep: kill any leftover `claude` processes from a prior
        //    run that was force-killed or orphaned by a server restart. Two
        //    claudes writing to the same jsonl corrupt it. This is always
        //    safe — if there's nothing to kill, pkill is a no-op.
        self.session_manager
            .kill_session_processes(session_id, "^claude ", 9)
            .await;

        // 2. Repair pass: if this session is marked dirty (prior cancel or
        //    timeout), the on-disk claude jsonl may have a dangling tool_use
        //    or a truncated last line. Walk the file and fix it before the
        //    next --continue invocation. Best-effort: if anything goes wrong
        //    we clear the dirty flag and fall through to the --continue
        //    fallback below.
        let is_dirty = self.dirty_sessions.read().await.contains(&session_id);
        if is_dirty {
            if let Err(e) = self.repair_session_jsonl(session_id).await {
                tracing::warn!(
                    "repair_session_jsonl failed for session {}: {}. \
                     --continue may fail; will fall back to fresh session.",
                    session_id,
                    e
                );
            }
            self.dirty_sessions.write().await.remove(&session_id);
        }

        // Build the prompt — on first message we may inject workflow memory,
        // subsequent messages use --continue so we don't need prior history.
        let is_first = self.session_manager.is_first_message(session_id).await;

        // Workspace chat sessions don't have a workflow agent_id, so memory
        // injection is a no-op for them. Workflow runs (future code path)
        // will pass a real agent_id and tags here.
        let final_prompt = self
            .build_chat_prompt(
                &user_message_content,
                is_first,
                None, // workspace chat: no agent_id
                session.project_id,
                vec![], // no trigger-specific tags
            )
            .await;

        // Run claude with a fallback path: if --continue fails because the
        // jsonl is unrepairable (tool_use/tool_result mismatch), delete the
        // jsonl and retry once without --continue. User loses prior context
        // but at least gets an answer this turn.
        let (result, buffer) = self
            .run_claude_with_fallback(session_id, &final_prompt, !is_first, &session.ai_provider)
            .await;

        // Mark first message sent regardless of success
        self.session_manager
            .mark_first_message_sent(session_id)
            .await;

        match result {
            Ok(exec_result) => {
                // Save a final assistant summary message
                let full_output = buffer.lock().await.clone();
                let summary = extract_final_result(&full_output)
                    .unwrap_or_else(|| "(no result text)".to_string());

                self.workspace_service
                    .append_message(SendMessageRequest {
                        session_id,
                        role: "assistant".to_string(),
                        content: summary,
                        metadata: Some(serde_json::json!({
                            "exit_code": exec_result.exit_code,
                        })),
                    })
                    .await?;

                // Parse token usage from stream-json
                let (tokens_in, tokens_out) = parse_token_usage(&full_output);
                if tokens_in.is_some() || tokens_out.is_some() {
                    let _ = self
                        .workspace_service
                        .update_session(
                            session_id,
                            UpdateSessionFields {
                                tokens_input: Some(session.tokens_input + tokens_in.unwrap_or(0)),
                                tokens_output: Some(
                                    session.tokens_output + tokens_out.unwrap_or(0),
                                ),
                                ..Default::default()
                            },
                        )
                        .await;
                }

                // Sync the cached branch_name with whatever /workspace HEAD
                // points at now — the AI may have switched branches mid-turn.
                self.sync_current_branch(session_id, session.branch_name.as_deref())
                    .await;

                Ok(())
            }
            Err(e) => {
                // User-initiated cancels are handled by the HTTP cancel_run
                // handler, which writes a single terminal "Run cancelled by
                // user." system+assistant pair. Writing more messages here
                // produces the duplicate cascade the UI used to show.
                let is_cancel = self.drain_cancel.read().await.contains(&session_id)
                    || matches!(
                        &e,
                        WorkspaceError::AiCliFailed { reason, .. }
                            if reason == "Run cancelled by user"
                    );
                if is_cancel {
                    return Err(e);
                }

                // Save BOTH a system breadcrumb and an assistant-role message.
                // The assistant message is what the UI watches to clear its
                // "Thinking…" indicator — without it the spinner spins forever
                // on any executor failure.
                let error_text = format!("Error: {}", e);
                let _ = self
                    .workspace_service
                    .append_message(SendMessageRequest {
                        session_id,
                        role: "system".to_string(),
                        content: error_text.clone(),
                        metadata: None,
                    })
                    .await;
                let _ = self
                    .workspace_service
                    .append_message(SendMessageRequest {
                        session_id,
                        role: "assistant".to_string(),
                        content: error_text,
                        metadata: Some(serde_json::json!({
                            "error": true,
                            "error_kind": format!("{:?}", e).split('{').next().unwrap_or("Unknown").trim().to_string(),
                        })),
                    })
                    .await;
                Err(e)
            }
        }
    }

    /// Run claude once with the given `continue_conversation` flag. Returns
    /// the exec result plus the collected stdout buffer. Handles cancel +
    /// timeout + process-tree kill internally.
    async fn run_claude_once(
        &self,
        session_id: i32,
        prompt: &str,
        continue_conversation: bool,
        provider: &str,
    ) -> (
        Result<temps_agents::sandbox::SandboxExecResult, WorkspaceError>,
        Arc<Mutex<String>>,
    ) {
        let env = std::collections::HashMap::new();
        let cmd = self
            .session_manager
            .build_chat_cmd(prompt, 25, continue_conversation, provider);

        let buffer: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
        let workspace_service_for_callback = self.workspace_service.clone();

        let on_output: OnEventCallback = {
            let buffer = buffer.clone();
            Arc::new(move |line: String| {
                let buffer = buffer.clone();
                let ws_service = workspace_service_for_callback.clone();
                Box::pin(async move {
                    {
                        let mut b = buffer.lock().await;
                        b.push_str(&line);
                        b.push('\n');
                    }
                    let _ = ws_service
                        .append_message(SendMessageRequest {
                            session_id,
                            role: "ai_event".to_string(),
                            content: line,
                            metadata: None,
                        })
                        .await;
                })
            })
        };

        let cancel_token = CancellationToken::new();
        {
            self.active_runs
                .write()
                .await
                .insert(session_id, cancel_token.clone());
        }

        const EXEC_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10 * 60);
        let exec_fut = self
            .session_manager
            .exec(session_id, cmd, env, Some(on_output));

        let result = tokio::select! {
            biased;
            _ = cancel_token.cancelled() => {
                self.dirty_sessions.write().await.insert(session_id);
                Err(WorkspaceError::AiCliFailed {
                    session_id,
                    reason: "Run cancelled by user".to_string(),
                })
            }
            r = tokio::time::timeout(EXEC_TIMEOUT, exec_fut) => match r {
                Ok(inner) => inner,
                Err(_) => {
                    self.dirty_sessions.write().await.insert(session_id);
                    self.session_manager
                        .kill_session_processes(session_id, "^claude ", 15)
                        .await;
                    let sm = self.session_manager.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                        sm.kill_session_processes(session_id, "^claude ", 9).await;
                    });
                    Err(WorkspaceError::AiCliFailed {
                        session_id,
                        reason: format!(
                            "AI CLI run exceeded {}s timeout and was aborted",
                            EXEC_TIMEOUT.as_secs()
                        ),
                    })
                }
            }
        };

        self.active_runs.write().await.remove(&session_id);

        (result, buffer)
    }

    /// Run claude with a fallback: if the first attempt with `--continue`
    /// fails because the jsonl is corrupted (tool_use/tool_result mismatch),
    /// delete the jsonl and retry once without `--continue`. On the retry
    /// the user loses prior context but at least gets a response. Returns
    /// the final result + the buffer from whichever attempt succeeded
    /// (or the last failed attempt).
    async fn run_claude_with_fallback(
        &self,
        session_id: i32,
        prompt: &str,
        continue_conversation: bool,
        provider: &str,
    ) -> (
        Result<temps_agents::sandbox::SandboxExecResult, WorkspaceError>,
        Arc<Mutex<String>>,
    ) {
        let (first_result, first_buffer) = self
            .run_claude_once(session_id, prompt, continue_conversation, provider)
            .await;

        // Only consider a fallback if we actually tried to --continue. A
        // fresh-session run has nothing to fall back to.
        if !continue_conversation {
            return (first_result, first_buffer);
        }

        // Decide if this looks like a jsonl corruption error. Heuristics:
        //   - exec returned Ok but exit_code != 0, AND
        //   - stdout contains a known mismatch marker
        // We intentionally keep this conservative — false positives here
        // nuke the user's conversation history, which is worse than
        // surfacing the raw error.
        let is_mismatch = match &first_result {
            Ok(r) if r.exit_code != 0 => {
                let out = first_buffer.lock().await;
                let s = out.as_str();
                s.contains("tool_use") && (s.contains("tool_result") || s.contains("mismatch"))
                    || s.contains("unexpected `tool_use` block")
                    || s.contains("messages.0.content")
            }
            _ => false,
        };

        if !is_mismatch {
            return (first_result, first_buffer);
        }

        tracing::warn!(
            "run_claude_with_fallback: session {} jsonl appears corrupted, \
             deleting and retrying without --continue",
            session_id
        );

        // Tell the user what's happening so they understand why context is lost.
        let _ = self
            .workspace_service
            .append_message(SendMessageRequest {
                session_id,
                role: "system".to_string(),
                content: "Previous conversation state was corrupted by an interrupted run. \
                          Starting a fresh Claude session — prior context is lost."
                    .to_string(),
                metadata: None,
            })
            .await;

        // Nuke every .jsonl in the claude projects dir for this sandbox.
        // We don't know the exact filename, so shotgun-delete them all.
        self.session_manager
            .kill_session_processes(session_id, "^claude ", 9)
            .await;
        let delete_cmd = vec![
            "sh".to_string(),
            "-c".to_string(),
            "rm -f /home/temps/.claude/projects/-workspace/*.jsonl 2>/dev/null; exit 0".to_string(),
        ];
        let _ = self
            .session_manager
            .exec(session_id, delete_cmd, HashMap::new(), None)
            .await;

        // Second attempt: fresh session (no --continue).
        let (second_result, second_buffer) = self
            .run_claude_once(session_id, prompt, false, provider)
            .await;
        (second_result, second_buffer)
    }

    /// Eagerly provision the sandbox for a session if it isn't already
    /// running. Used on session start/reopen so the terminal tab has a
    /// live container to attach to without waiting for a first chat
    /// message. No-op if the sandbox is already alive.
    pub async fn ensure_sandbox(&self, session_id: i32) -> Result<(), WorkspaceError> {
        let lock = self.lock_for(session_id).await;
        let _guard = lock.lock().await;

        let session = self.workspace_service.get_session(session_id).await?;
        if session.status == "closed" {
            return Err(WorkspaceError::SessionNotActive {
                session_id,
                status: "closed".to_string(),
            });
        }
        if self.session_manager.is_alive(session_id).await {
            return Ok(());
        }

        const SETUP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);
        match tokio::time::timeout(SETUP_TIMEOUT, self.initialize_sandbox(&session)).await {
            Ok(r) => r,
            Err(_) => Err(WorkspaceError::SandboxCreationFailed {
                session_id,
                reason: format!(
                    "Sandbox provisioning exceeded {}s timeout",
                    SETUP_TIMEOUT.as_secs()
                ),
            }),
        }
    }

    /// Initialize the sandbox for this session: clone repo, create container, inject skill.
    async fn initialize_sandbox(
        &self,
        session: &temps_entities::workspace_sessions::Model,
    ) -> Result<(), WorkspaceError> {
        // Load the project
        let project = projects::Entity::find_by_id(session.project_id)
            .one(self.db.as_ref())
            .await?
            .ok_or(WorkspaceError::ProjectNotFound {
                project_id: session.project_id,
            })?;

        // Create temp work directory. The work_dir is bind-mounted into the
        // sandbox and persists across container recreation, so on reopen/retry
        // we want to *reuse* the existing checkout rather than re-cloning. A
        // dir is considered "already initialized" if it contains a `.git`
        // subdirectory — anything else (e.g. an empty leftover dir) is treated
        // as fresh and may be cloned into.
        let work_dir = std::env::temp_dir().join(format!("workspace-{}", session.id));
        tokio::fs::create_dir_all(&work_dir).await?;
        let work_dir_already_initialized = work_dir.join(".git").exists();

        // Clone the repo if git provider is configured. There are three cases:
        //   1. Neither branch_name nor base_branch_name set → clone project's main.
        //   2. branch_name set, no base → clone that branch directly (existing remote branch).
        //   3. base_branch_name + branch_name set → clone the base branch, then
        //      create a new local branch off it. The new branch only lives in
        //      the sandbox until something pushes it.
        let (branch_to_clone, new_branch_to_create) = match (
            session.base_branch_name.as_deref(),
            session.branch_name.as_deref(),
        ) {
            (Some(base), Some(new_branch)) => (base, Some(new_branch)),
            (None, Some(branch)) => (branch, None),
            (None, None) => (project.main_branch.as_str(), None),
            // base set without branch_name should have been rejected by
            // create_session validation; treat defensively as "use base".
            (Some(base), None) => (base, None),
        };
        if work_dir_already_initialized {
            tracing::info!(
                "Reusing existing work_dir for session {} ({}) — skipping git clone",
                session.id,
                work_dir.display()
            );
        } else if let Some(connection_id) = project.git_provider_connection_id {
            self.git_provider_manager
                .clone_repository(
                    connection_id,
                    &project.repo_owner,
                    &project.repo_name,
                    &work_dir,
                    Some(branch_to_clone),
                )
                .await
                .map_err(|e| WorkspaceError::SandboxCreationFailed {
                    session_id: session.id,
                    reason: format!("Git clone failed: {}", e),
                })?;

            // If we need to fork a new branch off the cloned base, do it now
            // before the sandbox container is created. The branch lives only
            // in the local clone.
            if let Some(new_branch) = new_branch_to_create {
                let work_dir_for_branch = work_dir.clone();
                let new_branch_owned = new_branch.to_string();
                tokio::task::spawn_blocking(move || {
                    temps_git::services::git_ops::create_and_checkout_branch_at(
                        &work_dir_for_branch,
                        &new_branch_owned,
                    )
                })
                .await
                .map_err(|e| WorkspaceError::SandboxCreationFailed {
                    session_id: session.id,
                    reason: format!("Branch creation task panicked: {}", e),
                })?
                .map_err(|e| WorkspaceError::SandboxCreationFailed {
                    session_id: session.id,
                    reason: format!("Could not create branch: {}", e),
                })?;
                tracing::info!(
                    "Created branch '{}' off '{}' for workspace session {}",
                    new_branch,
                    branch_to_clone,
                    session.id
                );
            }
        } else {
            // No git provider — write a placeholder README so the sandbox
            // has something to mount
            tokio::fs::write(
                work_dir.join("README.md"),
                format!(
                    "# {}\n\nNo git provider configured for this project.\n",
                    project.name
                ),
            )
            .await?;
        }

        // Issue a deployment token for this session so the sandbox can
        // call back to the Temps API (analytics, errors, deploys, etc.)
        let session_token = self
            .issue_session_token(session.project_id, session.id)
            .await?;

        // Build env vars to inject at container creation. Workspace chat
        // sessions don't have an associated workflow slug, so memory writes
        // from the chat sandbox will fail until we add a chat-scoped memory
        // model. Workflow runs use a different code path that DOES set the slug.
        let (api_key, auth_type) = self.resolve_ai_credentials().await?;
        let env_vars = WorkspaceSessionManager::build_env_vars_with_workflow(
            &self.get_temps_api_url(),
            &session_token,
            api_key.as_deref(),
            &auth_type,
            Some(session.project_id),
            None, // chat sessions: no workflow scope
        );

        // Create the sandbox with per-session resource overrides (when set
        // on the workspace_sessions row). Each is None → provider default.
        self.session_manager
            .create_sandbox(
                session.id,
                session.project_id,
                work_dir.clone(),
                env_vars,
                session.cpu_milli.map(|m| m as f32 / 1000.0),
                session.memory_limit_mb,
                session.pids_limit,
            )
            .await?;

        // Inject the Temps platform skill file
        let _ = self.session_manager.inject_skill_file(session.id).await;

        // Seed ~/.claude.json so the terminal's first `claude` launch
        // doesn't block on the onboarding/theme picker. Best-effort — a
        // failure here shouldn't abort sandbox creation.
        if let Err(e) = self.session_manager.seed_claude_config(session.id).await {
            tracing::warn!(
                "Failed to seed claude config for session {}: {}",
                session.id,
                e
            );
        }

        // Collect linked-service env vars + git provider tokens and write
        // them into `/root/.env`. A global `~/.claude/CLAUDE.md` is also
        // installed instructing the agent to source the file before any
        // command that needs credentials. This way we can refresh tokens
        // by rewriting the file — no container restart needed.
        let mut managed_env: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();

        // Linked external services (DATABASE_URL, REDIS_URL, ...)
        match self
            .external_service_manager
            .get_project_service_environment_variables(session.project_id)
            .await
        {
            Ok(by_service) => {
                for (_service_id, vars) in by_service {
                    for (k, v) in vars {
                        managed_env.insert(k, v);
                    }
                }
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to load linked-service env vars for project {}: {}",
                    session.project_id,
                    e
                );
            }
        }

        // Git provider token (so gh / glab can authenticate inside the sandbox).
        // We also remember the (token, provider) pair so we can wire it into
        // git's credential store + git config below — the AI shouldn't have
        // to source ~/.env or paste a token to push/pull.
        let mut git_creds: Option<(String, String)> = None;
        if let Some(connection_id) = project.git_provider_connection_id {
            match self
                .git_provider_manager
                .get_connection_access_token(connection_id)
                .await
            {
                Ok((token, provider_type)) => match provider_type.as_str() {
                    "github" => {
                        managed_env.insert("GH_TOKEN".to_string(), token.clone());
                        managed_env.insert("GITHUB_TOKEN".to_string(), token.clone());
                        git_creds = Some((token, "github".to_string()));
                    }
                    "gitlab" => {
                        managed_env.insert("GITLAB_TOKEN".to_string(), token.clone());
                        managed_env.insert("GL_TOKEN".to_string(), token.clone());
                        git_creds = Some((token, "gitlab".to_string()));
                    }
                    other => {
                        tracing::debug!(
                            "Unknown git provider type '{}' — skipping token injection",
                            other
                        );
                    }
                },
                Err(e) => {
                    tracing::warn!(
                        "Failed to fetch git provider token for connection {}: {}",
                        connection_id,
                        e
                    );
                }
            }
        }

        // Always inject the global CLAUDE.md (with project context) even
        // when there are no env vars to write — the agent still needs to
        // know which project this sandbox belongs to.
        let project_ctx = crate::services::session_manager::ProjectContext {
            id: project.id,
            slug: &project.slug,
            name: &project.name,
            repo_owner: &project.repo_owner,
            repo_name: &project.repo_name,
            branch: branch_to_clone,
        };

        if let Err(e) = self
            .session_manager
            .inject_env_file(session.id, &managed_env, Some(&project_ctx))
            .await
        {
            tracing::warn!("Failed to inject ~/.env into session {}: {}", session.id, e);
        }

        // Wire git config (user.name + user.email from the session owner) and
        // credentials so the AI can `git pull` / `git push` without ever
        // touching a token. We write `~/.git-credentials` with the provider
        // token and enable `credential.helper=store` so git picks it up
        // automatically — no env sourcing required.
        if let Err(e) = self
            .setup_git_credentials(session.id, session.user_id, git_creds.as_ref())
            .await
        {
            tracing::warn!(
                "Failed to set up git credentials for session {}: {}",
                session.id,
                e
            );
        }

        // Install the memory script. The script itself enforces that
        // TEMPS_WORKFLOW_SLUG is set before any memory operation, so chat
        // sessions get a clear error if the AI tries to use memory.
        let _ = self.session_manager.inject_memory_script(session.id).await;

        // Update session with sandbox container ID
        let handle = self.session_manager.get_handle(session.id).await;
        if let Some(handle) = handle {
            let _ = self
                .workspace_service
                .update_session(
                    session.id,
                    UpdateSessionFields {
                        sandbox_container_id: Some(handle.sandbox_id.clone()),
                        work_dir: Some(work_dir.to_string_lossy().to_string()),
                        ..Default::default()
                    },
                )
                .await;
        }

        tracing::info!("Initialized sandbox for workspace session {}", session.id);
        Ok(())
    }

    /// Configure git inside the sandbox so the AI can pull/push without
    /// having to know about tokens.
    ///
    /// What this writes:
    /// 1. `git config --global user.email` and `user.name` — set to the
    ///    session owner's email/name so commits are attributed correctly.
    /// 2. `~/.git-credentials` containing
    ///    `https://x-access-token:<token>@github.com` (or gitlab.com).
    /// 3. `git config --global credential.helper store` so git auto-loads
    ///    the credentials file for any HTTPS push/pull.
    ///
    /// The token is the SAME one already in `~/.env` (`GH_TOKEN` /
    /// `GITLAB_TOKEN`) — we just put it where git looks for it natively so
    /// no `. ~/.env &&` prefix is needed for `git` commands.
    async fn setup_git_credentials(
        &self,
        session_id: i32,
        user_id: i32,
        git_creds: Option<&(String, String)>,
    ) -> Result<(), WorkspaceError> {
        // Look up the session owner's name + email for git config.
        let user = users::Entity::find_by_id(user_id)
            .one(self.db.as_ref())
            .await?;

        // Build a single bash command that sets everything up. Using one
        // exec keeps the round-trip count low and atomic from the AI's POV.
        // We single-quote the values and escape embedded single quotes via
        // the standard `'\''` shell idiom — same as inject_env_file.
        let shell_quote = |v: &str| v.replace('\'', "'\\''");

        let mut script = String::from("set -e\n");
        script.push_str("mkdir -p /home/temps\n");

        if let Some(u) = user.as_ref() {
            script.push_str(&format!(
                "git config --global user.email '{}'\n",
                shell_quote(&u.email)
            ));
            script.push_str(&format!(
                "git config --global user.name '{}'\n",
                shell_quote(&u.name)
            ));
        }

        // Sensible defaults regardless of provider.
        script.push_str("git config --global init.defaultBranch main\n");
        script.push_str("git config --global pull.rebase false\n");

        if let Some((token, provider)) = git_creds {
            let host = match provider.as_str() {
                "github" => "github.com",
                "gitlab" => "gitlab.com",
                _ => "github.com",
            };
            // x-access-token is the conventional username for token auth on
            // both GitHub and GitLab — the token is the password.
            script.push_str(&format!(
                "umask 077 && printf 'https://x-access-token:%s@%s\\n' '{}' '{}' > /home/temps/.git-credentials\n",
                shell_quote(token),
                host,
            ));
            script.push_str("git config --global credential.helper store\n");
            // Force HTTPS even if the AI tries to clone via SSH — we don't
            // ship SSH keys into the sandbox.
            script.push_str(&format!(
                "git config --global url.'https://{host}/'.insteadOf 'git@{host}:'\n",
                host = host,
            ));
        }

        // Make sure everything is owned by the sandbox user.
        script.push_str("chown -R temps:temps /home/temps/.git-credentials /home/temps/.gitconfig 2>/dev/null || true\n");

        let cmd = vec!["sh".to_string(), "-c".to_string(), script];
        self.session_manager
            .exec(session_id, cmd, HashMap::new(), None)
            .await?;

        tracing::debug!(
            "Configured git for session {} (user_email={}, has_token={})",
            session_id,
            user.as_ref().map(|u| u.email.as_str()).unwrap_or("<none>"),
            git_creds.is_some()
        );
        Ok(())
    }

    /// Resolve AI provider credentials from the global settings table.
    async fn resolve_ai_credentials(&self) -> Result<(Option<String>, String), WorkspaceError> {
        let settings_row = settings::Entity::find_by_id(1)
            .one(self.db.as_ref())
            .await?;

        if let Some(settings_row) = settings_row {
            if let Some(sandbox_config) = settings_row.data.get("agent_sandbox") {
                let auth_type = sandbox_config
                    .get("auth_type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("api_key")
                    .to_string();

                let api_key_encrypted = sandbox_config
                    .get("api_key_encrypted")
                    .and_then(|v| v.as_str());

                if let Some(encrypted) = api_key_encrypted {
                    match self.encryption_service.decrypt(encrypted) {
                        Ok(plain_bytes) => {
                            let plain = String::from_utf8(plain_bytes).map_err(|e| {
                                WorkspaceError::Validation {
                                    message: format!("Decrypted key is not valid UTF-8: {}", e),
                                }
                            })?;
                            return Ok((Some(plain), auth_type));
                        }
                        Err(e) => {
                            tracing::warn!("Failed to decrypt AI provider key: {}", e);
                        }
                    }
                }

                return Ok((None, auth_type));
            }
        }

        Ok((None, "api_key".to_string()))
    }

    fn get_temps_api_url(&self) -> String {
        std::env::var("TEMPS_INTERNAL_API_URL")
            .unwrap_or_else(|_| "http://host.docker.internal:3000".to_string())
    }

    /// Issue a deployment token scoped to this project for the workspace session.
    /// This is the token the sandbox uses to authenticate back to the Temps API
    /// (e.g. for `temps errors list`, `temps analytics`, etc).
    async fn issue_session_token(
        &self,
        project_id: i32,
        session_id: i32,
    ) -> Result<String, WorkspaceError> {
        use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
        use temps_entities::deployment_tokens;

        let token_name = format!("workspace-session-{}", session_id);

        // Drop any pre-existing token with this name so refresh/recreate cycles
        // don't trip the unique-name conflict in create_token. Best-effort: if
        // the lookup or delete fails we'll surface the original Conflict from
        // create_token below.
        if let Ok(Some(existing)) = deployment_tokens::Entity::find()
            .filter(deployment_tokens::Column::ProjectId.eq(project_id))
            .filter(deployment_tokens::Column::Name.eq(&token_name))
            .one(self.db.as_ref())
            .await
        {
            if let Err(e) = self
                .deployment_token_service
                .delete_token(project_id, existing.id)
                .await
            {
                tracing::warn!(
                    "Failed to delete stale workspace session token {} for project {}: {}",
                    existing.id,
                    project_id,
                    e
                );
            }
        }

        let request = CreateDeploymentTokenRequest {
            name: token_name,
            environment_id: None,
            deployment_id: None,
            permissions: Some(vec!["*".to_string()]),
            expires_at: None,
        };

        let response = self
            .deployment_token_service
            .create_token(project_id, None, request)
            .await
            .map_err(|e| WorkspaceError::SandboxCreationFailed {
                session_id,
                reason: format!("Failed to issue deployment token: {}", e),
            })?;

        Ok(response.token)
    }
}

/// Extract the final result text from Claude stream-json output.
/// Returns the content of the `{"type":"result","result":"..."}` line if present.
fn extract_final_result(output: &str) -> Option<String> {
    for line in output.lines().rev() {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
            if value.get("type").and_then(|v| v.as_str()) == Some("result") {
                if let Some(result) = value.get("result").and_then(|v| v.as_str()) {
                    return Some(result.to_string());
                }
            }
        }
    }
    None
}

/// Parse cumulative token usage from a stream-json output buffer.
fn parse_token_usage(output: &str) -> (Option<i32>, Option<i32>) {
    let mut input_tokens: Option<i32> = None;
    let mut output_tokens: Option<i32> = None;

    for line in output.lines() {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
            if let Some(usage) = value
                .get("usage")
                .or_else(|| value.get("message").and_then(|m| m.get("usage")))
            {
                if let Some(n) = usage.get("input_tokens").and_then(|v| v.as_i64()) {
                    input_tokens = Some(n as i32);
                }
                if let Some(n) = usage.get("output_tokens").and_then(|v| v.as_i64()) {
                    output_tokens = Some(n as i32);
                }
            }
        }
    }

    (input_tokens, output_tokens)
}

/// Repair a potentially-corrupted Claude CLI session jsonl.
///
/// Claude persists conversation state to `~/.claude/projects/<hash>/<id>.jsonl`,
/// one JSON object per line. When we SIGKILL claude mid-turn, the file can
/// end with:
///   (a) a truncated last line (partial JSON) — drop it
///   (b) an `assistant` turn containing a `tool_use` block with no matching
///       `tool_result` on a later line — inject a synthetic tool_result
///
/// Both cases make `claude --continue` fail with "tool_use/tool_result
/// mismatch" or a JSON parse error. This function reads the raw bytes,
/// walks line by line, and rewrites the file if needed.
///
/// Returns true if the file was modified, false if it was already clean.
pub(crate) fn repair_claude_jsonl(raw: &[u8]) -> (Vec<u8>, bool) {
    let text = match std::str::from_utf8(raw) {
        Ok(s) => s,
        Err(_) => {
            // Non-UTF8 garbage — bail out, leave file alone. Caller will
            // likely fall back to fresh session on the next --continue error.
            return (raw.to_vec(), false);
        }
    };

    // Split into lines, drop trailing empty/malformed ones.
    let mut valid: Vec<serde_json::Value> = Vec::new();
    let mut had_trailing_garbage = false;
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<serde_json::Value>(line) {
            Ok(v) => valid.push(v),
            Err(_) => {
                // Partial write — discard this line and anything after it.
                had_trailing_garbage = true;
                break;
            }
        }
    }

    // Scan the valid lines and look for dangling tool_use blocks (assistant
    // turn with a tool_use whose id never appears as a tool_result).
    //
    // We collect all tool_use ids, then all tool_result ids, and diff.
    let mut tool_use_ids: Vec<String> = Vec::new();
    let mut tool_result_ids: std::collections::HashSet<String> = std::collections::HashSet::new();

    for entry in &valid {
        collect_tool_use_ids(entry, &mut tool_use_ids);
        collect_tool_result_ids(entry, &mut tool_result_ids);
    }

    let dangling: Vec<String> = tool_use_ids
        .into_iter()
        .filter(|id| !tool_result_ids.contains(id))
        .collect();

    let needs_rewrite = had_trailing_garbage || !dangling.is_empty();
    if !needs_rewrite {
        return (raw.to_vec(), false);
    }

    // For each dangling tool_use, append a synthetic user-turn tool_result
    // marking it as an interrupted run. Claude's conversation format expects
    // tool_results to appear in a subsequent user message.
    for tool_use_id in dangling {
        let synthetic = serde_json::json!({
            "type": "user",
            "message": {
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": tool_use_id,
                    "content": "Run cancelled by user before tool finished.",
                    "is_error": true,
                }]
            }
        });
        valid.push(synthetic);
    }

    let mut out = String::new();
    for entry in &valid {
        // serde_json::to_string never fails on a Value we just parsed.
        if let Ok(line) = serde_json::to_string(entry) {
            out.push_str(&line);
            out.push('\n');
        }
    }

    (out.into_bytes(), true)
}

fn collect_tool_use_ids(entry: &serde_json::Value, out: &mut Vec<String>) {
    // Walk any `content` array looking for `type: "tool_use"` blocks.
    if let Some(message) = entry.get("message") {
        if let Some(content) = message.get("content").and_then(|c| c.as_array()) {
            for block in content {
                if block.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                    if let Some(id) = block.get("id").and_then(|i| i.as_str()) {
                        out.push(id.to_string());
                    }
                }
            }
        }
    }
}

fn collect_tool_result_ids(entry: &serde_json::Value, out: &mut std::collections::HashSet<String>) {
    if let Some(message) = entry.get("message") {
        if let Some(content) = message.get("content").and_then(|c| c.as_array()) {
            for block in content {
                if block.get("type").and_then(|t| t.as_str()) == Some("tool_result") {
                    if let Some(id) = block.get("tool_use_id").and_then(|i| i.as_str()) {
                        out.insert(id.to_string());
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_final_result() {
        let output = r#"{"type":"system","model":"claude-sonnet"}
{"type":"assistant","message":{"content":"thinking..."}}
{"type":"result","result":"The fix is to rename foo to bar","duration_ms":5000}
"#;
        let result = extract_final_result(output);
        assert_eq!(result, Some("The fix is to rename foo to bar".to_string()));
    }

    #[test]
    fn test_extract_final_result_missing() {
        let output = r#"{"type":"system"}
{"type":"assistant"}
"#;
        assert_eq!(extract_final_result(output), None);
    }

    #[test]
    fn test_extract_final_result_empty() {
        assert_eq!(extract_final_result(""), None);
    }

    #[test]
    fn test_parse_token_usage_top_level() {
        let output = r#"{"type":"result","usage":{"input_tokens":100,"output_tokens":50}}"#;
        let (input, output_t) = parse_token_usage(output);
        assert_eq!(input, Some(100));
        assert_eq!(output_t, Some(50));
    }

    #[test]
    fn test_parse_token_usage_nested_in_message() {
        let output =
            r#"{"type":"assistant","message":{"usage":{"input_tokens":200,"output_tokens":75}}}"#;
        let (input, output_t) = parse_token_usage(output);
        assert_eq!(input, Some(200));
        assert_eq!(output_t, Some(75));
    }

    #[test]
    fn test_parse_token_usage_none() {
        let output = r#"{"type":"system","model":"claude"}"#;
        let (input, output_t) = parse_token_usage(output);
        assert_eq!(input, None);
        assert_eq!(output_t, None);
    }

    #[test]
    fn test_parse_token_usage_takes_last() {
        let output = r#"{"usage":{"input_tokens":10,"output_tokens":5}}
{"usage":{"input_tokens":20,"output_tokens":15}}"#;
        let (input, output_t) = parse_token_usage(output);
        assert_eq!(input, Some(20));
        assert_eq!(output_t, Some(15));
    }

    // ── build_chat_prompt_with_memory ────────────────────────────────────────
    //
    // We test the free-function variant directly so we don't need to spin up
    // the full MessageExecutor with all its dependencies. The MessageExecutor
    // method just delegates to this function.

    use sea_orm::{DatabaseBackend, MockDatabase};
    use temps_entities::workflow_memory;

    fn mock_memory_service(facts: Vec<workflow_memory::Model>) -> Arc<WorkflowMemoryService> {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![facts])
            .into_connection();
        Arc::new(WorkflowMemoryService::new(Arc::new(db)))
    }

    fn make_test_fact(id: i64, fact: &str) -> workflow_memory::Model {
        let now = chrono::Utc::now();
        workflow_memory::Model {
            id,
            project_id: 10,
            agent_id: 5,
            fact: fact.to_string(),
            tags: serde_json::json!([]),
            confidence: 0.9,
            times_used: 0,
            source_run_ids: serde_json::json!([]),
            superseded_by: None,
            created_at: now,
            updated_at: now,
            last_used_at: None,
        }
    }

    #[tokio::test]
    async fn test_build_chat_prompt_no_memory_service_returns_user_content() {
        let result = build_chat_prompt_with_memory(None, "hello", true, Some(5), 10, vec![]).await;
        assert_eq!(result, "hello");
    }

    #[tokio::test]
    async fn test_build_chat_prompt_no_agent_id_returns_user_content() {
        let memory = mock_memory_service(vec![]);
        let result = build_chat_prompt_with_memory(
            Some(&memory),
            "hello",
            true,
            None, // no agent
            10,
            vec![],
        )
        .await;
        assert_eq!(result, "hello");
    }

    #[tokio::test]
    async fn test_build_chat_prompt_subsequent_message_skips_memory() {
        // Even if memory has facts, is_first_message=false skips them.
        let memory = mock_memory_service(vec![make_test_fact(1, "should not appear")]);
        let result = build_chat_prompt_with_memory(
            Some(&memory),
            "follow-up",
            false, // not first message
            Some(5),
            10,
            vec![],
        )
        .await;
        assert_eq!(result, "follow-up");
    }

    #[tokio::test]
    async fn test_build_chat_prompt_first_message_with_memory_includes_section() {
        let memory = mock_memory_service(vec![make_test_fact(1, "OAuth state cookie missing")]);

        let result = build_chat_prompt_with_memory(
            Some(&memory),
            "fix the bug",
            true,
            Some(5),
            10,
            vec!["error_group_id:42".to_string()],
        )
        .await;

        assert!(
            result.contains("Things you've learned"),
            "memory section should be present"
        );
        assert!(result.contains("OAuth state cookie missing"));
        assert!(result.contains("## Current request"));
        assert!(result.contains("fix the bug"));
        // The memory section should come BEFORE the user request
        let memory_pos = result.find("Things you've learned").unwrap();
        let request_pos = result.find("## Current request").unwrap();
        assert!(memory_pos < request_pos);
    }

    #[tokio::test]
    async fn test_build_chat_prompt_empty_memory_returns_user_content() {
        // Memory service is set but the load query returns no rows.
        let memory = mock_memory_service(vec![]);
        let result =
            build_chat_prompt_with_memory(Some(&memory), "hello", true, Some(5), 10, vec![]).await;
        // No memory rows → no section → user content as-is
        assert_eq!(result, "hello");
    }
}
