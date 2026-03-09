//! Agent HTTP server setup and routing.

use axum::{
    middleware,
    routing::{delete, get, post},
    Extension, Router,
};
use std::sync::Arc;
use std::time::Duration;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use crate::auth::{require_agent_auth, AgentAuth};
use crate::handlers::{self, AgentApiDoc, AgentState};
use crate::AgentConfig;
use temps_deployer::{ContainerDeployer, ImageBuilder};

/// Build the agent Axum router with authentication middleware.
pub fn build_router(
    container_deployer: Arc<dyn ContainerDeployer>,
    image_builder: Arc<dyn ImageBuilder>,
    config: &AgentConfig,
) -> Router {
    let state = Arc::new(AgentState {
        container_deployer,
        image_builder,
    });

    let auth = Arc::new(AgentAuth::new(&config.token));

    // API routes — all protected by bearer token auth
    let api_routes = Router::new()
        .route("/agent/containers/deploy", post(handlers::deploy_container))
        .route(
            "/agent/containers/{id}/stop",
            post(handlers::stop_container),
        )
        .route("/agent/containers/{id}", delete(handlers::remove_container))
        .route(
            "/agent/containers/{id}/logs",
            get(handlers::get_container_logs),
        )
        .route(
            "/agent/containers/{id}/info",
            get(handlers::get_container_info),
        )
        .route("/agent/images/import", post(handlers::import_image))
        .route("/agent/images/{name}/exists", get(handlers::image_exists))
        .route("/agent/health", get(handlers::health_check))
        .layer(middleware::from_fn(require_agent_auth))
        .layer(Extension(auth))
        .with_state(state);

    // Swagger UI — no auth required so it's accessible for documentation
    let swagger_ui =
        SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", AgentApiDoc::openapi());

    api_routes.merge(swagger_ui)
}

/// Maximum number of consecutive heartbeat failures before escalating to error-level logging.
const HEARTBEAT_MAX_RETRIES: u32 = 3;

/// Base delay for exponential backoff between heartbeat retries (doubled each attempt).
const HEARTBEAT_RETRY_BASE_DELAY: Duration = Duration::from_secs(2);

/// Maximum backoff delay between heartbeat retries.
const HEARTBEAT_RETRY_MAX_DELAY: Duration = Duration::from_secs(15);

/// Spawn a background task that sends heartbeats to the control plane every 30 seconds.
///
/// On transient failures, retries up to `HEARTBEAT_MAX_RETRIES` times with exponential
/// backoff before giving up for this interval. This prevents a brief network blip from
/// causing the control plane to mark the node as offline (90s stale threshold).
fn spawn_heartbeat_loop(config: &AgentConfig) {
    let control_plane_url = config.control_plane_url.clone();
    let node_id = config.node_id;
    let token = config.token.clone();
    let labels = config.labels.clone();

    tokio::spawn(async move {
        let client = match reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .timeout(Duration::from_secs(10))
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("Failed to build heartbeat HTTP client: {}", e);
                return;
            }
        };

        let heartbeat_url = format!(
            "{}/api/internal/nodes/{}/heartbeat",
            control_plane_url, node_id
        );

        let mut interval = tokio::time::interval(Duration::from_secs(30));
        let mut consecutive_failures: u32 = 0;

        loop {
            interval.tick().await;

            let capacity = collect_capacity_metrics();
            let body = serde_json::json!({ "capacity": capacity, "labels": labels });

            let mut attempt = 0;
            let mut succeeded = false;

            loop {
                match client
                    .post(&heartbeat_url)
                    .bearer_auth(&token)
                    .json(&body)
                    .send()
                    .await
                {
                    Ok(response) if response.status().is_success() => {
                        if consecutive_failures > 0 {
                            tracing::info!(
                                node_id = node_id,
                                previous_failures = consecutive_failures,
                                "Heartbeat recovered after {} consecutive failure(s)",
                                consecutive_failures
                            );
                        }
                        consecutive_failures = 0;
                        succeeded = true;
                        tracing::debug!(node_id = node_id, "Heartbeat sent to control plane");
                        break;
                    }
                    Ok(response)
                        if response.status().is_server_error()
                            && attempt < HEARTBEAT_MAX_RETRIES =>
                    {
                        // Server errors are retryable
                        attempt += 1;
                        let delay = std::cmp::min(
                            HEARTBEAT_RETRY_BASE_DELAY * 2u32.saturating_pow(attempt - 1),
                            HEARTBEAT_RETRY_MAX_DELAY,
                        );
                        tracing::warn!(
                            node_id = node_id,
                            attempt = attempt,
                            status = %response.status(),
                            retry_in_ms = delay.as_millis() as u64,
                            "Heartbeat failed with server error, retrying"
                        );
                        tokio::time::sleep(delay).await;
                    }
                    Ok(response) => {
                        // Client error (4xx) or exhausted retries — don't retry
                        tracing::warn!(
                            node_id = node_id,
                            status = %response.status(),
                            "Heartbeat failed with status {}",
                            response.status()
                        );
                        break;
                    }
                    Err(e) if attempt < HEARTBEAT_MAX_RETRIES => {
                        // Network errors are retryable
                        attempt += 1;
                        let delay = std::cmp::min(
                            HEARTBEAT_RETRY_BASE_DELAY * 2u32.saturating_pow(attempt - 1),
                            HEARTBEAT_RETRY_MAX_DELAY,
                        );
                        tracing::warn!(
                            node_id = node_id,
                            attempt = attempt,
                            retry_in_ms = delay.as_millis() as u64,
                            "Heartbeat network error, retrying: {}",
                            e
                        );
                        tokio::time::sleep(delay).await;
                    }
                    Err(e) => {
                        tracing::warn!(
                            node_id = node_id,
                            attempts = attempt + 1,
                            "Heartbeat failed after {} attempt(s): {}",
                            attempt + 1,
                            e
                        );
                        break;
                    }
                }
            }

            if !succeeded {
                consecutive_failures += 1;
                if consecutive_failures >= 3 {
                    tracing::error!(
                        node_id = node_id,
                        consecutive_failures = consecutive_failures,
                        "Heartbeat has failed {} consecutive times — node may be marked offline",
                        consecutive_failures
                    );
                }
            }
        }
    });
}

/// Collect system resource metrics for heartbeat capacity data.
fn collect_capacity_metrics() -> serde_json::Value {
    use sysinfo::{CpuExt, DiskExt, SystemExt};

    let mut sys = sysinfo::System::new();
    sys.refresh_cpu();
    sys.refresh_memory();
    sys.refresh_disks_list();
    sys.refresh_disks();

    // Use only the root mount point to avoid double-counting overlapping mounts
    let (disk_used, disk_total) = sys
        .disks()
        .iter()
        .find(|d| d.mount_point() == std::path::Path::new("/"))
        .map(|d| (d.total_space() - d.available_space(), d.total_space()))
        .unwrap_or((0, 0));

    serde_json::json!({
        "cpu_percent": sys.global_cpu_info().cpu_usage(),
        "memory_used_bytes": sys.used_memory(),
        "memory_total_bytes": sys.total_memory(),
        "disk_used_bytes": disk_used,
        "disk_total_bytes": disk_total,
    })
}

/// Start the agent server. This blocks until the server shuts down.
pub async fn start_agent_server(
    container_deployer: Arc<dyn ContainerDeployer>,
    image_builder: Arc<dyn ImageBuilder>,
    config: AgentConfig,
) -> Result<(), crate::AgentError> {
    let router = build_router(container_deployer, image_builder, &config);

    // Start heartbeat background loop
    spawn_heartbeat_loop(&config);

    let listener = tokio::net::TcpListener::bind(&config.listen_address)
        .await
        .map_err(|e| {
            crate::AgentError::ServerError(format!(
                "Failed to bind to {}: {}",
                config.listen_address, e
            ))
        })?;

    tracing::info!(
        address = %config.listen_address,
        node = %config.node_name,
        node_id = config.node_id,
        swagger_ui = format!("http://{}/swagger-ui/", config.listen_address),
        "Temps agent server started"
    );

    axum::serve(listener, router)
        .await
        .map_err(|e| crate::AgentError::ServerError(format!("Agent server error: {}", e)))?;

    Ok(())
}
