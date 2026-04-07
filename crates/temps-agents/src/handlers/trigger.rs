use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Extension, Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

use temps_auth::{permission_guard, RequireAuth};
use temps_core::audit::{AuditContext, AuditOperation};
use temps_core::problemdetails::Problem;
use temps_core::RequestMetadata;

use crate::ai_cli::{self, AiCliStatus};
use crate::error::AgentError;
use crate::handlers::runs::AgentRunResponse;
use crate::handlers::AppState;

#[derive(Debug, Deserialize, ToSchema)]
pub struct TriggerAgentRequest {
    pub trigger_source_type: Option<String>,
    pub trigger_source_id: Option<i32>,
    /// Optional context from the user (e.g. a research topic, bug description, or instructions).
    pub user_context: Option<String>,
}

// ── Audit ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
struct AgentRunTriggeredAudit {
    context: AuditContext,
    project_id: i32,
    agent_slug: String,
    run_id: i32,
    trigger_type: String,
}

impl AuditOperation for AgentRunTriggeredAudit {
    fn operation_type(&self) -> String {
        "AGENT_RUN_TRIGGERED".to_string()
    }
    fn user_id(&self) -> i32 {
        self.context.user_id
    }
    fn ip_address(&self) -> Option<String> {
        self.context.ip_address.clone()
    }
    fn user_agent(&self) -> &str {
        &self.context.user_agent
    }
    fn serialize(&self) -> temps_core::anyhow::Result<String> {
        serde_json::to_string(self)
            .map_err(|e| temps_core::anyhow::anyhow!("Failed to serialize audit: {}", e))
    }
}

// ── Routes ────────────────────────────────────────────────────────────────────

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/projects/{project_id}/agents/{slug}/trigger",
            post(trigger_agent),
        )
        .route(
            "/projects/{project_id}/agents/cli-status",
            get(get_cli_status),
        )
        .route(
            "/projects/{project_id}/agents/sandbox-status",
            get(get_sandbox_status),
        )
        // Global sandbox status and management (for settings page)
        .route("/settings/sandbox-status", get(get_global_sandbox_status))
        .route("/settings/sandbox-rebuild", post(rebuild_sandbox_image))
        .route(
            "/projects/{project_id}/agents/smoke-test",
            post(smoke_test_agent),
        )
        .route("/settings/agent-token", post(save_agent_token))
        .route("/settings/agent-models", get(list_available_models))
}

// ── Handlers ──────────────────────────────────────────────────────────────────

#[utoipa::path(
    tag = "Agents",
    post,
    path = "/projects/{project_id}/agents/{slug}/trigger",
    params(
        ("project_id" = i32, Path, description = "Project ID"),
        ("slug" = String, Path, description = "Agent slug"),
    ),
    request_body = TriggerAgentRequest,
    responses(
        (status = 202, description = "Agent run created and queued", body = AgentRunResponse),
        (status = 400, description = "Validation error"),
        (status = 402, description = "Daily budget exceeded"),
        (status = 404, description = "Agent not found"),
        (status = 422, description = "AI CLI not installed"),
        (status = 429, description = "Cooldown active"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Insufficient permissions"),
        (status = 500, description = "Internal server error"),
    ),
    security(("bearer_auth" = []))
)]
async fn trigger_agent(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<AppState>>,
    Extension(metadata): Extension<RequestMetadata>,
    Path((project_id, slug)): Path<(i32, String)>,
    Json(request): Json<TriggerAgentRequest>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, ProjectsWrite);

    // Load agent to get config_id and verify it is enabled
    let agent = app_state
        .config_service
        .get_agent_by_slug(project_id, &slug)
        .await
        .map_err(Problem::from)?
        .ok_or_else(|| {
            Problem::from(AgentError::AgentNotFound {
                project_id,
                slug: slug.clone(),
            })
        })?;

    if !agent.enabled {
        return Err(Problem::from(AgentError::Validation {
            message: format!(
                "Agent '{}' is disabled for project {}. Enable it in the config first.",
                slug, project_id
            ),
        }));
    }

    // Create the run record with "pending" status
    let run = app_state
        .run_service
        .create_run(
            project_id,
            agent.id,
            "manual".to_string(),
            request.trigger_source_id,
            request.trigger_source_type.clone(),
            request.user_context,
        )
        .await
        .map_err(Problem::from)?;

    // Spawn the executor in the background
    // TODO: Manual triggers should use the two-phase interactive flow (analyze → review → fix)
    // instead of autonomous execution. This requires unifying the agent executor with the
    // autofixer's workflow engine. For now, manual triggers run autonomously.
    let executor = app_state.executor.clone();
    let run_id = run.id;
    tokio::spawn(async move {
        executor.execute_run(run_id).await;
    });

    // Audit log (non-fatal)
    let audit = AgentRunTriggeredAudit {
        context: AuditContext {
            user_id: auth.user_id(),
            ip_address: Some(metadata.ip_address.clone()),
            user_agent: metadata.user_agent.clone(),
        },
        project_id,
        agent_slug: slug.clone(),
        run_id: run.id,
        trigger_type: "manual".to_string(),
    };
    if let Err(e) = app_state.audit_service.create_audit_log(&audit).await {
        tracing::error!(
            "Failed to create audit log for agent trigger (project {}, slug {}, run {}): {}",
            project_id,
            slug,
            run.id,
            e
        );
    }

    // Resolve sandbox against global setting
    let global_sandbox_enabled = {
        use sea_orm::EntityTrait;
        temps_entities::settings::Entity::find_by_id(1)
            .one(app_state.db.as_ref())
            .await
            .ok()
            .flatten()
            .and_then(|s| {
                s.data
                    .get("agent_sandbox")
                    .and_then(|v| v.get("enabled"))
                    .and_then(|v| v.as_bool())
            })
            .unwrap_or(false)
    };
    let run_resp = AgentRunResponse::from_with_agent(
        run,
        Some(agent.slug),
        Some(agent.name),
        agent.sandbox_enabled.unwrap_or(global_sandbox_enabled),
    );

    Ok((StatusCode::ACCEPTED, Json(run_resp)))
}

// ── CLI Status ────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct CliStatusQuery {
    /// AI provider to check: "claude_cli" or "codex_cli". Defaults to "claude_cli".
    pub provider: Option<String>,
}

#[utoipa::path(
    tag = "Agents",
    get,
    path = "/projects/{project_id}/agents/cli-status",
    params(
        ("project_id" = i32, Path, description = "Project ID"),
        ("provider" = Option<String>, Query, description = "AI provider: claude_cli or codex_cli"),
    ),
    responses(
        (status = 200, description = "CLI status"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Insufficient permissions"),
    ),
    security(("bearer_auth" = []))
)]
async fn get_cli_status(
    RequireAuth(auth): RequireAuth,
    State(_app_state): State<Arc<AppState>>,
    Path(_project_id): Path<i32>,
    Query(query): Query<CliStatusQuery>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, ProjectsRead);

    let provider_name = query.provider.as_deref().unwrap_or("claude_cli");

    let status = match ai_cli::create_provider(provider_name) {
        Some(provider) => provider.get_status().await,
        None => AiCliStatus {
            provider: provider_name.into(),
            installed: false,
            version: None,
            authenticated: false,
            auth_method: None,
            email: None,
            subscription_type: None,
            setup_hint: Some(format!(
                "Unknown AI provider '{}'. Supported: claude_cli, opencode, codex_cli",
                provider_name
            )),
        },
    };

    Ok(Json(status))
}

// ── Sandbox Status ───────────────────────────────────────────────────────────

#[derive(Debug, Serialize, ToSchema)]
struct SandboxStatusResponse {
    docker_available: bool,
    image_ready: bool,
    image_name: String,
    error: Option<String>,
}

async fn get_sandbox_status(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<AppState>>,
    Path(_project_id): Path<i32>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, ProjectsRead);

    let sandbox_registry = &app_state.executor.sandbox_registry();
    let provider = sandbox_registry.provider();

    // Check Docker connectivity
    let docker_available = provider.is_available().await;

    // Check if sandbox image exists
    let (image_ready, image_name, error) = if docker_available {
        match provider.image_status().await {
            Ok((ready, name)) => (ready, name, None),
            Err(e) => (false, String::new(), Some(e.to_string())),
        }
    } else {
        (
            false,
            String::new(),
            Some("Docker is not available on this server".to_string()),
        )
    };

    Ok(Json(SandboxStatusResponse {
        docker_available,
        image_ready,
        image_name,
        error,
    }))
}

async fn get_global_sandbox_status(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, SettingsRead);

    let provider = app_state.executor.sandbox_registry().provider();
    let docker_available = provider.is_available().await;

    let (image_ready, image_name, error) = if docker_available {
        match provider.image_status().await {
            Ok((ready, name)) => (ready, name, None),
            Err(e) => (false, String::new(), Some(e.to_string())),
        }
    } else {
        (
            false,
            String::new(),
            Some("Docker is not available on this server".to_string()),
        )
    };

    Ok(Json(SandboxStatusResponse {
        docker_available,
        image_ready,
        image_name,
        error,
    }))
}

#[derive(Debug, Serialize, ToSchema)]
struct SandboxRebuildResponse {
    success: bool,
    image_name: String,
    error: Option<String>,
}

async fn rebuild_sandbox_image(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, SettingsWrite);

    let provider = app_state.executor.sandbox_registry().provider();

    match provider.rebuild_image().await {
        Ok(image_name) => Ok(Json(SandboxRebuildResponse {
            success: true,
            image_name,
            error: None,
        })),
        Err(e) => Ok(Json(SandboxRebuildResponse {
            success: false,
            image_name: String::new(),
            error: Some(e.to_string()),
        })),
    }
}

// ── Smoke Test ───────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, ToSchema)]
struct SmokeTestResponse {
    /// Whether the smoke test passed
    passed: bool,
    /// Where the test ran: "host" or "sandbox"
    environment: String,
    /// Claude CLI installed?
    cli_installed: bool,
    /// Claude CLI authenticated?
    cli_authenticated: bool,
    /// Claude CLI version
    cli_version: Option<String>,
    /// Auth email / method
    auth_info: Option<String>,
    /// What the user needs to do if the test failed
    setup_hint: Option<String>,
    /// Full output for debugging
    detail: Option<String>,
}

/// Run a smoke test to verify Claude CLI works in the environment where agents
/// will actually execute (host or sandbox container).
async fn smoke_test_agent(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<AppState>>,
    Path(_project_id): Path<i32>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, ProjectsWrite);

    // Check if sandbox is enabled globally
    let global_sandbox = {
        use sea_orm::EntityTrait;
        temps_entities::settings::Entity::find_by_id(1)
            .one(app_state.db.as_ref())
            .await
            .ok()
            .flatten()
            .and_then(|s| {
                s.data.get("agent_sandbox").cloned().and_then(|v| {
                    serde_json::from_value::<temps_core::AgentSandboxSettings>(v).ok()
                })
            })
            .unwrap_or_default()
    };

    if global_sandbox.enabled {
        // Test inside a sandbox container
        let registry = app_state.executor.sandbox_registry();
        let provider = registry.provider();

        if !provider.is_available().await {
            return Ok(Json(SmokeTestResponse {
                passed: false,
                environment: "sandbox".into(),
                cli_installed: false,
                cli_authenticated: false,
                cli_version: None,
                auth_info: None,
                setup_hint: Some(
                    "Docker is not available. Install Docker or disable sandbox mode.".into(),
                ),
                detail: None,
            }));
        }

        // Create a temporary sandbox for the smoke test
        let test_run_id = 99999;
        let work_dir = std::env::temp_dir().join("agent-smoke-test");
        let _ = tokio::fs::create_dir_all(&work_dir).await;

        // Inject the saved credential so the smoke test reflects real auth state
        let mut test_env = std::collections::HashMap::new();
        if let Some(ref encrypted_key) = global_sandbox.api_key_encrypted {
            if let Ok(key) = app_state.encryption_service.decrypt_string(encrypted_key) {
                if global_sandbox.auth_type == "subscription" {
                    test_env.insert("CLAUDE_CODE_OAUTH_TOKEN".to_string(), key);
                } else {
                    match global_sandbox.default_provider.as_str() {
                        "codex_cli" => {
                            test_env.insert("OPENAI_API_KEY".to_string(), key);
                        }
                        _ => {
                            test_env.insert("ANTHROPIC_API_KEY".to_string(), key);
                        }
                    }
                }
            }
        }

        let image = format!("temps-sandbox-{}:latest", global_sandbox.runtime);
        let sandbox_config = crate::sandbox::SandboxCreateConfig {
            run_id: test_run_id,
            host_work_dir: work_dir.clone(),
            image: Some(image),
            cpu_limit: Some(1.0),
            memory_limit_mb: Some(512),
            pids_limit: None,
            network_mode: Some("host".to_string()),
            env_vars: test_env,
            idle_timeout: std::time::Duration::from_secs(60),
        };

        let _handle = match registry.get_or_create(sandbox_config).await {
            Ok(h) => h,
            Err(e) => {
                return Ok(Json(SmokeTestResponse {
                    passed: false,
                    environment: "sandbox".into(),
                    cli_installed: false,
                    cli_authenticated: false,
                    cli_version: None,
                    auth_info: None,
                    setup_hint: Some(format!(
                        "Failed to create sandbox: {}. Try rebuilding the image.",
                        e
                    )),
                    detail: None,
                }));
            }
        };

        // Run `claude auth status` inside the sandbox
        let result = registry
            .exec(
                test_run_id,
                vec![
                    "claude".to_string(),
                    "auth".to_string(),
                    "status".to_string(),
                ],
                std::collections::HashMap::new(),
                None,
            )
            .await;

        // Clean up
        let _ = registry.release(test_run_id).await;
        let _ = tokio::fs::remove_dir_all(&work_dir).await;

        match result {
            Ok(exec_result) => {
                let output = exec_result.stdout.trim().to_string();
                let (authenticated, version, auth_info) = parse_auth_status(&output);
                let cli_installed = exec_result.exit_code != 127; // 127 = command not found

                Ok(Json(SmokeTestResponse {
                    passed: authenticated,
                    environment: "sandbox".into(),
                    cli_installed,
                    cli_authenticated: authenticated,
                    cli_version: version,
                    auth_info,
                    setup_hint: if !authenticated {
                        Some("Claude CLI inside the sandbox is not authenticated. Run 'claude setup-token' on the host (credentials are copied into the sandbox), or set ANTHROPIC_API_KEY as an environment variable.".into())
                    } else {
                        None
                    },
                    detail: Some(output),
                }))
            }
            Err(e) => Ok(Json(SmokeTestResponse {
                passed: false,
                environment: "sandbox".into(),
                cli_installed: false,
                cli_authenticated: false,
                cli_version: None,
                auth_info: None,
                setup_hint: Some(format!("Smoke test failed: {}", e)),
                detail: None,
            })),
        }
    } else {
        // Test on host
        let status = ai_cli::create_provider("claude_cli")
            .map(|p| futures::executor::block_on(p.get_status()));

        match status {
            Some(s) => Ok(Json(SmokeTestResponse {
                passed: s.authenticated,
                environment: "host".into(),
                cli_installed: s.installed,
                cli_authenticated: s.authenticated,
                cli_version: s.version,
                auth_info: s.email.or(s.auth_method),
                setup_hint: s.setup_hint,
                detail: None,
            })),
            None => Ok(Json(SmokeTestResponse {
                passed: false,
                environment: "host".into(),
                cli_installed: false,
                cli_authenticated: false,
                cli_version: None,
                auth_info: None,
                setup_hint: Some(
                    "Claude CLI not found. Install: npm install -g @anthropic-ai/claude-code"
                        .into(),
                ),
                detail: None,
            })),
        }
    }
}

fn parse_auth_status(output: &str) -> (bool, Option<String>, Option<String>) {
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(output) {
        let authenticated = json
            .get("loggedIn")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let email = json.get("email").and_then(|v| v.as_str()).map(String::from);
        let version = json
            .get("version")
            .and_then(|v| v.as_str())
            .map(String::from);
        (authenticated, version, email)
    } else {
        // Fallback: check for "Logged in" in plain text
        let authenticated = output.contains("Logged in") || output.contains("loggedIn");
        (authenticated, None, None)
    }
}

// ── Agent Token ──────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, ToSchema)]
struct SaveAgentTokenRequest {
    /// The OAuth token from `claude setup-token` or an API key.
    /// Will be encrypted before storage.
    token: String,
}

#[derive(Debug, Serialize, ToSchema)]
struct SaveAgentTokenResponse {
    saved: bool,
}

/// Save an encrypted AI provider token for use in sandbox containers.
async fn save_agent_token(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<AppState>>,
    Json(request): Json<SaveAgentTokenRequest>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, SettingsWrite);

    use sea_orm::EntityTrait;

    // Encrypt the token
    let encrypted = app_state
        .encryption_service
        .encrypt_string(&request.token)
        .map_err(|e| {
            Problem::from(AgentError::EncryptionError {
                message: format!("Failed to encrypt token: {}", e),
            })
        })?;

    // Load current settings
    let record = temps_entities::settings::Entity::find_by_id(1)
        .one(app_state.db.as_ref())
        .await
        .map_err(|e| Problem::from(AgentError::Database(e)))?;

    let mut settings_data = record
        .map(|r| r.data)
        .unwrap_or_else(|| serde_json::json!({}));

    // Update the agent_sandbox.api_key_encrypted field
    if let Some(sandbox) = settings_data.get_mut("agent_sandbox") {
        sandbox["api_key_encrypted"] = serde_json::Value::String(encrypted);
    } else {
        settings_data["agent_sandbox"] = serde_json::json!({ "api_key_encrypted": encrypted });
    }

    // Save back
    use sea_orm::{ActiveModelTrait, Set};
    let active = temps_entities::settings::ActiveModel {
        id: Set(1),
        data: Set(settings_data),
        ..Default::default()
    };
    active
        .update(app_state.db.as_ref())
        .await
        .map_err(|e| Problem::from(AgentError::Database(e)))?;

    Ok(Json(SaveAgentTokenResponse { saved: true }))
}

// ── Available Models ─────────────────────────────────────────────────────────

#[derive(Debug, Serialize, ToSchema)]
struct AvailableModelsResponse {
    provider: String,
    models: Vec<String>,
}

/// List available models from the installed AI CLI.
async fn list_available_models(
    RequireAuth(auth): RequireAuth,
    State(_app_state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, SettingsRead);

    use std::process::Stdio;
    use tokio::process::Command;

    // Try opencode models first (gives structured list)
    let opencode_result = Command::new("opencode")
        .args(["models"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await;

    if let Ok(output) = opencode_result {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let models: Vec<String> = stdout
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect();
            return Ok(Json(AvailableModelsResponse {
                provider: "opencode".into(),
                models,
            }));
        }
    }

    // Fallback: hardcoded list for Claude CLI (no models command)
    Ok(Json(AvailableModelsResponse {
        provider: "claude_cli".into(),
        models: vec![
            "claude-sonnet-4-6".into(),
            "claude-opus-4-6".into(),
            "claude-haiku-4-5".into(),
            "sonnet".into(),
            "opus".into(),
            "haiku".into(),
        ],
    }))
}
