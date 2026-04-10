use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

use temps_auth::{permission_guard, RequireAuth};
use temps_core::problemdetails::Problem;

use crate::handlers::AppState;
use crate::services::definition_service::{
    CreateMcpDefinitionRequest, CreateSkillDefinitionRequest, UpdateMcpDefinitionRequest,
    UpdateSkillDefinitionRequest,
};

// ── Response DTOs ───────────────────────────────────────────────────────────

#[derive(Debug, Serialize, ToSchema)]
pub struct SkillDefinitionResponse {
    pub id: i32,
    pub project_id: i32,
    pub slug: String,
    pub name: String,
    pub description: Option<String>,
    pub content: String,
    pub created_at: String,
    pub updated_at: String,
}

impl From<temps_entities::project_skill_definitions::Model> for SkillDefinitionResponse {
    fn from(m: temps_entities::project_skill_definitions::Model) -> Self {
        Self {
            id: m.id,
            project_id: m.project_id,
            slug: m.slug,
            name: m.name,
            description: m.description,
            content: m.content,
            created_at: m.created_at.to_rfc3339(),
            updated_at: m.updated_at.to_rfc3339(),
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct McpDefinitionResponse {
    pub id: i32,
    pub project_id: i32,
    pub slug: String,
    pub name: String,
    pub description: Option<String>,
    pub config: serde_json::Value,
    pub created_at: String,
    pub updated_at: String,
}

impl From<temps_entities::project_mcp_definitions::Model> for McpDefinitionResponse {
    fn from(m: temps_entities::project_mcp_definitions::Model) -> Self {
        Self {
            id: m.id,
            project_id: m.project_id,
            slug: m.slug,
            name: m.name,
            description: m.description,
            config: m.config,
            created_at: m.created_at.to_rfc3339(),
            updated_at: m.updated_at.to_rfc3339(),
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ListResponse<T: Serialize> {
    pub items: Vec<T>,
    pub total: usize,
}

// ── Request DTOs (re-export for OpenAPI) ────────────────────────────────────

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateSkillRequest {
    pub slug: String,
    pub name: String,
    pub description: Option<String>,
    pub content: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateSkillRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub content: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateMcpRequest {
    pub slug: String,
    pub name: String,
    pub description: Option<String>,
    pub config: serde_json::Value,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateMcpRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub config: Option<serde_json::Value>,
}

// ── Routes ──────────────────────────────────────────────────────────────────

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        // Skills
        .route(
            "/projects/{project_id}/skills",
            get(list_skills).post(create_skill),
        )
        .route(
            "/projects/{project_id}/skills/{slug}",
            get(get_skill).put(update_skill).delete(delete_skill),
        )
        // MCP servers
        .route(
            "/projects/{project_id}/mcp-servers",
            get(list_mcps).post(create_mcp),
        )
        .route(
            "/projects/{project_id}/mcp-servers/{slug}",
            get(get_mcp).put(update_mcp).delete(delete_mcp),
        )
}

// ── Skill handlers ──────────────────────────────────────────────────────────

async fn list_skills(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<AppState>>,
    Path(project_id): Path<i32>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, ProjectsRead);

    let items = app_state
        .definition_service
        .list_skills(project_id)
        .await
        .map_err(Problem::from)?;

    let total = items.len();
    Ok(Json(ListResponse {
        items: items
            .into_iter()
            .map(SkillDefinitionResponse::from)
            .collect(),
        total,
    }))
}

async fn get_skill(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<AppState>>,
    Path((project_id, slug)): Path<(i32, String)>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, ProjectsRead);

    let skill = app_state
        .definition_service
        .get_skill(project_id, &slug)
        .await
        .map_err(Problem::from)?;

    Ok(Json(SkillDefinitionResponse::from(skill)))
}

async fn create_skill(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<AppState>>,
    Path(project_id): Path<i32>,
    Json(request): Json<CreateSkillRequest>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, ProjectsWrite);

    let skill = app_state
        .definition_service
        .create_skill(
            project_id,
            CreateSkillDefinitionRequest {
                slug: request.slug,
                name: request.name,
                description: request.description,
                content: request.content,
            },
        )
        .await
        .map_err(Problem::from)?;

    Ok((
        StatusCode::CREATED,
        Json(SkillDefinitionResponse::from(skill)),
    ))
}

async fn update_skill(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<AppState>>,
    Path((project_id, slug)): Path<(i32, String)>,
    Json(request): Json<UpdateSkillRequest>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, ProjectsWrite);

    let skill = app_state
        .definition_service
        .update_skill(
            project_id,
            &slug,
            UpdateSkillDefinitionRequest {
                name: request.name,
                description: request.description,
                content: request.content,
            },
        )
        .await
        .map_err(Problem::from)?;

    Ok(Json(SkillDefinitionResponse::from(skill)))
}

async fn delete_skill(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<AppState>>,
    Path((project_id, slug)): Path<(i32, String)>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, ProjectsWrite);

    app_state
        .definition_service
        .delete_skill(project_id, &slug)
        .await
        .map_err(Problem::from)?;

    Ok(StatusCode::NO_CONTENT)
}

// ── MCP handlers ────────────────────────────────────────────────────────────

async fn list_mcps(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<AppState>>,
    Path(project_id): Path<i32>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, ProjectsRead);

    let items = app_state
        .definition_service
        .list_mcps(project_id)
        .await
        .map_err(Problem::from)?;

    let total = items.len();
    Ok(Json(ListResponse {
        items: items.into_iter().map(McpDefinitionResponse::from).collect(),
        total,
    }))
}

async fn get_mcp(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<AppState>>,
    Path((project_id, slug)): Path<(i32, String)>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, ProjectsRead);

    let mcp = app_state
        .definition_service
        .get_mcp(project_id, &slug)
        .await
        .map_err(Problem::from)?;

    Ok(Json(McpDefinitionResponse::from(mcp)))
}

async fn create_mcp(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<AppState>>,
    Path(project_id): Path<i32>,
    Json(request): Json<CreateMcpRequest>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, ProjectsWrite);

    let mcp = app_state
        .definition_service
        .create_mcp(
            project_id,
            CreateMcpDefinitionRequest {
                slug: request.slug,
                name: request.name,
                description: request.description,
                config: request.config,
            },
        )
        .await
        .map_err(Problem::from)?;

    Ok((StatusCode::CREATED, Json(McpDefinitionResponse::from(mcp))))
}

async fn update_mcp(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<AppState>>,
    Path((project_id, slug)): Path<(i32, String)>,
    Json(request): Json<UpdateMcpRequest>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, ProjectsWrite);

    let mcp = app_state
        .definition_service
        .update_mcp(
            project_id,
            &slug,
            UpdateMcpDefinitionRequest {
                name: request.name,
                description: request.description,
                config: request.config,
            },
        )
        .await
        .map_err(Problem::from)?;

    Ok(Json(McpDefinitionResponse::from(mcp)))
}

async fn delete_mcp(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<AppState>>,
    Path((project_id, slug)): Path<(i32, String)>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, ProjectsWrite);

    app_state
        .definition_service
        .delete_mcp(project_id, &slug)
        .await
        .map_err(Problem::from)?;

    Ok(StatusCode::NO_CONTENT)
}
