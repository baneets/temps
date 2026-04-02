pub mod autofixer;
pub mod config;
pub mod runs;
pub mod trigger;

use axum::Router;
use std::sync::Arc;

use crate::services::autofixer::AutofixerService;
use crate::services::config_service::AgentConfigService;
use crate::services::executor::AgentExecutor;
use crate::services::run_service::AgentRunService;

pub struct AppState {
    pub config_service: Arc<AgentConfigService>,
    pub run_service: Arc<AgentRunService>,
    pub executor: Arc<AgentExecutor>,
    pub audit_service: Arc<dyn temps_core::AuditLogger>,
    pub autofixer_service: Arc<AutofixerService>,
}

pub fn configure_routes() -> Router<Arc<AppState>> {
    Router::new()
        .merge(autofixer::routes())
        .merge(config::routes())
        .merge(runs::routes())
        .merge(trigger::routes())
}
