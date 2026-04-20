//! providers services and utilities

pub mod env_vars_provider_impl;
pub mod externalsvc;
pub mod parameter_strategies;
pub mod postgres_lifecycle;
pub mod postgres_upgrade_service;
pub mod query_service;
pub mod remote_service_client;
pub mod services;
pub use services::*;
pub mod plugin;
mod types;
mod utils;
pub use externalsvc::S3Credentials;
pub use externalsvc::ServiceType;
pub use query_service::QueryService;
pub mod handlers;

// Export plugin
pub use env_vars_provider_impl::ExternalServicesEnvProvider;
pub use plugin::ProvidersPlugin;
