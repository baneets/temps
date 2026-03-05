//! HTTP handlers for the agent API.
//!
//! These wrap the local `ContainerDeployer` and `ImageBuilder` traits,
//! exposing them over HTTP for remote control from the control plane.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Serialize;
use std::sync::Arc;
use temps_deployer::{ContainerDeployer, DeployRequest, ImageBuilder};
use utoipa::{OpenApi, ToSchema};

use crate::NodeHealthReport;

/// Shared state for all agent handlers.
pub struct AgentState {
    pub container_deployer: Arc<dyn ContainerDeployer>,
    pub image_builder: Arc<dyn ImageBuilder>,
}

/// Response wrapper for consistent agent API responses.
#[derive(Serialize, ToSchema)]
pub struct AgentResponse<T: Serialize> {
    success: bool,
    #[schema(nullable = true)]
    data: Option<T>,
    #[schema(nullable = true)]
    error: Option<String>,
}

impl<T: Serialize> AgentResponse<T> {
    fn ok(data: T) -> Json<Self> {
        Json(Self {
            success: true,
            data: Some(data),
            error: None,
        })
    }
}

fn error_response(status: StatusCode, message: String) -> impl IntoResponse {
    (
        status,
        Json(AgentResponse::<()> {
            success: false,
            data: None,
            error: Some(message),
        }),
    )
}

#[derive(OpenApi)]
#[openapi(
    paths(
        deploy_container,
        stop_container,
        remove_container,
        get_container_logs,
        get_container_info,
        image_exists,
        health_check,
    ),
    components(schemas(
        AgentResponse<temps_deployer::DeployResult>,
        AgentResponse<String>,
        AgentResponse<bool>,
        AgentResponse<temps_deployer::ContainerInfo>,
        AgentResponse<NodeHealthReport>,
        NodeHealthReport,
        temps_deployer::DeployRequest,
        temps_deployer::DeployResult,
        temps_deployer::ContainerInfo,
        temps_deployer::ContainerStatus,
        temps_deployer::PortMapping,
        temps_deployer::Protocol,
        temps_deployer::ResourceLimits,
        temps_deployer::RestartPolicy,
        temps_deployer::ContainerLogConfig,
    )),
    info(
        title = "Temps Agent API",
        description = "Worker node agent API for container management. All endpoints require Bearer token authentication.",
        version = "1.0.0"
    ),
    security(
        ("bearer_auth" = [])
    ),
    modifiers(&SecurityAddon)
)]
pub struct AgentApiDoc;

struct SecurityAddon;

impl utoipa::Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        if let Some(components) = openapi.components.as_mut() {
            components.add_security_scheme(
                "bearer_auth",
                utoipa::openapi::security::SecurityScheme::Http(
                    utoipa::openapi::security::Http::new(
                        utoipa::openapi::security::HttpAuthScheme::Bearer,
                    ),
                ),
            );
        }
    }
}

/// Deploy a new container on this worker node
#[utoipa::path(
    tag = "Containers",
    post,
    path = "/agent/containers/deploy",
    request_body = DeployRequest,
    responses(
        (status = 200, description = "Container deployed successfully", body = AgentResponse<temps_deployer::DeployResult>),
        (status = 401, description = "Unauthorized — invalid or missing bearer token"),
        (status = 500, description = "Deploy failed")
    ),
    security(("bearer_auth" = []))
)]
pub async fn deploy_container(
    State(state): State<Arc<AgentState>>,
    Json(request): Json<DeployRequest>,
) -> impl IntoResponse {
    match state.container_deployer.deploy_container(request).await {
        Ok(result) => AgentResponse::ok(result).into_response(),
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Deploy failed: {}", e),
        )
        .into_response(),
    }
}

/// Stop a running container
#[utoipa::path(
    tag = "Containers",
    post,
    path = "/agent/containers/{id}/stop",
    params(
        ("id" = String, Path, description = "Container ID or name")
    ),
    responses(
        (status = 200, description = "Container stopped", body = AgentResponse<String>),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Stop failed")
    ),
    security(("bearer_auth" = []))
)]
pub async fn stop_container(
    State(state): State<Arc<AgentState>>,
    Path(container_id): Path<String>,
) -> impl IntoResponse {
    match state.container_deployer.stop_container(&container_id).await {
        Ok(()) => AgentResponse::ok("stopped".to_string()).into_response(),
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Stop failed for container {}: {}", container_id, e),
        )
        .into_response(),
    }
}

/// Remove a container
#[utoipa::path(
    tag = "Containers",
    delete,
    path = "/agent/containers/{id}",
    params(
        ("id" = String, Path, description = "Container ID or name")
    ),
    responses(
        (status = 200, description = "Container removed", body = AgentResponse<String>),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Remove failed")
    ),
    security(("bearer_auth" = []))
)]
pub async fn remove_container(
    State(state): State<Arc<AgentState>>,
    Path(container_id): Path<String>,
) -> impl IntoResponse {
    match state
        .container_deployer
        .remove_container(&container_id)
        .await
    {
        Ok(()) => AgentResponse::ok("removed".to_string()).into_response(),
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Remove failed for container {}: {}", container_id, e),
        )
        .into_response(),
    }
}

/// Get container logs
#[utoipa::path(
    tag = "Containers",
    get,
    path = "/agent/containers/{id}/logs",
    params(
        ("id" = String, Path, description = "Container ID or name")
    ),
    responses(
        (status = 200, description = "Container logs", body = AgentResponse<String>),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Failed to get logs")
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_container_logs(
    State(state): State<Arc<AgentState>>,
    Path(container_id): Path<String>,
) -> impl IntoResponse {
    match state
        .container_deployer
        .get_container_logs(&container_id)
        .await
    {
        Ok(logs) => AgentResponse::ok(logs).into_response(),
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to get logs for container {}: {}", container_id, e),
        )
        .into_response(),
    }
}

/// Get container info (status, ports, environment)
#[utoipa::path(
    tag = "Containers",
    get,
    path = "/agent/containers/{id}/info",
    params(
        ("id" = String, Path, description = "Container ID or name")
    ),
    responses(
        (status = 200, description = "Container info", body = AgentResponse<temps_deployer::ContainerInfo>),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Failed to get info")
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_container_info(
    State(state): State<Arc<AgentState>>,
    Path(container_id): Path<String>,
) -> impl IntoResponse {
    match state
        .container_deployer
        .get_container_info(&container_id)
        .await
    {
        Ok(info) => AgentResponse::ok(info).into_response(),
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to get info for container {}: {}", container_id, e),
        )
        .into_response(),
    }
}

/// Check if a Docker image exists on this node
#[utoipa::path(
    tag = "Images",
    get,
    path = "/agent/images/{name}/exists",
    params(
        ("name" = String, Path, description = "Docker image name (URL-encoded if it contains slashes)")
    ),
    responses(
        (status = 200, description = "Image existence check result", body = AgentResponse<bool>),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Failed to check image")
    ),
    security(("bearer_auth" = []))
)]
pub async fn image_exists(
    State(state): State<Arc<AgentState>>,
    Path(image_name): Path<String>,
) -> impl IntoResponse {
    match state.container_deployer.image_exists(&image_name).await {
        Ok(exists) => AgentResponse::ok(exists).into_response(),
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to check image {}: {}", image_name, e),
        )
        .into_response(),
    }
}

/// Health check — returns system metrics for this worker node
#[utoipa::path(
    tag = "Health",
    get,
    path = "/agent/health",
    responses(
        (status = 200, description = "Node health report", body = AgentResponse<NodeHealthReport>),
        (status = 401, description = "Unauthorized")
    ),
    security(("bearer_auth" = []))
)]
pub async fn health_check() -> impl IntoResponse {
    // Gather basic system metrics
    let report = NodeHealthReport {
        cpu_percent: 0.0, // TODO: gather real metrics via sysinfo
        memory_used_bytes: 0,
        memory_total_bytes: 0,
        disk_used_bytes: 0,
        disk_total_bytes: 0,
        running_containers: 0,
    };

    AgentResponse::ok(report)
}
