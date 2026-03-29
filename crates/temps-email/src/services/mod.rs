//! Email services

mod domain_service;
mod email_service;
mod provider_service;
mod tracking_service;
#[cfg(test)]
mod tracking_service_integration_tests;
mod validation_service;

pub use domain_service::{CreateDomainRequest, DomainService, DomainWithDnsRecords};
pub use email_service::{
    EmailService, EmailStats, ListEmailsOptions, SendEmailRequest, SendEmailResponse,
    TrackingRewriter,
};
pub use provider_service::{
    CreateProviderRequest, ProviderCredentials, ProviderService, TestEmailResult,
};
pub use tracking_service::{ExtractedLink, TrackingEvent, TrackingService, TransformResult};
pub use validation_service::{
    MiscResult, MxResult, ProxyConfig, ReachabilityStatus, SmtpResult, SyntaxResult,
    ValidateEmailRequest, ValidateEmailResponse, ValidationConfig, ValidationService,
};
