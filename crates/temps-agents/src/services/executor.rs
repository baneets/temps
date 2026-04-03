use chrono::Utc;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, Order, QueryFilter, QueryOrder};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::fs;
use tokio::process::Command;

use temps_core::jobs::GitPushEventJob;
use temps_core::{EncryptionService, Job, JobQueue};
use temps_entities::{error_events, error_groups, projects};
use temps_git::services::git_provider_manager_trait::GitProviderManagerTrait;
use temps_notifications::services::NotificationService;
use temps_notifications::types::{Notification, NotificationPriority};

use crate::ai_cli::{create_provider, AiCliProvider, AiRunConfig, AiRunResult};
use crate::error::AgentError;

/// Wrapper that lets an `Arc<dyn AiCliProvider>` be used as `Box<dyn AiCliProvider>`.
struct ArcAiCliProvider(Arc<dyn AiCliProvider>);

#[async_trait::async_trait]
impl AiCliProvider for ArcAiCliProvider {
    fn name(&self) -> &str {
        self.0.name()
    }
    async fn check_installed(&self) -> bool {
        self.0.check_installed().await
    }
    async fn get_status(&self) -> crate::ai_cli::AiCliStatus {
        self.0.get_status().await
    }
    async fn run(&self, config: AiRunConfig) -> Result<AiRunResult, AgentError> {
        self.0.run(config).await
    }
    async fn continue_conversation(&self, config: AiRunConfig) -> Result<AiRunResult, AgentError> {
        self.0.continue_conversation(config).await
    }
}
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
    notification_service: Option<Arc<NotificationService>>,
    /// If set, this provider is used instead of resolving one from config.
    /// Intended for testing — inject a fake provider that simulates AI behaviour.
    ai_provider_override: Option<Arc<dyn AiCliProvider>>,
}

impl AgentExecutor {
    pub fn new(
        db: Arc<DatabaseConnection>,
        git_provider_manager: Arc<dyn GitProviderManagerTrait>,
        encryption_service: Arc<EncryptionService>,
        queue: Arc<dyn JobQueue>,
        run_service: Arc<AgentRunService>,
        config_service: Arc<AgentConfigService>,
        notification_service: Option<Arc<NotificationService>>,
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
        }
    }

    /// For testing: inject a custom AI CLI provider instead of resolving from config.
    pub fn with_ai_provider(mut self, provider: Arc<dyn AiCliProvider>) -> Self {
        self.ai_provider_override = Some(provider);
        self
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

        // Always attempt cleanup of the temp directory
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

        // Step 9: Decrypt API key (if provided — subscription mode doesn't need one)
        let api_key = if let Some(ref encrypted) = config.api_key_encrypted {
            self.encryption_service
                .decrypt_string(encrypted)
                .map_err(|e| AgentError::EncryptionError {
                    message: format!(
                        "Failed to decrypt API key for project {}: {}",
                        run.project_id, e
                    ),
                })?
        } else {
            // No API key stored — using subscription mode.
            // The CLI will use its own built-in auth (claude setup-token).
            String::new()
        };

        // Step 10: Resolve AI CLI provider (use injected override for testing)
        let provider: Box<dyn AiCliProvider> = if let Some(ref p) = self.ai_provider_override {
            Box::new(ArcAiCliProvider(p.clone()))
        } else {
            let p = create_provider(&config.ai_provider).ok_or_else(|| {
                AgentError::AiCliNotInstalled {
                    provider: config.ai_provider.clone(),
                }
            })?;
            if !p.check_installed().await {
                return Err(AgentError::AiCliNotInstalled {
                    provider: config.ai_provider.clone(),
                });
            }
            p
        };

        // Step 11: Update status → "fixing"
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

        // Step 12: Run AI CLI with real-time streaming
        // Save all JSON events to agent_run_logs for real-time streaming.
        // The frontend renders them nicely — raw JSON is never shown to users.
        let run_service_for_stream = self.run_service.clone();
        let stream_run_id = run_id;
        let on_event: crate::ai_cli::OnEventCallback = Arc::new(move |line: String| {
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

        let ai_config = AiRunConfig {
            work_dir: work_dir.clone(),
            prompt,
            api_key,
            max_turns: config.max_turns,
            timeout: Duration::from_secs(config.timeout_seconds as u64),
            on_event: Some(on_event),
        };

        let ai_result = provider.run(ai_config).await?;

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
        let push_job = Job::GitPushEvent(GitPushEventJob {
            owner: project.repo_owner.clone(),
            repo: project.repo_name.clone(),
            branch: Some(branch_name.clone()),
            tag: None,
            commit: pr.head_branch.clone(),
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

        // Step 23: Send notification if available
        if let Some(ref ns) = self.notification_service {
            let notification = Notification::new(
                format!("Autopilot fixed: {} in {}", error_type, project.name),
                format!(
                    "Autopilot run #{} created PR #{} to fix '{}'. Review and merge: {}",
                    run_id, pr.number, error_message, pr.url
                ),
            )
            .with_priority(NotificationPriority::Normal)
            .with_metadata("run_id", run_id.to_string())
            .with_metadata("project", project.name.clone())
            .with_metadata("branch", branch_name);

            if let Err(e) = ns.send_notification(notification).await {
                tracing::warn!(
                    "Run {}: failed to send completion notification: {}",
                    run_id,
                    e
                );
            }
        }

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

        let executor = AgentExecutor::new(db, git, enc, queue.clone(), run_svc, config_svc, None)
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
        let executor = AgentExecutor::new(db, git, enc, queue.clone(), run_svc, config_svc, None)
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

        let executor = AgentExecutor::new(db, git, enc, queue.clone(), run_svc, config_svc, None)
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

        let executor = AgentExecutor::new(db, git, enc, queue.clone(), run_svc, config_svc, None)
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
        let executor = AgentExecutor::new(db, git, enc, queue.clone(), run_svc, config_svc, None)
            .with_ai_provider(ai);

        executor.execute_run(run_id).await;

        // PR should not have been pushed
        assert!(recorder.pushed.lock().unwrap().is_empty());
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
        let executor = AgentExecutor::new(db, git, enc, queue.clone(), run_svc, config_svc, None)
            .with_ai_provider(ai);

        executor.execute_run(run_id).await;

        // Clone should not even have been attempted
        assert!(recorder.cloned.lock().unwrap().is_empty());
        assert!(recorder.pushed.lock().unwrap().is_empty());
    }
}
