//! Node Registration Handlers
//!
//! Internal API endpoints for worker nodes to register with the control plane
//! and send heartbeats. These endpoints use token-based authentication
//! (not the regular user auth) — the node presents the registration token
//! which is verified against the hashed token stored in the nodes table.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use sea_orm::DatabaseConnection;
use serde::{Deserialize, Serialize};
use sha2::Digest;
use temps_auth::RequireAuth;
use temps_config::ConfigService;
use tracing::{error, info, warn};
use utoipa::{OpenApi, ToSchema};

use crate::handlers::types::AppState;
use crate::services::node_service::{
    HeartbeatRequest, NodeError, NodeService, RegisterNodeRequest,
};
use temps_core::problemdetails::{self, Problem};

/// App state for node registration handlers
pub struct NodeAppState {
    pub node_service: Arc<NodeService>,
    pub db: Arc<DatabaseConnection>,
    pub config_service: Arc<ConfigService>,
    pub encryption_service: Arc<temps_core::EncryptionService>,
}

#[derive(Deserialize, ToSchema)]
pub struct RegisterNodeApiRequest {
    /// Unique name for this node
    pub name: String,
    /// Registration token (plaintext, will be hashed before storage)
    pub token: String,
    /// Join token to authorize this registration (must match the token generated in Settings)
    pub join_token: Option<String>,
    /// Node's reachable address (e.g., "10.100.0.2" or "192.168.1.50")
    pub address: String,
    /// Private/WireGuard address for inter-node communication
    pub private_address: String,
    /// Public endpoint for WireGuard (e.g., "203.0.113.1:51820")
    pub public_endpoint: Option<String>,
    /// WireGuard public key
    pub wg_public_key: Option<String>,
    /// Node role (default: "worker")
    pub role: Option<String>,
    /// Labels for scheduling (e.g., {"region": "us-east", "gpu": "true"})
    pub labels: Option<serde_json::Value>,
}

#[derive(Serialize, ToSchema)]
pub struct RegisterNodeResponse {
    pub id: i32,
    pub name: String,
    pub status: String,
    pub message: String,
}

#[derive(Deserialize, ToSchema)]
pub struct HeartbeatApiRequest {
    /// Resource capacity/usage info as JSON (cpu_usage, memory_usage, etc.)
    pub capacity: Option<serde_json::Value>,
}

#[derive(Serialize, ToSchema)]
pub struct HeartbeatResponse {
    pub status: String,
    pub message: String,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct NodeInfoResponse {
    pub id: i32,
    pub name: String,
    pub address: String,
    pub private_address: String,
    pub role: String,
    pub status: String,
    pub labels: serde_json::Value,
    pub last_heartbeat: Option<String>,
    pub created_at: String,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct NodeListResponse {
    pub nodes: Vec<NodeInfoResponse>,
    pub total: usize,
}

#[derive(OpenApi)]
#[openapi(
    paths(register_node, node_heartbeat, admin_list_nodes, admin_get_node,),
    components(schemas(
        RegisterNodeApiRequest,
        RegisterNodeResponse,
        HeartbeatApiRequest,
        HeartbeatResponse,
        NodeInfoResponse,
        NodeListResponse,
    )),
    info(
        title = "Node Registration API",
        description = "Internal API for worker nodes to register and send heartbeats to the control plane.",
        version = "1.0.0"
    )
)]
pub struct NodesApiDoc;

/// Configure agent-facing node routes (bearer token auth via NodeAppState).
/// These are mounted separately from the plugin system.
pub fn configure_routes() -> Router<Arc<NodeAppState>> {
    Router::new()
        .route("/internal/nodes/register", post(register_node))
        .route("/internal/nodes/{node_id}/heartbeat", post(node_heartbeat))
}

/// Configure UI-facing admin node routes (session auth via RequireAuth).
/// These are registered through the plugin system's AppState.
pub fn configure_admin_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/internal/nodes", get(admin_list_nodes))
        .route("/internal/nodes/{node_id}", get(admin_get_node))
}

/// SHA-256 hash a token string
fn sha256_hash(token: &str) -> String {
    let digest = sha2::Sha256::digest(token.as_bytes());
    format!("{:x}", digest)
}

/// Extract and verify the bearer token from request headers.
fn extract_bearer_token(headers: &HeaderMap) -> Result<String, Problem> {
    let auth_header = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| {
            problemdetails::new(StatusCode::UNAUTHORIZED)
                .with_title("Missing Authorization")
                .with_detail("Bearer token required for node authentication")
        })?;

    let token = auth_header.strip_prefix("Bearer ").ok_or_else(|| {
        problemdetails::new(StatusCode::UNAUTHORIZED)
            .with_title("Invalid Authorization")
            .with_detail("Authorization header must use Bearer scheme")
    })?;

    Ok(token.to_string())
}

/// Register a new worker node or reconnect an existing one
#[utoipa::path(
    tag = "Nodes",
    post,
    path = "/internal/nodes/register",
    request_body = RegisterNodeApiRequest,
    responses(
        (status = 201, description = "Node registered successfully", body = RegisterNodeResponse),
        (status = 200, description = "Node reconnected successfully", body = RegisterNodeResponse),
        (status = 400, description = "Validation error", ),
        (status = 500, description = "Internal server error", )
    )
)]
async fn register_node(
    State(app_state): State<Arc<NodeAppState>>,
    Json(request): Json<RegisterNodeApiRequest>,
) -> Result<impl IntoResponse, Problem> {
    // Validate join token against the stored hash in settings
    let settings = app_state.config_service.get_settings().await.map_err(|e| {
        error!("Failed to read settings for join token validation: {}", e);
        problemdetails::new(StatusCode::INTERNAL_SERVER_ERROR)
            .with_title("Internal Server Error")
            .with_detail("Failed to validate join token")
    })?;

    match settings.multi_node.join_token_hash {
        Some(ref stored_hash) => {
            // Join token is configured — require it
            match &request.join_token {
                Some(provided_token) => {
                    let provided_hash = sha256_hash(provided_token);
                    if provided_hash != *stored_hash {
                        warn!(
                            "Node registration rejected: invalid join token for node '{}'",
                            request.name
                        );
                        return Err(problemdetails::new(StatusCode::FORBIDDEN)
                            .with_title("Invalid Join Token")
                            .with_detail("The provided join token is invalid"));
                    }
                }
                None => {
                    warn!(
                        "Node registration rejected: missing join token for node '{}'",
                        request.name
                    );
                    return Err(problemdetails::new(StatusCode::FORBIDDEN)
                        .with_title("Join Token Required")
                        .with_detail("A join token is required to register a node. Generate one in Settings > Worker Nodes."));
                }
            }
        }
        None => {
            // No join token configured — block all registrations
            warn!("Node registration rejected: multi-node not enabled (no join token configured) for node '{}'", request.name);
            return Err(problemdetails::new(StatusCode::FORBIDDEN)
                .with_title("Registration Disabled")
                .with_detail("Node registration is not enabled. Generate a join token in Settings > Worker Nodes to enable multi-node."));
        }
    }

    let token_hash = sha256_hash(&request.token);

    // Encrypt the plaintext token so the control plane can authenticate
    // with the agent for remote deployments
    let token_encrypted = app_state
        .encryption_service
        .encrypt(request.token.as_bytes())
        .map_err(|e| {
            error!("Failed to encrypt node token: {}", e);
            problemdetails::new(StatusCode::INTERNAL_SERVER_ERROR)
                .with_title("Internal Server Error")
                .with_detail("Failed to process node registration")
        })?;

    let register_request = RegisterNodeRequest {
        name: request.name,
        token_hash,
        token_encrypted: Some(token_encrypted),
        address: request.address,
        private_address: request.private_address,
        public_endpoint: request.public_endpoint,
        wg_public_key: request.wg_public_key,
        role: request.role.unwrap_or_else(|| "worker".to_string()),
        labels: request.labels.unwrap_or(serde_json::json!({})),
    };

    let node = app_state
        .node_service
        .register(register_request)
        .await
        .map_err(Problem::from)?;

    info!(node_id = node.id, name = %node.name, "Node registered successfully");

    Ok((
        StatusCode::CREATED,
        Json(RegisterNodeResponse {
            id: node.id,
            name: node.name,
            status: node.status,
            message: "Node registered successfully. Send heartbeats to stay active.".to_string(),
        }),
    ))
}

/// Receive a heartbeat from a worker node
#[utoipa::path(
    tag = "Nodes",
    post,
    path = "/internal/nodes/{node_id}/heartbeat",
    params(
        ("node_id" = i32, Path, description = "Node ID")
    ),
    request_body = HeartbeatApiRequest,
    responses(
        (status = 200, description = "Heartbeat received", body = HeartbeatResponse),
        (status = 401, description = "Unauthorized", ),
        (status = 404, description = "Node not found", ),
        (status = 500, description = "Internal server error", )
    )
)]
async fn node_heartbeat(
    State(app_state): State<Arc<NodeAppState>>,
    headers: HeaderMap,
    Path(node_id): Path<i32>,
    Json(request): Json<HeartbeatApiRequest>,
) -> Result<impl IntoResponse, Problem> {
    // Verify the node's token
    let token = extract_bearer_token(&headers)?;

    // Get the node and verify token hash
    let node = app_state
        .node_service
        .get_by_id(node_id)
        .await
        .map_err(Problem::from)?;

    let token_hash = sha256_hash(&token);
    if node.token_hash != token_hash {
        warn!(node_id, "Invalid heartbeat token");
        return Err(problemdetails::new(StatusCode::UNAUTHORIZED)
            .with_title("Invalid Token")
            .with_detail(format!("Invalid authentication token for node {}", node_id)));
    }

    let heartbeat = HeartbeatRequest {
        capacity: request.capacity.unwrap_or(serde_json::json!({})),
    };

    app_state
        .node_service
        .heartbeat(node_id, heartbeat)
        .await
        .map_err(Problem::from)?;

    Ok(Json(HeartbeatResponse {
        status: "ok".to_string(),
        message: "Heartbeat received".to_string(),
    }))
}

/// List all registered nodes (admin — session auth via RequireAuth)
#[utoipa::path(
    tag = "Nodes",
    get,
    path = "/internal/nodes",
    responses(
        (status = 200, description = "List of nodes", body = NodeListResponse),
        (status = 401, description = "Unauthorized", ),
        (status = 500, description = "Internal server error", )
    ),
    security(("bearer_auth" = []))
)]
async fn admin_list_nodes(
    RequireAuth(_auth): RequireAuth,
    State(app_state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, Problem> {
    let nodes = app_state
        .node_service
        .list_all()
        .await
        .map_err(Problem::from)?;

    let total = nodes.len();
    let node_responses: Vec<NodeInfoResponse> = nodes
        .into_iter()
        .map(|n| NodeInfoResponse {
            id: n.id,
            name: n.name,
            address: n.address,
            private_address: n.private_address,
            role: n.role,
            status: n.status,
            labels: n.labels,
            last_heartbeat: n.last_heartbeat.map(|t| t.to_rfc3339()),
            created_at: n.created_at.to_rfc3339(),
        })
        .collect();

    Ok(Json(NodeListResponse {
        nodes: node_responses,
        total,
    }))
}

/// Get a specific node by ID (admin — session auth via RequireAuth)
#[utoipa::path(
    tag = "Nodes",
    get,
    path = "/internal/nodes/{node_id}",
    params(
        ("node_id" = i32, Path, description = "Node ID")
    ),
    responses(
        (status = 200, description = "Node details", body = NodeInfoResponse),
        (status = 401, description = "Unauthorized", ),
        (status = 404, description = "Node not found", ),
        (status = 500, description = "Internal server error", )
    ),
    security(("bearer_auth" = []))
)]
async fn admin_get_node(
    RequireAuth(_auth): RequireAuth,
    State(app_state): State<Arc<AppState>>,
    Path(node_id): Path<i32>,
) -> Result<impl IntoResponse, Problem> {
    let node = app_state
        .node_service
        .get_by_id(node_id)
        .await
        .map_err(Problem::from)?;

    Ok(Json(NodeInfoResponse {
        id: node.id,
        name: node.name,
        address: node.address,
        private_address: node.private_address,
        role: node.role,
        status: node.status,
        labels: node.labels,
        last_heartbeat: node.last_heartbeat.map(|t| t.to_rfc3339()),
        created_at: node.created_at.to_rfc3339(),
    }))
}

impl From<NodeError> for Problem {
    fn from(error: NodeError) -> Self {
        match error {
            NodeError::NotFound { ref name } => problemdetails::new(StatusCode::NOT_FOUND)
                .with_title("Node Not Found")
                .with_detail(format!("Node '{}' not found", name)),
            NodeError::NotFoundById { node_id } => problemdetails::new(StatusCode::NOT_FOUND)
                .with_title("Node Not Found")
                .with_detail(format!("Node with id {} not found", node_id)),
            NodeError::AlreadyExists { ref name } => problemdetails::new(StatusCode::CONFLICT)
                .with_title("Node Already Exists")
                .with_detail(format!("Node '{}' already exists", name)),
            NodeError::Validation { ref message } => problemdetails::new(StatusCode::BAD_REQUEST)
                .with_title("Validation Error")
                .with_detail(message.clone()),
            NodeError::Database(ref e) => {
                error!("Database error in node operation: {}", e);
                problemdetails::new(StatusCode::INTERNAL_SERVER_ERROR)
                    .with_title("Internal Server Error")
                    .with_detail("An internal error occurred")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use sea_orm::{DatabaseBackend, MockDatabase};
    use temps_entities::nodes;
    use tower::ServiceExt;

    fn sample_node() -> nodes::Model {
        nodes::Model {
            id: 1,
            name: "worker-1".to_string(),
            token_hash: sha256_hash("test-token"),
            token_encrypted: None,
            address: "https://10.100.0.2:3100".to_string(),
            private_address: "10.100.0.2".to_string(),
            public_endpoint: None,
            wg_public_key: None,
            role: "worker".to_string(),
            status: "active".to_string(),
            labels: serde_json::json!({}),
            capacity: serde_json::json!({}),
            last_heartbeat: Some(chrono::Utc::now()),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    fn make_app(db: sea_orm::DatabaseConnection) -> axum::Router {
        make_app_with_settings(db, temps_core::AppSettings::default())
    }

    fn make_app_with_settings(
        db: sea_orm::DatabaseConnection,
        settings: temps_core::AppSettings,
    ) -> axum::Router {
        let db = Arc::new(db);
        // Create a separate mock DB for ConfigService that returns settings
        let settings_json = settings.to_json();
        let config_db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![vec![temps_entities::settings::Model {
                id: 1,
                data: settings_json,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            }]])
            .into_connection();
        let server_config = Arc::new(temps_config::ServerConfig {
            address: "127.0.0.1:3000".to_string(),
            database_url: "postgres://test".to_string(),
            tls_address: None,
            console_address: "127.0.0.1:0".to_string(),
            data_dir: std::path::PathBuf::from("/tmp/temps-test"),
            auth_secret: "test-secret".to_string(),
            encryption_key: "test-key".to_string(),
            api_base_url: "/api".to_string(),
            postgres_max_connections: None,
            postgres_min_connections: None,
            postgres_connect_timeout_secs: None,
            postgres_acquire_timeout_secs: None,
            postgres_idle_timeout_secs: None,
            postgres_max_lifetime_secs: None,
        });
        let config_service = Arc::new(temps_config::ConfigService::new(
            server_config,
            Arc::new(config_db),
        ));
        let node_service = Arc::new(NodeService::new(db.clone()));
        let encryption_service = Arc::new(
            temps_core::EncryptionService::new("01234567890123456789012345678901").unwrap(),
        );
        let app_state = Arc::new(NodeAppState {
            node_service,
            db,
            config_service,
            encryption_service,
        });
        configure_routes().with_state(app_state)
    }

    fn settings_with_join_token() -> temps_core::AppSettings {
        let mut settings = temps_core::AppSettings::default();
        settings.multi_node.join_token_hash = Some(sha256_hash("test-join-token"));
        settings
    }

    #[tokio::test]
    async fn test_register_node_success() {
        let node = sample_node();
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            // Check for duplicate name (returns empty)
            .append_query_results(vec![Vec::<nodes::Model>::new()])
            // Insert returns the new node
            .append_query_results(vec![vec![node.clone()]])
            .into_connection();

        let app = make_app_with_settings(db, settings_with_join_token());
        let body = serde_json::json!({
            "name": "worker-1",
            "token": "test-token",
            "join_token": "test-join-token",
            "address": "https://10.100.0.2:3100",
            "private_address": "10.100.0.2"
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/internal/nodes/register")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CREATED);
    }

    #[tokio::test]
    async fn test_register_node_blocked_without_join_token_configured() {
        let db = MockDatabase::new(DatabaseBackend::Postgres).into_connection();
        // Default settings — no join token configured
        let app = make_app(db);

        let body = serde_json::json!({
            "name": "worker-1",
            "token": "test-token",
            "address": "https://10.100.0.2:3100",
            "private_address": "10.100.0.2"
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/internal/nodes/register")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_register_node_empty_name_returns_400() {
        let db = MockDatabase::new(DatabaseBackend::Postgres).into_connection();
        let app = make_app_with_settings(db, settings_with_join_token());

        let body = serde_json::json!({
            "name": "",
            "token": "test-token",
            "join_token": "test-join-token",
            "address": "https://10.100.0.2:3100",
            "private_address": "10.100.0.2"
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/internal/nodes/register")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_heartbeat_missing_auth_returns_401() {
        let db = MockDatabase::new(DatabaseBackend::Postgres).into_connection();
        let app = make_app(db);

        let body = serde_json::json!({ "capacity": {} });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/internal/nodes/1/heartbeat")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_heartbeat_wrong_token_returns_401() {
        let node = sample_node();
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            // get_by_id returns the node
            .append_query_results(vec![vec![node]])
            .into_connection();

        let app = make_app(db);

        let body = serde_json::json!({ "capacity": {} });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/internal/nodes/1/heartbeat")
                    .header("content-type", "application/json")
                    .header("authorization", "Bearer wrong-token")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn test_sha256_hash_deterministic() {
        let hash1 = sha256_hash("test-token");
        let hash2 = sha256_hash("test-token");
        assert_eq!(hash1, hash2);
        assert_eq!(hash1.len(), 64); // SHA-256 produces 64 hex chars
    }

    #[test]
    fn test_sha256_hash_different_inputs() {
        let hash1 = sha256_hash("token-a");
        let hash2 = sha256_hash("token-b");
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_node_error_to_problem_not_found() {
        let problem: Problem = NodeError::NotFound {
            name: "worker-x".to_string(),
        }
        .into();
        assert_eq!(problem.status_code, StatusCode::NOT_FOUND);
    }

    #[test]
    fn test_node_error_to_problem_not_found_by_id() {
        let problem: Problem = NodeError::NotFoundById { node_id: 42 }.into();
        assert_eq!(problem.status_code, StatusCode::NOT_FOUND);
    }

    #[test]
    fn test_node_error_to_problem_already_exists() {
        let problem: Problem = NodeError::AlreadyExists {
            name: "worker-1".to_string(),
        }
        .into();
        assert_eq!(problem.status_code, StatusCode::CONFLICT);
    }

    #[test]
    fn test_node_error_to_problem_validation() {
        let problem: Problem = NodeError::Validation {
            message: "bad input".to_string(),
        }
        .into();
        assert_eq!(problem.status_code, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_register_node_with_valid_join_token_succeeds() {
        let node = sample_node();
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![Vec::<nodes::Model>::new()])
            .append_query_results(vec![vec![node.clone()]])
            .into_connection();

        let mut settings = temps_core::AppSettings::default();
        settings.multi_node.join_token_hash = Some(sha256_hash("valid-join-token"));

        let app = make_app_with_settings(db, settings);
        let body = serde_json::json!({
            "name": "worker-1",
            "token": "test-token",
            "join_token": "valid-join-token",
            "address": "https://10.100.0.2:3100",
            "private_address": "10.100.0.2"
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/internal/nodes/register")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CREATED);
    }

    #[tokio::test]
    async fn test_register_node_with_invalid_join_token_returns_403() {
        let db = MockDatabase::new(DatabaseBackend::Postgres).into_connection();

        let mut settings = temps_core::AppSettings::default();
        settings.multi_node.join_token_hash = Some(sha256_hash("correct-token"));

        let app = make_app_with_settings(db, settings);
        let body = serde_json::json!({
            "name": "worker-1",
            "token": "test-token",
            "join_token": "wrong-token",
            "address": "https://10.100.0.2:3100",
            "private_address": "10.100.0.2"
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/internal/nodes/register")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_register_node_missing_join_token_when_required_returns_403() {
        let db = MockDatabase::new(DatabaseBackend::Postgres).into_connection();

        let mut settings = temps_core::AppSettings::default();
        settings.multi_node.join_token_hash = Some(sha256_hash("some-token"));

        let app = make_app_with_settings(db, settings);
        let body = serde_json::json!({
            "name": "worker-1",
            "token": "test-token",
            "address": "https://10.100.0.2:3100",
            "private_address": "10.100.0.2"
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/internal/nodes/register")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    // Note: admin_list_nodes and admin_get_node use RequireAuth (session auth)
    // and are tested through the plugin system's auth middleware integration.
    // The agent-facing routes (register, heartbeat) are tested above with bearer tokens.
}
