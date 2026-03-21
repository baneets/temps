use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use temps_core::plugin::{
    PluginContext, PluginError, PluginRoutes, ServiceRegistrationContext, TempsPlugin,
};
use tracing;
use utoipa::openapi::OpenApi;
use utoipa::OpenApi as OpenApiTrait;

use crate::{
    handlers::{self, create_compose_app_state, ComposeAppState},
    services::ComposeService,
};

pub struct ComposePlugin;

impl ComposePlugin {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ComposePlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl TempsPlugin for ComposePlugin {
    fn name(&self) -> &'static str {
        "compose"
    }

    fn register_services<'a>(
        &'a self,
        context: &'a ServiceRegistrationContext,
    ) -> Pin<Box<dyn Future<Output = Result<(), PluginError>> + Send + 'a>> {
        Box::pin(async move {
            let db = context.require_service::<sea_orm::DatabaseConnection>();

            let compose_service = Arc::new(ComposeService::new(db.clone()));
            context.register_service(compose_service.clone());

            let audit_service = context.require_service::<dyn temps_core::AuditLogger>();

            let compose_app_state = create_compose_app_state(compose_service, audit_service).await;
            context.register_service(compose_app_state);

            tracing::debug!("Compose plugin services registered successfully");
            Ok(())
        })
    }

    fn configure_routes(&self, context: &PluginContext) -> Option<PluginRoutes> {
        let compose_app_state = context.require_service::<ComposeAppState>();

        let compose_routes = handlers::configure_routes().with_state(compose_app_state);

        Some(PluginRoutes {
            router: compose_routes,
        })
    }

    fn openapi_schema(&self) -> Option<OpenApi> {
        Some(<handlers::compose_handler::ComposeApiDoc as OpenApiTrait>::openapi())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_compose_plugin_name() {
        let plugin = ComposePlugin::new();
        assert_eq!(plugin.name(), "compose");
    }

    #[tokio::test]
    async fn test_compose_plugin_default() {
        let plugin = ComposePlugin;
        assert_eq!(plugin.name(), "compose");
    }
}
