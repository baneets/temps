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

    let run_resp = AgentRunResponse::from_with_agent(
        run,
        Some(agent.slug),
        Some(agent.name),
        agent.sandbox_enabled.unwrap_or(false),
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
                "Unknown AI provider '{}'. Supported: claude_cli, codex_cli",
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
