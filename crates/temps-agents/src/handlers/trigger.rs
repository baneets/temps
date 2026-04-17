use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse,
    },
    routing::{get, post},
    Extension, Json, Router,
};
use futures::Stream;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

use temps_auth::{permission_guard, RequireAuth};
use temps_core::audit::{AuditContext, AuditOperation};
use temps_core::problemdetails::{self, Problem};
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

// Note: the ephemeral CLI dry-run endpoint (`/projects/{id}/workflows/dry-run`)
// lives in `handlers::workflows` — it owns its own routes, request type, and
// audit struct.

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/projects/{project_id}/agents/{slug}/trigger",
            post(trigger_agent),
        )
        // Public webhook endpoint — X-Webhook-Token header auth
        .route("/agents/webhook/{webhook_id}", post(webhook_trigger))
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

    // If the caller is linking the run to an error group, validate it here so
    // they get a 400 synchronously. The executor also revalidates (and checks
    // cross-project ownership) in `load_error_context`, but failing early
    // surfaces the problem immediately in the HTTP response instead of in a
    // background run that errors out mid-execution.
    if request.trigger_source_type.as_deref() == Some("error_group") {
        use sea_orm::EntityTrait;
        let group_id = request.trigger_source_id.ok_or_else(|| {
            Problem::from(AgentError::Validation {
                message: "trigger_source_id is required when trigger_source_type is 'error_group'"
                    .to_string(),
            })
        })?;
        let group = temps_entities::error_groups::Entity::find_by_id(group_id)
            .one(app_state.db.as_ref())
            .await
            .map_err(AgentError::Database)
            .map_err(Problem::from)?
            .ok_or_else(|| {
                Problem::from(AgentError::Validation {
                    message: format!(
                        "Error group {} not found for project {}",
                        group_id, project_id
                    ),
                })
            })?;
        if group.project_id != project_id {
            return Err(Problem::from(AgentError::Validation {
                message: format!(
                    "Error group {} does not belong to project {}",
                    group_id, project_id
                ),
            }));
        }
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

// ── Public Webhook Trigger ────────────────────────────────────────────────────

#[derive(Debug, Deserialize, ToSchema)]
pub struct WebhookTriggerRequest {
    /// Arbitrary JSON payload from the caller. Passed to the agent as user_context.
    #[serde(flatten)]
    pub payload: serde_json::Value,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct WebhookTriggerResponse {
    pub run_id: i32,
    pub status: String,
}

/// Public webhook endpoint. Authenticated via `X-Webhook-Token` header.
///
/// `POST /api/agents/webhook/{webhook_id}`
/// Header: `X-Webhook-Token: <secret>`
///
/// The `webhook_id` in the URL is a short non-secret identifier (safe to log).
/// The actual credential is the secret token in the header.
///
/// Accepts any JSON body, which is passed as `user_context` to the agent run.
#[utoipa::path(
    tag = "Agents",
    post,
    path = "/agents/webhook/{webhook_id}",
    params(
        ("webhook_id" = String, Path, description = "Webhook ID (non-secret)"),
    ),
    request_body = WebhookTriggerRequest,
    responses(
        (status = 202, description = "Agent run created", body = WebhookTriggerResponse),
        (status = 401, description = "Missing or invalid X-Webhook-Token header"),
        (status = 404, description = "Invalid webhook ID"),
        (status = 422, description = "Agent disabled"),
    ),
)]
async fn webhook_trigger(
    State(app_state): State<Arc<AppState>>,
    Path(webhook_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<WebhookTriggerRequest>,
) -> Result<impl IntoResponse, Problem> {
    // Extract X-Webhook-Token header
    let provided_token = headers
        .get("x-webhook-token")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| {
            problemdetails::new(StatusCode::UNAUTHORIZED)
                .with_title("Missing Webhook Token")
                .with_detail("X-Webhook-Token header is required")
        })?;

    // Look up agent by webhook ID (non-secret URL identifier)
    let agent = app_state
        .config_service
        .get_agent_by_webhook_id(&webhook_id)
        .await
        .map_err(Problem::from)?
        .ok_or_else(|| {
            problemdetails::new(StatusCode::NOT_FOUND)
                .with_title("Invalid Webhook ID")
                .with_detail("No agent found for the provided webhook ID")
        })?;

    // Validate the secret token from the header
    let expected_token = agent.webhook_token.as_deref().ok_or_else(|| {
        problemdetails::new(StatusCode::NOT_FOUND)
            .with_title("Webhook Not Configured")
            .with_detail("This agent does not have webhook triggers enabled")
    })?;

    // Constant-time comparison to prevent timing attacks
    if provided_token.len() != expected_token.len()
        || !provided_token
            .as_bytes()
            .iter()
            .zip(expected_token.as_bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
    {
        return Err(problemdetails::new(StatusCode::UNAUTHORIZED)
            .with_title("Invalid Webhook Token")
            .with_detail("The provided X-Webhook-Token does not match"));
    }

    if !agent.enabled {
        return Err(problemdetails::new(StatusCode::UNPROCESSABLE_ENTITY)
            .with_title("Agent Disabled")
            .with_detail(format!(
                "Agent '{}' is disabled. Enable it before triggering via webhook.",
                agent.slug
            )));
    }

    // Serialize the payload as user_context
    let user_context =
        if request.payload.is_null() || request.payload.as_object().is_some_and(|m| m.is_empty()) {
            None
        } else {
            Some(serde_json::to_string(&request.payload).unwrap_or_default())
        };

    // Create the run
    let run = app_state
        .run_service
        .create_run(
            agent.project_id,
            agent.id,
            "webhook".to_string(),
            None,
            Some("webhook".to_string()),
            user_context,
        )
        .await
        .map_err(Problem::from)?;

    // Spawn the executor
    let executor = app_state.executor.clone();
    let run_id = run.id;
    tokio::spawn(async move {
        executor.execute_run(run_id).await;
    });

    Ok((
        StatusCode::ACCEPTED,
        Json(WebhookTriggerResponse {
            run_id: run.id,
            status: "pending".to_string(),
        }),
    ))
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

async fn rebuild_sandbox_image(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<AppState>>,
) -> Result<Sse<impl Stream<Item = Result<Event, std::convert::Infallible>>>, Problem> {
    permission_guard!(auth, SettingsWrite);

    let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(64);

    // Spawn the build in the background, sending progress to the channel.
    let provider = app_state.executor.sandbox_registry().provider_arc();
    tokio::spawn(async move {
        let result = provider.rebuild_image_with_progress(tx.clone()).await;
        match result {
            Ok(name) => {
                let _ = tx
                    .send(
                        serde_json::json!({
                            "type": "done",
                            "success": true,
                            "image_name": name,
                        })
                        .to_string(),
                    )
                    .await;
            }
            Err(e) => {
                let _ = tx
                    .send(
                        serde_json::json!({
                            "type": "done",
                            "success": false,
                            "error": e.to_string(),
                        })
                        .to_string(),
                    )
                    .await;
            }
        }
    });

    let stream = async_stream::stream! {
        while let Some(msg) = rx.recv().await {
            yield Ok(Event::default().data(msg));
        }
    };

    Ok(Sse::new(stream).keep_alive(sse_keep_alive()))
}

/// Shared SSE keep-alive: sends `: heartbeat` every 15s so clients behind
/// slow networks or idle proxies detect dropped connections quickly. See
/// `handlers::runs::sse_keep_alive` for the full rationale.
fn sse_keep_alive() -> KeepAlive {
    KeepAlive::new()
        .interval(std::time::Duration::from_secs(15))
        .text("heartbeat")
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
            container_name_override: None,
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

// `list_available_models` was retired in favour of the per-provider
// `models` field on the `/settings/ai-providers` catalog response. Each
// provider declares its own valid model ids in `ai_cli::catalog`, which
// keeps Claude/Codex/OpenCode from sharing one global list (and from
// shelling out to a host-side `opencode` binary that often isn't there).
