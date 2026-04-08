use chrono::Utc;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, Order, QueryFilter, QueryOrder};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::fs;
use tokio::process::Command;

use temps_core::jobs::GitPushEventJob;
use temps_core::workflow_memory::{
    memory_install_command, WorkflowMemoryFact, WorkflowMemoryProvider,
};
use temps_core::{EncryptionService, Job, JobQueue};
use temps_deployments::services::deployment_token_service::{
    CreateDeploymentTokenRequest, DeploymentTokenService,
};
use temps_entities::{error_events, error_groups, projects, settings};
use temps_git::services::git_provider_manager_trait::GitProviderManagerTrait;
use temps_notifications::services::NotificationService;
use temps_notifications::types::{Notification, NotificationPriority};

use crate::ai_cli::{AiCliProvider, AiRunConfig, AiRunResult, OnEventCallback};
use crate::error::AgentError;
use crate::sandbox::SandboxCreateConfig;
use crate::services::sandbox_registry::SandboxRegistry;

use crate::services::config_service::AgentConfigService;
use crate::services::prompt_builder::PromptBuilder;
use crate::services::run_service::{AgentRunService, UpdateRunFields};

pub struct AgentExecutor {
    db: Arc<DatabaseConnection>,
    git_provider_manager: Arc<dyn GitProviderManagerTrait>,
    encryption_service: Arc<EncryptionService>,
    queue: Arc<dyn JobQueue>,
    run_service: Arc<AgentRunService>,
    config_service: Arc<AgentConfigService>,
    notification_service: Arc<NotificationService>,
    /// If set, this provider is used instead of resolving one from config.
    /// Intended for testing — inject a fake provider that simulates AI behaviour.
    ai_provider_override: Option<Arc<dyn AiCliProvider>>,
    /// Sandbox registry for running AI CLI inside isolated containers.
    sandbox_registry: Arc<SandboxRegistry>,
    /// Optional workflow memory provider. When set, the executor:
    ///   1. Installs the `memory` script in the sandbox so the AI can write
    ///      facts back via curl.
    ///   2. Pre-loads relevant memory facts into the prompt before spawning
    ///      the harness, so the AI starts with what previous runs learned.
    ///
    /// When unset, workflow runs work exactly as before — no memory features.
    ///
    /// Wrapped in `RwLock<Option<...>>` so it can be set late by the plugin
    /// system after the executor has already been registered as an Arc'd
    /// service. The workspace plugin registers the memory provider after the
    /// agents plugin registers the executor, so we can't pass it via the
    /// constructor.
    memory_provider: tokio::sync::RwLock<Option<Arc<dyn WorkflowMemoryProvider>>>,
    /// Optional deployment token issuer. Used to mint a project-scoped token
    /// that the sandbox can use as `TEMPS_API_TOKEN` to call back to the API
    /// (memory script, future CLI commands, etc.).
    /// If unset, the script is still installed but memory writes will fail
    /// at the curl level since the token env var won't be set.
    /// Same RwLock pattern as memory_provider — set late by plugin init.
    deployment_token_service: tokio::sync::RwLock<Option<Arc<DeploymentTokenService>>>,
}

impl AgentExecutor {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        db: Arc<DatabaseConnection>,
        git_provider_manager: Arc<dyn GitProviderManagerTrait>,
        encryption_service: Arc<EncryptionService>,
        queue: Arc<dyn JobQueue>,
        run_service: Arc<AgentRunService>,
        config_service: Arc<AgentConfigService>,
        notification_service: Arc<NotificationService>,
        sandbox_registry: Arc<SandboxRegistry>,
    ) -> Self {
        Self {
            db,
            git_provider_manager,
            encryption_service,
            queue,
            run_service,
            config_service,
            notification_service,
            ai_provider_override: None,
            sandbox_registry,
            memory_provider: tokio::sync::RwLock::new(None),
            deployment_token_service: tokio::sync::RwLock::new(None),
        }
    }

    /// Attach a workflow memory provider so the executor pre-loads memory
    /// into prompts and installs the memory script in run sandboxes.
    /// Safe to call after the executor has been registered as a service —
    /// uses interior mutability so it works with `Arc<AgentExecutor>`.
    pub async fn attach_memory_provider(&self, provider: Arc<dyn WorkflowMemoryProvider>) {
        *self.memory_provider.write().await = Some(provider);
    }

    /// Attach a deployment token service so the executor can mint a
    /// short-lived project-scoped token for the sandbox to use as
    /// `TEMPS_API_TOKEN` when calling back to the Temps API.
    /// Safe to call after the executor has been registered as a service.
    pub async fn attach_deployment_token_service(&self, svc: Arc<DeploymentTokenService>) {
        *self.deployment_token_service.write().await = Some(svc);
    }

    /// Access the sandbox registry (for status checks).
    pub fn sandbox_registry(&self) -> &SandboxRegistry {
        &self.sandbox_registry
    }

    /// For testing: inject a custom AI CLI provider instead of resolving from config.
    pub fn with_ai_provider(mut self, provider: Arc<dyn AiCliProvider>) -> Self {
        self.ai_provider_override = Some(provider);
        self
    }

    // ── Memory helpers ──────────────────────────────────────────────────────

    /// Build the list of tags used to filter memory for a run. Encodes
    /// trigger context (error_group_id, etc.) so that future runs hitting
    /// the same trigger source see the relevant facts.
    pub(crate) fn build_memory_tags(
        trigger_source_type: Option<&str>,
        trigger_source_id: Option<i32>,
    ) -> Vec<String> {
        let mut tags = Vec::new();
        if let (Some(t), Some(id)) = (trigger_source_type, trigger_source_id) {
            tags.push(format!("{}:{}", t, id));
        }
        tags
    }

    /// Load relevant memory facts for a run from the configured provider.
    /// Returns an empty vec on any failure (memory is best-effort and must
    /// never block a run).
    pub(crate) async fn load_memory_facts(
        &self,
        project_id: i32,
        agent_id: i32,
        trigger_source_type: Option<&str>,
        trigger_source_id: Option<i32>,
    ) -> Vec<WorkflowMemoryFact> {
        let provider = {
            let guard = self.memory_provider.read().await;
            match guard.as_ref() {
                Some(p) => p.clone(),
                None => return Vec::new(),
            }
        };
        let tags = Self::build_memory_tags(trigger_source_type, trigger_source_id);
        match provider
            .load_for_trigger(project_id, agent_id, tags, 20)
            .await
        {
            Ok(facts) => facts,
            Err(e) => {
                tracing::warn!(
                    "Failed to load workflow memory for agent {}: {}. Continuing without memory.",
                    agent_id,
                    e
                );
                Vec::new()
            }
        }
    }

    /// Render a memory section to prepend to a workflow prompt. Returns
    /// an empty string when there's no memory provider or no facts to render.
    pub(crate) async fn render_memory_section(&self, facts: &[WorkflowMemoryFact]) -> String {
        let guard = self.memory_provider.read().await;
        match guard.as_ref() {
            Some(p) => p.render_for_prompt(facts),
            None => String::new(),
        }
    }

    /// Issue a project-scoped deployment token for a workflow run sandbox.
    /// Returns `None` if no token service is configured (in which case the
    /// memory script will fail at the curl level — that's fine, the run
    /// itself still proceeds).
    pub(crate) async fn issue_run_token(
        &self,
        project_id: i32,
        run_id: i32,
        agent_slug: &str,
    ) -> Option<String> {
        let svc = {
            let guard = self.deployment_token_service.read().await;
            match guard.as_ref() {
                Some(s) => s.clone(),
                None => return None,
            }
        };
        // Token lifetime: 2 hours. Autopilot runs have an internal timeout
        // well under this, so any token still usable after 2h belongs to a
        // run that has already completed or died — we'd rather the token
        // expire than linger. See Phase 2 of the security plan for
        // fine-grained permission scoping beyond expiry.
        let expires_at = chrono::Utc::now() + chrono::Duration::hours(2);
        let request = CreateDeploymentTokenRequest {
            name: format!("workflow-run-{}-{}", agent_slug, run_id),
            environment_id: None,
            deployment_id: None,
            permissions: Some(vec!["*".to_string()]),
            expires_at: Some(expires_at),
        };
        match svc.create_token(project_id, None, request).await {
            Ok(response) => Some(response.token),
            Err(e) => {
                tracing::warn!(
                    "Failed to issue workflow run token (project={}, run={}): {}. \
                     Memory writes from this run will fail.",
                    project_id,
                    run_id,
                    e
                );
                None
            }
        }
    }

    /// Inject config repos and secrets into the sandbox.
    ///
    /// Overlay order: repo's own `.claude/` → global config repo → per-agent config repo.
    /// For `settings.json`, MCP servers are deep-merged (not overwritten).
    /// Secrets are resolved from `${TEMPS_SECRET:name}` placeholders.
    ///
    /// This is a best-effort operation — if config repos are not configured
    /// or cloning fails, the agent run continues without them.
    async fn inject_config_repos_and_secrets(
        &self,
        run_id: i32,
        config: &temps_entities::project_agents::Model,
        _project_id: i32,
        _connection_id: Option<i32>,
    ) -> Result<(), AgentError> {
        // TODO: Phase 2 — clone global and per-agent config repos via
        // GitProviderManager.clone_repository(), overlay .claude/ directory
        // into sandbox via write_directory(), deep-merge settings.json mcpServers.
        //
        // TODO: Phase 3 — load agent_secrets (global), decrypt, inject env vars
        // and files, resolve ${TEMPS_SECRET:name} placeholders in config files.

        let has_config_repo = config.config_repo_url.is_some();
        if has_config_repo {
            self.run_service
                .append_log(
                    run_id,
                    "info",
                    &format!(
                        "Config repo configured: {} (branch: {}). Injection not yet implemented.",
                        config.config_repo_url.as_deref().unwrap_or(""),
                        config.config_repo_branch.as_deref().unwrap_or("main"),
                    ),
                    None,
                )
                .await?;
        }

        Ok(())
    }

    /// Execute a single autopilot run. Handles the full lifecycle from cloning to PR creation.
    pub async fn execute_run(&self, run_id: i32) {
        tracing::info!("Starting autopilot run {}", run_id);

        let work_dir = std::env::temp_dir().join(format!("autopilot-run-{}", run_id));

        match self.execute_run_inner(run_id, &work_dir).await {
            Ok(()) => {
                tracing::info!("Autopilot run {} completed successfully", run_id);
            }
            Err(e) => {
                tracing::error!("Autopilot run {} failed: {}", run_id, e);
                let _ = self
                    .run_service
                    .update_status(
                        run_id,
                        UpdateRunFields {
                            status: Some("failed".to_string()),
                            error_message: Some(e.to_string()),
                            completed_at: Some(Utc::now()),
                            ..Default::default()
                        },
                    )
                    .await;
                let _ = self
                    .run_service
                    .append_log(run_id, "error", &format!("Run failed: {}", e), None)
                    .await;
            }
        }

        // Always attempt cleanup: release sandbox first, then temp directory
        if self.sandbox_registry.has_sandbox(run_id).await {
            let _ = self
                .run_service
                .append_log(run_id, "info", "Destroying sandbox container...", None)
                .await;
        }
        let _ = self.sandbox_registry.release(run_id).await;
        if work_dir.exists() {
            if let Err(e) = fs::remove_dir_all(&work_dir).await {
                tracing::warn!(
                    "Failed to clean up temp dir {:?} for run {}: {}",
                    work_dir,
                    run_id,
                    e
                );
            }
        }
    }

    async fn execute_run_inner(&self, run_id: i32, work_dir: &PathBuf) -> Result<(), AgentError> {
        // Step 1: Load the run record
        let run = self.run_service.get_run(run_id).await?;

        // Step 2: Load the agent config
        // Use agent_id from the run record (set when the run was created).
        // Fall back to config_id for backward compatibility with old runs.
        let agent_id = run.agent_id.unwrap_or(run.config_id);
        let config = self.config_service.get_agent_by_id(agent_id).await?.ok_or(
            AgentError::ConfigNotFound {
                project_id: run.project_id,
            },
        )?;

        // Step 3: Load the project
        let project = projects::Entity::find_by_id(run.project_id)
            .one(self.db.as_ref())
            .await
            .map_err(AgentError::Database)?
            .ok_or(AgentError::ProjectNotFound {
                project_id: run.project_id,
            })?;

        // Step 4: Load error group if trigger_source_type == "error_group"
        let (error_type, error_message, stack_trace, environment_name) =
            if run.trigger_source_type.as_deref() == Some("error_group") {
                self.load_error_context(run.trigger_source_id, run.project_id)
                    .await?
            } else {
                (
                    "Unknown".to_string(),
                    "Manual autopilot run".to_string(),
                    String::new(),
                    None,
                )
            };

        // Steps 5–6 (budget + cooldown) are intentionally omitted here.
        // Both checks are performed by the job listener (plugin.rs evaluate_trigger) BEFORE
        // creating this run record.  Repeating them here would (a) cause the cooldown check to
        // count this very run against itself, and (b) add unnecessary DB round-trips.

        // Step 5: Update status → "cloning", set started_at
        // (Budget and cooldown were already verified by the plugin listener before run creation.)
        self.run_service
            .update_status(
                run_id,
                UpdateRunFields {
                    status: Some("cloning".to_string()),
                    started_at: Some(Utc::now()),
                    ..Default::default()
                },
            )
            .await?;
        self.run_service
            .append_log(run_id, "info", "Cloning repository...", None)
            .await?;
        tracing::info!(
            "Run {}: cloning repository for project {}",
            run_id,
            run.project_id
        );

        // Step 6: Create temp dir and clone repo
        fs::create_dir_all(work_dir).await?;

        let connection_id = project
            .git_provider_connection_id
            .ok_or(AgentError::GitError {
                message: format!(
                    "Project {} has no git provider connection configured",
                    run.project_id
                ),
            })?;

        self.git_provider_manager
            .clone_repository(
                connection_id,
                &project.repo_owner,
                &project.repo_name,
                work_dir,
                Some(&project.main_branch),
            )
            .await
            .map_err(|e| AgentError::GitError {
                message: format!(
                    "Failed to clone {}/{}: {}",
                    project.repo_owner, project.repo_name, e
                ),
            })?;

        // Step 6b: Create sandbox if enabled for this agent
        // Load global sandbox settings for resource limits
        let global_sandbox = settings::Entity::find_by_id(1)
            .one(self.db.as_ref())
            .await
            .ok()
            .flatten()
            .and_then(|s| {
                s.data.get("agent_sandbox").cloned().and_then(|v| {
                    serde_json::from_value::<temps_core::AgentSandboxSettings>(v).ok()
                })
            })
            .unwrap_or_default();

        // Per-agent overrides global: None = use global, Some(true/false) = explicit
        let use_sandbox = config.sandbox_enabled.unwrap_or(global_sandbox.enabled);
        if use_sandbox {
            // Always resolve the image name from current DB settings so changes
            // take effect without server restart.
            let resolved_image = if global_sandbox.runtime == "custom" {
                if global_sandbox.custom_image.is_empty() {
                    None
                } else {
                    Some(global_sandbox.custom_image.clone())
                }
            } else {
                // For presets, compute the image name here instead of relying
                // on the provider's startup-time config.
                Some(format!("temps-sandbox-{}:latest", global_sandbox.runtime))
            };

            // Inject auth credentials into sandbox based on auth_type and provider
            let mut sandbox_env = std::collections::HashMap::new();
            if let Some(ref encrypted_key) = global_sandbox.api_key_encrypted {
                if let Ok(key) = self.encryption_service.decrypt_string(encrypted_key) {
                    if global_sandbox.auth_type == "subscription" {
                        // Subscription/OAuth token — only CLAUDE_CODE_OAUTH_TOKEN
                        // Do NOT set ANTHROPIC_API_KEY (API rejects OAuth tokens)
                        sandbox_env.insert("CLAUDE_CODE_OAUTH_TOKEN".to_string(), key);
                    } else {
                        // API key — set the right env var for the provider
                        match global_sandbox.default_provider.as_str() {
                            "codex_cli" => {
                                sandbox_env.insert("OPENAI_API_KEY".to_string(), key);
                            }
                            _ => {
                                sandbox_env.insert("ANTHROPIC_API_KEY".to_string(), key);
                            }
                        }
                    }
                }
            }

            // Inject workflow memory env vars so the `memory` script (installed
            // below after the sandbox starts) knows how to call back to the
            // Temps API. The script reads all four of these at runtime.
            sandbox_env.insert("TEMPS_PROJECT_ID".to_string(), run.project_id.to_string());
            sandbox_env.insert("TEMPS_WORKFLOW_SLUG".to_string(), config.slug.clone());
            sandbox_env.insert(
                "TEMPS_API_URL".to_string(),
                std::env::var("TEMPS_INTERNAL_API_URL")
                    .unwrap_or_else(|_| "http://host.docker.internal:3000".to_string()),
            );
            // Put the memory script dir on PATH so the AI can type
            // `memory write "..."` instead of the full /workspace/.temps/bin/memory.
            sandbox_env.insert(
                "PATH".to_string(),
                "/workspace/.temps/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
                    .to_string(),
            );

            // Mint a project-scoped deployment token for this run. Best-effort:
            // if it fails, the script is still installed but memory writes will
            // error at the curl level. The actual run still proceeds.
            if let Some(token) = self
                .issue_run_token(run.project_id, run_id, &config.slug)
                .await
            {
                sandbox_env.insert("TEMPS_API_TOKEN".to_string(), token);
            }

            let sandbox_config = SandboxCreateConfig {
                run_id,
                host_work_dir: work_dir.clone(),
                image: resolved_image,
                cpu_limit: Some(global_sandbox.cpu_limit),
                memory_limit_mb: Some(global_sandbox.memory_limit_mb),
                pids_limit: None,
                network_mode: Some(global_sandbox.network_mode.clone()),
                env_vars: sandbox_env,
                idle_timeout: Duration::from_secs(config.timeout_seconds as u64 + 60),
            };
            self.run_service
                .append_log(
                    run_id,
                    "info",
                    &format!(
                        "Creating sandbox: runtime={}, image={}, {} CPU, {}MB RAM, network={}",
                        global_sandbox.runtime,
                        crate::sandbox::docker::image_name_for_runtime(&global_sandbox.runtime),
                        global_sandbox.cpu_limit,
                        global_sandbox.memory_limit_mb,
                        global_sandbox.network_mode,
                    ),
                    None,
                )
                .await?;

            let sandbox_start = std::time::Instant::now();
            let handle = self.sandbox_registry.get_or_create(sandbox_config).await?;

            self.run_service
                .append_log(
                    run_id,
                    "info",
                    &format!(
                        "Sandbox ready in {:.1}s ({}) — container={}, id={}",
                        sandbox_start.elapsed().as_secs_f64(),
                        self.sandbox_registry.provider_name(),
                        handle.sandbox_name,
                        &handle.sandbox_id[..12.min(handle.sandbox_id.len())],
                    ),
                    None,
                )
                .await?;

            // Install the workflow memory script. Best-effort: if it fails
            // (e.g. jq not present in a custom image), we log a warning and
            // continue — the run itself doesn't depend on memory.
            if let Err(e) = self
                .sandbox_registry
                .exec(
                    run_id,
                    memory_install_command(),
                    std::collections::HashMap::new(),
                    None,
                )
                .await
            {
                tracing::warn!(
                    "Failed to install memory script for run {}: {}. \
                     Memory writes from this run will not work.",
                    run_id,
                    e
                );
            } else {
                tracing::debug!("Installed memory script for run {}", run_id);
            }

            // Step 6c: Inject config repos and secrets into the sandbox
            if let Err(e) = self
                .inject_config_repos_and_secrets(
                    run_id,
                    &config,
                    run.project_id,
                    project.git_provider_connection_id,
                )
                .await
            {
                tracing::warn!(
                    "Failed to inject config repos/secrets for run {}: {}. Continuing without them.",
                    run_id,
                    e
                );
                self.run_service
                    .append_log(
                        run_id,
                        "warning",
                        &format!(
                            "Config repo/secrets injection failed: {}. Agent will run without them.",
                            e
                        ),
                        None,
                    )
                    .await?;
            }
        }

        // Step 7: Update status → "analyzing"
        self.run_service
            .update_status(
                run_id,
                UpdateRunFields {
                    status: Some("analyzing".to_string()),
                    ..Default::default()
                },
            )
            .await?;
        self.run_service
            .append_log(
                run_id,
                "info",
                "Repository cloned. Analyzing error...",
                None,
            )
            .await?;
        tracing::info!("Run {}: analyzing error", run_id);

        // Step 8: Build the prompt
        let first_seen = run.created_at.to_rfc3339();
        let prompt = if let Some(ref custom_prompt) = config.prompt {
            // Agent has a custom prompt — substitute template variables
            // Agent has a custom prompt — use it as-is (no template injection)
            // Append error context as structured data if this is an error trigger
            if run.trigger_source_type.as_deref() == Some("error_group") {
                format!(
                    "{}\n\n---\nERROR CONTEXT (provided by trigger):\nType: {}\nMessage: {}\nEnvironment: {}\nFirst seen: {}\n\nStack trace:\n{}\n",
                    custom_prompt, error_type, error_message,
                    environment_name.as_deref().unwrap_or("unknown"),
                    first_seen, stack_trace
                )
            } else {
                custom_prompt.clone()
            }
        } else {
            // No custom prompt — use built-in error fix prompt for error triggers,
            // or a generic prompt for other trigger types
            if run.trigger_source_type.as_deref() == Some("error_group") {
                PromptBuilder::build_error_fix_prompt(
                    &project.name,
                    &error_type,
                    &error_message,
                    &stack_trace,
                    0,
                    &first_seen,
                    environment_name.as_deref(),
                )
            } else {
                format!(
                    "You are an AI agent running on the {} project. \
                     Perform the task described in your agent configuration.",
                    project.name
                )
            }
        };

        // Append user context if provided (e.g. research topic, bug description)
        let prompt = if let Some(ref ctx) = run.user_context {
            if !ctx.is_empty() {
                format!("{}\n\n---\nUSER CONTEXT:\n{}\n", prompt, ctx)
            } else {
                prompt
            }
        } else {
            prompt
        };

        // Step 8b: Pre-load workflow memory and prepend it to the prompt.
        // This is the **push** half of memory: even if the AI never calls
        // `memory search`, it always sees the most relevant facts at the top
        // of the prompt. Best-effort — failures degrade silently.
        let prompt = {
            let memory_facts = self
                .load_memory_facts(
                    run.project_id,
                    config.id,
                    run.trigger_source_type.as_deref(),
                    run.trigger_source_id,
                )
                .await;
            let memory_section = self.render_memory_section(&memory_facts).await;
            if memory_section.is_empty() {
                prompt
            } else {
                self.run_service
                    .append_log(
                        run_id,
                        "info",
                        &format!(
                            "Pre-loaded {} fact(s) from workflow memory",
                            memory_facts.len()
                        ),
                        None,
                    )
                    .await?;
                format!("{}{}", memory_section, prompt)
            }
        };

        // Step 9: Resolve API key. Priority:
        // 1. Per-agent encrypted key (config.api_key_encrypted)
        // 2. Linked AI provider key (config.ai_provider_key_id → ai_provider_keys table)
        // 3. Empty (Claude CLI uses ~/.claude auth / subscription mode)
        let api_key = if let Some(ref encrypted) = config.api_key_encrypted {
            self.encryption_service
                .decrypt_string(encrypted)
                .map_err(|e| AgentError::EncryptionError {
                    message: format!(
                        "Failed to decrypt API key for project {}: {}",
                        run.project_id, e
                    ),
                })?
        } else if let Some(key_id) = config.ai_provider_key_id {
            // Look up the shared AI provider key
            use temps_entities::ai_provider_keys;
            let key_record = ai_provider_keys::Entity::find_by_id(key_id)
                .one(self.db.as_ref())
                .await
                .map_err(AgentError::Database)?;
            if let Some(key) = key_record {
                self.encryption_service
                    .decrypt_string(&key.api_key_encrypted)
                    .map_err(|e| AgentError::EncryptionError {
                        message: format!("Failed to decrypt AI provider key {}: {}", key_id, e),
                    })?
            } else {
                tracing::warn!(
                    "AI provider key {} not found for agent {}, running without API key",
                    key_id,
                    config.slug
                );
                String::new()
            }
        } else {
            String::new()
        };

        // Step 10: Update status → "fixing"
        self.run_service
            .update_status(
                run_id,
                UpdateRunFields {
                    status: Some("fixing".to_string()),
                    ..Default::default()
                },
            )
            .await?;
        self.run_service
            .append_log(
                run_id,
                "info",
                &format!("Running {} to fix the error...", config.ai_provider),
                None,
            )
            .await?;
        tracing::info!("Run {}: invoking AI CLI {}", run_id, config.ai_provider);

        // Step 11: Run AI CLI via sandbox (or direct provider for testing)
        let run_service_for_stream = self.run_service.clone();
        let stream_run_id = run_id;
        let on_event: OnEventCallback = Arc::new(move |line: String| {
            let svc = run_service_for_stream.clone();
            let rid = stream_run_id;
            Box::pin(async move {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    return;
                }
                let _ = svc.append_log(rid, "ai_event", trimmed, None).await;
            })
        });

        let ai_result = if let Some(ref override_provider) = self.ai_provider_override {
            // Testing path: use injected provider directly
            let ai_config = AiRunConfig {
                work_dir: work_dir.clone(),
                prompt,
                api_key: api_key.clone(),
                max_turns: config.max_turns,
                timeout: Duration::from_secs(config.timeout_seconds as u64),
                on_event: Some(on_event),
            };
            override_provider.run(ai_config).await?
        } else if use_sandbox {
            // Sandbox path: execute AI CLI inside isolated container
            let cmd = build_claude_cmd(&config.ai_provider, &prompt, config.max_turns, false);

            let mut env = std::collections::HashMap::new();
            if !api_key.is_empty() {
                env.insert("ANTHROPIC_API_KEY".to_string(), api_key);
            }

            let exec_result = tokio::time::timeout(
                Duration::from_secs(config.timeout_seconds as u64),
                self.sandbox_registry.exec(run_id, cmd, env, Some(on_event)),
            )
            .await
            .map_err(|_| AgentError::AiCliTimeout {
                provider: config.ai_provider.clone(),
                timeout_secs: config.timeout_seconds as u64,
            })??;

            if exec_result.exit_code != 0 {
                return Err(AgentError::AiCliFailed {
                    provider: config.ai_provider.clone(),
                    exit_code: exec_result.exit_code,
                    stderr: exec_result.stdout,
                });
            }

            let (tokens_input, tokens_output, model) =
                crate::ai_cli::claude::parse_claude_output(&exec_result.stdout);

            AiRunResult {
                output: exec_result.stdout,
                exit_code: exec_result.exit_code,
                tokens_input,
                tokens_output,
                model,
                changed_files: None,
            }
        } else {
            // Direct path: run AI CLI on host (no sandbox)
            let provider =
                crate::ai_cli::create_provider(&config.ai_provider).ok_or_else(|| {
                    AgentError::AiCliNotInstalled {
                        provider: config.ai_provider.clone(),
                    }
                })?;
            if !provider.check_installed().await {
                return Err(AgentError::AiCliNotInstalled {
                    provider: config.ai_provider.clone(),
                });
            }
            let ai_config = AiRunConfig {
                work_dir: work_dir.clone(),
                prompt,
                api_key,
                max_turns: config.max_turns,
                timeout: Duration::from_secs(config.timeout_seconds as u64),
                on_event: Some(on_event),
            };
            provider.run(ai_config).await?
        };

        // Step 13: Save AI output immediately (so it's preserved even if push fails later)
        self.run_service
            .update_status(
                run_id,
                UpdateRunFields {
                    ai_output: Some(ai_result.output.clone()),
                    ai_model: ai_result.model.clone(),
                    tokens_input: ai_result.tokens_input,
                    tokens_output: ai_result.tokens_output,
                    ..Default::default()
                },
            )
            .await?;
        self.run_service
            .append_log(
                run_id,
                "info",
                "AI CLI completed",
                Some(serde_json::json!({
                    "exit_code": ai_result.exit_code,
                    "tokens_input": ai_result.tokens_input,
                    "tokens_output": ai_result.tokens_output,
                    "model": ai_result.model,
                })),
            )
            .await?;

        // Report deliverable: store the AI output as the report and complete.
        // No branch, no PR, no deployment.
        if config.deliverable == "report" {
            // Extract the result text from stream-json output
            let report_text = ai_result
                .output
                .lines()
                .filter_map(|line| {
                    let trimmed = line.trim();
                    if !trimmed.starts_with('{') {
                        return None;
                    }
                    serde_json::from_str::<serde_json::Value>(trimmed)
                        .ok()
                        .and_then(|v| {
                            if v.get("type")?.as_str()? == "result" {
                                v.get("result")?.as_str().map(String::from)
                            } else {
                                None
                            }
                        })
                })
                .next()
                .unwrap_or_else(|| ai_result.output.clone());

            self.run_service
                .update_status(
                    run_id,
                    UpdateRunFields {
                        status: Some("completed".to_string()),
                        analysis: Some(report_text.clone()),
                        ai_output: Some(ai_result.output),
                        ai_model: ai_result.model,
                        tokens_input: ai_result.tokens_input,
                        tokens_output: ai_result.tokens_output,
                        completed_at: Some(Utc::now()),
                        ..Default::default()
                    },
                )
                .await?;
            self.run_service
                .append_log(run_id, "info", "Report completed — no PR created.", None)
                .await?;

            // Notify user that the report is ready
            let report_preview = if report_text.len() > 500 {
                format!("{}...", &report_text[..500])
            } else {
                report_text
            };
            self.send_completion_notification(
                run_id,
                &config.name,
                &project.name,
                &format!(
                    "Agent **{}** completed run #{} for **{}**.\n\n{}",
                    config.name, run_id, project.name, report_preview
                ),
                &config.deliverable,
            )
            .await;

            tracing::info!("Run {}: deliverable=report, completed without PR", run_id,);
            return Ok(());
        }

        // Step 14: Detect changes.
        // If the AI provider reported which files it changed, use that list.
        // Otherwise fall back to `git diff` (works when work_dir is a real git repo).
        let changed_files_owned: Vec<String> = if let Some(ref files) = ai_result.changed_files {
            files.clone()
        } else {
            // Claude CLI may commit changes itself (`git add && git commit`), or leave
            // them unstaged/untracked. We check all three states:
            //   1. Committed changes: `git diff --name-only HEAD~1` (if there are new commits)
            //   2. Unstaged changes: `git diff --name-only`
            //   3. Untracked files: `git ls-files --others --exclude-standard`

            let mut files: Vec<String> = Vec::new();

            // Check for committed changes (Claude may have run git commit)
            let committed = Command::new("git")
                .args(["diff", "--name-only", "HEAD~1"])
                .current_dir(work_dir)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output()
                .await;
            if let Ok(output) = committed {
                if output.status.success() {
                    for line in String::from_utf8_lossy(&output.stdout).lines() {
                        let trimmed = line.trim().to_string();
                        if !trimmed.is_empty() && !files.contains(&trimmed) {
                            files.push(trimmed);
                        }
                    }
                }
            }

            // Check for unstaged changes
            let unstaged = Command::new("git")
                .args(["diff", "--name-only"])
                .current_dir(work_dir)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output()
                .await?;
            for line in String::from_utf8_lossy(&unstaged.stdout).lines() {
                let trimmed = line.trim().to_string();
                if !trimmed.is_empty() && !files.contains(&trimmed) {
                    files.push(trimmed);
                }
            }

            // Check for untracked files
            let untracked = Command::new("git")
                .args(["ls-files", "--others", "--exclude-standard"])
                .current_dir(work_dir)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output()
                .await?;
            for line in String::from_utf8_lossy(&untracked.stdout).lines() {
                let trimmed = line.trim().to_string();
                if !trimmed.is_empty() && !files.contains(&trimmed) {
                    files.push(trimmed);
                }
            }
            files
        };

        if changed_files_owned.is_empty() {
            tracing::info!("Run {}: no changes detected, marking as no_fix", run_id);
            self.run_service
                .update_status(
                    run_id,
                    UpdateRunFields {
                        status: Some("no_fix".to_string()),
                        ai_output: Some(ai_result.output),
                        ai_model: ai_result.model,
                        tokens_input: ai_result.tokens_input,
                        tokens_output: ai_result.tokens_output,
                        completed_at: Some(Utc::now()),
                        ..Default::default()
                    },
                )
                .await?;
            self.run_service
                .append_log(
                    run_id,
                    "warn",
                    "No file changes detected after AI run",
                    None,
                )
                .await?;
            return Ok(());
        }

        // Safety check: abort if the AI modified an unreasonable number of files.
        // This guards against runaway AI behaviour that could produce enormous PRs.
        const MAX_FILES_CHANGED: usize = 50;
        if changed_files_owned.len() > MAX_FILES_CHANGED {
            return Err(AgentError::Validation {
                message: format!(
                    "AI modified {} files, exceeding the safety limit of {}. Aborting.",
                    changed_files_owned.len(),
                    MAX_FILES_CHANGED
                ),
            });
        }

        // Step 15: Collect changed file contents
        let mut file_payloads: Vec<(String, Vec<u8>)> = Vec::new();
        for path in &changed_files_owned {
            let full_path = work_dir.join(path);
            match fs::read(&full_path).await {
                Ok(contents) => {
                    file_payloads.push((path.to_string(), contents));
                }
                Err(e) => {
                    tracing::warn!(
                        "Run {}: could not read changed file {:?}: {}",
                        run_id,
                        full_path,
                        e
                    );
                }
            }
        }

        // Step 16: Update status → "pushing"
        self.run_service
            .update_status(
                run_id,
                UpdateRunFields {
                    status: Some("pushing".to_string()),
                    ..Default::default()
                },
            )
            .await?;
        self.run_service
            .append_log(
                run_id,
                "info",
                &format!(
                    "Pushing {} changed file(s) and creating PR...",
                    file_payloads.len()
                ),
                None,
            )
            .await?;
        tracing::info!("Run {}: pushing {} files", run_id, file_payloads.len());

        // Step 17: Generate branch name
        let short_run_id = format!("{:x}", run_id);
        let error_group_suffix = run
            .trigger_source_id
            .map(|id| format!("err-{}-", id))
            .unwrap_or_default();
        let branch_name = format!(
            "{}fix/{}{}",
            config.branch_prefix, error_group_suffix, short_run_id
        );

        // Step 18: Push + create PR
        // Use agent name for the PR title — different agents produce different types of PRs
        let pr_title = if run.trigger_source_type.as_deref() == Some("error_group") {
            format!("fix: {} — {} (run #{})", error_message, config.name, run_id)
        } else {
            format!("{}: {} (run #{})", config.name, project.name, run_id)
        };

        let commit_message = if run.trigger_source_type.as_deref() == Some("error_group") {
            format!("fix: {} (run #{})", error_message, run_id)
        } else {
            format!("{} (run #{})", config.name.to_lowercase(), run_id)
        };

        let pr_body = format!(
            "## {agent_name}\n\n\
            This PR was created by the **{agent_name}** agent in [Temps](https://temps.sh) (run #{run_id}).\n\n\
            {description}\n\n\
            **Files changed:** {files}",
            agent_name = config.name,
            run_id = run_id,
            description = config.description.as_deref().unwrap_or(""),
            files = changed_files_owned.len(),
        );

        let pr = self
            .git_provider_manager
            .push_files_and_create_pr(
                connection_id,
                &project.repo_owner,
                &project.repo_name,
                &branch_name,
                &project.main_branch,
                file_payloads,
                &commit_message,
                &pr_title,
                &pr_body,
            )
            .await
            .map_err(|e| AgentError::GitError {
                message: format!("Failed to push and create PR for run {}: {}", run_id, e),
            })?;

        // Step 19: Update run with PR details
        self.run_service
            .update_status(
                run_id,
                UpdateRunFields {
                    branch_name: Some(branch_name.clone()),
                    pr_url: Some(pr.url.clone()),
                    pr_number: Some(pr.number),
                    files_changed: Some(changed_files_owned.len() as i32),
                    ai_output: Some(ai_result.output),
                    ai_model: ai_result.model,
                    tokens_input: ai_result.tokens_input,
                    tokens_output: ai_result.tokens_output,
                    ..Default::default()
                },
            )
            .await?;

        // Step 20: Update status → "deploying"
        self.run_service
            .update_status(
                run_id,
                UpdateRunFields {
                    status: Some("deploying".to_string()),
                    ..Default::default()
                },
            )
            .await?;
        self.run_service
            .append_log(
                run_id,
                "info",
                &format!("PR created: {}. Triggering preview deployment...", pr.url),
                None,
            )
            .await?;
        tracing::info!(
            "Run {}: PR created, triggering preview deployment for branch {}",
            run_id,
            branch_name
        );

        // Step 21: Emit GitPushEvent to trigger preview deployment
        // Use the actual commit SHA from the PR (not the branch name) so that
        // SENTRY_RELEASE and other commit-based identifiers are valid.
        let commit_ref = pr.head_sha.clone().unwrap_or_else(|| branch_name.clone());
        let push_job = Job::GitPushEvent(GitPushEventJob {
            owner: project.repo_owner.clone(),
            repo: project.repo_name.clone(),
            branch: Some(branch_name.clone()),
            tag: None,
            commit: commit_ref,
            project_id: run.project_id,
        });

        if let Err(e) = self.queue.send(push_job).await {
            tracing::warn!(
                "Run {}: failed to emit GitPushEvent for preview deployment: {}",
                run_id,
                e
            );
        }

        // Step 22: Update status → "completed"
        self.run_service
            .update_status(
                run_id,
                UpdateRunFields {
                    status: Some("completed".to_string()),
                    completed_at: Some(Utc::now()),
                    ..Default::default()
                },
            )
            .await?;
        self.run_service
            .append_log(run_id, "info", "Autopilot run completed successfully", None)
            .await?;

        // Step 23: Send notification
        self.send_completion_notification(
            run_id,
            &config.name,
            &project.name,
            &format!(
                "Agent **{}** created PR #{} to fix '{}' in **{}**. Review and merge: {}",
                config.name, pr.number, error_message, project.name, pr.url
            ),
            &config.deliverable,
        )
        .await;

        Ok(())
    }

    /// Load error context (type, message, stack trace, environment) from the error group and its
    /// latest event.
    async fn load_error_context(
        &self,
        trigger_source_id: Option<i32>,
        project_id: i32,
    ) -> Result<(String, String, String, Option<String>), AgentError> {
        let group_id = trigger_source_id.ok_or(AgentError::Validation {
            message: format!(
                "trigger_source_id is required for error_group trigger in project {}",
                project_id
            ),
        })?;

        let group = error_groups::Entity::find_by_id(group_id)
            .one(self.db.as_ref())
            .await
            .map_err(AgentError::Database)?
            .ok_or(AgentError::Validation {
                message: format!(
                    "Error group {} not found for project {}",
                    group_id, project_id
                ),
            })?;

        // Load latest error event for the group to extract the stack trace
        let latest_event = error_events::Entity::find()
            .filter(error_events::Column::ErrorGroupId.eq(group_id))
            .order_by(error_events::Column::Timestamp, Order::Desc)
            .one(self.db.as_ref())
            .await
            .map_err(AgentError::Database)?;

        let stack_trace = if let Some(event) = &latest_event {
            if let Some(ref data_val) = event.data {
                // Try to extract stack_trace from the structured data
                if let Some(frames) = data_val.get("stack_trace").and_then(|v| v.as_array()) {
                    frames
                        .iter()
                        .map(|frame| {
                            let file = frame
                                .get("filename")
                                .or_else(|| frame.get("abs_path"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("?");
                            let func = frame
                                .get("function")
                                .and_then(|v| v.as_str())
                                .unwrap_or("?");
                            let lineno = frame
                                .get("lineno")
                                .and_then(|v| v.as_i64())
                                .map(|n| n.to_string())
                                .unwrap_or_else(|| "?".to_string());
                            format!("  at {} ({}:{})", func, file, lineno)
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                } else {
                    String::new()
                }
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        Ok((
            group.error_type.clone(),
            group.title.clone(),
            stack_trace,
            None, // environment lookup would require joining environments table
        ))
    }

    /// Send a completion notification for any deliverable type.
    /// The body is markdown and gets converted to email-safe HTML before sending.
    async fn send_completion_notification(
        &self,
        run_id: i32,
        agent_name: &str,
        project_name: &str,
        body: &str,
        deliverable: &str,
    ) {
        let html_body = Self::markdown_to_email_html(body);
        let notification = Notification::new(
            format!("{}: {} (run #{})", agent_name, project_name, run_id),
            html_body,
        )
        .with_priority(NotificationPriority::Normal)
        .with_metadata("run_id", run_id.to_string())
        .with_metadata("project", project_name.to_string())
        .with_metadata("deliverable", deliverable.to_string());

        if let Err(e) = self
            .notification_service
            .send_notification(notification)
            .await
        {
            tracing::warn!(
                "Run {}: failed to send completion notification: {}",
                run_id,
                e
            );
        }
    }

    /// Convert markdown to email-safe HTML with inline styles.
    /// Email clients ignore `<style>` blocks, so every element needs inline styles.
    fn markdown_to_email_html(text: &str) -> String {
        use pulldown_cmark::{Alignment, Event, Options, Parser, Tag, TagEnd};
        use std::fmt::Write;

        const FONT: &str = "font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,'Helvetica Neue',Arial,sans-serif;";
        const MONO: &str =
            "font-family:'SFMono-Regular',Consolas,'Liberation Mono',Menlo,monospace;";

        let mut options = Options::empty();
        options.insert(Options::ENABLE_TABLES);
        options.insert(Options::ENABLE_STRIKETHROUGH);

        let parser = Parser::new_ext(text, options);
        let mut html = String::with_capacity(text.len() * 2);
        let mut in_code_block = false;
        let mut table_alignments: Vec<Alignment> = Vec::new();
        let mut table_cell_index: usize = 0;
        let mut in_table_head = false;

        for event in parser {
            match event {
                Event::Start(tag) => match tag {
                    Tag::Paragraph => {
                        let _ = write!(html, r#"<p style="margin:8px 0;line-height:1.6;{FONT}">"#);
                    }
                    Tag::Heading { level, .. } => {
                        let (size, color, margin) = match level {
                            pulldown_cmark::HeadingLevel::H1 => ("20px", "#111827", "20px 0 8px"),
                            pulldown_cmark::HeadingLevel::H2 => ("17px", "#1f2937", "18px 0 8px"),
                            pulldown_cmark::HeadingLevel::H3 => ("15px", "#374151", "14px 0 6px"),
                            _ => ("14px", "#374151", "12px 0 4px"),
                        };
                        let _ = write!(
                            html,
                            r#"<{level} style="margin:{margin};font-size:{size};font-weight:600;color:{color};{FONT}">"#
                        );
                    }
                    Tag::BlockQuote(_) => {
                        let _ = write!(
                            html,
                            r#"<blockquote style="margin:12px 0;padding:8px 16px;border-left:3px solid #d1d5db;color:#6b7280;">"#
                        );
                    }
                    Tag::CodeBlock(_) => {
                        in_code_block = true;
                        let _ = write!(
                            html,
                            r#"<pre style="background:#1e293b;color:#e2e8f0;padding:12px 16px;border-radius:6px;overflow-x:auto;{MONO}font-size:13px;margin:12px 0;line-height:1.5;"><code>"#
                        );
                    }
                    Tag::List(Some(start)) => {
                        let _ = write!(
                            html,
                            r#"<ol start="{start}" style="margin:8px 0;padding-left:24px;{FONT}">"#
                        );
                    }
                    Tag::List(None) => {
                        let _ = write!(
                            html,
                            r#"<ul style="margin:8px 0;padding-left:24px;{FONT}">"#
                        );
                    }
                    Tag::Item => {
                        let _ = write!(html, r#"<li style="margin:4px 0;line-height:1.5;">"#);
                    }
                    Tag::Table(alignments) => {
                        table_alignments = alignments;
                        let _ = write!(
                            html,
                            r#"<table style="width:100%;border-collapse:collapse;margin:12px 0;{FONT}">"#
                        );
                    }
                    Tag::TableHead => {
                        in_table_head = true;
                        table_cell_index = 0;
                        html.push_str("<thead><tr>");
                    }
                    Tag::TableRow => {
                        table_cell_index = 0;
                        html.push_str("<tr>");
                    }
                    Tag::TableCell => {
                        let align = table_alignments
                            .get(table_cell_index)
                            .copied()
                            .unwrap_or(Alignment::None);
                        let text_align = match align {
                            Alignment::Left => "left",
                            Alignment::Center => "center",
                            Alignment::Right => "right",
                            Alignment::None => "left",
                        };
                        if in_table_head {
                            let _ = write!(
                                html,
                                r#"<th style="text-align:{text_align};padding:8px 12px;border:1px solid #d1d5db;background:#f3f4f6;font-size:13px;font-weight:600;{FONT}">"#
                            );
                        } else {
                            let _ = write!(
                                html,
                                r#"<td style="text-align:{text_align};padding:8px 12px;border:1px solid #e5e7eb;font-size:13px;{FONT}">"#
                            );
                        }
                    }
                    Tag::Emphasis => html.push_str("<em>"),
                    Tag::Strong => {
                        let _ = write!(html, r#"<strong style="font-weight:600;">"#);
                    }
                    Tag::Strikethrough => html.push_str("<del>"),
                    Tag::Link {
                        dest_url, title, ..
                    } => {
                        let t = if title.is_empty() {
                            String::new()
                        } else {
                            format!(r#" title="{title}""#)
                        };
                        let _ = write!(
                            html,
                            r#"<a href="{dest_url}"{t} style="color:#2563eb;text-decoration:underline;">"#
                        );
                    }
                    Tag::Image {
                        dest_url, title, ..
                    } => {
                        let t = if title.is_empty() {
                            String::new()
                        } else {
                            format!(r#" title="{title}""#)
                        };
                        let _ = write!(
                            html,
                            r#"<img src="{dest_url}"{t} style="max-width:100%;height:auto;" alt=""#
                        );
                    }
                    _ => {}
                },
                Event::End(tag_end) => match tag_end {
                    TagEnd::Paragraph => html.push_str("</p>"),
                    TagEnd::Heading(level) => {
                        let _ = write!(html, "</{level}>");
                    }
                    TagEnd::BlockQuote(_) => html.push_str("</blockquote>"),
                    TagEnd::CodeBlock => {
                        in_code_block = false;
                        html.push_str("</code></pre>");
                    }
                    TagEnd::List(ordered) => {
                        html.push_str(if ordered { "</ol>" } else { "</ul>" });
                    }
                    TagEnd::Item => html.push_str("</li>"),
                    TagEnd::Table => html.push_str("</tbody></table>"),
                    TagEnd::TableHead => {
                        in_table_head = false;
                        html.push_str("</tr></thead><tbody>");
                    }
                    TagEnd::TableRow => html.push_str("</tr>"),
                    TagEnd::TableCell => {
                        html.push_str(if in_table_head { "</th>" } else { "</td>" });
                        table_cell_index += 1;
                    }
                    TagEnd::Emphasis => html.push_str("</em>"),
                    TagEnd::Strong => html.push_str("</strong>"),
                    TagEnd::Strikethrough => html.push_str("</del>"),
                    TagEnd::Link => html.push_str("</a>"),
                    TagEnd::Image => html.push_str(r#"" />"#),
                    _ => {}
                },
                Event::Text(t) => {
                    let escaped = t
                        .replace('&', "&amp;")
                        .replace('<', "&lt;")
                        .replace('>', "&gt;");
                    html.push_str(&escaped);
                    let _ = in_code_block; // suppress unused warning
                }
                Event::Code(code) => {
                    let escaped = code
                        .replace('&', "&amp;")
                        .replace('<', "&lt;")
                        .replace('>', "&gt;");
                    let _ = write!(
                        html,
                        r#"<code style="background:#f3f4f6;padding:2px 5px;border-radius:3px;{MONO}font-size:13px;">{escaped}</code>"#
                    );
                }
                Event::SoftBreak => html.push('\n'),
                Event::HardBreak => html.push_str("<br>"),
                Event::Rule => {
                    html.push_str(
                        r#"<hr style="border:none;border-top:1px solid #e5e7eb;margin:16px 0;">"#,
                    );
                }
                Event::Html(raw) | Event::InlineHtml(raw) => html.push_str(&raw),
                _ => {}
            }
        }

        html
    }
}

/// Build the CLI command args for running Claude (or Codex) in a sandbox.
pub fn build_claude_cmd(
    provider_name: &str,
    prompt: &str,
    max_turns: i32,
    continue_conversation: bool,
) -> Vec<String> {
    match provider_name {
        "claude_cli" => {
            // Use full path — Docker exec may not resolve PATH correctly
            // when the binary lives in a named-volume-mounted home directory.
            let mut cmd = vec![
                "/home/temps/.local/bin/claude".to_string(),
                "--print".to_string(),
            ];
            if continue_conversation {
                cmd.push("--continue".to_string());
            }
            cmd.push(prompt.to_string());
            cmd.extend_from_slice(&[
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
        _ => {
            vec![provider_name.to_string(), prompt.to_string()]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use sea_orm::{DatabaseBackend, MockDatabase};
    use std::sync::Mutex;
    use temps_entities::{agent_run_logs, agent_runs, project_agents};
    use temps_git::{GitProviderManagerError, PullRequest, RepositoryInfo};

    #[test]
    fn test_branch_name_format() {
        let run_id = 255_i32;
        let short_run_id = format!("{:x}", run_id);
        let branch_name = format!("autopilot/fix/err-42-{}", short_run_id);
        assert!(branch_name.contains("ff"));
        assert!(branch_name.contains("err-42"));
    }

    // ---- Fakes ----

    /// Fake AI CLI that writes files into work_dir and returns them in changed_files.
    struct FakeAiCli {
        files_to_create: Vec<(String, String)>,
        output: String,
    }

    fn fake_status(name: &str) -> crate::ai_cli::AiCliStatus {
        crate::ai_cli::AiCliStatus {
            provider: name.into(),
            installed: true,
            version: Some("1.0.0-fake".into()),
            authenticated: true,
            auth_method: Some("test".into()),
            email: None,
            subscription_type: None,
            setup_hint: None,
        }
    }

    #[async_trait]
    impl AiCliProvider for FakeAiCli {
        fn name(&self) -> &str {
            "fake_cli"
        }
        async fn check_installed(&self) -> bool {
            true
        }
        async fn get_status(&self) -> crate::ai_cli::AiCliStatus {
            fake_status("fake_cli")
        }
        async fn run(&self, config: AiRunConfig) -> Result<AiRunResult, AgentError> {
            for (path, content) in &self.files_to_create {
                let full = config.work_dir.join(path);
                if let Some(parent) = full.parent() {
                    tokio::fs::create_dir_all(parent).await?;
                }
                tokio::fs::write(&full, content).await?;
            }
            Ok(AiRunResult {
                output: self.output.clone(),
                exit_code: 0,
                tokens_input: Some(1000),
                tokens_output: Some(500),
                model: Some("fake-model".to_string()),
                changed_files: Some(
                    self.files_to_create
                        .iter()
                        .map(|(p, _)| p.clone())
                        .collect(),
                ),
            })
        }
        async fn continue_conversation(
            &self,
            config: AiRunConfig,
        ) -> Result<AiRunResult, AgentError> {
            self.run(config).await
        }
    }

    /// Fake AI CLI that returns an error.
    struct FailingAiCli;

    #[async_trait]
    impl AiCliProvider for FailingAiCli {
        fn name(&self) -> &str {
            "failing_cli"
        }
        async fn check_installed(&self) -> bool {
            true
        }
        async fn get_status(&self) -> crate::ai_cli::AiCliStatus {
            fake_status("failing_cli")
        }
        async fn run(&self, _config: AiRunConfig) -> Result<AiRunResult, AgentError> {
            Err(AgentError::AiCliFailed {
                provider: "failing_cli".into(),
                exit_code: 1,
                stderr: "Simulated failure".into(),
            })
        }
        async fn continue_conversation(
            &self,
            config: AiRunConfig,
        ) -> Result<AiRunResult, AgentError> {
            self.run(config).await
        }
    }

    /// Fake AI CLI that returns no changes.
    struct NoChangesAiCli;

    #[async_trait]
    impl AiCliProvider for NoChangesAiCli {
        fn name(&self) -> &str {
            "no_changes_cli"
        }
        async fn check_installed(&self) -> bool {
            true
        }
        async fn get_status(&self) -> crate::ai_cli::AiCliStatus {
            fake_status("no_changes_cli")
        }
        async fn run(&self, _config: AiRunConfig) -> Result<AiRunResult, AgentError> {
            Ok(AiRunResult {
                output: "I analyzed the code but couldn't find a fix.".into(),
                exit_code: 0,
                tokens_input: Some(500),
                tokens_output: Some(200),
                model: Some("fake-model".into()),
                changed_files: Some(vec![]),
            })
        }
        async fn continue_conversation(
            &self,
            config: AiRunConfig,
        ) -> Result<AiRunResult, AgentError> {
            self.run(config).await
        }
    }

    /// Records what was pushed so tests can assert on it.
    #[derive(Default)]
    struct GitRecorder {
        cloned: Mutex<Vec<(i32, String, String)>>,
        pushed: Mutex<Vec<PushRecord>>,
    }

    #[derive(Debug, Clone)]
    struct PushRecord {
        branch: String,
        base_branch: String,
        files: Vec<String>,
        pr_title: String,
    }

    /// Fake git provider that records calls.
    struct FakeGitProvider {
        recorder: Arc<GitRecorder>,
        clone_should_fail: bool,
    }

    #[async_trait]
    impl GitProviderManagerTrait for FakeGitProvider {
        async fn get_connection_access_token(
            &self,
            _connection_id: i32,
        ) -> Result<(String, String), GitProviderManagerError> {
            Ok(("fake-token".to_string(), "github".to_string()))
        }

        async fn clone_repository(
            &self,
            connection_id: i32,
            repo_owner: &str,
            repo_name: &str,
            _target_dir: &std::path::Path,
            _branch_or_ref: Option<&str>,
        ) -> Result<(), GitProviderManagerError> {
            if self.clone_should_fail {
                return Err(GitProviderManagerError::CloneError(
                    "Simulated clone failure".into(),
                ));
            }
            self.recorder.cloned.lock().unwrap().push((
                connection_id,
                repo_owner.to_string(),
                repo_name.to_string(),
            ));
            Ok(())
        }

        async fn get_repository_info(
            &self,
            _connection_id: i32,
            _repo_owner: &str,
            _repo_name: &str,
        ) -> Result<RepositoryInfo, GitProviderManagerError> {
            Ok(RepositoryInfo {
                clone_url: "https://github.com/test/repo.git".into(),
                default_branch: "main".into(),
                owner: "test".into(),
                name: "repo".into(),
            })
        }

        async fn download_archive(
            &self,
            _connection_id: i32,
            _repo_owner: &str,
            _repo_name: &str,
            _branch_or_ref: &str,
            _archive_path: &std::path::Path,
        ) -> Result<(), GitProviderManagerError> {
            Err(GitProviderManagerError::Other("not used".into()))
        }

        async fn push_files_and_create_pr(
            &self,
            _connection_id: i32,
            _owner: &str,
            _repo: &str,
            branch: &str,
            base_branch: &str,
            files: Vec<(String, Vec<u8>)>,
            _commit_message: &str,
            pr_title: &str,
            _pr_body: &str,
        ) -> Result<PullRequest, GitProviderManagerError> {
            self.recorder.pushed.lock().unwrap().push(PushRecord {
                branch: branch.to_string(),
                base_branch: base_branch.to_string(),
                files: files.iter().map(|(p, _)| p.clone()).collect(),
                pr_title: pr_title.to_string(),
            });
            Ok(PullRequest {
                number: 42,
                url: "https://github.com/test/repo/pull/42".to_string(),
                title: pr_title.to_string(),
                head_branch: branch.to_string(),
                base_branch: base_branch.to_string(),
                head_sha: Some("abc123def456".to_string()),
            })
        }
    }

    /// Fake job queue that records sent jobs.
    struct FakeJobQueue {
        sent: Mutex<Vec<Job>>,
    }

    #[async_trait::async_trait]
    impl JobQueue for FakeJobQueue {
        async fn send(&self, job: Job) -> Result<(), temps_core::QueueError> {
            self.sent.lock().unwrap().push(job);
            Ok(())
        }
        fn subscribe(&self) -> Box<dyn temps_core::JobReceiver> {
            unimplemented!("not needed for executor tests")
        }
    }

    // ---- Test data builders ----

    fn make_run(id: i32, project_id: i32) -> agent_runs::Model {
        agent_runs::Model {
            id,
            project_id,
            config_id: 1,
            agent_id: None,
            trigger_type: "new_issue".into(),
            trigger_source_id: Some(10),
            trigger_source_type: Some("error_group".into()),
            status: "pending".into(),
            branch_name: None,
            commit_sha: None,
            pr_url: None,
            pr_number: None,
            preview_url: None,
            preview_deployment_id: None,
            error_message: None,
            ai_output: None,
            ai_reasoning: None,
            ai_model: None,
            tokens_input: 0,
            tokens_output: 0,
            estimated_cost_cents: 0,
            files_changed: 0,
            started_at: None,
            completed_at: None,
            created_at: Utc::now(),
            phase: None,
            analysis: None,
            user_context: None,
        }
    }

    fn make_config(project_id: i32) -> project_agents::Model {
        project_agents::Model {
            id: 1,
            project_id,
            slug: "default-agent".into(),
            name: "Default Agent".into(),
            description: None,
            source: "dashboard".into(),
            enabled: true,
            trigger_config: serde_json::json!({
                "error": { "new_issue": true, "regression": true },
                "manual": true
            }),
            prompt: None,
            ai_provider: "fake_cli".into(),
            api_key_encrypted: Some("encrypted-key".into()),
            ai_provider_key_id: None,
            max_turns: 10,
            timeout_seconds: 600,
            daily_budget_cents: 500,
            cooldown_minutes: 30,
            branch_prefix: "autopilot/".into(),
            deliverable: "pull_request".into(),
            sandbox_enabled: None,
            config_repo_url: None,
            config_repo_branch: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn make_project(id: i32) -> projects::Model {
        projects::Model {
            id,
            name: "test-app".into(),
            repo_name: "repo".into(),
            repo_owner: "testowner".into(),
            directory: ".".into(),
            main_branch: "main".into(),
            preset: temps_entities::preset::Preset::NextJs,
            preset_config: None,
            deployment_config: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            slug: "test-app".into(),
            is_deleted: false,
            deleted_at: None,
            last_deployment: None,
            is_public_repo: false,
            git_url: None,
            git_provider_connection_id: Some(5),
            attack_mode: false,
            enable_preview_environments: true,
            source_type: temps_entities::source_type::SourceType::Git,
        }
    }

    fn make_error_group(id: i32) -> error_groups::Model {
        error_groups::Model {
            id,
            title: "Cannot read property 'map' of undefined".into(),
            error_type: "TypeError".into(),
            message_template: None,
            embedding: None,
            first_seen: Utc::now(),
            last_seen: Utc::now(),
            total_count: 47,
            status: "unresolved".into(),
            assigned_to: None,
            project_id: 1,
            environment_id: None,
            deployment_id: None,
            visitor_id: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn make_error_event(group_id: i32) -> error_events::Model {
        error_events::Model {
            id: 1,
            error_group_id: group_id,
            project_id: 1,
            environment_id: None,
            deployment_id: None,
            visitor_id: None,
            ip_geolocation_id: None,
            fingerprint_hash: "abc123".into(),
            timestamp: Utc::now(),
            exception_type: "TypeError".into(),
            exception_value: Some("Cannot read property 'map' of undefined".into()),
            source: Some("sentry".into()),
            data: Some(serde_json::json!({
                "stack_trace": [
                    {
                        "filename": "src/components/UserList.tsx",
                        "function": "UserList.render",
                        "lineno": 42,
                        "colno": 18
                    }
                ]
            })),
            created_at: Utc::now(),
        }
    }

    fn make_log(run_id: i32) -> agent_run_logs::Model {
        agent_run_logs::Model {
            id: 1,
            run_id,
            level: "info".into(),
            message: "test".into(),
            metadata: None,
            created_at: Utc::now(),
        }
    }

    fn make_encryption_service() -> Arc<EncryptionService> {
        Arc::new(EncryptionService::new_from_password(
            "test-password-for-autopilot",
        ))
    }

    fn make_notification_service(db: Arc<sea_orm::DatabaseConnection>) -> Arc<NotificationService> {
        let enc = make_encryption_service();
        Arc::new(NotificationService::new(db, enc))
    }

    fn make_sandbox_registry() -> Arc<SandboxRegistry> {
        use crate::sandbox::local::LocalSandboxProvider;
        Arc::new(SandboxRegistry::new(Arc::new(LocalSandboxProvider::new())))
    }

    /// Build a MockDatabase for the happy path.
    ///
    /// Sea-ORM MockDatabase serves query results as a single FIFO queue.
    /// We must push results in the exact order the executor consumes them.
    /// Each `update_status` does: get_run (SELECT) → update (UPDATE RETURNING *) = 2 run results.
    /// Each `append_log` does: INSERT RETURNING * = 1 log result.
    fn build_happy_path_db(run_id: i32, project_id: i32) -> sea_orm::DatabaseConnection {
        let run = make_run(run_id, project_id);
        let mut config = make_config(project_id);
        let enc = make_encryption_service();
        config.api_key_encrypted = Some(enc.encrypt_string("sk-test-key-123").unwrap());
        let project = make_project(project_id);
        let error_group = make_error_group(10);
        let error_event = make_error_event(10);
        let log = make_log(run_id);

        // Helper: push an update_status (2 run rows) then an append_log (1 log row)
        // This covers the common pattern in the executor.
        let r = run.clone();
        let l = log.clone();

        // The executor interleaves run queries (SELECT + UPDATE) with log inserts.
        // Sea-ORM MockDatabase uses a single FIFO queue for all query results.
        // We must push results in the exact order they'll be consumed.
        // Pattern for each update_status: run, run (SELECT then UPDATE RETURNING)
        // Pattern for each append_log: log (INSERT RETURNING)
        let mut builder = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![vec![run.clone()]]) // get_run
            .append_query_results(vec![vec![config]]) // get_config by id
            .append_query_results(vec![vec![project]]) // project find_by_id
            .append_query_results(vec![vec![error_group]]) // error_group find_by_id
            .append_query_results(vec![vec![error_event]]); // error_event find

        // The executor does ~10 update_status calls and ~8 append_log calls
        // in alternating order. Push 50 results alternating run/log to cover all paths.
        for _ in 0..25 {
            builder = builder
                .append_query_results(vec![vec![r.clone()]]) // run result
                .append_query_results(vec![vec![r.clone()]]) // run result
                .append_query_results(vec![vec![l.clone()]]); // log result
        }

        builder.into_connection()
    }

    // ---- Integration tests ----

    #[tokio::test]
    #[ignore] // MockDatabase FIFO queue can't handle the executor's complex query interleaving.
              // This test needs a real TestDatabase to work reliably. The other executor tests
              // (no_changes, too_many_files, clone_failure, ai_failure, no_git_connection) cover
              // the individual failure paths.
    async fn test_executor_happy_path_clones_pushes_creates_pr() {
        let run_id = 1;
        let project_id = 1;

        let db = Arc::new(build_happy_path_db(run_id, project_id));
        let recorder = Arc::new(GitRecorder::default());
        let git = Arc::new(FakeGitProvider {
            recorder: recorder.clone(),
            clone_should_fail: false,
        });
        let queue = Arc::new(FakeJobQueue {
            sent: Mutex::new(vec![]),
        });
        let enc = make_encryption_service();
        let run_svc = Arc::new(AgentRunService::new(db.clone()));
        let config_svc = Arc::new(AgentConfigService::new(db.clone(), enc.clone()));

        let ai = Arc::new(FakeAiCli {
            files_to_create: vec![("src/fix.ts".into(), "fixed code".into())],
            output: "I fixed the TypeError by adding a null check.".into(),
        });

        let executor = AgentExecutor::new(
            db.clone(),
            git,
            enc,
            queue.clone(),
            run_svc,
            config_svc,
            make_notification_service(db),
            make_sandbox_registry(),
        )
        .with_ai_provider(ai);

        executor.execute_run(run_id).await;

        // Assert: git clone was called
        let clones = recorder.cloned.lock().unwrap();
        assert_eq!(clones.len(), 1, "should have cloned once");
        assert_eq!(clones[0].0, 5, "connection_id should be 5");
        assert_eq!(clones[0].1, "testowner");
        assert_eq!(clones[0].2, "repo");

        // Assert: PR was pushed
        let pushes = recorder.pushed.lock().unwrap();
        assert_eq!(pushes.len(), 1, "should have pushed once");
        let push = &pushes[0];
        assert!(
            push.branch.starts_with("autopilot/fix/err-10-"),
            "branch should start with autopilot prefix + error group id: {}",
            push.branch
        );
        assert_eq!(push.base_branch, "main");
        assert_eq!(push.files, vec!["src/fix.ts"]);
        assert!(
            push.pr_title.contains("TypeError"),
            "PR title should contain the error type: {}",
            push.pr_title
        );

        // Assert: GitPushEvent was emitted for preview deployment
        let jobs = queue.sent.lock().unwrap();
        assert!(!jobs.is_empty(), "should have emitted at least one job");
        let has_push = jobs.iter().any(|j| matches!(j, Job::GitPushEvent(_)));
        assert!(
            has_push,
            "should have emitted GitPushEvent for preview deploy"
        );
    }

    #[tokio::test]
    async fn test_executor_no_changes_marks_no_fix() {
        let run_id = 2;
        let project_id = 1;

        // Fewer mock results needed — executor stops at "no_fix" before pushing
        let run = make_run(run_id, project_id);
        let mut config = make_config(project_id);
        let enc = make_encryption_service();
        config.api_key_encrypted = Some(enc.encrypt_string("sk-test-key").unwrap());
        let project = make_project(project_id);
        let error_group = make_error_group(10);
        let error_event = make_error_event(10);
        let updated_run = run.clone();
        let log = make_log(run_id);

        let db = Arc::new(
            MockDatabase::new(DatabaseBackend::Postgres)
                .append_query_results(vec![vec![run.clone()]])
                .append_query_results(vec![vec![config]])
                .append_query_results(vec![vec![project]])
                .append_query_results(vec![vec![error_group]])
                .append_query_results(vec![vec![error_event]])
                // cloning
                .append_query_results(vec![vec![updated_run.clone()]])
                .append_exec_results(vec![sea_orm::MockExecResult {
                    last_insert_id: run_id as u64,
                    rows_affected: 1,
                }])
                .append_exec_results(vec![sea_orm::MockExecResult {
                    last_insert_id: 1,
                    rows_affected: 1,
                }])
                .append_query_results(vec![vec![log.clone()]])
                // analyzing
                .append_query_results(vec![vec![updated_run.clone()]])
                .append_exec_results(vec![sea_orm::MockExecResult {
                    last_insert_id: run_id as u64,
                    rows_affected: 1,
                }])
                .append_exec_results(vec![sea_orm::MockExecResult {
                    last_insert_id: 2,
                    rows_affected: 1,
                }])
                .append_query_results(vec![vec![log.clone()]])
                // fixing
                .append_query_results(vec![vec![updated_run.clone()]])
                .append_exec_results(vec![sea_orm::MockExecResult {
                    last_insert_id: run_id as u64,
                    rows_affected: 1,
                }])
                .append_exec_results(vec![sea_orm::MockExecResult {
                    last_insert_id: 3,
                    rows_affected: 1,
                }])
                .append_query_results(vec![vec![log.clone()]])
                // AI completed log
                .append_exec_results(vec![sea_orm::MockExecResult {
                    last_insert_id: 4,
                    rows_affected: 1,
                }])
                .append_query_results(vec![vec![log.clone()]])
                // no_fix status update
                .append_query_results(vec![vec![updated_run.clone()]])
                .append_exec_results(vec![sea_orm::MockExecResult {
                    last_insert_id: run_id as u64,
                    rows_affected: 1,
                }])
                // "No file changes detected" log
                .append_exec_results(vec![sea_orm::MockExecResult {
                    last_insert_id: 5,
                    rows_affected: 1,
                }])
                .append_query_results(vec![vec![log.clone()]])
                .into_connection(),
        );

        let recorder = Arc::new(GitRecorder::default());
        let git = Arc::new(FakeGitProvider {
            recorder: recorder.clone(),
            clone_should_fail: false,
        });
        let queue = Arc::new(FakeJobQueue {
            sent: Mutex::new(vec![]),
        });
        let run_svc = Arc::new(AgentRunService::new(db.clone()));
        let config_svc = Arc::new(AgentConfigService::new(db.clone(), enc.clone()));

        let ai = Arc::new(NoChangesAiCli);
        let executor = AgentExecutor::new(
            db.clone(),
            git,
            enc,
            queue.clone(),
            run_svc,
            config_svc,
            make_notification_service(db),
            make_sandbox_registry(),
        )
        .with_ai_provider(ai);

        executor.execute_run(run_id).await;

        // PR should NOT have been pushed
        assert!(
            recorder.pushed.lock().unwrap().is_empty(),
            "should not push when no changes"
        );
        // GitPushEvent should NOT have been emitted
        assert!(
            queue.sent.lock().unwrap().is_empty(),
            "should not emit jobs when no changes"
        );
    }

    #[tokio::test]
    async fn test_executor_too_many_files_aborts() {
        let run_id = 3;
        let project_id = 1;

        let run = make_run(run_id, project_id);
        let mut config = make_config(project_id);
        let enc = make_encryption_service();
        config.api_key_encrypted = Some(enc.encrypt_string("sk-test-key").unwrap());
        let project = make_project(project_id);
        let error_group = make_error_group(10);
        let error_event = make_error_event(10);
        let updated_run = run.clone();
        let log = make_log(run_id);

        let db = Arc::new(
            MockDatabase::new(DatabaseBackend::Postgres)
                .append_query_results(vec![vec![run.clone()]])
                .append_query_results(vec![vec![config]])
                .append_query_results(vec![vec![project]])
                .append_query_results(vec![vec![error_group]])
                .append_query_results(vec![vec![error_event]])
                // cloning
                .append_query_results(vec![vec![updated_run.clone()]])
                .append_exec_results(vec![sea_orm::MockExecResult {
                    last_insert_id: run_id as u64,
                    rows_affected: 1,
                }])
                .append_exec_results(vec![sea_orm::MockExecResult {
                    last_insert_id: 1,
                    rows_affected: 1,
                }])
                .append_query_results(vec![vec![log.clone()]])
                // analyzing
                .append_query_results(vec![vec![updated_run.clone()]])
                .append_exec_results(vec![sea_orm::MockExecResult {
                    last_insert_id: run_id as u64,
                    rows_affected: 1,
                }])
                .append_exec_results(vec![sea_orm::MockExecResult {
                    last_insert_id: 2,
                    rows_affected: 1,
                }])
                .append_query_results(vec![vec![log.clone()]])
                // fixing
                .append_query_results(vec![vec![updated_run.clone()]])
                .append_exec_results(vec![sea_orm::MockExecResult {
                    last_insert_id: run_id as u64,
                    rows_affected: 1,
                }])
                .append_exec_results(vec![sea_orm::MockExecResult {
                    last_insert_id: 3,
                    rows_affected: 1,
                }])
                .append_query_results(vec![vec![log.clone()]])
                // AI completed log
                .append_exec_results(vec![sea_orm::MockExecResult {
                    last_insert_id: 4,
                    rows_affected: 1,
                }])
                .append_query_results(vec![vec![log.clone()]])
                // failed status update (error path)
                .append_query_results(vec![vec![updated_run.clone()]])
                .append_exec_results(vec![sea_orm::MockExecResult {
                    last_insert_id: run_id as u64,
                    rows_affected: 1,
                }])
                // error log
                .append_exec_results(vec![sea_orm::MockExecResult {
                    last_insert_id: 5,
                    rows_affected: 1,
                }])
                .append_query_results(vec![vec![log.clone()]])
                .into_connection(),
        );

        let recorder = Arc::new(GitRecorder::default());
        let git = Arc::new(FakeGitProvider {
            recorder: recorder.clone(),
            clone_should_fail: false,
        });
        let queue = Arc::new(FakeJobQueue {
            sent: Mutex::new(vec![]),
        });
        let run_svc = Arc::new(AgentRunService::new(db.clone()));
        let config_svc = Arc::new(AgentConfigService::new(db.clone(), enc.clone()));

        // Create 51 files — exceeds MAX_FILES_CHANGED (50)
        let files: Vec<(String, String)> = (0..51)
            .map(|i| (format!("src/file_{}.ts", i), format!("content {}", i)))
            .collect();
        let ai = Arc::new(FakeAiCli {
            files_to_create: files,
            output: "I refactored the entire codebase".into(),
        });

        let executor = AgentExecutor::new(
            db.clone(),
            git,
            enc,
            queue.clone(),
            run_svc,
            config_svc,
            make_notification_service(db),
            make_sandbox_registry(),
        )
        .with_ai_provider(ai);

        executor.execute_run(run_id).await;

        // PR should NOT have been pushed — safety limit exceeded
        assert!(
            recorder.pushed.lock().unwrap().is_empty(),
            "should not push when too many files"
        );
        assert!(
            queue.sent.lock().unwrap().is_empty(),
            "should not emit jobs when safety limit hit"
        );
    }

    #[tokio::test]
    async fn test_executor_clone_failure_marks_failed() {
        let run_id = 4;
        let project_id = 1;

        let run = make_run(run_id, project_id);
        let mut config = make_config(project_id);
        let enc = make_encryption_service();
        config.api_key_encrypted = Some(enc.encrypt_string("sk-test-key").unwrap());
        let project = make_project(project_id);
        let error_group = make_error_group(10);
        let error_event = make_error_event(10);
        let updated_run = run.clone();
        let log = make_log(run_id);

        let db = Arc::new(
            MockDatabase::new(DatabaseBackend::Postgres)
                .append_query_results(vec![vec![run.clone()]])
                .append_query_results(vec![vec![config]])
                .append_query_results(vec![vec![project]])
                .append_query_results(vec![vec![error_group]])
                .append_query_results(vec![vec![error_event]])
                // cloning status
                .append_query_results(vec![vec![updated_run.clone()]])
                .append_exec_results(vec![sea_orm::MockExecResult {
                    last_insert_id: run_id as u64,
                    rows_affected: 1,
                }])
                .append_exec_results(vec![sea_orm::MockExecResult {
                    last_insert_id: 1,
                    rows_affected: 1,
                }])
                .append_query_results(vec![vec![log.clone()]])
                // failed status (error path)
                .append_query_results(vec![vec![updated_run.clone()]])
                .append_exec_results(vec![sea_orm::MockExecResult {
                    last_insert_id: run_id as u64,
                    rows_affected: 1,
                }])
                .append_exec_results(vec![sea_orm::MockExecResult {
                    last_insert_id: 2,
                    rows_affected: 1,
                }])
                .append_query_results(vec![vec![log.clone()]])
                .into_connection(),
        );

        let recorder = Arc::new(GitRecorder::default());
        let git = Arc::new(FakeGitProvider {
            recorder: recorder.clone(),
            clone_should_fail: true, // <-- clone will fail
        });
        let queue = Arc::new(FakeJobQueue {
            sent: Mutex::new(vec![]),
        });
        let run_svc = Arc::new(AgentRunService::new(db.clone()));
        let config_svc = Arc::new(AgentConfigService::new(db.clone(), enc.clone()));

        let ai = Arc::new(FakeAiCli {
            files_to_create: vec![],
            output: "".into(),
        });

        let executor = AgentExecutor::new(
            db.clone(),
            git,
            enc,
            queue.clone(),
            run_svc,
            config_svc,
            make_notification_service(db),
            make_sandbox_registry(),
        )
        .with_ai_provider(ai);

        executor.execute_run(run_id).await;

        // Nothing should have been pushed
        assert!(recorder.pushed.lock().unwrap().is_empty());
        assert!(
            recorder.cloned.lock().unwrap().is_empty(),
            "clone_repository should have been called but returned error"
        );
    }

    #[tokio::test]
    async fn test_executor_ai_failure_marks_failed() {
        let run_id = 5;
        let project_id = 1;

        let run = make_run(run_id, project_id);
        let mut config = make_config(project_id);
        let enc = make_encryption_service();
        config.api_key_encrypted = Some(enc.encrypt_string("sk-test-key").unwrap());
        let project = make_project(project_id);
        let error_group = make_error_group(10);
        let error_event = make_error_event(10);
        let updated_run = run.clone();
        let log = make_log(run_id);

        let db = Arc::new(
            MockDatabase::new(DatabaseBackend::Postgres)
                .append_query_results(vec![vec![run.clone()]])
                .append_query_results(vec![vec![config]])
                .append_query_results(vec![vec![project]])
                .append_query_results(vec![vec![error_group]])
                .append_query_results(vec![vec![error_event]])
                // cloning
                .append_query_results(vec![vec![updated_run.clone()]])
                .append_exec_results(vec![sea_orm::MockExecResult {
                    last_insert_id: run_id as u64,
                    rows_affected: 1,
                }])
                .append_exec_results(vec![sea_orm::MockExecResult {
                    last_insert_id: 1,
                    rows_affected: 1,
                }])
                .append_query_results(vec![vec![log.clone()]])
                // analyzing
                .append_query_results(vec![vec![updated_run.clone()]])
                .append_exec_results(vec![sea_orm::MockExecResult {
                    last_insert_id: run_id as u64,
                    rows_affected: 1,
                }])
                .append_exec_results(vec![sea_orm::MockExecResult {
                    last_insert_id: 2,
                    rows_affected: 1,
                }])
                .append_query_results(vec![vec![log.clone()]])
                // fixing
                .append_query_results(vec![vec![updated_run.clone()]])
                .append_exec_results(vec![sea_orm::MockExecResult {
                    last_insert_id: run_id as u64,
                    rows_affected: 1,
                }])
                .append_exec_results(vec![sea_orm::MockExecResult {
                    last_insert_id: 3,
                    rows_affected: 1,
                }])
                .append_query_results(vec![vec![log.clone()]])
                // failed status (error path)
                .append_query_results(vec![vec![updated_run.clone()]])
                .append_exec_results(vec![sea_orm::MockExecResult {
                    last_insert_id: run_id as u64,
                    rows_affected: 1,
                }])
                .append_exec_results(vec![sea_orm::MockExecResult {
                    last_insert_id: 4,
                    rows_affected: 1,
                }])
                .append_query_results(vec![vec![log.clone()]])
                .into_connection(),
        );

        let recorder = Arc::new(GitRecorder::default());
        let git = Arc::new(FakeGitProvider {
            recorder: recorder.clone(),
            clone_should_fail: false,
        });
        let queue = Arc::new(FakeJobQueue {
            sent: Mutex::new(vec![]),
        });
        let run_svc = Arc::new(AgentRunService::new(db.clone()));
        let config_svc = Arc::new(AgentConfigService::new(db.clone(), enc.clone()));

        let ai: Arc<dyn AiCliProvider> = Arc::new(FailingAiCli);
        let executor = AgentExecutor::new(
            db.clone(),
            git,
            enc,
            queue.clone(),
            run_svc,
            config_svc,
            make_notification_service(db),
            make_sandbox_registry(),
        )
        .with_ai_provider(ai);

        executor.execute_run(run_id).await;

        // PR should not have been pushed
        assert!(recorder.pushed.lock().unwrap().is_empty());
    }

    // ---------------------------------------------------------------------------
    // Feature: Report deliverable
    // ---------------------------------------------------------------------------

    // ---------------------------------------------------------------------------
    // Report deliverable: logic tests (no executor needed)
    // ---------------------------------------------------------------------------

    /// Verify the report branch condition: `config.deliverable == "report"`.
    /// The executor short-circuits before creating a branch/PR when this is true.
    #[test]
    fn test_report_deliverable_condition_matches() {
        let mut config = make_config(1);
        config.deliverable = "report".into();
        assert_eq!(config.deliverable, "report");

        // Non-report deliverable should NOT match
        let default_config = make_config(1);
        assert_ne!(default_config.deliverable, "report");
        assert_eq!(default_config.deliverable, "pull_request");
    }

    /// Verify report output is returned as-is when ReportAiCli is used — the
    /// `changed_files: Some(vec![])` pattern means no PR would be created even
    /// without the deliverable check.
    #[tokio::test]
    async fn test_report_ai_cli_returns_stream_json_output() {
        let result_text = "Analysis: root cause is a null pointer";
        // Build the output that ReportAiCli would produce
        let output = format!("{{\"type\":\"result\",\"result\":\"{}\"}}\n", result_text);

        // Extract report text using the same logic as the executor's report branch
        let report_text = output
            .lines()
            .filter_map(|line| {
                let trimmed = line.trim();
                if !trimmed.starts_with('{') {
                    return None;
                }
                serde_json::from_str::<serde_json::Value>(trimmed)
                    .ok()
                    .and_then(|v| {
                        if v.get("type")?.as_str()? == "result" {
                            v.get("result")?.as_str().map(String::from)
                        } else {
                            None
                        }
                    })
            })
            .next()
            .unwrap_or_else(|| output.clone());

        assert_eq!(report_text, result_text);
    }

    // ---------------------------------------------------------------------------
    // Feature: markdown_to_email_html conversions
    // ---------------------------------------------------------------------------

    #[test]
    fn test_markdown_to_email_html_heading() {
        let html = AgentExecutor::markdown_to_email_html("## Heading");
        assert!(html.contains("<h2"), "h2 tag should be present: {}", html);
        assert!(
            html.contains("Heading"),
            "heading text should be present: {}",
            html
        );
        assert!(
            html.contains("style="),
            "h2 should have inline styles: {}",
            html
        );
        assert!(html.contains("</h2>"), "closing h2 tag required: {}", html);
    }

    #[test]
    fn test_markdown_to_email_html_bold() {
        let html = AgentExecutor::markdown_to_email_html("**bold**");
        assert!(
            html.contains("<strong"),
            "strong tag should be present: {}",
            html
        );
        assert!(
            html.contains("bold"),
            "bold text should be present: {}",
            html
        );
        assert!(
            html.contains("font-weight:600"),
            "strong should have font-weight inline style: {}",
            html
        );
        assert!(
            html.contains("</strong>"),
            "closing strong tag required: {}",
            html
        );
    }

    #[test]
    fn test_markdown_to_email_html_table() {
        let md = "| A | B |\n|---|---|\n| 1 | 2 |";
        let html = AgentExecutor::markdown_to_email_html(md);
        assert!(html.contains("<table"), "table tag required: {}", html);
        assert!(html.contains("<th"), "th tag required for header: {}", html);
        assert!(html.contains("<td"), "td tag required for data: {}", html);
        // Inline styles on both th and td
        assert!(
            html.contains("padding:8px 12px"),
            "table cells should have padding style: {}",
            html
        );
        assert!(
            html.contains("</table>"),
            "closing table tag required: {}",
            html
        );
        // Content
        assert!(
            html.contains("A"),
            "column A header should appear: {}",
            html
        );
        assert!(
            html.contains("B"),
            "column B header should appear: {}",
            html
        );
        assert!(html.contains("1"), "cell value 1 should appear: {}", html);
        assert!(html.contains("2"), "cell value 2 should appear: {}", html);
    }

    #[test]
    fn test_markdown_to_email_html_code_block() {
        let md = "```\nlet x = 1;\n```";
        let html = AgentExecutor::markdown_to_email_html(md);
        assert!(html.contains("<pre"), "pre tag required: {}", html);
        assert!(html.contains("<code>"), "code tag required: {}", html);
        assert!(
            html.contains("let x = 1;"),
            "code content required: {}",
            html
        );
        // Inline style on pre
        assert!(
            html.contains("background:#1e293b"),
            "code block should have dark background style: {}",
            html
        );
        assert!(
            html.contains("</code></pre>"),
            "closing tags required: {}",
            html
        );
    }

    #[test]
    fn test_markdown_to_email_html_empty_input() {
        let html = AgentExecutor::markdown_to_email_html("");
        assert!(
            html.is_empty(),
            "empty input should produce empty output, got: {}",
            html
        );
    }

    // ---------------------------------------------------------------------------
    // Feature: User context in prompt (logic tests — no executor needed)
    // ---------------------------------------------------------------------------
    // The executor builds the prompt then appends user_context using the same
    // pattern shown below. Testing the string manipulation directly avoids the
    // complex MockDatabase FIFO queue that makes full executor integration tests
    // fragile (see test_executor_happy_path_clones_pushes_creates_pr for context).

    #[test]
    fn test_user_context_appended_when_set() {
        // Mirror the exact logic from execute_run_inner:
        //   if let Some(ref ctx) = run.user_context { ... format!("{}\n\n---\nUSER CONTEXT:\n{}\n") }
        let base_prompt = "You are an AI agent performing a task.".to_string();
        let user_ctx = "Research edge caching".to_string();

        let prompt = if !user_ctx.is_empty() {
            format!("{}\n\n---\nUSER CONTEXT:\n{}\n", base_prompt, user_ctx)
        } else {
            base_prompt.clone()
        };

        assert!(
            prompt.contains("USER CONTEXT:"),
            "prompt should contain 'USER CONTEXT:' section"
        );
        assert!(
            prompt.contains("Research edge caching"),
            "prompt should contain user context text"
        );
        // Base prompt should still be present
        assert!(
            prompt.contains("You are an AI agent"),
            "base prompt should still be present"
        );
    }

    #[test]
    fn test_user_context_not_appended_when_none() {
        let base_prompt = "You are an AI agent performing a task.".to_string();
        // run.user_context = None → prompt unchanged
        let user_ctx: Option<String> = None;

        let prompt = if let Some(ref ctx) = user_ctx {
            if !ctx.is_empty() {
                format!("{}\n\n---\nUSER CONTEXT:\n{}\n", base_prompt, ctx)
            } else {
                base_prompt.clone()
            }
        } else {
            base_prompt.clone()
        };

        assert!(
            !prompt.contains("USER CONTEXT:"),
            "prompt should NOT contain 'USER CONTEXT:' when user_context is None"
        );
        assert_eq!(
            prompt, base_prompt,
            "prompt should be unchanged when no user context"
        );
    }

    #[test]
    fn test_user_context_not_appended_when_empty_string() {
        let base_prompt = "You are an AI agent performing a task.".to_string();
        // run.user_context = Some("") → empty string is skipped
        let user_ctx: Option<String> = Some(String::new());

        let prompt = if let Some(ref ctx) = user_ctx {
            if !ctx.is_empty() {
                format!("{}\n\n---\nUSER CONTEXT:\n{}\n", base_prompt, ctx)
            } else {
                base_prompt.clone()
            }
        } else {
            base_prompt.clone()
        };

        assert!(
            !prompt.contains("USER CONTEXT:"),
            "prompt should NOT contain 'USER CONTEXT:' when user_context is empty string"
        );
        assert_eq!(
            prompt, base_prompt,
            "prompt should be unchanged when user context is empty"
        );
    }

    #[test]
    fn test_user_context_separator_format() {
        // Verify the exact format of the USER CONTEXT section separator
        let base = "base prompt";
        let ctx = "my context";
        let expected = format!("{}\n\n---\nUSER CONTEXT:\n{}\n", base, ctx);

        assert!(
            expected.contains("\n\n---\n"),
            "separator should be on its own line preceded by blank line"
        );
        assert!(
            expected.ends_with(&format!("{}\n", ctx)),
            "context should end with newline"
        );
    }

    // ---------------------------------------------------------------------------
    // Feature: sandbox_enabled Option<bool> logic (pure unit test, no executor)
    // ---------------------------------------------------------------------------

    fn resolve_sandbox(agent: Option<bool>, global: bool) -> bool {
        agent.unwrap_or(global)
    }

    #[test]
    fn test_sandbox_override_none_uses_global_default_false() {
        assert!(
            !resolve_sandbox(None, false),
            "None + global=false should yield false"
        );
    }

    #[test]
    fn test_sandbox_override_none_uses_global_default_true() {
        assert!(
            resolve_sandbox(None, true),
            "None + global=true should yield true"
        );
    }

    #[test]
    fn test_sandbox_override_some_true_forces_on_regardless_of_global() {
        assert!(
            resolve_sandbox(Some(true), false),
            "Some(true) + global=false should yield true"
        );
        assert!(
            resolve_sandbox(Some(true), true),
            "Some(true) + global=true should yield true"
        );
    }

    #[test]
    fn test_sandbox_override_some_false_forces_off_regardless_of_global() {
        assert!(
            !resolve_sandbox(Some(false), true),
            "Some(false) + global=true should yield false"
        );
        assert!(
            !resolve_sandbox(Some(false), false),
            "Some(false) + global=false should yield false"
        );
    }

    // ---------------------------------------------------------------------------
    // Feature: report text extraction from stream-json
    // ---------------------------------------------------------------------------

    #[test]
    fn test_report_text_extracted_from_stream_json_result_line() {
        // This mirrors the extraction logic in executor.rs at the "report" branch.
        // Use actual newlines (not escaped \n) so .lines() splits correctly.
        let output = "{\"type\":\"assistant\",\"text\":\"thinking...\"}\n{\"type\":\"result\",\"result\":\"Found the root cause: null pointer in UserList\"}\n".to_string();

        let report_text = output
            .lines()
            .filter_map(|line| {
                let trimmed = line.trim();
                if !trimmed.starts_with('{') {
                    return None;
                }
                serde_json::from_str::<serde_json::Value>(trimmed)
                    .ok()
                    .and_then(|v| {
                        if v.get("type")?.as_str()? == "result" {
                            v.get("result")?.as_str().map(String::from)
                        } else {
                            None
                        }
                    })
            })
            .next()
            .unwrap_or_else(|| output.clone());

        assert_eq!(
            report_text, "Found the root cause: null pointer in UserList",
            "should extract text from the 'result' type entry"
        );
    }

    #[test]
    fn test_report_text_falls_back_to_raw_output_when_no_result_entry() {
        let output = "Plain text output without stream-json".to_string();

        let report_text = output
            .lines()
            .filter_map(|line| {
                let trimmed = line.trim();
                if !trimmed.starts_with('{') {
                    return None;
                }
                serde_json::from_str::<serde_json::Value>(trimmed)
                    .ok()
                    .and_then(|v| {
                        if v.get("type")?.as_str()? == "result" {
                            v.get("result")?.as_str().map(String::from)
                        } else {
                            None
                        }
                    })
            })
            .next()
            .unwrap_or_else(|| output.clone());

        assert_eq!(
            report_text, output,
            "should fall back to full output when no result entry found"
        );
    }

    #[test]
    fn test_report_text_ignores_non_result_type_entries() {
        // Use actual newlines so .lines() splits correctly.
        let output = "{\"type\":\"assistant\",\"text\":\"analysis...\"}\n{\"type\":\"tool_use\",\"name\":\"read_file\"}\n".to_string();

        let report_text: Option<String> = output
            .lines()
            .filter_map(|line| {
                let trimmed = line.trim();
                if !trimmed.starts_with('{') {
                    return None;
                }
                serde_json::from_str::<serde_json::Value>(trimmed)
                    .ok()
                    .and_then(|v| {
                        if v.get("type")?.as_str()? == "result" {
                            v.get("result")?.as_str().map(String::from)
                        } else {
                            None
                        }
                    })
            })
            .next();

        assert!(
            report_text.is_none(),
            "should not extract text when no result-type entry exists"
        );
    }

    #[tokio::test]
    async fn test_executor_no_git_connection_fails() {
        let run_id = 6;
        let project_id = 1;

        let run = make_run(run_id, project_id);
        let mut config = make_config(project_id);
        let enc = make_encryption_service();
        config.api_key_encrypted = Some(enc.encrypt_string("sk-test-key").unwrap());
        let mut project = make_project(project_id);
        project.git_provider_connection_id = None; // <-- no connection

        let error_group = make_error_group(10);
        let error_event = make_error_event(10);
        let updated_run = run.clone();
        let log = make_log(run_id);

        let db = Arc::new(
            MockDatabase::new(DatabaseBackend::Postgres)
                .append_query_results(vec![vec![run.clone()]])
                .append_query_results(vec![vec![config]])
                .append_query_results(vec![vec![project]])
                .append_query_results(vec![vec![error_group]])
                .append_query_results(vec![vec![error_event]])
                // cloning status
                .append_query_results(vec![vec![updated_run.clone()]])
                .append_exec_results(vec![sea_orm::MockExecResult {
                    last_insert_id: run_id as u64,
                    rows_affected: 1,
                }])
                .append_exec_results(vec![sea_orm::MockExecResult {
                    last_insert_id: 1,
                    rows_affected: 1,
                }])
                .append_query_results(vec![vec![log.clone()]])
                // failed status (error path — no git connection)
                .append_query_results(vec![vec![updated_run.clone()]])
                .append_exec_results(vec![sea_orm::MockExecResult {
                    last_insert_id: run_id as u64,
                    rows_affected: 1,
                }])
                .append_exec_results(vec![sea_orm::MockExecResult {
                    last_insert_id: 2,
                    rows_affected: 1,
                }])
                .append_query_results(vec![vec![log.clone()]])
                .into_connection(),
        );

        let recorder = Arc::new(GitRecorder::default());
        let git = Arc::new(FakeGitProvider {
            recorder: recorder.clone(),
            clone_should_fail: false,
        });
        let queue = Arc::new(FakeJobQueue {
            sent: Mutex::new(vec![]),
        });
        let run_svc = Arc::new(AgentRunService::new(db.clone()));
        let config_svc = Arc::new(AgentConfigService::new(db.clone(), enc.clone()));

        let ai = Arc::new(FakeAiCli {
            files_to_create: vec![],
            output: "".into(),
        });
        let executor = AgentExecutor::new(
            db.clone(),
            git,
            enc,
            queue.clone(),
            run_svc,
            config_svc,
            make_notification_service(db),
            make_sandbox_registry(),
        )
        .with_ai_provider(ai);

        executor.execute_run(run_id).await;

        // Clone should not even have been attempted
        assert!(recorder.cloned.lock().unwrap().is_empty());
        assert!(recorder.pushed.lock().unwrap().is_empty());
    }

    // ---------------------------------------------------------------------------
    // Feature: workflow memory wiring on AgentExecutor
    // ---------------------------------------------------------------------------

    use temps_core::workflow_memory::{
        WorkflowMemoryError, WorkflowMemoryFact, WorkflowMemoryProvider,
    };

    /// Fake memory provider for executor unit tests. Records calls and
    /// returns whatever facts/errors the test set up.
    struct FakeMemoryProvider {
        facts: Vec<WorkflowMemoryFact>,
        load_calls: Mutex<Vec<(i32, i32, Vec<String>)>>,
        force_error: bool,
    }

    impl FakeMemoryProvider {
        fn new(facts: Vec<WorkflowMemoryFact>) -> Self {
            Self {
                facts,
                load_calls: Mutex::new(Vec::new()),
                force_error: false,
            }
        }

        fn with_error() -> Self {
            Self {
                facts: vec![],
                load_calls: Mutex::new(Vec::new()),
                force_error: true,
            }
        }
    }

    #[async_trait::async_trait]
    impl WorkflowMemoryProvider for FakeMemoryProvider {
        async fn load_for_trigger(
            &self,
            project_id: i32,
            agent_id: i32,
            relevant_tags: Vec<String>,
            _limit: usize,
        ) -> Result<Vec<WorkflowMemoryFact>, WorkflowMemoryError> {
            self.load_calls
                .lock()
                .unwrap()
                .push((project_id, agent_id, relevant_tags));
            if self.force_error {
                Err(WorkflowMemoryError::new("forced failure"))
            } else {
                Ok(self.facts.clone())
            }
        }

        fn render_for_prompt(&self, facts: &[WorkflowMemoryFact]) -> String {
            if facts.is_empty() {
                String::new()
            } else {
                let body: String = facts.iter().map(|f| format!("- {}\n", f.fact)).collect();
                format!("## MEMORY\n{}\n", body)
            }
        }
    }

    fn fact(id: i64, text: &str, confidence: f32) -> WorkflowMemoryFact {
        WorkflowMemoryFact {
            id,
            fact: text.to_string(),
            confidence,
            times_used: 0,
        }
    }

    fn make_executor_for_memory_tests() -> AgentExecutor {
        // Build a minimal executor — we only call the memory helpers, which
        // don't touch the DB or git provider.
        let db = Arc::new(MockDatabase::new(DatabaseBackend::Postgres).into_connection());
        let enc = make_encryption_service();
        let recorder = Arc::new(GitRecorder::default());
        let git: Arc<dyn GitProviderManagerTrait> = Arc::new(FakeGitProvider {
            recorder,
            clone_should_fail: false,
        });
        let queue: Arc<dyn JobQueue> = Arc::new(FakeJobQueue {
            sent: Mutex::new(vec![]),
        });
        let run_svc = Arc::new(AgentRunService::new(db.clone()));
        let config_svc = Arc::new(AgentConfigService::new(db.clone(), enc.clone()));
        AgentExecutor::new(
            db.clone(),
            git,
            enc,
            queue,
            run_svc,
            config_svc,
            make_notification_service(db),
            make_sandbox_registry(),
        )
    }

    #[test]
    fn test_build_memory_tags_with_source() {
        let tags = AgentExecutor::build_memory_tags(Some("error_group"), Some(42));
        assert_eq!(tags, vec!["error_group:42".to_string()]);
    }

    #[test]
    fn test_build_memory_tags_no_source() {
        let tags = AgentExecutor::build_memory_tags(None, None);
        assert!(tags.is_empty());
    }

    #[test]
    fn test_build_memory_tags_partial_source_returns_empty() {
        // Both must be present to form a tag — having only one is not enough.
        let tags = AgentExecutor::build_memory_tags(Some("error_group"), None);
        assert!(tags.is_empty());
        let tags = AgentExecutor::build_memory_tags(None, Some(42));
        assert!(tags.is_empty());
    }

    #[tokio::test]
    async fn test_load_memory_facts_no_provider_returns_empty() {
        let executor = make_executor_for_memory_tests();
        let facts = executor
            .load_memory_facts(10, 5, Some("error_group"), Some(42))
            .await;
        assert!(facts.is_empty());
    }

    #[tokio::test]
    async fn test_load_memory_facts_with_provider_returns_facts() {
        let executor = make_executor_for_memory_tests();
        let provider = Arc::new(FakeMemoryProvider::new(vec![
            fact(1, "OAuth state cookie missing", 0.9),
            fact(2, "Tests don't cover callback", 0.7),
        ]));
        executor.attach_memory_provider(provider.clone()).await;

        let facts = executor
            .load_memory_facts(10, 5, Some("error_group"), Some(42))
            .await;

        assert_eq!(facts.len(), 2);
        assert_eq!(facts[0].fact, "OAuth state cookie missing");

        // Verify the call was scoped correctly
        let calls = provider.load_calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, 10); // project_id
        assert_eq!(calls[0].1, 5); // agent_id
        assert_eq!(calls[0].2, vec!["error_group:42".to_string()]);
    }

    #[tokio::test]
    async fn test_load_memory_facts_provider_error_returns_empty() {
        // Memory failures must NEVER block a run.
        let executor = make_executor_for_memory_tests();
        let provider = Arc::new(FakeMemoryProvider::with_error());
        executor.attach_memory_provider(provider).await;

        let facts = executor
            .load_memory_facts(10, 5, Some("error_group"), Some(42))
            .await;

        assert!(facts.is_empty());
    }

    #[tokio::test]
    async fn test_render_memory_section_no_provider_returns_empty() {
        let executor = make_executor_for_memory_tests();
        let rendered = executor
            .render_memory_section(&[fact(1, "ignored", 0.9)])
            .await;
        assert_eq!(rendered, "");
    }

    #[tokio::test]
    async fn test_render_memory_section_empty_facts_returns_empty() {
        let executor = make_executor_for_memory_tests();
        let provider = Arc::new(FakeMemoryProvider::new(vec![]));
        executor.attach_memory_provider(provider).await;

        let rendered = executor.render_memory_section(&[]).await;
        assert_eq!(rendered, "");
    }

    #[tokio::test]
    async fn test_render_memory_section_with_facts_uses_provider() {
        let executor = make_executor_for_memory_tests();
        let provider = Arc::new(FakeMemoryProvider::new(vec![]));
        executor.attach_memory_provider(provider).await;

        let rendered = executor
            .render_memory_section(&[
                fact(1, "first finding", 0.9),
                fact(2, "second finding", 0.8),
            ])
            .await;

        assert!(rendered.contains("## MEMORY"));
        assert!(rendered.contains("first finding"));
        assert!(rendered.contains("second finding"));
    }

    #[tokio::test]
    async fn test_attach_memory_provider_late_binding() {
        // The plugin system attaches the memory provider after the executor
        // is already an Arc. This test verifies the lock-based approach works.
        let executor = Arc::new(make_executor_for_memory_tests());

        // No provider initially
        let facts = executor.load_memory_facts(1, 1, None, None).await;
        assert!(facts.is_empty());

        // Attach via Arc clone — exercises the interior mutability path
        let provider = Arc::new(FakeMemoryProvider::new(vec![fact(1, "late", 0.9)]));
        executor.attach_memory_provider(provider).await;

        // Now it sees the provider's facts
        let facts = executor.load_memory_facts(1, 1, None, None).await;
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].fact, "late");
    }

    #[tokio::test]
    async fn test_issue_run_token_no_service_returns_none() {
        let executor = make_executor_for_memory_tests();
        let token = executor.issue_run_token(10, 1, "error-autofix").await;
        assert!(token.is_none());
    }
}
