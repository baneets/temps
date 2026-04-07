pub mod memory;
pub mod sessions;

use std::sync::Arc;

use axum::Router;
use sea_orm::DatabaseConnection;
use temps_core::AuditLogger;

use crate::services::memory_service::WorkflowMemoryService;
use crate::services::message_executor::MessageExecutor;
use crate::services::session_manager::WorkspaceSessionManager;
use crate::services::workspace_service::WorkspaceService;

/// Shared state for workspace HTTP handlers.
pub struct WorkspaceAppState {
    pub db: Arc<DatabaseConnection>,
    pub workspace_service: Arc<WorkspaceService>,
    pub session_manager: Arc<WorkspaceSessionManager>,
    pub message_executor: Option<Arc<MessageExecutor>>,
    pub memory_service: Arc<WorkflowMemoryService>,
    pub audit_service: Arc<dyn AuditLogger>,
    /// Platform settings service. Used to build preview URLs
    /// (`ws-<sid>-<port>.<preview_domain>`) for workspace session responses.
    pub platform_config_service: Arc<temps_config::ConfigService>,
    /// Docker client used by the terminal WebSocket handler to open a PTY
    /// exec against the session's sandbox container. Optional: when missing
    /// (e.g. local sandbox provider, non-Docker env), the terminal endpoint
    /// returns 503.
    pub docker: Option<Arc<bollard::Docker>>,
}

/// Configure all workspace routes.
pub fn configure_routes() -> Router<Arc<WorkspaceAppState>> {
    sessions::routes().merge(memory::routes())
}
