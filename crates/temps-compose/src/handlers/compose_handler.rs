use crate::handlers::audit::{
    AuditContext, StackCreatedAudit, StackDeletedAudit, StackStateChangedAudit, StackUpdatedAudit,
};
use crate::handlers::types::ComposeAppState;
use crate::services::{ComposeError, ContainerMetrics};
use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use temps_auth::permission_guard;
use temps_auth::RequireAuth;
use temps_core::problemdetails;
use temps_core::problemdetails::{Problem, ProblemDetails};
use temps_core::RequestMetadata;
use tracing::error;
use utoipa::{OpenApi, ToSchema};

impl From<ComposeError> for Problem {
    fn from(error: ComposeError) -> Self {
        match error {
            ComposeError::NotFound { .. } => problemdetails::new(StatusCode::NOT_FOUND)
                .with_title("Stack Not Found")
                .with_detail(error.to_string()),

            ComposeError::Validation { .. } => problemdetails::new(StatusCode::BAD_REQUEST)
                .with_title("Validation Error")
                .with_detail(error.to_string()),

            ComposeError::InvalidState { .. } => problemdetails::new(StatusCode::CONFLICT)
                .with_title("Invalid State")
                .with_detail(error.to_string()),

            ComposeError::Docker { .. } => problemdetails::new(StatusCode::INTERNAL_SERVER_ERROR)
                .with_title("Docker Compose Error")
                .with_detail(error.to_string()),

            ComposeError::Database(_) => problemdetails::new(StatusCode::INTERNAL_SERVER_ERROR)
                .with_title("Internal Server Error")
                .with_detail(error.to_string()),

            ComposeError::RepoSync { .. } => problemdetails::new(StatusCode::BAD_GATEWAY)
                .with_title("Repository Sync Failed")
                .with_detail(error.to_string()),
        }
    }
}

#[derive(OpenApi)]
#[openapi(
    paths(
        list_stacks,
        create_stack,
        get_stack,
        update_stack,
        delete_stack,
        deploy_stack,
        stop_stack,
        restart_stack,
        pull_stack,
        get_stack_containers,
        get_stack_logs,
        get_stack_stats,
        sync_stack,
        list_stack_routes,
        create_stack_route,
        delete_stack_route,
        toggle_stack_route,
    ),
    components(
        schemas(
            CreateStackRequest,
            UpdateStackRequest,
            StackResponse,
            PaginatedStacksResponse,
            StackContainersResponse,
            StackLogsResponse,
            StackStatsResponse,
            StackRouteResponse,
            CreateStackRouteRequest,
            ToggleStackRouteRequest,
        )
    ),
    info(
        title = "Compose Stacks API",
        description = "API endpoints for managing Docker Compose stacks",
        version = "1.0.0"
    ),
    tags(
        (name = "Stacks", description = "Docker Compose stack management endpoints")
    )
)]
pub struct ComposeApiDoc;

#[derive(Deserialize, ToSchema, Clone)]
pub struct CreateStackRequest {
    pub name: String,
    pub description: Option<String>,
    /// Compose content (required unless repo_url is provided)
    pub compose_content: Option<String>,
    pub env_content: Option<String>,
    pub node_id: Option<i32>,
    /// Repository URL to fetch compose file from
    pub repo_url: Option<String>,
    /// Branch to clone (defaults to repo default)
    pub repo_branch: Option<String>,
    /// Path to compose file in repo (defaults to "docker-compose.yml")
    pub repo_compose_path: Option<String>,
    /// Access token for private repos
    pub repo_access_token: Option<String>,
}

#[derive(Deserialize, ToSchema, Clone)]
pub struct UpdateStackRequest {
    pub name: Option<String>,
    pub description: Option<Option<String>>,
    pub compose_content: Option<String>,
    pub env_content: Option<Option<String>>,
}

#[derive(Serialize, ToSchema)]
pub struct StackResponse {
    pub id: i32,
    pub name: String,
    pub description: Option<String>,
    pub compose_content: String,
    pub env_content: Option<String>,
    pub node_id: Option<i32>,
    pub state: String,
    pub repo_url: Option<String>,
    pub repo_branch: Option<String>,
    pub repo_compose_path: Option<String>,
    pub last_synced_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl From<temps_entities::compose_stacks::Model> for StackResponse {
    fn from(model: temps_entities::compose_stacks::Model) -> Self {
        Self {
            id: model.id,
            name: model.name,
            description: model.description,
            compose_content: model.compose_content,
            env_content: model.env_content,
            node_id: model.node_id,
            state: model.state,
            repo_url: model.repo_url,
            repo_branch: model.repo_branch,
            repo_compose_path: model.repo_compose_path,
            last_synced_at: model.last_synced_at.map(|dt| dt.to_string()),
            created_at: model.created_at.to_string(),
            updated_at: model.updated_at.to_string(),
        }
    }
}

#[derive(Serialize, ToSchema)]
pub struct PaginatedStacksResponse {
    pub items: Vec<StackResponse>,
    pub total: u64,
}

#[derive(Deserialize)]
pub struct PaginationParams {
    pub page: Option<u64>,
    pub page_size: Option<u64>,
}

/// List all compose stacks
#[utoipa::path(
    tag = "Stacks",
    get,
    path = "/stacks",
    params(
        ("page" = Option<u64>, Query, description = "Page number (default: 1)"),
        ("page_size" = Option<u64>, Query, description = "Page size (default: 20, max: 100)")
    ),
    responses(
        (status = 200, description = "List of stacks", body = PaginatedStacksResponse),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "Insufficient permissions", body = ProblemDetails),
        (status = 500, description = "Internal server error", body = ProblemDetails)
    ),
    security(("bearer_auth" = []))
)]
async fn list_stacks(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<ComposeAppState>>,
    axum::extract::Query(params): axum::extract::Query<PaginationParams>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, StacksRead);

    let (stacks, total) = app_state
        .compose_service
        .list(params.page, params.page_size)
        .await?;

    Ok(Json(PaginatedStacksResponse {
        items: stacks.into_iter().map(StackResponse::from).collect(),
        total,
    }))
}

/// Create a new compose stack
#[utoipa::path(
    tag = "Stacks",
    post,
    path = "/stacks",
    request_body = CreateStackRequest,
    responses(
        (status = 201, description = "Stack created", body = StackResponse),
        (status = 400, description = "Validation error", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "Insufficient permissions", body = ProblemDetails),
        (status = 500, description = "Internal server error", body = ProblemDetails)
    ),
    security(("bearer_auth" = []))
)]
async fn create_stack(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<ComposeAppState>>,
    Extension(metadata): Extension<RequestMetadata>,
    Json(request): Json<CreateStackRequest>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, StacksCreate);

    let stack = if let Some(repo_url) = request.repo_url {
        app_state
            .compose_service
            .create_from_repo(
                request.name.clone(),
                request.description,
                repo_url,
                request.repo_branch,
                request.repo_compose_path,
                request.repo_access_token,
                request.node_id,
            )
            .await?
    } else {
        let compose_content = request
            .compose_content
            .ok_or_else(|| ComposeError::Validation {
                message: "Either compose_content or repo_url must be provided".into(),
            })?;
        app_state
            .compose_service
            .create(
                request.name.clone(),
                request.description,
                compose_content,
                request.env_content,
                request.node_id,
            )
            .await?
    };

    let audit = StackCreatedAudit {
        context: AuditContext {
            user_id: auth.user_id(),
            ip_address: Some(metadata.ip_address.clone()),
            user_agent: metadata.user_agent.clone(),
        },
        stack_id: stack.id,
        name: stack.name.clone(),
    };
    if let Err(e) = app_state.audit_service.create_audit_log(&audit).await {
        error!("Failed to create audit log: {}", e);
    }

    Ok((StatusCode::CREATED, Json(StackResponse::from(stack))))
}

/// Get a compose stack by ID
#[utoipa::path(
    tag = "Stacks",
    get,
    path = "/stacks/{id}",
    params(
        ("id" = i32, Path, description = "Stack ID")
    ),
    responses(
        (status = 200, description = "Stack details", body = StackResponse),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "Insufficient permissions", body = ProblemDetails),
        (status = 404, description = "Stack not found", body = ProblemDetails),
        (status = 500, description = "Internal server error", body = ProblemDetails)
    ),
    security(("bearer_auth" = []))
)]
async fn get_stack(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<ComposeAppState>>,
    Path(id): Path<i32>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, StacksRead);

    let stack = app_state.compose_service.get(id).await?;
    Ok(Json(StackResponse::from(stack)))
}

/// Update a compose stack
#[utoipa::path(
    tag = "Stacks",
    patch,
    path = "/stacks/{id}",
    params(
        ("id" = i32, Path, description = "Stack ID")
    ),
    request_body = UpdateStackRequest,
    responses(
        (status = 200, description = "Stack updated", body = StackResponse),
        (status = 400, description = "Validation error", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "Insufficient permissions", body = ProblemDetails),
        (status = 404, description = "Stack not found", body = ProblemDetails),
        (status = 500, description = "Internal server error", body = ProblemDetails)
    ),
    security(("bearer_auth" = []))
)]
async fn update_stack(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<ComposeAppState>>,
    Extension(metadata): Extension<RequestMetadata>,
    Path(id): Path<i32>,
    Json(request): Json<UpdateStackRequest>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, StacksWrite);

    let stack = app_state
        .compose_service
        .update(
            id,
            request.name,
            request.description,
            request.compose_content,
            request.env_content,
        )
        .await?;

    let audit = StackUpdatedAudit {
        context: AuditContext {
            user_id: auth.user_id(),
            ip_address: Some(metadata.ip_address.clone()),
            user_agent: metadata.user_agent.clone(),
        },
        stack_id: stack.id,
        name: stack.name.clone(),
    };
    if let Err(e) = app_state.audit_service.create_audit_log(&audit).await {
        error!("Failed to create audit log: {}", e);
    }

    Ok(Json(StackResponse::from(stack)))
}

/// Delete a compose stack
#[utoipa::path(
    tag = "Stacks",
    delete,
    path = "/stacks/{id}",
    params(
        ("id" = i32, Path, description = "Stack ID")
    ),
    responses(
        (status = 204, description = "Stack deleted"),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "Insufficient permissions", body = ProblemDetails),
        (status = 404, description = "Stack not found", body = ProblemDetails),
        (status = 409, description = "Stack is running", body = ProblemDetails),
        (status = 500, description = "Internal server error", body = ProblemDetails)
    ),
    security(("bearer_auth" = []))
)]
async fn delete_stack(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<ComposeAppState>>,
    Extension(metadata): Extension<RequestMetadata>,
    Path(id): Path<i32>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, StacksDelete);

    let stack = app_state.compose_service.get(id).await?;
    let stack_name = stack.name.clone();

    app_state.compose_service.destroy(id).await?;

    let audit = StackDeletedAudit {
        context: AuditContext {
            user_id: auth.user_id(),
            ip_address: Some(metadata.ip_address.clone()),
            user_agent: metadata.user_agent.clone(),
        },
        stack_id: id,
        name: stack_name,
    };
    if let Err(e) = app_state.audit_service.create_audit_log(&audit).await {
        error!("Failed to create audit log: {}", e);
    }

    Ok(StatusCode::NO_CONTENT)
}

/// Deploy (start) a compose stack
#[utoipa::path(
    tag = "Stacks",
    post,
    path = "/stacks/{id}/deploy",
    params(
        ("id" = i32, Path, description = "Stack ID")
    ),
    responses(
        (status = 200, description = "Stack deployed", body = StackResponse),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "Insufficient permissions", body = ProblemDetails),
        (status = 404, description = "Stack not found", body = ProblemDetails),
        (status = 500, description = "Internal server error", body = ProblemDetails)
    ),
    security(("bearer_auth" = []))
)]
async fn deploy_stack(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<ComposeAppState>>,
    Extension(metadata): Extension<RequestMetadata>,
    Path(id): Path<i32>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, StacksWrite);

    let stack = app_state.compose_service.deploy(id).await?;

    let audit = StackStateChangedAudit {
        context: AuditContext {
            user_id: auth.user_id(),
            ip_address: Some(metadata.ip_address.clone()),
            user_agent: metadata.user_agent.clone(),
        },
        stack_id: stack.id,
        name: stack.name.clone(),
        new_state: "running".to_string(),
    };
    if let Err(e) = app_state.audit_service.create_audit_log(&audit).await {
        error!("Failed to create audit log: {}", e);
    }

    Ok(Json(StackResponse::from(stack)))
}

/// Stop a compose stack
#[utoipa::path(
    tag = "Stacks",
    post,
    path = "/stacks/{id}/stop",
    params(
        ("id" = i32, Path, description = "Stack ID")
    ),
    responses(
        (status = 200, description = "Stack stopped", body = StackResponse),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "Insufficient permissions", body = ProblemDetails),
        (status = 404, description = "Stack not found", body = ProblemDetails),
        (status = 500, description = "Internal server error", body = ProblemDetails)
    ),
    security(("bearer_auth" = []))
)]
async fn stop_stack(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<ComposeAppState>>,
    Extension(metadata): Extension<RequestMetadata>,
    Path(id): Path<i32>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, StacksWrite);

    let stack = app_state.compose_service.stop(id).await?;

    let audit = StackStateChangedAudit {
        context: AuditContext {
            user_id: auth.user_id(),
            ip_address: Some(metadata.ip_address.clone()),
            user_agent: metadata.user_agent.clone(),
        },
        stack_id: stack.id,
        name: stack.name.clone(),
        new_state: "stopped".to_string(),
    };
    if let Err(e) = app_state.audit_service.create_audit_log(&audit).await {
        error!("Failed to create audit log: {}", e);
    }

    Ok(Json(StackResponse::from(stack)))
}

/// Restart a compose stack
#[utoipa::path(
    tag = "Stacks",
    post,
    path = "/stacks/{id}/restart",
    params(
        ("id" = i32, Path, description = "Stack ID")
    ),
    responses(
        (status = 200, description = "Stack restarted", body = StackResponse),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "Insufficient permissions", body = ProblemDetails),
        (status = 404, description = "Stack not found", body = ProblemDetails),
        (status = 500, description = "Internal server error", body = ProblemDetails)
    ),
    security(("bearer_auth" = []))
)]
async fn restart_stack(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<ComposeAppState>>,
    Extension(metadata): Extension<RequestMetadata>,
    Path(id): Path<i32>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, StacksWrite);

    let stack = app_state.compose_service.restart(id).await?;

    let audit = StackStateChangedAudit {
        context: AuditContext {
            user_id: auth.user_id(),
            ip_address: Some(metadata.ip_address.clone()),
            user_agent: metadata.user_agent.clone(),
        },
        stack_id: stack.id,
        name: stack.name.clone(),
        new_state: "restarted".to_string(),
    };
    if let Err(e) = app_state.audit_service.create_audit_log(&audit).await {
        error!("Failed to create audit log: {}", e);
    }

    Ok(Json(StackResponse::from(stack)))
}

/// Pull latest images for a compose stack
#[utoipa::path(
    tag = "Stacks",
    post,
    path = "/stacks/{id}/pull",
    params(
        ("id" = i32, Path, description = "Stack ID")
    ),
    responses(
        (status = 200, description = "Images pulled", body = StackResponse),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "Insufficient permissions", body = ProblemDetails),
        (status = 404, description = "Stack not found", body = ProblemDetails),
        (status = 500, description = "Internal server error", body = ProblemDetails)
    ),
    security(("bearer_auth" = []))
)]
async fn pull_stack(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<ComposeAppState>>,
    Path(id): Path<i32>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, StacksWrite);

    let stack = app_state.compose_service.pull(id).await?;
    Ok(Json(StackResponse::from(stack)))
}

#[derive(Serialize, ToSchema)]
pub struct StackContainersResponse {
    pub raw: String,
}

/// List containers in a compose stack
#[utoipa::path(
    tag = "Stacks",
    get,
    path = "/stacks/{id}/containers",
    params(
        ("id" = i32, Path, description = "Stack ID")
    ),
    responses(
        (status = 200, description = "Stack containers", body = StackContainersResponse),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "Insufficient permissions", body = ProblemDetails),
        (status = 404, description = "Stack not found", body = ProblemDetails),
        (status = 500, description = "Internal server error", body = ProblemDetails)
    ),
    security(("bearer_auth" = []))
)]
async fn get_stack_containers(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<ComposeAppState>>,
    Path(id): Path<i32>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, StacksRead);

    let raw = app_state.compose_service.containers(id).await?;
    Ok(Json(StackContainersResponse { raw }))
}

#[derive(Deserialize)]
pub struct LogsQueryParams {
    pub service: Option<String>,
    pub tail: Option<u32>,
}

#[derive(Serialize, ToSchema)]
pub struct StackLogsResponse {
    pub logs: String,
}

/// Get logs from a compose stack
#[utoipa::path(
    tag = "Stacks",
    get,
    path = "/stacks/{id}/logs",
    params(
        ("id" = i32, Path, description = "Stack ID"),
        ("service" = Option<String>, Query, description = "Service name filter"),
        ("tail" = Option<u32>, Query, description = "Number of lines (default: 200)")
    ),
    responses(
        (status = 200, description = "Stack logs", body = StackLogsResponse),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "Insufficient permissions", body = ProblemDetails),
        (status = 404, description = "Stack not found", body = ProblemDetails),
        (status = 500, description = "Internal server error", body = ProblemDetails)
    ),
    security(("bearer_auth" = []))
)]
async fn get_stack_logs(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<ComposeAppState>>,
    Path(id): Path<i32>,
    axum::extract::Query(params): axum::extract::Query<LogsQueryParams>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, StacksRead);

    let tail = params.tail.unwrap_or(200);
    let logs = app_state
        .compose_service
        .logs(id, params.service.as_deref(), tail)
        .await?;
    Ok(Json(StackLogsResponse { logs }))
}

#[derive(Serialize, ToSchema)]
pub struct StackStatsResponse {
    pub containers: Vec<ContainerMetrics>,
}

/// Get resource metrics for a compose stack
#[utoipa::path(
    tag = "Stacks",
    get,
    path = "/stacks/{id}/stats",
    params(
        ("id" = i32, Path, description = "Stack ID")
    ),
    responses(
        (status = 200, description = "Stack metrics", body = StackStatsResponse),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "Insufficient permissions", body = ProblemDetails),
        (status = 404, description = "Stack not found", body = ProblemDetails),
        (status = 500, description = "Internal server error", body = ProblemDetails)
    ),
    security(("bearer_auth" = []))
)]
async fn get_stack_stats(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<ComposeAppState>>,
    Path(id): Path<i32>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, StacksRead);

    let containers = app_state.compose_service.stats(id).await?;
    Ok(Json(StackStatsResponse { containers }))
}

/// Sync a stack's compose file from its linked repository
#[utoipa::path(
    tag = "Stacks",
    post,
    path = "/stacks/{id}/sync",
    params(
        ("id" = i32, Path, description = "Stack ID")
    ),
    responses(
        (status = 200, description = "Stack synced from repository", body = StackResponse),
        (status = 400, description = "Stack not linked to a repository", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "Insufficient permissions", body = ProblemDetails),
        (status = 404, description = "Stack not found", body = ProblemDetails),
        (status = 502, description = "Repository sync failed", body = ProblemDetails),
        (status = 500, description = "Internal server error", body = ProblemDetails)
    ),
    security(("bearer_auth" = []))
)]
async fn sync_stack(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<ComposeAppState>>,
    Extension(metadata): Extension<RequestMetadata>,
    Path(id): Path<i32>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, StacksWrite);

    let stack = app_state.compose_service.sync_from_repo(id).await?;

    let audit = StackStateChangedAudit {
        context: AuditContext {
            user_id: auth.user_id(),
            ip_address: Some(metadata.ip_address.clone()),
            user_agent: metadata.user_agent.clone(),
        },
        stack_id: stack.id,
        name: stack.name.clone(),
        new_state: "synced".to_string(),
    };
    if let Err(e) = app_state.audit_service.create_audit_log(&audit).await {
        error!("Failed to create audit log: {}", e);
    }

    Ok(Json(StackResponse::from(stack)))
}

// --- Stack route types and handlers ---

#[derive(Serialize, ToSchema)]
pub struct StackRouteResponse {
    pub id: i32,
    pub stack_id: i32,
    pub domain: String,
    pub target_port: i32,
    pub service_name: Option<String>,
    pub enabled: bool,
    pub created_at: String,
    pub updated_at: String,
}

impl From<temps_entities::compose_stack_routes::Model> for StackRouteResponse {
    fn from(model: temps_entities::compose_stack_routes::Model) -> Self {
        Self {
            id: model.id,
            stack_id: model.stack_id,
            domain: model.domain,
            target_port: model.target_port,
            service_name: model.service_name,
            enabled: model.enabled,
            created_at: model.created_at.to_string(),
            updated_at: model.updated_at.to_string(),
        }
    }
}

#[derive(Deserialize, ToSchema, Clone)]
pub struct CreateStackRouteRequest {
    pub domain: String,
    pub target_port: i32,
    pub service_name: Option<String>,
}

#[derive(Deserialize, ToSchema, Clone)]
pub struct ToggleStackRouteRequest {
    pub enabled: bool,
}

/// List domain routes for a stack
#[utoipa::path(
    tag = "Stacks",
    get,
    path = "/stacks/{id}/routes",
    params(
        ("id" = i32, Path, description = "Stack ID")
    ),
    responses(
        (status = 200, description = "Stack routes", body = Vec<StackRouteResponse>),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "Insufficient permissions", body = ProblemDetails),
        (status = 404, description = "Stack not found", body = ProblemDetails),
        (status = 500, description = "Internal server error", body = ProblemDetails)
    ),
    security(("bearer_auth" = []))
)]
async fn list_stack_routes(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<ComposeAppState>>,
    Path(id): Path<i32>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, StacksRead);

    let routes = app_state.compose_service.list_routes(id).await?;
    let response: Vec<StackRouteResponse> = routes.into_iter().map(Into::into).collect();
    Ok(Json(response))
}

/// Create a domain route for a stack
#[utoipa::path(
    tag = "Stacks",
    post,
    path = "/stacks/{id}/routes",
    params(
        ("id" = i32, Path, description = "Stack ID")
    ),
    request_body = CreateStackRouteRequest,
    responses(
        (status = 201, description = "Route created", body = StackRouteResponse),
        (status = 400, description = "Validation error", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "Insufficient permissions", body = ProblemDetails),
        (status = 404, description = "Stack not found", body = ProblemDetails),
        (status = 500, description = "Internal server error", body = ProblemDetails)
    ),
    security(("bearer_auth" = []))
)]
async fn create_stack_route(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<ComposeAppState>>,
    Path(id): Path<i32>,
    Json(request): Json<CreateStackRouteRequest>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, StacksWrite);

    let route = app_state
        .compose_service
        .create_route(
            id,
            request.domain,
            request.target_port,
            request.service_name,
        )
        .await?;

    Ok((StatusCode::CREATED, Json(StackRouteResponse::from(route))))
}

/// Delete a domain route from a stack
#[utoipa::path(
    tag = "Stacks",
    delete,
    path = "/stacks/{stack_id}/routes/{route_id}",
    params(
        ("stack_id" = i32, Path, description = "Stack ID"),
        ("route_id" = i32, Path, description = "Route ID")
    ),
    responses(
        (status = 204, description = "Route deleted"),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "Insufficient permissions", body = ProblemDetails),
        (status = 404, description = "Stack or route not found", body = ProblemDetails),
        (status = 500, description = "Internal server error", body = ProblemDetails)
    ),
    security(("bearer_auth" = []))
)]
async fn delete_stack_route(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<ComposeAppState>>,
    Path((stack_id, route_id)): Path<(i32, i32)>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, StacksWrite);

    app_state
        .compose_service
        .delete_route(stack_id, route_id)
        .await?;

    Ok(StatusCode::NO_CONTENT)
}

/// Toggle a domain route's enabled state
#[utoipa::path(
    tag = "Stacks",
    patch,
    path = "/stacks/{stack_id}/routes/{route_id}",
    params(
        ("stack_id" = i32, Path, description = "Stack ID"),
        ("route_id" = i32, Path, description = "Route ID")
    ),
    request_body = ToggleStackRouteRequest,
    responses(
        (status = 200, description = "Route updated", body = StackRouteResponse),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "Insufficient permissions", body = ProblemDetails),
        (status = 404, description = "Stack or route not found", body = ProblemDetails),
        (status = 500, description = "Internal server error", body = ProblemDetails)
    ),
    security(("bearer_auth" = []))
)]
async fn toggle_stack_route(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<ComposeAppState>>,
    Path((stack_id, route_id)): Path<(i32, i32)>,
    Json(request): Json<ToggleStackRouteRequest>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, StacksWrite);

    let route = app_state
        .compose_service
        .toggle_route(stack_id, route_id, request.enabled)
        .await?;

    Ok(Json(StackRouteResponse::from(route)))
}

pub fn configure_routes() -> Router<Arc<ComposeAppState>> {
    Router::new()
        .route("/stacks", get(list_stacks).post(create_stack))
        .route(
            "/stacks/{id}",
            get(get_stack).patch(update_stack).delete(delete_stack),
        )
        .route("/stacks/{id}/deploy", post(deploy_stack))
        .route("/stacks/{id}/stop", post(stop_stack))
        .route("/stacks/{id}/restart", post(restart_stack))
        .route("/stacks/{id}/pull", post(pull_stack))
        .route("/stacks/{id}/sync", post(sync_stack))
        .route("/stacks/{id}/containers", get(get_stack_containers))
        .route("/stacks/{id}/logs", get(get_stack_logs))
        .route("/stacks/{id}/stats", get(get_stack_stats))
        .route(
            "/stacks/{id}/routes",
            get(list_stack_routes).post(create_stack_route),
        )
        .route(
            "/stacks/{stack_id}/routes/{route_id}",
            axum::routing::delete(delete_stack_route).patch(toggle_stack_route),
        )
}
