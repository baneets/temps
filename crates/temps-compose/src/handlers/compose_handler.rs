use crate::handlers::audit::{
    AuditContext, StackCreatedAudit, StackDeletedAudit, StackStateChangedAudit, StackUpdatedAudit,
};
use crate::handlers::types::ComposeAppState;
use crate::services::ComposeError;
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
    ),
    components(
        schemas(
            CreateStackRequest,
            UpdateStackRequest,
            StackResponse,
            PaginatedStacksResponse,
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
    pub compose_content: String,
    pub env_content: Option<String>,
    pub node_id: Option<i32>,
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

    let stack = app_state
        .compose_service
        .create(
            request.name.clone(),
            request.description,
            request.compose_content,
            request.env_content,
            request.node_id,
        )
        .await?;

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

    app_state.compose_service.delete(id).await?;

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

    let stack = app_state.compose_service.set_state(id, "running").await?;

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

    let stack = app_state.compose_service.set_state(id, "stopped").await?;

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

    let stack = app_state.compose_service.set_state(id, "running").await?;

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

    // For now, just return the current stack state.
    // Actual docker compose pull will be implemented with the Docker executor.
    let stack = app_state.compose_service.get(id).await?;
    Ok(Json(StackResponse::from(stack)))
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
}
