pub mod ai_providers;
pub mod autofixer;
pub mod config;
pub mod definitions;
pub mod preview_gateway;
pub mod runs;
pub mod secrets;
pub mod trigger;
pub mod workflows;

use axum::Router;
use std::sync::Arc;

use crate::services::autofixer::AutofixerService;
use crate::services::config_service::AgentConfigService;
use crate::services::definition_service::DefinitionService;
use crate::services::executor::AgentExecutor;
use crate::services::run_service::AgentRunService;
use crate::services::secret_service::SecretService;

pub struct AppState {
    pub db: Arc<sea_orm::DatabaseConnection>,
    pub encryption_service: Arc<temps_core::EncryptionService>,
    pub config_service: Arc<AgentConfigService>,
    pub run_service: Arc<AgentRunService>,
    pub executor: Arc<AgentExecutor>,
    pub audit_service: Arc<dyn temps_core::AuditLogger>,
    pub autofixer_service: Arc<AutofixerService>,
    pub secret_service: Arc<SecretService>,
    pub definition_service: Arc<DefinitionService>,
    /// Docker client used by the preview gateway supervisor handlers.
    pub docker: Arc<bollard::Docker>,
    /// Platform settings service used by the preview gateway handlers to
    /// persist image / auto-upgrade changes.
    pub platform_config_service: Arc<temps_config::ConfigService>,
}

pub fn configure_routes() -> Router<Arc<AppState>> {
    Router::new()
        .merge(ai_providers::routes())
        .merge(autofixer::routes())
        .merge(config::routes())
        .merge(definitions::routes())
        .merge(preview_gateway::routes())
        .merge(runs::routes())
        .merge(secrets::routes())
        .merge(trigger::routes())
        .merge(workflows::routes())
}
