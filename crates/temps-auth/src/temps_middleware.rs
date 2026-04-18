//! TempsMiddleware implementation for authentication
//!
//! This module provides the TempsMiddleware trait implementation for authentication,
//! allowing the auth middleware to integrate properly with the plugin system while
//! maintaining access to the AuthState services.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use axum::{extract::Request, http::StatusCode, middleware::Next, response::Response};
use temps_core::plugin::{
    MiddlewareCondition, MiddlewarePriority, PluginContext, PluginError, TempsMiddleware,
};

use crate::{
    apikey_service::ApiKeyService, auth_service::AuthService,
    deployment_token_service::DeploymentTokenValidationService, user_service::UserService,
};
use temps_core::CookieCrypto;

/// Authentication middleware that implements TempsMiddleware
pub struct AuthMiddleware {
    api_key_service: Arc<ApiKeyService>,
    auth_service: Arc<AuthService>,
    user_service: Arc<UserService>,
    cookie_crypto: Arc<CookieCrypto>,
    deployment_token_service: DeploymentTokenValidationService,
}

impl AuthMiddleware {
    pub fn new(
        api_key_service: Arc<ApiKeyService>,
        auth_service: Arc<AuthService>,
        user_service: Arc<UserService>,
        cookie_crypto: Arc<CookieCrypto>,
        db: Arc<sea_orm::DatabaseConnection>,
    ) -> Self {
        let deployment_token_service = DeploymentTokenValidationService::new(db);
        Self {
            api_key_service,
            auth_service,
            user_service,
            cookie_crypto,
            deployment_token_service,
        }
    }
}

// Note: Default implementation removed since AuthState is required

impl TempsMiddleware for AuthMiddleware {
    fn name(&self) -> &'static str {
        "auth_middleware"
    }

    fn plugin_name(&self) -> &'static str {
        "auth"
    }

    fn priority(&self) -> MiddlewarePriority {
        MiddlewarePriority::Security
    }

    fn condition(&self) -> MiddlewareCondition {
        MiddlewareCondition::Always
    }

    fn initialize(&mut self, _context: &PluginContext) -> Result<(), PluginError> {
        // AuthState is already provided in constructor, nothing to initialize
        Ok(())
    }

    fn execute<'a>(
        &'a self,
        req: Request,
        next: Next,
    ) -> Pin<Box<dyn Future<Output = Result<Response, StatusCode>> + Send + 'a>> {
        Box::pin(async move {
            // Use the AuthState directly

            // Call the simplified auth middleware
            match self.execute_auth_middleware_logic(req, next).await {
                Ok(response) => Ok(response),
                Err(status) => Ok(Response::builder()
                    .status(status)
                    .body(axum::body::Body::empty())
                    .unwrap()),
            }
        })
    }
}
impl AuthMiddleware {
    /// Simplified auth middleware that replicates the core logic without Send issues
    async fn execute_auth_middleware_logic(
        &self,
        mut req: Request,
        next: Next,
    ) -> Result<Response, StatusCode> {
        let mut user = None;

        // Extract auth context - simplified to avoid Send issues
        let auth_context = if let Some(auth_header) = req.headers().get("authorization") {
            if let Ok(auth_str) = auth_header.to_str() {
                if auth_str.starts_with("Bearer ") {
                    let token = auth_str.trim_start_matches("Bearer ");

                    // Try API key first (they have a specific format: tk_...)
                    if token.starts_with("tk_") {
                        if let Ok((api_user, role, permissions, key_name, key_id)) =
                            self.api_key_service.validate_api_key(token).await
                        {
                            user = Some(api_user.clone());
                            Some(crate::context::AuthContext::new_api_key(
                                api_user,
                                role,
                                permissions,
                                key_name,
                                key_id,
                            ))
                        } else {
                            None
                        }
                    } else if token.starts_with("dt_") {
                        // Try deployment token (they have a specific format: dt_...)
                        if let Ok(validated) =
                            self.deployment_token_service.validate_token(token).await
                        {
                            Some(crate::context::AuthContext::new_deployment_token(
                                validated.project_id,
                                validated.environment_id,
                                validated.deployment_id,
                                validated.token_id,
                                validated.name,
                                validated.permissions,
                            ))
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            // Try session cookie (only "session" cookie is used for user authentication)
            if let Some(session_token) =
                self.extract_user_session_from_cookies(req.headers(), &self.cookie_crypto)
            {
                if let Ok(session_user) = self.auth_service.verify_session(&session_token).await {
                    let user_role = if self
                        .user_service
                        .is_admin(session_user.id)
                        .await
                        .unwrap_or(false)
                    {
                        crate::permissions::Role::Admin
                    } else {
                        crate::permissions::Role::User
                    };
                    user = Some(session_user.clone());

                    Some(crate::context::AuthContext::new_session(
                        session_user,
                        user_role,
                    ))
                } else {
                    None
                }
            } else {
                None
            }
        };

        // Extract cookies for RequestMetadata
        let visitor_id_cookie =
            crate::middleware::extract_visitor_id_cookie(&req, &self.cookie_crypto);
        let session_id_cookie =
            crate::middleware::extract_session_id_cookie(&req, &self.cookie_crypto);

        // Create base URL from request headers
        let raw_host = req
            .headers()
            .get("host")
            .and_then(|h| h.to_str().ok())
            .unwrap_or("localhost")
            .to_string();

        // Determine scheme
        let scheme = if req
            .headers()
            .get("x-forwarded-proto")
            .and_then(|h| h.to_str().ok())
            == Some("https")
        {
            "https"
        } else {
            "http"
        };
        let is_secure = scheme == "https";
        // `base_url` keeps the port so generated links point back at the same
        // listener the client reached us on. `host` is port-stripped so it can
        // be used as a route-table key (the proxy normalizes identically).
        let base_url = format!("{}://{}", scheme, raw_host);
        let host = temps_core::host_without_port(&raw_host).to_string();

        // Create RequestMetadata
        let metadata = temps_core::RequestMetadata {
            ip_address: req
                .headers()
                .get("x-forwarded-for")
                .and_then(|h| h.to_str().ok())
                .and_then(|s| s.split(',').next())
                .unwrap_or("unknown")
                .to_string(),
            user_agent: req
                .headers()
                .get("user-agent")
                .and_then(|h| h.to_str().ok())
                .unwrap_or("unknown")
                .to_string(),
            headers: req.headers().clone(),
            visitor_id_cookie,
            session_id_cookie,
            base_url,
            scheme: scheme.to_string(),
            host,
            is_secure,
        };

        // Insert extensions
        req.extensions_mut().insert(metadata);

        // Insert authenticated user and context. Anonymous requests stay
        // anonymous: there is no implicit promotion to a Reader role for
        // unauthenticated callers. Issue an authenticated reader API key
        // if you want read-only programmatic access.
        if let Some(user) = user {
            req.extensions_mut().insert(user);
        }
        if let Some(auth_ctx) = auth_context {
            req.extensions_mut().insert(auth_ctx);
        }

        // Run the next middleware/handler
        Ok(next.run(req).await)
    }

    /// Extract user session from "session" cookie only (for authentication)
    fn extract_user_session_from_cookies(
        &self,
        headers: &axum::http::HeaderMap,
        crypto: &temps_core::CookieCrypto,
    ) -> Option<String> {
        use cookie::Cookie;

        // Iterate through ALL cookie headers (there can be multiple)
        for cookie_header in headers.get_all("cookie") {
            if let Ok(cookie_str) = cookie_header.to_str() {
                // Parse cookies and find the "session" cookie for user authentication
                for cookie in Cookie::split_parse(cookie_str).filter_map(Result::ok) {
                    if cookie.name() == "session" {
                        // Decrypt the session ID - if it fails, treat as no valid session
                        if let Ok(decrypted_session_id) = crypto.decrypt(cookie.value()) {
                            return Some(decrypted_session_id);
                        }
                    }
                }
            }
        }
        None
    }
}
