use std::sync::Arc;
use temps_core::AuditLogger;

use crate::services::ComposeService;

pub struct ComposeAppState {
    pub compose_service: Arc<ComposeService>,
    pub audit_service: Arc<dyn AuditLogger>,
}

pub async fn create_compose_app_state(
    compose_service: Arc<ComposeService>,
    audit_service: Arc<dyn AuditLogger>,
) -> Arc<ComposeAppState> {
    Arc::new(ComposeAppState {
        compose_service,
        audit_service,
    })
}
