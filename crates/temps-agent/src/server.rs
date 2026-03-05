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

/// Spawn a background task that sends heartbeats to the control plane every 30 seconds.
fn spawn_heartbeat_loop(config: &AgentConfig) {
    let control_plane_url = config.control_plane_url.clone();
    let node_id = config.node_id;
    let token = config.token.clone();

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

        loop {
            interval.tick().await;

            let body = serde_json::json!({ "capacity": {} });

            match client
                .post(&heartbeat_url)
                .bearer_auth(&token)
                .json(&body)
                .send()
                .await
            {
                Ok(response) if response.status().is_success() => {
                    tracing::info!(node_id = node_id, "Heartbeat sent to control plane");
                }
                Ok(response) => {
                    tracing::warn!(
                        node_id = node_id,
                        status = %response.status(),
                        "Heartbeat failed with status {}",
                        response.status()
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        node_id = node_id,
                        error = %e,
                        "Heartbeat failed: {}",
                        e
                    );
                }
            }
        }
    });
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
