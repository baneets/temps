use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Extension, Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

use temps_auth::{permission_guard, RequireAuth};
use temps_core::audit::{AuditContext, AuditOperation};
use temps_core::problemdetails::{self, Problem};
use temps_core::RequestMetadata;
use temps_entities::project_agents;

use crate::error::AgentError;
use crate::handlers::AppState;
use crate::services::config_service::UpsertAgentRequest;

impl From<AgentError> for Problem {
    fn from(error: AgentError) -> Self {
        match error {
            AgentError::ConfigNotFound { .. } => problemdetails::new(StatusCode::NOT_FOUND)
                .with_title("Autopilot Config Not Found")
                .with_detail(error.to_string()),
            AgentError::AgentNotFound { .. } => problemdetails::new(StatusCode::NOT_FOUND)
                .with_title("Agent Not Found")
                .with_detail(error.to_string()),
            AgentError::RunNotFound { .. } => problemdetails::new(StatusCode::NOT_FOUND)
                .with_title("Autopilot Run Not Found")
                .with_detail(error.to_string()),
            AgentError::ProjectNotFound { .. } => problemdetails::new(StatusCode::NOT_FOUND)
                .with_title("Project Not Found")
                .with_detail(error.to_string()),
            AgentError::BudgetExceeded { .. } => problemdetails::new(StatusCode::PAYMENT_REQUIRED)
                .with_title("Daily Budget Exceeded")
                .with_detail(error.to_string()),
            AgentError::CooldownActive { .. } => problemdetails::new(StatusCode::TOO_MANY_REQUESTS)
                .with_title("Cooldown Active")
                .with_detail(error.to_string()),
            AgentError::AiCliNotInstalled { .. } => {
                problemdetails::new(StatusCode::UNPROCESSABLE_ENTITY)
                    .with_title("AI CLI Not Installed")
                    .with_detail(error.to_string())
            }
            AgentError::AiCliFailed { .. } => {
                problemdetails::new(StatusCode::INTERNAL_SERVER_ERROR)
                    .with_title("AI CLI Failed")
                    .with_detail(error.to_string())
            }
            AgentError::AiCliTimeout { .. } => problemdetails::new(StatusCode::GATEWAY_TIMEOUT)
                .with_title("AI CLI Timeout")
                .with_detail(error.to_string()),
            AgentError::GitError { .. } => problemdetails::new(StatusCode::INTERNAL_SERVER_ERROR)
                .with_title("Git Error")
                .with_detail(error.to_string()),
            AgentError::EncryptionError { .. } => {
                problemdetails::new(StatusCode::INTERNAL_SERVER_ERROR)
                    .with_title("Encryption Error")
                    .with_detail(error.to_string())
            }
            AgentError::Validation { .. } => problemdetails::new(StatusCode::BAD_REQUEST)
                .with_title("Validation Error")
                .with_detail(error.to_string()),
            AgentError::Database(_) => problemdetails::new(StatusCode::INTERNAL_SERVER_ERROR)
                .with_title("Internal Server Error")
                .with_detail(error.to_string()),
            AgentError::Io(_) => problemdetails::new(StatusCode::INTERNAL_SERVER_ERROR)
                .with_title("Internal Server Error")
                .with_detail(error.to_string()),
            AgentError::SandboxCreationFailed { .. } => {
                problemdetails::new(StatusCode::INTERNAL_SERVER_ERROR)
                    .with_title("Sandbox Creation Failed")
                    .with_detail(error.to_string())
            }
            AgentError::SandboxNotFound { .. } => {
                problemdetails::new(StatusCode::INTERNAL_SERVER_ERROR)
                    .with_title("Sandbox Not Found")
                    .with_detail(error.to_string())
            }
            AgentError::SandboxExecFailed { .. } => {
                problemdetails::new(StatusCode::INTERNAL_SERVER_ERROR)
                    .with_title("Sandbox Exec Failed")
                    .with_detail(error.to_string())
            }
            AgentError::SandboxProviderUnavailable { .. } => {
                problemdetails::new(StatusCode::SERVICE_UNAVAILABLE)
                    .with_title("Sandbox Provider Unavailable")
                    .with_detail(error.to_string())
            }
        }
    }
}

// ── Audit structs ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
struct AgentCreatedAudit {
    context: AuditContext,
    project_id: i32,
    agent_id: i32,
    slug: String,
}

#[derive(Debug, Clone, Serialize)]
struct AgentUpdatedAudit {
    context: AuditContext,
    project_id: i32,
    agent_id: i32,
    slug: String,
}

#[derive(Debug, Clone, Serialize)]
struct AgentDeletedAudit {
    context: AuditContext,
    project_id: i32,
    slug: String,
}

impl AuditOperation for AgentCreatedAudit {
    fn operation_type(&self) -> String {
        "AGENT_CREATED".to_string()
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

impl AuditOperation for AgentUpdatedAudit {
    fn operation_type(&self) -> String {
        "AGENT_UPDATED".to_string()
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

impl AuditOperation for AgentDeletedAudit {
    fn operation_type(&self) -> String {
        "AGENT_DELETED".to_string()
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

// ── Response DTOs ─────────────────────────────────────────────────────────────

/// Response DTO for a single agent — masks the encrypted API key.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct AgentConfigResponse {
    pub id: i32,
    pub project_id: i32,
    pub slug: String,
    pub name: String,
    pub description: Option<String>,
    pub source: String,
    pub enabled: bool,
    pub trigger_config: serde_json::Value,
    pub prompt: Option<String>,
    pub ai_provider: String,
    /// `true` if an API key is set; `false` otherwise.
    pub api_key_set: bool,
    pub ai_provider_key_id: Option<i32>,
    pub max_turns: i32,
    pub timeout_seconds: i32,
    pub daily_budget_cents: i32,
    pub cooldown_minutes: i32,
    pub branch_prefix: String,
    pub deliverable: String,
    pub sandbox_enabled: bool,
    pub created_at: String,
    pub updated_at: String,
}

impl From<project_agents::Model> for AgentConfigResponse {
    fn from(model: project_agents::Model) -> Self {
        Self {
            id: model.id,
            project_id: model.project_id,
            slug: model.slug,
            name: model.name,
            description: model.description,
            source: model.source,
            enabled: model.enabled,
            trigger_config: model.trigger_config,
            prompt: model.prompt,
            ai_provider: model.ai_provider,
            api_key_set: model.api_key_encrypted.is_some(),
            ai_provider_key_id: model.ai_provider_key_id,
            max_turns: model.max_turns,
            timeout_seconds: model.timeout_seconds,
            daily_budget_cents: model.daily_budget_cents,
            cooldown_minutes: model.cooldown_minutes,
            branch_prefix: model.branch_prefix,
            deliverable: model.deliverable,
            sandbox_enabled: model.sandbox_enabled,
            created_at: model.created_at.to_rfc3339(),
            updated_at: model.updated_at.to_rfc3339(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ListAgentsResponse {
    pub items: Vec<AgentConfigResponse>,
    pub total: usize,
}

// ── Routes ────────────────────────────────────────────────────────────────────

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/projects/{project_id}/agents",
            get(list_agents).post(create_agent),
        )
        .route(
            "/projects/{project_id}/agents/{slug}",
            get(get_agent).put(update_agent).delete(delete_agent),
        )
}

// ── Handlers ──────────────────────────────────────────────────────────────────

#[utoipa::path(
    tag = "Agents",
    get,
    path = "/projects/{project_id}/agents",
    params(
        ("project_id" = i32, Path, description = "Project ID"),
    ),
    responses(
        (status = 200, description = "List of agents for project", body = ListAgentsResponse),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Insufficient permissions"),
        (status = 500, description = "Internal server error"),
    ),
    security(("bearer_auth" = []))
)]
async fn list_agents(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<AppState>>,
    Path(project_id): Path<i32>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, ProjectsRead);

    let agents = app_state
        .config_service
        .list_agents(project_id)
        .await
        .map_err(Problem::from)?;

    let total = agents.len();
    Ok(Json(ListAgentsResponse {
        items: agents.into_iter().map(AgentConfigResponse::from).collect(),
        total,
    }))
}

#[utoipa::path(
    tag = "Agents",
    post,
    path = "/projects/{project_id}/agents",
    params(
        ("project_id" = i32, Path, description = "Project ID"),
    ),
    request_body = UpsertAgentRequest,
    responses(
        (status = 201, description = "Agent created", body = AgentConfigResponse),
        (status = 400, description = "Validation error"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Insufficient permissions"),
        (status = 500, description = "Internal server error"),
    ),
    security(("bearer_auth" = []))
)]
async fn create_agent(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<AppState>>,
    Extension(metadata): Extension<RequestMetadata>,
    Path(project_id): Path<i32>,
    Json(request): Json<UpsertAgentRequest>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, ProjectsWrite);

    let agent = app_state
        .config_service
        .create_agent(project_id, request)
        .await
        .map_err(Problem::from)?;

    let audit = AgentCreatedAudit {
        context: AuditContext {
            user_id: auth.user_id(),
            ip_address: Some(metadata.ip_address.clone()),
            user_agent: metadata.user_agent.clone(),
        },
        project_id,
        agent_id: agent.id,
        slug: agent.slug.clone(),
    };
    if let Err(e) = app_state.audit_service.create_audit_log(&audit).await {
        tracing::error!(
            "Failed to create audit log for agent creation (project {}, slug {}): {}",
            project_id,
            agent.slug,
            e
        );
    }

    Ok((StatusCode::CREATED, Json(AgentConfigResponse::from(agent))))
}

#[utoipa::path(
    tag = "Agents",
    get,
    path = "/projects/{project_id}/agents/{slug}",
    params(
        ("project_id" = i32, Path, description = "Project ID"),
        ("slug" = String, Path, description = "Agent slug"),
    ),
    responses(
        (status = 200, description = "Agent config", body = AgentConfigResponse),
        (status = 404, description = "Agent not found"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Insufficient permissions"),
    ),
    security(("bearer_auth" = []))
)]
async fn get_agent(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<AppState>>,
    Path((project_id, slug)): Path<(i32, String)>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, ProjectsRead);

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

    Ok(Json(AgentConfigResponse::from(agent)))
}

#[utoipa::path(
    tag = "Agents",
    put,
    path = "/projects/{project_id}/agents/{slug}",
    params(
        ("project_id" = i32, Path, description = "Project ID"),
        ("slug" = String, Path, description = "Agent slug"),
    ),
    request_body = UpsertAgentRequest,
    responses(
        (status = 200, description = "Agent updated", body = AgentConfigResponse),
        (status = 400, description = "Validation error"),
        (status = 404, description = "Agent not found"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Insufficient permissions"),
        (status = 500, description = "Internal server error"),
    ),
    security(("bearer_auth" = []))
)]
async fn update_agent(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<AppState>>,
    Extension(metadata): Extension<RequestMetadata>,
    Path((project_id, slug)): Path<(i32, String)>,
    Json(request): Json<UpsertAgentRequest>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, ProjectsWrite);

    let agent = app_state
        .config_service
        .update_agent(project_id, &slug, request)
        .await
        .map_err(Problem::from)?;

    let audit = AgentUpdatedAudit {
        context: AuditContext {
            user_id: auth.user_id(),
            ip_address: Some(metadata.ip_address.clone()),
            user_agent: metadata.user_agent.clone(),
        },
        project_id,
        agent_id: agent.id,
        slug: agent.slug.clone(),
    };
    if let Err(e) = app_state.audit_service.create_audit_log(&audit).await {
        tracing::error!(
            "Failed to create audit log for agent update (project {}, slug {}): {}",
            project_id,
            agent.slug,
            e
        );
    }

    Ok(Json(AgentConfigResponse::from(agent)))
}

#[utoipa::path(
    tag = "Agents",
    delete,
    path = "/projects/{project_id}/agents/{slug}",
    params(
        ("project_id" = i32, Path, description = "Project ID"),
        ("slug" = String, Path, description = "Agent slug"),
    ),
    responses(
        (status = 204, description = "Agent deleted"),
        (status = 404, description = "Agent not found"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Insufficient permissions"),
        (status = 500, description = "Internal server error"),
    ),
    security(("bearer_auth" = []))
)]
async fn delete_agent(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<AppState>>,
    Extension(metadata): Extension<RequestMetadata>,
    Path((project_id, slug)): Path<(i32, String)>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, ProjectsWrite);

    app_state
        .config_service
        .delete_agent(project_id, &slug)
        .await
        .map_err(Problem::from)?;

    let audit = AgentDeletedAudit {
        context: AuditContext {
            user_id: auth.user_id(),
            ip_address: Some(metadata.ip_address.clone()),
            user_agent: metadata.user_agent.clone(),
        },
        project_id,
        slug: slug.clone(),
    };
    if let Err(e) = app_state.audit_service.create_audit_log(&audit).await {
        tracing::error!(
            "Failed to create audit log for agent delete (project {}, slug {}): {}",
            project_id,
            &slug,
            e
        );
    }

    Ok(StatusCode::NO_CONTENT)
}
