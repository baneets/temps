use axum::extract::Request;
use axum::http::HeaderMap;
use axum::middleware::Next;
use axum::response::Response;
use cookie::Cookie;
use std::sync::Arc;

use crate::cookie_crypto::CookieCrypto;

#[derive(Clone)]
pub struct RequestMetadata {
    pub ip_address: String,
    pub user_agent: String,
    pub headers: HeaderMap,
    pub visitor_id_cookie: Option<String>,
    pub session_id_cookie: Option<String>,
    /// Full origin including scheme and `:port` suffix when present.
    /// Suitable for constructing user-visible absolute URLs (links, redirects).
    pub base_url: String,
    pub scheme: String, // "http" or "https"
    /// Hostname from the Host header with any `:port` suffix stripped.
    /// Matches the key used by the proxy route table, so handlers can safely
    /// pass this into `CachedPeerTable::get_route(&metadata.host)` without
    /// worrying about non-standard ports (e.g. the :8080 dev proxy).
    pub host: String,
    pub is_secure: bool, // true if HTTPS
}

const SESSION_ID_COOKIE_NAME: &str = "_temps_sid";
const VISITOR_ID_COOKIE_NAME: &str = "_temps_visitor_id";

/// Strip any `:port` suffix from a raw Host header.
///
/// The proxy's route table is keyed on the hostname only, so requests that
/// arrive on non-default ports (dev setups, `localho.st:8080`, etc.) must be
/// normalized before lookup. IPv6 literals are not supported in Host headers
/// without brackets, so naive `split(':')` is sufficient here.
pub fn host_without_port(raw_host: &str) -> &str {
    raw_host.split(':').next().unwrap_or(raw_host)
}

fn extract_encrypted_cookie(
    headers: &HeaderMap,
    name: &str,
    crypto: &CookieCrypto,
) -> Option<String> {
    for cookie_header in headers.get_all("Cookie") {
        if let Ok(cookie_str) = cookie_header.to_str() {
            for cookie in Cookie::split_parse(cookie_str).flatten() {
                if cookie.name() == name {
                    return crypto.decrypt(cookie.value()).ok();
                }
            }
        }
    }
    None
}

/// Build a `RequestMetadata` value from the incoming request. Used by the
/// middleware below, and also reusable from tests that synthesize requests.
pub fn build_from_request(req: &Request, crypto: &CookieCrypto) -> RequestMetadata {
    let headers = req.headers();

    let visitor_id_cookie = extract_encrypted_cookie(headers, VISITOR_ID_COOKIE_NAME, crypto);
    let session_id_cookie = extract_encrypted_cookie(headers, SESSION_ID_COOKIE_NAME, crypto);

    let raw_host = headers
        .get("host")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("localhost")
        .to_string();

    let scheme = if headers
        .get("x-forwarded-proto")
        .and_then(|h| h.to_str().ok())
        == Some("https")
    {
        "https"
    } else {
        "http"
    };
    let is_secure = scheme == "https";
    let base_url = format!("{}://{}", scheme, raw_host);
    let host = host_without_port(&raw_host).to_string();

    RequestMetadata {
        ip_address: headers
            .get("x-forwarded-for")
            .and_then(|h| h.to_str().ok())
            .and_then(|s| s.split(',').next())
            .unwrap_or("unknown")
            .to_string(),
        user_agent: headers
            .get("user-agent")
            .and_then(|h| h.to_str().ok())
            .unwrap_or("unknown")
            .to_string(),
        headers: headers.clone(),
        visitor_id_cookie,
        session_id_cookie,
        base_url,
        scheme: scheme.to_string(),
        host,
        is_secure,
    }
}

/// Middleware that constructs a `RequestMetadata` value from the incoming
/// request and inserts it into request extensions. Must run before any
/// handler that extracts `Extension<RequestMetadata>`.
///
/// Applied to both the admin and public routers in
/// `PluginManager::build_split_application` — public ingest endpoints
/// (session replay init, analytics events, etc.) depend on this metadata
/// even though they don't go through auth.
pub async fn request_metadata_middleware(
    crypto: Arc<CookieCrypto>,
    mut req: Request,
    next: Next,
) -> Response {
    let metadata = build_from_request(&req, crypto.as_ref());
    req.extensions_mut().insert(metadata);
    next.run(req).await
}

/// `TempsMiddleware` implementation for request-metadata injection. Owns an
/// `Arc<CookieCrypto>` so the plugin system can construct it once at startup
/// and reuse it across both the admin and public routers.
pub struct RequestMetadataMiddleware {
    crypto: Arc<CookieCrypto>,
}

impl RequestMetadataMiddleware {
    pub fn new(crypto: Arc<CookieCrypto>) -> Self {
        Self { crypto }
    }
}

impl crate::plugin::TempsMiddleware for RequestMetadataMiddleware {
    fn name(&self) -> &'static str {
        "request_metadata_middleware"
    }

    fn plugin_name(&self) -> &'static str {
        "core"
    }

    fn priority(&self) -> crate::plugin::MiddlewarePriority {
        // Runs before auth (Security=0) so auth handlers can also read the
        // metadata extension. Observability=100 is the natural slot for
        // request-context enrichment.
        crate::plugin::MiddlewarePriority::Observability
    }

    fn apply_to_public(&self) -> bool {
        true
    }

    fn execute<'a>(
        &'a self,
        mut req: Request,
        next: Next,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Response, axum::http::StatusCode>> + Send + 'a>,
    > {
        Box::pin(async move {
            let metadata = build_from_request(&req, self.crypto.as_ref());
            req.extensions_mut().insert(metadata);
            Ok(next.run(req).await)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request as HttpRequest, StatusCode};
    use axum::routing::get;
    use axum::{Extension, Router};
    use tower::ServiceExt;

    fn test_crypto() -> Arc<CookieCrypto> {
        let key = [42u8; 32];
        Arc::new(CookieCrypto::from_bytes(&key))
    }

    #[test]
    fn strips_port_when_present() {
        assert_eq!(
            host_without_port("sandbox-test.localho.st:8080"),
            "sandbox-test.localho.st"
        );
    }

    #[test]
    fn passes_through_when_no_port() {
        assert_eq!(host_without_port("example.com"), "example.com");
    }

    #[test]
    fn handles_empty_string() {
        assert_eq!(host_without_port(""), "");
    }

    #[tokio::test]
    async fn middleware_injects_metadata_extension() {
        let crypto = test_crypto();
        let app = Router::new()
            .route(
                "/echo",
                get(|Extension(meta): Extension<RequestMetadata>| async move {
                    format!(
                        "host={};scheme={};ua={}",
                        meta.host, meta.scheme, meta.user_agent
                    )
                }),
            )
            .layer(axum::middleware::from_fn({
                let crypto = crypto.clone();
                move |req, next| {
                    let crypto = crypto.clone();
                    async move { request_metadata_middleware(crypto, req, next).await }
                }
            }));

        let response = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/echo")
                    .header("host", "example.com:8080")
                    .header("user-agent", "regression-test/1.0")
                    .header("x-forwarded-proto", "https")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        let body_str = std::str::from_utf8(&body).unwrap();
        assert!(body_str.contains("host=example.com"));
        assert!(body_str.contains("scheme=https"));
        assert!(body_str.contains("ua=regression-test/1.0"));
    }

    #[tokio::test]
    async fn handler_without_middleware_fails_with_missing_extension() {
        // Regression guard: without the middleware, an
        // Extension<RequestMetadata> extractor on a route returns 500. This
        // pins the failure mode so we can rely on the middleware tests
        // above to catch the wiring break.
        let app = Router::new().route(
            "/echo",
            get(|Extension(_meta): Extension<RequestMetadata>| async move { "ok" }),
        );

        let response = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/echo")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}
