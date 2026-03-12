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
    /// Direct Docker client for service operations (create/exec/backup).
    /// None if Docker is not available (shouldn't happen on a real agent).
    pub docker: Option<bollard::Docker>,
}

/// Response wrapper for consistent agent API responses.
#[derive(Serialize, ToSchema)]
pub struct AgentResponse<T: Serialize> {
    pub(crate) success: bool,
    #[schema(nullable = true)]
    pub(crate) data: Option<T>,
    #[schema(nullable = true)]
    pub(crate) error: Option<String>,
}

impl<T: Serialize> AgentResponse<T> {
    pub(crate) fn ok(data: T) -> Json<Self> {
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
        list_containers,
        image_exists,
        import_image,
        health_check,
        crate::service_handlers::create_service,
        crate::service_handlers::stop_service,
        crate::service_handlers::start_service,
        crate::service_handlers::remove_service,
        crate::service_handlers::service_status,
        crate::service_handlers::service_exec,
        crate::service_handlers::list_services,
        crate::service_handlers::backup_service,
        crate::service_handlers::restore_service,
    ),
    components(schemas(
        AgentResponse<temps_deployer::DeployResult>,
        AgentResponse<String>,
        AgentResponse<bool>,
        AgentResponse<temps_deployer::ContainerInfo>,
        AgentResponse<NodeHealthReport>,
        AgentResponse<crate::ServiceCreateResponse>,
        AgentResponse<crate::ServiceExecResponse>,
        AgentResponse<crate::ServiceStatus>,
        AgentResponse<Vec<crate::ServiceStatus>>,
        AgentResponse<crate::ServiceBackupResponse>,
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
        crate::ServiceCreateRequest,
        crate::ServiceCreateResponse,
        crate::ServicePortMapping,
        crate::ServiceExecRequest,
        crate::ServiceExecResponse,
        crate::ServiceBackupRequest,
        crate::ServiceBackupResponse,
        crate::ServiceRestoreRequest,
        crate::S3CredentialsPayload,
        crate::ServiceStatus,
    )),
    info(
        title = "Temps Agent API",
        description = "Worker node agent API for container and service management. All endpoints require Bearer token authentication.",
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
    let container_name = request.container_name.clone();
    let image_name = request.image_name.clone();
    tracing::info!(
        container = %container_name,
        image = %image_name,
        ports = ?request.port_mappings.iter().map(|p| format!("{}:{}", p.host_port, p.container_port)).collect::<Vec<_>>(),
        "Deploying container"
    );
    match state.container_deployer.deploy_container(request).await {
        Ok(result) => {
            tracing::info!(
                container = %container_name,
                container_id = %result.container_id,
                image = %image_name,
                "Container deployed successfully"
            );
            AgentResponse::ok(result).into_response()
        }
        Err(e) => {
            tracing::error!(
                container = %container_name,
                image = %image_name,
                "Deploy failed: {}",
                e
            );
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Deploy failed: {}", e),
            )
            .into_response()
        }
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
    tracing::info!(container_id = %container_id, "Stopping container");
    match state.container_deployer.stop_container(&container_id).await {
        Ok(()) => {
            tracing::info!(container_id = %container_id, "Container stopped");
            AgentResponse::ok("stopped".to_string()).into_response()
        }
        Err(e) => {
            tracing::error!(container_id = %container_id, "Stop failed: {}", e);
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Stop failed for container {}: {}", container_id, e),
            )
            .into_response()
        }
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
    tracing::info!(container_id = %container_id, "Removing container");
    match state
        .container_deployer
        .remove_container(&container_id)
        .await
    {
        Ok(()) => {
            tracing::info!(container_id = %container_id, "Container removed");
            AgentResponse::ok("removed".to_string()).into_response()
        }
        Err(e) => {
            tracing::error!(container_id = %container_id, "Remove failed: {}", e);
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Remove failed for container {}: {}", container_id, e),
            )
            .into_response()
        }
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
    tracing::debug!(container_id = %container_id, "Fetching container logs");
    match state
        .container_deployer
        .get_container_logs(&container_id)
        .await
    {
        Ok(logs) => AgentResponse::ok(logs).into_response(),
        Err(e) => {
            tracing::error!(container_id = %container_id, "Failed to get logs: {}", e);
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to get logs for container {}: {}", container_id, e),
            )
            .into_response()
        }
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
    tracing::debug!(container_id = %container_id, "Fetching container info");
    match state
        .container_deployer
        .get_container_info(&container_id)
        .await
    {
        Ok(info) => AgentResponse::ok(info).into_response(),
        Err(e) => {
            tracing::error!(container_id = %container_id, "Failed to get info: {}", e);
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to get info for container {}: {}", container_id, e),
            )
            .into_response()
        }
    }
}

/// List all containers on this worker node
#[utoipa::path(
    tag = "Containers",
    get,
    path = "/agent/containers",
    responses(
        (status = 200, description = "List of containers", body = AgentResponse<Vec<temps_deployer::ContainerInfo>>),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Failed to list containers")
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_containers(State(state): State<Arc<AgentState>>) -> impl IntoResponse {
    tracing::debug!("Listing containers");
    match state.container_deployer.list_containers().await {
        Ok(containers) => AgentResponse::ok(containers).into_response(),
        Err(e) => {
            tracing::error!("Failed to list containers: {}", e);
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to list containers: {}", e),
            )
            .into_response()
        }
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
    tracing::debug!(image = %image_name, "Checking if image exists");
    match state.container_deployer.image_exists(&image_name).await {
        Ok(exists) => {
            tracing::debug!(image = %image_name, exists = exists, "Image existence check complete");
            AgentResponse::ok(exists).into_response()
        }
        Err(e) => {
            tracing::error!(image = %image_name, "Failed to check image: {}", e);
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to check image {}: {}", image_name, e),
            )
            .into_response()
        }
    }
}

/// Import a Docker image from a tar archive streamed in the request body.
///
/// The control plane calls this to transfer locally-built images to worker nodes.
/// The image tag is passed via the `x-image-tag` header.
#[utoipa::path(
    tag = "Images",
    post,
    path = "/agent/images/import",
    request_body(content = Vec<u8>, content_type = "application/x-tar"),
    responses(
        (status = 200, description = "Image imported successfully", body = AgentResponse<String>),
        (status = 400, description = "Missing x-image-tag header"),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Import failed")
    ),
    security(("bearer_auth" = []))
)]
pub async fn import_image(
    State(state): State<Arc<AgentState>>,
    headers: axum::http::HeaderMap,
    body: axum::body::Body,
) -> impl IntoResponse {
    let tag = match headers.get("x-image-tag").and_then(|v| v.to_str().ok()) {
        Some(t) => t.to_string(),
        None => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "Missing required x-image-tag header".to_string(),
            )
            .into_response();
        }
    };

    tracing::info!(image = %tag, "Receiving image tar from control plane");

    // Stream the body to a temp file
    let tmp_dir = std::env::temp_dir();
    let tmp_path = tmp_dir.join(format!("temps-image-import-{}.tar", uuid::Uuid::new_v4()));

    let write_result = async {
        use http_body_util::BodyExt;

        let mut file = tokio::fs::File::create(&tmp_path).await?;
        let mut total_bytes: u64 = 0;

        let mut body = body;
        while let Some(frame) = BodyExt::frame(&mut body).await {
            let frame =
                frame.map_err(|e| std::io::Error::other(format!("Body read error: {}", e)))?;
            if let Ok(data) = frame.into_data() {
                tokio::io::AsyncWriteExt::write_all(&mut file, &data).await?;
                total_bytes += data.len() as u64;
            }
        }
        tokio::io::AsyncWriteExt::flush(&mut file).await?;

        tracing::info!(
            image = %tag,
            size_mb = format!("{:.1}", total_bytes as f64 / 1_048_576.0),
            "Image tar received, loading into Docker"
        );
        Ok::<_, std::io::Error>(())
    }
    .await;

    if let Err(e) = write_result {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to write image tar: {}", e),
        )
        .into_response();
    }

    // Import the image via the image builder (docker load)
    let result = state
        .image_builder
        .import_image(tmp_path.clone(), &tag)
        .await;

    // Clean up temp file
    let _ = tokio::fs::remove_file(&tmp_path).await;

    match result {
        Ok(image_id) => {
            tracing::info!(image = %tag, image_id = %image_id, "Image imported successfully");
            AgentResponse::ok(image_id).into_response()
        }
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to import image '{}': {}", tag, e),
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
pub async fn health_check(State(state): State<Arc<AgentState>>) -> impl IntoResponse {
    let report = collect_system_metrics(&state).await;
    AgentResponse::ok(report)
}

/// Collect real system metrics using sysinfo.
async fn collect_system_metrics(state: &AgentState) -> NodeHealthReport {
    use sysinfo::{CpuExt, DiskExt, SystemExt};

    let mut sys = sysinfo::System::new();
    sys.refresh_cpu();
    sys.refresh_memory();
    sys.refresh_disks_list();
    sys.refresh_disks();

    let cpu_percent = sys.global_cpu_info().cpu_usage() as f64;
    let memory_used_bytes = sys.used_memory();
    let memory_total_bytes = sys.total_memory();

    // Use only the root mount point to avoid double-counting overlapping mounts
    let (disk_used, disk_total) = sys
        .disks()
        .iter()
        .find(|d| d.mount_point() == std::path::Path::new("/"))
        .map(|d| (d.total_space() - d.available_space(), d.total_space()))
        .unwrap_or((0, 0));

    // Count running containers via the deployer
    let running_containers = match state.container_deployer.list_containers().await {
        Ok(containers) => containers.len() as u64,
        Err(_) => 0,
    };

    NodeHealthReport {
        cpu_percent,
        memory_used_bytes,
        memory_total_bytes,
        disk_used_bytes: disk_used,
        disk_total_bytes: disk_total,
        running_containers,
    }
}
