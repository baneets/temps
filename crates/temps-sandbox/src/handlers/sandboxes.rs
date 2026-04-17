//! Standalone sandbox API.
//!
//! All routes live under `/v1/sandbox/*`. The surface intentionally
//! mirrors the `@vercel/sandbox` npm SDK so drop-in clients work without
//! code changes beyond base URL + auth (see `tests/vercel_compat.rs` for
//! the pinned contract).
//!
//! Shape contract:
//! - Every handler calls through `SandboxService` — no DB access here.
//! - Errors map through `From<SandboxError> for Problem` (RFC 7807).
//! - IDs in URLs are opaque `sbx_<hex>` strings; the i32 is internal.

use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse,
    },
    routing::{get, post, put},
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use futures::stream::{self, Stream, StreamExt};
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use std::time::Duration;
use temps_agents::sandbox::ExecStream;
use tokio_stream::wrappers::BroadcastStream;
use utoipa::ToSchema;

use temps_auth::{permissions::Permission, RequireAuth};
use temps_core::problemdetails::{self, Problem};

use crate::error::SandboxError;
use crate::handlers::SandboxAppState;

/// Reject the request unless the caller carries a sandbox-specific scope
/// *or* the legacy project-scoped permission.
///
/// Two-tier check lets existing tokens (issued before sandbox scopes
/// existed, which only have `projects:read` / `projects:write`) keep
/// working while new sandbox-only API tokens don't need broader project
/// access just to manage sandboxes. The fallback is the quiet-deprecation
/// path: operators can migrate tokens without breaking integrations.
fn sandbox_permission_guard(
    auth: &temps_auth::context::AuthContext,
    primary: Permission,
    fallback: Permission,
) -> Result<(), Problem> {
    if auth.has_permission(&primary) || auth.has_permission(&fallback) {
        return Ok(());
    }
    Err(
        temps_core::error_builder::ErrorBuilder::new(axum::http::StatusCode::FORBIDDEN)
            .type_("https://temps.sh/probs/insufficient-permissions")
            .title("Insufficient Permissions")
            .detail(format!(
                "This operation requires the {} permission",
                primary
            ))
            .value("required_permission", primary.to_string())
            .value("accepted_fallback", fallback.to_string())
            .value("user_role", auth.effective_role.to_string())
            .build(),
    )
}
use crate::services::exec::{ExecOptions, ExecResult};
use crate::services::fs::StatInfo;
use crate::services::job_tracker::{JobState, JobStatus};
use crate::services::sandbox_service::{CreateSandboxRequest, SandboxSource, SandboxSummary};

// ── Error → Problem conversion ──────────────────────────────────────────────

impl From<SandboxError> for Problem {
    fn from(error: SandboxError) -> Self {
        match error {
            SandboxError::NotFound { .. } => problemdetails::new(StatusCode::NOT_FOUND)
                .with_title("Sandbox Not Found")
                .with_detail(error.to_string()),
            SandboxError::JobNotFound { .. } => problemdetails::new(StatusCode::NOT_FOUND)
                .with_title("Job Not Found")
                .with_detail(error.to_string()),
            SandboxError::CreateFailed { .. } => {
                problemdetails::new(StatusCode::INTERNAL_SERVER_ERROR)
                    .with_title("Sandbox Creation Failed")
                    .with_detail(error.to_string())
            }
            SandboxError::ExecFailed { .. } => {
                problemdetails::new(StatusCode::INTERNAL_SERVER_ERROR)
                    .with_title("Sandbox Exec Failed")
                    .with_detail(error.to_string())
            }
            SandboxError::FileOp { .. } => problemdetails::new(StatusCode::INTERNAL_SERVER_ERROR)
                .with_title("Sandbox FS Operation Failed")
                .with_detail(error.to_string()),
            SandboxError::Validation { .. } => problemdetails::new(StatusCode::BAD_REQUEST)
                .with_title("Validation Error")
                .with_detail(error.to_string()),
            SandboxError::InvalidState { .. } => problemdetails::new(StatusCode::CONFLICT)
                .with_title("Invalid Sandbox State")
                .with_detail(error.to_string()),
            SandboxError::Timeout { .. } => problemdetails::new(StatusCode::GATEWAY_TIMEOUT)
                .with_title("Sandbox Operation Timed Out")
                .with_detail(error.to_string()),
            SandboxError::Unavailable { .. } => {
                problemdetails::new(StatusCode::SERVICE_UNAVAILABLE)
                    .with_title("Sandbox Subsystem Unavailable")
                    .with_detail(error.to_string())
            }
            SandboxError::PasswordHashFailed { .. } => {
                problemdetails::new(StatusCode::INTERNAL_SERVER_ERROR)
                    .with_title("Password Hashing Failed")
                    .with_detail(error.to_string())
            }
            SandboxError::Database(_) => problemdetails::new(StatusCode::INTERNAL_SERVER_ERROR)
                .with_title("Internal Server Error")
                .with_detail(error.to_string()),
            SandboxError::Io(_) => problemdetails::new(StatusCode::INTERNAL_SERVER_ERROR)
                .with_title("Internal Server Error")
                .with_detail(error.to_string()),
        }
    }
}

// ── DTOs ────────────────────────────────────────────────────────────────────

/// Initial content to seed into the sandbox work dir. Mirrors the
/// `@vercel/sandbox` `source` option. `type` is one of:
/// - `git` — clone `url`; optionally check out `revision`
/// - `tarball` — download `url` (must be tar or tar.gz) and extract
///
/// For private git repos, pass credentials one of two ways:
/// 1. **Inline (SDK-compatible):** `username` + `password`. GitHub
///    tokens use `username: "x-access-token"`.
/// 2. **Stored connection (temps-native):** `git_connection_id`
///    references a row in the caller's git provider connections. Temps
///    resolves the token server-side and injects it safely.
///
/// `git_connection_id` is mutually exclusive with `username`/`password`.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub enum SourceBody {
    Git {
        url: String,
        #[serde(default)]
        revision: Option<String>,
        #[serde(default)]
        depth: Option<u32>,
        #[serde(default)]
        username: Option<String>,
        #[serde(default)]
        password: Option<String>,
        #[serde(default)]
        git_connection_id: Option<i32>,
    },
    Tarball {
        url: String,
    },
}

/// Reject credentials baked into the URL (`https://user:pass@host/...`).
/// We want credentials to flow through the `username`/`password` or
/// `git_connection_id` channels so the token goes through the safe
/// `GIT_ASKPASS` path and never lands in `.git/config` or logs.
fn url_contains_credentials(url: &str) -> bool {
    // Look for `://<something>@` where `<something>` contains `:` — that's
    // the `user:password` form. A plain `@` without `:` is fine (user-only,
    // which git treats as "prompt for password").
    if let Some(scheme_end) = url.find("://") {
        let rest = &url[scheme_end + 3..];
        if let Some(at_idx) = rest.find('@') {
            let userinfo = &rest[..at_idx];
            if userinfo.contains(':') {
                return true;
            }
        }
    }
    false
}

/// Reject URLs that point at private/loopback/metadata IPs or use
/// non-HTTP schemes. Without this, an authenticated user can drive the
/// sandbox to fetch from internal Docker network services (control
/// plane API, neighbor sandboxes, cloud metadata), turning sandbox
/// seeding into an SSRF primitive.
fn validate_seed_url(url: &str, kind: &str) -> Result<(), SandboxError> {
    use temps_core::url_validation::{validate_external_url, UrlValidationError};
    validate_external_url(url).map_err(|e| {
        let detail = match e {
            UrlValidationError::InvalidScheme => "scheme must be http or https".to_string(),
            UrlValidationError::InvalidFormat(m) => format!("invalid url: {m}"),
            UrlValidationError::PrivateIp
            | UrlValidationError::LoopbackIp
            | UrlValidationError::LinkLocalIp
            | UrlValidationError::CloudMetadata
            | UrlValidationError::MulticastIp
            | UrlValidationError::BroadcastIp
            | UrlValidationError::DocumentationIp
            | UrlValidationError::UnspecifiedIp
            | UrlValidationError::DomainResolvesToBlockedIp => {
                "host points to a private, loopback, or metadata address".to_string()
            }
            UrlValidationError::DnsResolutionFailed(m) => format!("dns: {m}"),
        };
        SandboxError::Validation {
            message: format!("{kind} source: {detail}"),
        }
    })?;
    Ok(())
}

impl SourceBody {
    /// Validate the body before converting it into the service-layer
    /// `SandboxSource`. Called by handlers that accept a SourceBody.
    pub fn validate(&self) -> Result<(), SandboxError> {
        match self {
            SourceBody::Git {
                url,
                username,
                password,
                git_connection_id,
                ..
            } => {
                if url.trim().is_empty() {
                    return Err(SandboxError::Validation {
                        message: "git source: url must not be empty".into(),
                    });
                }
                if url_contains_credentials(url) {
                    return Err(SandboxError::Validation {
                        message: "git source: url must not contain embedded credentials — use username/password or git_connection_id".into(),
                    });
                }
                validate_seed_url(url, "git")?;
                let inline = username.is_some() || password.is_some();
                if inline && git_connection_id.is_some() {
                    return Err(SandboxError::Validation {
                        message: "git source: username/password and git_connection_id are mutually exclusive".into(),
                    });
                }
                if username.is_some() != password.is_some() {
                    return Err(SandboxError::Validation {
                        message: "git source: username and password must be provided together"
                            .into(),
                    });
                }
                Ok(())
            }
            SourceBody::Tarball { url } => {
                if url.trim().is_empty() {
                    return Err(SandboxError::Validation {
                        message: "tarball source: url must not be empty".into(),
                    });
                }
                validate_seed_url(url, "tarball")?;
                Ok(())
            }
        }
    }
}

impl From<SourceBody> for SandboxSource {
    fn from(s: SourceBody) -> Self {
        match s {
            SourceBody::Git {
                url,
                revision,
                depth,
                username,
                password,
                git_connection_id,
            } => SandboxSource::Git {
                url,
                revision,
                depth,
                username,
                password,
                git_connection_id,
            },
            SourceBody::Tarball { url } => SandboxSource::Tarball { url },
        }
    }
}

#[derive(Debug, Deserialize, ToSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct CreateSandboxBody {
    /// Docker image override. `null` uses the platform default.
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    /// Idle timeout in seconds. Clamped to `[60, 86400]`.
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    /// Extra env vars baked into the container on create.
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub cpu_limit: Option<f64>,
    #[serde(default)]
    pub memory_limit_mb: Option<u64>,
    #[serde(default)]
    pub pids_limit: Option<i64>,
    /// Optional initial content to seed into the work dir. Clones a
    /// repo or extracts a tarball after the sandbox is created.
    #[serde(default)]
    pub source: Option<SourceBody>,
    /// Optional preview-URL password. When set, every preview URL served
    /// for this sandbox is gated behind a login form. 8–256 characters.
    /// Omit to leave preview URLs open (the sandbox ID remains the only
    /// gate). The plaintext is never returned; only the last-4 hint is
    /// surfaced in `SandboxResponse.preview_password_hint`.
    #[serde(default)]
    pub preview_password: Option<String>,
}

impl From<CreateSandboxBody> for CreateSandboxRequest {
    fn from(b: CreateSandboxBody) -> Self {
        Self {
            image: b.image,
            name: b.name,
            timeout_secs: b.timeout_secs,
            env: b.env,
            cpu_limit: b.cpu_limit,
            memory_limit_mb: b.memory_limit_mb,
            pids_limit: b.pids_limit,
            source: b.source.map(SandboxSource::from),
            preview_password: b.preview_password,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SandboxResponse {
    pub id: String,
    pub name: String,
    pub status: String,
    pub image: Option<String>,
    pub work_dir: String,
    pub created_at: String,
    pub expires_at: String,
    /// Public URL template with a `{port}` placeholder. Clients substitute
    /// the dev-server port to construct the preview URL for any port the
    /// sandbox exposes (matches the shape of `sandbox.domain(port)` from
    /// `@vercel/sandbox`).
    ///
    /// Empty string when preview URLs aren't configured for this install
    /// (e.g. local dev without a `preview_domain` setting).
    pub preview_url_template: String,

    /// Last 4 chars of the preview password when one is configured.
    /// Absent means "no password set — preview URL relies on the
    /// unguessable 16-hex public_id". Never contains the full plaintext.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview_password_hint: Option<String>,
}

impl SandboxResponse {
    /// Build a response from an entity row, attaching the preview URL
    /// template for the given public_id. Keeping URL construction out of
    /// `From` impls is intentional — the template depends on platform
    /// settings which only the service layer has access to.
    pub fn with_template(s: SandboxSummary, template: String) -> Self {
        Self {
            id: s.public_id,
            name: s.name,
            status: s.status,
            image: s.image,
            work_dir: s.work_dir,
            created_at: s.created_at.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            expires_at: s.expires_at.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            preview_url_template: template,
            preview_password_hint: s.preview_password_hint,
        }
    }
}

impl From<SandboxSummary> for SandboxResponse {
    fn from(s: SandboxSummary) -> Self {
        // Fallback path for call sites that haven't been wired to the
        // preview-URL template yet (tests, intermediate conversions).
        Self::with_template(s, String::new())
    }
}

impl From<temps_entities::sandboxes::Model> for SandboxResponse {
    fn from(m: temps_entities::sandboxes::Model) -> Self {
        SandboxResponse::from(SandboxSummary::from(&m))
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ListSandboxesResponse {
    pub items: Vec<SandboxResponse>,
    pub total: u64,
    pub page: u64,
    pub page_size: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaginationParams {
    pub page: Option<u64>,
    pub page_size: Option<u64>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ExtendTimeoutBody {
    /// Extra seconds to add to the existing `expires_at`.
    pub extra_secs: u64,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecBody {
    pub cmd: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub cwd: Option<String>,
}

impl From<ExecBody> for ExecOptions {
    fn from(b: ExecBody) -> Self {
        Self {
            cmd: b.cmd,
            env: b.env,
            cwd: b.cwd,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ExecResponse {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl From<ExecResult> for ExecResponse {
    fn from(r: ExecResult) -> Self {
        Self {
            exit_code: r.exit_code,
            stdout: r.stdout,
            stderr: r.stderr,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ExecDetachedResponse {
    pub job_id: String,
}

/// Snapshot of a background job. `status` is one of "running" | "exited"
/// | "failed"; `exit_code` is populated only when `status == "exited"`.
#[derive(Debug, Serialize, ToSchema)]
pub struct JobStatusResponse {
    pub status: String,
    pub exit_code: Option<i32>,
    pub reason: Option<String>,
    pub stdout: String,
    pub stderr: String,
}

impl From<JobState> for JobStatusResponse {
    fn from(s: JobState) -> Self {
        let (status, exit_code, reason) = match s.status {
            JobStatus::Running => ("running".to_string(), None, None),
            JobStatus::Exited { exit_code } => ("exited".to_string(), Some(exit_code), None),
            JobStatus::Failed { reason } => ("failed".to_string(), None, Some(reason)),
        };
        Self {
            status,
            exit_code,
            reason,
            stdout: s.stdout,
            stderr: s.stderr,
        }
    }
}

/// Row in the jobs list. Omits stdout/stderr so a noisy dev server doesn't
/// bloat the list payload — callers drill into `GET /jobs/{id}` for the
/// full buffer.
#[derive(Debug, Serialize, ToSchema)]
pub struct JobSummaryResponse {
    pub id: String,
    pub status: String,
    pub exit_code: Option<i32>,
    pub reason: Option<String>,
    pub cmd: String,
    pub started_at: String,
}

impl From<crate::services::job_tracker::JobSummary> for JobSummaryResponse {
    fn from(s: crate::services::job_tracker::JobSummary) -> Self {
        let (status, exit_code, reason) = match s.status {
            JobStatus::Running => ("running".to_string(), None, None),
            JobStatus::Exited { exit_code } => ("exited".to_string(), Some(exit_code), None),
            JobStatus::Failed { reason } => ("failed".to_string(), None, Some(reason)),
        };
        Self {
            id: s.id,
            status,
            exit_code,
            reason,
            cmd: s.cmd,
            started_at: s.started_at.to_rfc3339(),
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ListJobsResponse {
    pub items: Vec<JobSummaryResponse>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct WriteFileBody {
    /// Absolute path inside the sandbox. Must start with `/`.
    pub path: String,
    /// File contents, base64-encoded. Required — lets callers ship binary
    /// data over JSON without charset games.
    pub contents_b64: String,
    /// Unix permission mask (e.g. 0o644). Defaults to 0o644 when absent.
    #[serde(default)]
    pub mode: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadFileQuery {
    pub path: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ReadFileResponse {
    pub path: String,
    /// File contents, base64-encoded. Symmetric with `WriteFileBody`.
    pub contents_b64: String,
    pub size: u64,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct MkdirBody {
    pub path: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatQuery {
    pub path: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct StatResponse {
    pub path: String,
    pub exists: bool,
    pub is_dir: bool,
    pub is_file: bool,
    pub size: u64,
}

impl From<StatInfo> for StatResponse {
    fn from(s: StatInfo) -> Self {
        Self {
            path: s.path,
            exists: s.exists,
            is_dir: s.is_dir,
            is_file: s.is_file,
            size: s.size,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DomainQuery {
    pub port: u16,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DomainResponse {
    pub url: String,
}

/// Build a `SandboxResponse` with the preview URL template attached.
/// Shared by every handler that returns a single sandbox so the template
/// wiring lives in one place.
async fn build_response(
    state: &Arc<SandboxAppState>,
    row: temps_entities::sandboxes::Model,
) -> SandboxResponse {
    let parts = state.sandbox_service.preview_parts().await;
    let template = parts.host_template(&row.public_id);
    SandboxResponse::with_template(SandboxSummary::from(&row), template)
}

/// Batched variant of `build_response` for list endpoints that return
/// `SandboxSummary` already — computes preview parts once per page
/// instead of per-row.
async fn build_summary_responses(
    state: &Arc<SandboxAppState>,
    items: Vec<SandboxSummary>,
) -> Vec<SandboxResponse> {
    let parts = state.sandbox_service.preview_parts().await;
    items
        .into_iter()
        .map(|s| {
            let template = parts.host_template(&s.public_id);
            SandboxResponse::with_template(s, template)
        })
        .collect()
}

// ── Handlers ────────────────────────────────────────────────────────────────

#[utoipa::path(
    tag = "Sandboxes",
    post,
    path = "/v1/sandbox",
    request_body = CreateSandboxBody,
    responses(
        (status = 201, description = "Sandbox created", body = SandboxResponse),
        (status = 400, description = "Validation error"),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Internal server error")
    ),
    security(("bearer_auth" = []))
)]
pub async fn create_sandbox(
    RequireAuth(auth): RequireAuth,
    State(state): State<Arc<SandboxAppState>>,
    Json(body): Json<CreateSandboxBody>,
) -> Result<impl IntoResponse, Problem> {
    sandbox_permission_guard(&auth, Permission::SandboxesWrite, Permission::ProjectsWrite)?;
    if let Some(src) = body.source.as_ref() {
        src.validate()?;
    }
    let row = state
        .sandbox_service
        .create_sandbox(auth.user_id(), body.into())
        .await?;
    let resp = build_response(&state, row).await;
    Ok((StatusCode::CREATED, Json(resp)))
}

#[utoipa::path(
    tag = "Sandboxes",
    get,
    path = "/v1/sandbox",
    params(("page" = Option<u64>, Query, description = "Page (1-indexed)"),
           ("page_size" = Option<u64>, Query, description = "Items per page (default 20, max 100)")),
    responses((status = 200, description = "List sandboxes", body = ListSandboxesResponse)),
    security(("bearer_auth" = []))
)]
pub async fn list_sandboxes(
    RequireAuth(auth): RequireAuth,
    State(state): State<Arc<SandboxAppState>>,
    Query(q): Query<PaginationParams>,
) -> Result<impl IntoResponse, Problem> {
    sandbox_permission_guard(&auth, Permission::SandboxesRead, Permission::ProjectsRead)?;
    let page = q.page.unwrap_or(1);
    let page_size = q.page_size.unwrap_or(20);
    let (items, total) = state
        .sandbox_service
        .list_for_user(auth.user_id(), Some(page), Some(page_size))
        .await?;
    let items = build_summary_responses(&state, items).await;
    let resp = ListSandboxesResponse {
        items,
        total,
        page,
        page_size,
    };
    Ok(Json(resp))
}

#[utoipa::path(
    tag = "Sandboxes",
    get,
    path = "/v1/sandbox/{id}",
    responses(
        (status = 200, description = "Sandbox details", body = SandboxResponse),
        (status = 404, description = "Not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_sandbox(
    RequireAuth(auth): RequireAuth,
    State(state): State<Arc<SandboxAppState>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, Problem> {
    sandbox_permission_guard(&auth, Permission::SandboxesRead, Permission::ProjectsRead)?;
    let row = state
        .sandbox_service
        .find_by_public_id(&id, auth.user_id())
        .await?;
    Ok(Json(build_response(&state, row).await))
}

#[utoipa::path(
    tag = "Sandboxes",
    post,
    path = "/v1/sandbox/{id}/stop",
    responses(
        (status = 204, description = "Sandbox stopped and destroyed"),
        (status = 404, description = "Not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn stop_sandbox(
    RequireAuth(auth): RequireAuth,
    State(state): State<Arc<SandboxAppState>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, Problem> {
    sandbox_permission_guard(&auth, Permission::SandboxesWrite, Permission::ProjectsWrite)?;
    state
        .sandbox_service
        .destroy_sandbox(&id, auth.user_id())
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    tag = "Sandboxes",
    post,
    path = "/v1/sandbox/{id}/destroy",
    responses(
        (status = 204, description = "Sandbox destroyed (alias for `/stop` with an explicit verb)"),
        (status = 404, description = "Not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn destroy_sandbox(
    RequireAuth(auth): RequireAuth,
    State(state): State<Arc<SandboxAppState>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, Problem> {
    sandbox_permission_guard(&auth, Permission::SandboxesWrite, Permission::ProjectsWrite)?;
    state
        .sandbox_service
        .destroy_sandbox(&id, auth.user_id())
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    tag = "Sandboxes",
    post,
    path = "/v1/sandbox/{id}/extend-timeout",
    request_body = ExtendTimeoutBody,
    responses(
        (status = 200, description = "Timeout extended", body = SandboxResponse),
        (status = 400, description = "Validation error"),
        (status = 404, description = "Not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn extend_timeout(
    RequireAuth(auth): RequireAuth,
    State(state): State<Arc<SandboxAppState>>,
    Path(id): Path<String>,
    Json(body): Json<ExtendTimeoutBody>,
) -> Result<impl IntoResponse, Problem> {
    sandbox_permission_guard(&auth, Permission::SandboxesWrite, Permission::ProjectsWrite)?;
    let row = state
        .sandbox_service
        .extend_timeout(&id, auth.user_id(), body.extra_secs)
        .await?;
    Ok(Json(build_response(&state, row).await))
}

#[utoipa::path(
    tag = "Sandboxes",
    post,
    path = "/v1/sandbox/{id}/pause",
    responses(
        (status = 200, description = "Sandbox paused (container stopped, state preserved)", body = SandboxResponse),
        (status = 404, description = "Not found"),
        (status = 409, description = "Sandbox is in an incompatible state (e.g. already destroyed)")
    ),
    security(("bearer_auth" = []))
)]
pub async fn pause_sandbox(
    RequireAuth(auth): RequireAuth,
    State(state): State<Arc<SandboxAppState>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, Problem> {
    sandbox_permission_guard(&auth, Permission::SandboxesWrite, Permission::ProjectsWrite)?;
    let row = state
        .sandbox_service
        .pause_sandbox(&id, auth.user_id())
        .await?;
    Ok(Json(build_response(&state, row).await))
}

#[utoipa::path(
    tag = "Sandboxes",
    post,
    path = "/v1/sandbox/{id}/resume",
    responses(
        (status = 200, description = "Sandbox resumed; expires_at refreshed", body = SandboxResponse),
        (status = 404, description = "Not found"),
        (status = 409, description = "Sandbox is not in a resumable state")
    ),
    security(("bearer_auth" = []))
)]
pub async fn resume_sandbox(
    RequireAuth(auth): RequireAuth,
    State(state): State<Arc<SandboxAppState>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, Problem> {
    sandbox_permission_guard(&auth, Permission::SandboxesWrite, Permission::ProjectsWrite)?;
    let row = state
        .sandbox_service
        .resume_sandbox(&id, auth.user_id())
        .await?;
    Ok(Json(build_response(&state, row).await))
}

#[utoipa::path(
    tag = "Sandboxes",
    post,
    path = "/v1/sandbox/{id}/restart",
    responses(
        (status = 200, description = "Sandbox container restarted in place", body = SandboxResponse),
        (status = 404, description = "Not found"),
        (status = 409, description = "Sandbox is stopped (use /resume) or already destroyed")
    ),
    security(("bearer_auth" = []))
)]
pub async fn restart_sandbox(
    RequireAuth(auth): RequireAuth,
    State(state): State<Arc<SandboxAppState>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, Problem> {
    sandbox_permission_guard(&auth, Permission::SandboxesWrite, Permission::ProjectsWrite)?;
    let row = state
        .sandbox_service
        .restart_sandbox(&id, auth.user_id())
        .await?;
    Ok(Json(build_response(&state, row).await))
}

#[utoipa::path(
    tag = "Sandboxes",
    post,
    path = "/v1/sandbox/{id}/source",
    request_body = SourceBody,
    responses(
        (status = 200, description = "Source content seeded into the sandbox work dir", body = SandboxResponse),
        (status = 400, description = "Validation error (embedded creds, conflicting fields, etc.)"),
        (status = 404, description = "Sandbox not found"),
        (status = 409, description = "Sandbox is not running"),
        (status = 500, description = "Source seed failed inside sandbox")
    ),
    security(("bearer_auth" = []))
)]
pub async fn source_sandbox(
    RequireAuth(auth): RequireAuth,
    State(state): State<Arc<SandboxAppState>>,
    Path(id): Path<String>,
    Json(body): Json<SourceBody>,
) -> Result<impl IntoResponse, Problem> {
    sandbox_permission_guard(&auth, Permission::SandboxesWrite, Permission::ProjectsWrite)?;
    body.validate()?;
    let source: SandboxSource = body.into();
    let row = state
        .sandbox_service
        .clone_source(&id, auth.user_id(), &source)
        .await?;
    Ok(Json(build_response(&state, row).await))
}

// ── Exec ────────────────────────────────────────────────────────────────────

#[utoipa::path(
    tag = "Sandboxes",
    post,
    path = "/v1/sandbox/{id}/exec",
    request_body = ExecBody,
    responses(
        (status = 200, description = "Command finished (non-zero exit is NOT an error)", body = ExecResponse),
        (status = 404, description = "Sandbox not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn exec(
    RequireAuth(auth): RequireAuth,
    State(state): State<Arc<SandboxAppState>>,
    Path(id): Path<String>,
    Json(body): Json<ExecBody>,
) -> Result<impl IntoResponse, Problem> {
    sandbox_permission_guard(&auth, Permission::SandboxesExec, Permission::ProjectsWrite)?;
    let result = state
        .sandbox_service
        .exec(&id, auth.user_id(), body.into())
        .await?;
    Ok(Json(ExecResponse::from(result)))
}

#[utoipa::path(
    tag = "Sandboxes",
    post,
    path = "/v1/sandbox/{id}/exec-detached",
    request_body = ExecBody,
    responses(
        (status = 202, description = "Command accepted; poll /jobs/{job_id}", body = ExecDetachedResponse),
        (status = 404, description = "Sandbox not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn exec_detached(
    RequireAuth(auth): RequireAuth,
    State(state): State<Arc<SandboxAppState>>,
    Path(id): Path<String>,
    Json(body): Json<ExecBody>,
) -> Result<impl IntoResponse, Problem> {
    sandbox_permission_guard(&auth, Permission::SandboxesExec, Permission::ProjectsWrite)?;
    let job_id = state
        .sandbox_service
        .exec_detached(&id, auth.user_id(), body.into())
        .await?;
    Ok((StatusCode::ACCEPTED, Json(ExecDetachedResponse { job_id })))
}

#[utoipa::path(
    tag = "Sandboxes",
    get,
    path = "/v1/sandbox/{id}/jobs",
    responses(
        (status = 200, description = "Detached jobs for this sandbox", body = ListJobsResponse),
        (status = 404, description = "Sandbox not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_jobs(
    RequireAuth(auth): RequireAuth,
    State(state): State<Arc<SandboxAppState>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, Problem> {
    sandbox_permission_guard(&auth, Permission::SandboxesRead, Permission::ProjectsRead)?;
    let items = state
        .sandbox_service
        .list_jobs(&id, auth.user_id())
        .await?
        .into_iter()
        .map(JobSummaryResponse::from)
        .collect();
    Ok(Json(ListJobsResponse { items }))
}

#[utoipa::path(
    tag = "Sandboxes",
    get,
    path = "/v1/sandbox/{id}/jobs/{job_id}",
    responses(
        (status = 200, description = "Job status snapshot", body = JobStatusResponse),
        (status = 404, description = "Sandbox or job not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn job_status(
    RequireAuth(auth): RequireAuth,
    State(state): State<Arc<SandboxAppState>>,
    Path((id, job_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, Problem> {
    sandbox_permission_guard(&auth, Permission::SandboxesRead, Permission::ProjectsRead)?;
    let state_snapshot = state
        .sandbox_service
        .job_status(&id, auth.user_id(), &job_id)
        .await?;
    Ok(Json(JobStatusResponse::from(state_snapshot)))
}

/// SSE endpoint streaming each stdout/stderr line from a detached job
/// as it's produced. Mirrors the `Command.logs()` async iterator shape
/// on `@vercel/sandbox` — events carry `{ stream, data }`.
///
/// Late subscribers only see events produced after they connect. The
/// JobState snapshot (`GET /jobs/{job_id}`) covers the history.
///
/// A "done" sentinel event fires when the broadcast channel closes
/// (the exec task has exited and dropped the sender), signalling
/// callers they can stop reading.
#[utoipa::path(
    tag = "Sandboxes",
    get,
    path = "/v1/sandbox/{id}/jobs/{job_id}/logs",
    responses(
        (status = 200, description = "SSE stream of log events"),
        (status = 404, description = "Sandbox or job not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn job_logs(
    RequireAuth(auth): RequireAuth,
    State(state): State<Arc<SandboxAppState>>,
    Path((id, job_id)): Path<(String, String)>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, Problem> {
    sandbox_permission_guard(&auth, Permission::SandboxesRead, Permission::ProjectsRead)?;
    let rx = state
        .sandbox_service
        .subscribe_job_logs(&id, auth.user_id(), &job_id)
        .await?;

    // `BroadcastStream` maps `RecvError::Lagged` → an error yielded on the
    // stream; we convert that into a sentinel `lagged` event so the client
    // can reconcile by polling `job_status` for the missed range. `Closed`
    // terminates the stream — we cap it with a `done` event.
    let live = BroadcastStream::new(rx).map(|msg| match msg {
        Ok(ev) => {
            let stream_name = match ev.stream {
                ExecStream::Stdout => "stdout",
                ExecStream::Stderr => "stderr",
            };
            let payload = serde_json::json!({
                "stream": stream_name,
                "data": ev.line,
            });
            Ok::<_, Infallible>(Event::default().event("log").data(payload.to_string()))
        }
        Err(_lagged) => Ok::<_, Infallible>(
            Event::default()
                .event("lagged")
                .data("subscriber fell behind; reconcile via GET /jobs/{job_id}"),
        ),
    });

    let done = stream::once(async { Ok::<_, Infallible>(Event::default().event("done").data("")) });

    let stream = live.chain(done);
    Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))))
}

#[derive(Debug, Deserialize, ToSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct KillJobBody {
    /// When true, sends SIGKILL immediately. Defaults to SIGTERM so the
    /// process gets a chance to flush (mirrors `Command.kill()` in
    /// `@vercel/sandbox`, which also accepts a signal override).
    #[serde(default)]
    pub force: bool,
}

/// Terminate a detached job. Aborts the server-side tracking task and
/// sends SIGTERM (or SIGKILL if `force=true`) to any matching processes
/// inside the sandbox container. Returns 204 on success; 404 if the
/// sandbox or job is unknown.
#[utoipa::path(
    tag = "Sandboxes",
    post,
    path = "/v1/sandbox/{id}/jobs/{job_id}/kill",
    request_body = KillJobBody,
    responses(
        (status = 204, description = "Job killed"),
        (status = 404, description = "Sandbox or job not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn kill_job(
    RequireAuth(auth): RequireAuth,
    State(state): State<Arc<SandboxAppState>>,
    Path((id, job_id)): Path<(String, String)>,
    body: Option<Json<KillJobBody>>,
) -> Result<impl IntoResponse, Problem> {
    sandbox_permission_guard(&auth, Permission::SandboxesExec, Permission::ProjectsWrite)?;
    let Json(body) = body.unwrap_or(Json(KillJobBody::default()));
    state
        .sandbox_service
        .kill_job(&id, auth.user_id(), &job_id, body.force)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

// ── Filesystem ──────────────────────────────────────────────────────────────

#[utoipa::path(
    tag = "Sandboxes",
    get,
    path = "/v1/sandbox/{id}/fs/read",
    params(("path" = String, Query, description = "Absolute file path inside the sandbox")),
    responses(
        (status = 200, description = "File contents (base64)", body = ReadFileResponse),
        (status = 400, description = "Validation error"),
        (status = 404, description = "Sandbox or file not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn read_file(
    RequireAuth(auth): RequireAuth,
    State(state): State<Arc<SandboxAppState>>,
    Path(id): Path<String>,
    Query(q): Query<ReadFileQuery>,
) -> Result<impl IntoResponse, Problem> {
    sandbox_permission_guard(&auth, Permission::SandboxesRead, Permission::ProjectsRead)?;
    let bytes = state
        .sandbox_service
        .fs_read(&id, auth.user_id(), &q.path)
        .await?;
    let size = bytes.len() as u64;
    Ok(Json(ReadFileResponse {
        path: q.path,
        contents_b64: B64.encode(&bytes),
        size,
    }))
}

#[utoipa::path(
    tag = "Sandboxes",
    post,
    path = "/v1/sandbox/{id}/fs/write",
    request_body = WriteFileBody,
    responses(
        (status = 204, description = "File written"),
        (status = 400, description = "Validation error or invalid base64"),
        (status = 404, description = "Sandbox not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn write_file(
    RequireAuth(auth): RequireAuth,
    State(state): State<Arc<SandboxAppState>>,
    Path(id): Path<String>,
    Json(body): Json<WriteFileBody>,
) -> Result<impl IntoResponse, Problem> {
    sandbox_permission_guard(&auth, Permission::SandboxesWrite, Permission::ProjectsWrite)?;
    let contents = B64.decode(body.contents_b64.as_bytes()).map_err(|e| {
        Problem::from(SandboxError::Validation {
            message: format!("contents_b64 is not valid base64: {}", e),
        })
    })?;
    let mode = body.mode.unwrap_or(0o644);
    state
        .sandbox_service
        .fs_write(&id, auth.user_id(), &body.path, &contents, mode)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct WriteFilesBody {
    /// List of files to write. Each entry must include an absolute
    /// `path` and base64-encoded `contents_b64`. Empty list is a no-op.
    pub files: Vec<WriteFileBody>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct WriteFilesResponse {
    /// Number of files successfully written before the first failure
    /// (if any). On full success this equals `files.len()`.
    pub written: usize,
}

/// Batch-write multiple files in a single request. Mirrors
/// `@vercel/sandbox` `writeFiles()`. Semantics are fail-fast: if any
/// file errors, previously-written entries are left in place and the
/// error describes which file broke.
#[utoipa::path(
    tag = "Sandboxes",
    post,
    path = "/v1/sandbox/{id}/fs/write-batch",
    request_body = WriteFilesBody,
    responses(
        (status = 200, description = "All files written", body = WriteFilesResponse),
        (status = 400, description = "Validation error or invalid base64"),
        (status = 404, description = "Sandbox not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn write_files(
    RequireAuth(auth): RequireAuth,
    State(state): State<Arc<SandboxAppState>>,
    Path(id): Path<String>,
    Json(body): Json<WriteFilesBody>,
) -> Result<impl IntoResponse, Problem> {
    sandbox_permission_guard(&auth, Permission::SandboxesWrite, Permission::ProjectsWrite)?;
    let mut entries = Vec::with_capacity(body.files.len());
    for f in body.files {
        let contents = B64.decode(f.contents_b64.as_bytes()).map_err(|e| {
            Problem::from(SandboxError::Validation {
                message: format!("contents_b64 for '{}' is not valid base64: {}", f.path, e),
            })
        })?;
        entries.push(crate::services::fs::BatchWriteEntry {
            path: f.path,
            contents,
            mode: f.mode,
        });
    }
    let written = state
        .sandbox_service
        .fs_write_batch(&id, auth.user_id(), entries)
        .await?;
    Ok(Json(WriteFilesResponse { written }))
}

#[utoipa::path(
    tag = "Sandboxes",
    get,
    path = "/v1/sandbox/{id}/fs/stat",
    params(("path" = String, Query, description = "Absolute path inside the sandbox")),
    responses(
        (status = 200, description = "Stat info (exists=false when missing — not an error)", body = StatResponse),
        (status = 400, description = "Validation error")
    ),
    security(("bearer_auth" = []))
)]
pub async fn stat_path(
    RequireAuth(auth): RequireAuth,
    State(state): State<Arc<SandboxAppState>>,
    Path(id): Path<String>,
    Query(q): Query<StatQuery>,
) -> Result<impl IntoResponse, Problem> {
    sandbox_permission_guard(&auth, Permission::SandboxesRead, Permission::ProjectsRead)?;
    let info = state
        .sandbox_service
        .fs_stat(&id, auth.user_id(), &q.path)
        .await?;
    Ok(Json(StatResponse::from(info)))
}

#[utoipa::path(
    tag = "Sandboxes",
    post,
    path = "/v1/sandbox/{id}/fs/mkdir",
    request_body = MkdirBody,
    responses(
        (status = 204, description = "Directory created (or already existed)"),
        (status = 400, description = "Validation error")
    ),
    security(("bearer_auth" = []))
)]
pub async fn mkdir(
    RequireAuth(auth): RequireAuth,
    State(state): State<Arc<SandboxAppState>>,
    Path(id): Path<String>,
    Json(body): Json<MkdirBody>,
) -> Result<impl IntoResponse, Problem> {
    sandbox_permission_guard(&auth, Permission::SandboxesWrite, Permission::ProjectsWrite)?;
    state
        .sandbox_service
        .fs_mkdir(&id, auth.user_id(), &body.path)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

// ── Preview URL (`domain(port)`) ────────────────────────────────────────────

#[utoipa::path(
    tag = "Sandboxes",
    get,
    path = "/v1/sandbox/{id}/domain",
    params(("port" = u16, Query, description = "Port inside the sandbox (1..=65535)")),
    responses(
        (status = 200, description = "Preview URL for the port", body = DomainResponse),
        (status = 400, description = "Invalid port"),
        (status = 404, description = "Sandbox not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn domain(
    RequireAuth(auth): RequireAuth,
    State(state): State<Arc<SandboxAppState>>,
    Path(id): Path<String>,
    Query(q): Query<DomainQuery>,
) -> Result<impl IntoResponse, Problem> {
    sandbox_permission_guard(&auth, Permission::SandboxesRead, Permission::ProjectsRead)?;
    let url = state
        .sandbox_service
        .domain(&id, auth.user_id(), q.port)
        .await?;
    Ok(Json(DomainResponse { url }))
}

// ── Preview password ────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, ToSchema)]
pub struct SetPreviewPasswordBody {
    /// Plaintext password to protect the sandbox's preview URLs. Hashed
    /// server-side with argon2id — we never persist or echo this back.
    /// Must be between 8 and 256 characters.
    pub password: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SetPreviewPasswordResponse {
    /// Last 4 chars of the password we just stored. Surface in the UI so
    /// users can confirm which password is live without re-entering it.
    pub preview_password_hint: String,
}

#[utoipa::path(
    tag = "Sandboxes",
    put,
    path = "/v1/sandbox/{id}/preview-password",
    request_body = SetPreviewPasswordBody,
    responses(
        (status = 200, description = "Preview password set or rotated", body = SetPreviewPasswordResponse),
        (status = 400, description = "Password too short or too long"),
        (status = 404, description = "Sandbox not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn set_preview_password(
    RequireAuth(auth): RequireAuth,
    State(state): State<Arc<SandboxAppState>>,
    Path(id): Path<String>,
    Json(body): Json<SetPreviewPasswordBody>,
) -> Result<impl IntoResponse, Problem> {
    sandbox_permission_guard(&auth, Permission::SandboxesWrite, Permission::ProjectsWrite)?;
    let hint = state
        .sandbox_service
        .set_preview_password(&id, auth.user_id(), &body.password)
        .await?;
    Ok(Json(SetPreviewPasswordResponse {
        preview_password_hint: hint,
    }))
}

#[utoipa::path(
    tag = "Sandboxes",
    delete,
    path = "/v1/sandbox/{id}/preview-password",
    responses(
        (status = 204, description = "Preview password removed (sandbox is now URL-only protected)"),
        (status = 404, description = "Sandbox not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn clear_preview_password(
    RequireAuth(auth): RequireAuth,
    State(state): State<Arc<SandboxAppState>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, Problem> {
    sandbox_permission_guard(&auth, Permission::SandboxesWrite, Permission::ProjectsWrite)?;
    state
        .sandbox_service
        .clear_preview_password(&id, auth.user_id())
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

// ── Routing ─────────────────────────────────────────────────────────────────

pub fn routes() -> Router<Arc<SandboxAppState>> {
    Router::new()
        .route("/v1/sandbox", post(create_sandbox).get(list_sandboxes))
        .route("/v1/sandbox/{id}", get(get_sandbox))
        .route("/v1/sandbox/{id}/stop", post(stop_sandbox))
        .route("/v1/sandbox/{id}/destroy", post(destroy_sandbox))
        .route("/v1/sandbox/{id}/pause", post(pause_sandbox))
        .route("/v1/sandbox/{id}/resume", post(resume_sandbox))
        .route("/v1/sandbox/{id}/restart", post(restart_sandbox))
        .route("/v1/sandbox/{id}/source", post(source_sandbox))
        .route("/v1/sandbox/{id}/extend-timeout", post(extend_timeout))
        .route("/v1/sandbox/{id}/exec", post(exec))
        .route("/v1/sandbox/{id}/exec-detached", post(exec_detached))
        .route("/v1/sandbox/{id}/jobs", get(list_jobs))
        .route("/v1/sandbox/{id}/jobs/{job_id}", get(job_status))
        .route("/v1/sandbox/{id}/jobs/{job_id}/logs", get(job_logs))
        .route("/v1/sandbox/{id}/jobs/{job_id}/kill", post(kill_job))
        .route("/v1/sandbox/{id}/fs/read", get(read_file))
        .route("/v1/sandbox/{id}/fs/write", post(write_file))
        .route("/v1/sandbox/{id}/fs/write-batch", post(write_files))
        .route("/v1/sandbox/{id}/fs/stat", get(stat_path))
        .route("/v1/sandbox/{id}/fs/mkdir", post(mkdir))
        .route("/v1/sandbox/{id}/domain", get(domain))
        .route(
            "/v1/sandbox/{id}/preview-password",
            put(set_preview_password).delete(clear_preview_password),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn job_status_running_serializes_without_exit_or_reason() {
        let resp = JobStatusResponse::from(JobState {
            status: JobStatus::Running,
            stdout: "x".into(),
            stderr: String::new(),
        });
        assert_eq!(resp.status, "running");
        assert!(resp.exit_code.is_none());
        assert!(resp.reason.is_none());
    }

    #[test]
    fn job_status_exited_carries_code() {
        let resp = JobStatusResponse::from(JobState {
            status: JobStatus::Exited { exit_code: 7 },
            stdout: String::new(),
            stderr: String::new(),
        });
        assert_eq!(resp.status, "exited");
        assert_eq!(resp.exit_code, Some(7));
    }

    #[test]
    fn job_status_failed_carries_reason() {
        let resp = JobStatusResponse::from(JobState {
            status: JobStatus::Failed {
                reason: "provider down".into(),
            },
            stdout: String::new(),
            stderr: String::new(),
        });
        assert_eq!(resp.status, "failed");
        assert_eq!(resp.reason.as_deref(), Some("provider down"));
    }

    #[test]
    fn sandbox_response_converts_summary() {
        let now = Utc::now();
        let summary = SandboxSummary {
            public_id: "sbx_abc".into(),
            name: "name".into(),
            status: "running".into(),
            image: None,
            work_dir: "/workspace".into(),
            created_at: now,
            expires_at: now,
            preview_password_hint: None,
        };
        let r = SandboxResponse::from(summary);
        assert_eq!(r.id, "sbx_abc");
        assert_eq!(r.status, "running");
        assert!(r.created_at.ends_with('Z'));
    }

    #[test]
    fn create_body_converts_to_request() {
        let body = CreateSandboxBody {
            image: Some("node:20".into()),
            timeout_secs: Some(120),
            ..Default::default()
        };
        let req: CreateSandboxRequest = body.into();
        assert_eq!(req.image.as_deref(), Some("node:20"));
        assert_eq!(req.timeout_secs, Some(120));
    }

    // ── DTO stability: unknown fields are rejected ──────────────────────────
    //
    // Every input DTO carries `#[serde(deny_unknown_fields)]` so clients
    // can't silently pass fields that drift from the `@vercel/sandbox`
    // contract. These tests lock that behaviour in.

    fn assert_rejects_unknown<T: for<'de> serde::Deserialize<'de>>(json: &str) {
        match serde_json::from_str::<T>(json) {
            Ok(_) => panic!(
                "expected unknown-field rejection, but deserialization succeeded for json: {}",
                json
            ),
            Err(e) => {
                let msg = e.to_string();
                assert!(
                    msg.contains("unknown field"),
                    "expected unknown-field error, got: {}",
                    msg
                );
            }
        }
    }

    #[test]
    fn create_body_rejects_unknown_field() {
        assert_rejects_unknown::<CreateSandboxBody>(r#"{"bogus":"x"}"#);
    }

    #[test]
    fn exec_body_rejects_unknown_field() {
        assert_rejects_unknown::<ExecBody>(r#"{"cmd":["ls"],"surprise":1}"#);
    }

    #[test]
    fn extend_timeout_body_rejects_unknown_field() {
        assert_rejects_unknown::<ExtendTimeoutBody>(r#"{"extra_secs":60,"other":1}"#);
    }

    #[test]
    fn write_file_body_rejects_unknown_field() {
        assert_rejects_unknown::<WriteFileBody>(
            r#"{"path":"/a","contents_b64":"Zm9v","extra":true}"#,
        );
    }

    #[test]
    fn write_files_body_rejects_unknown_field() {
        assert_rejects_unknown::<WriteFilesBody>(r#"{"files":[],"oops":1}"#);
    }

    #[test]
    fn mkdir_body_rejects_unknown_field() {
        assert_rejects_unknown::<MkdirBody>(r#"{"path":"/x","extra":"y"}"#);
    }

    #[test]
    fn kill_job_body_rejects_unknown_field() {
        assert_rejects_unknown::<KillJobBody>(r#"{"force":true,"more":1}"#);
    }

    #[test]
    fn pagination_params_rejects_unknown_field() {
        assert_rejects_unknown::<PaginationParams>(r#"{"page":1,"unexpected":2}"#);
    }

    #[test]
    fn read_file_query_rejects_unknown_field() {
        assert_rejects_unknown::<ReadFileQuery>(r#"{"path":"/a","extra":"b"}"#);
    }

    #[test]
    fn stat_query_rejects_unknown_field() {
        assert_rejects_unknown::<StatQuery>(r#"{"path":"/a","extra":"b"}"#);
    }

    #[test]
    fn domain_query_rejects_unknown_field() {
        assert_rejects_unknown::<DomainQuery>(r#"{"port":8080,"extra":"b"}"#);
    }

    #[test]
    fn source_body_git_rejects_unknown_field() {
        assert_rejects_unknown::<SourceBody>(r#"{"type":"git","url":"https://x","other":1}"#);
    }

    // ── SourceBody::validate ────────────────────────────────────────────────
    //
    // These keep the credential-safety rules locked in: no embedded creds in
    // the URL, no conflicting auth modes, username/password must pair.

    #[test]
    fn validate_accepts_plain_git_url() {
        let body = SourceBody::Git {
            url: "https://github.com/foo/bar".into(),
            revision: None,
            depth: None,
            username: None,
            password: None,
            git_connection_id: None,
        };
        assert!(body.validate().is_ok());
    }

    #[test]
    fn validate_rejects_url_with_embedded_creds() {
        let body = SourceBody::Git {
            url: "https://user:secret@github.com/foo/bar".into(),
            revision: None,
            depth: None,
            username: None,
            password: None,
            git_connection_id: None,
        };
        let err = body.validate().expect_err("expected validation failure");
        assert!(
            matches!(err, crate::error::SandboxError::Validation { .. }),
            "expected Validation error, got {:?}",
            err
        );
    }

    #[test]
    fn validate_rejects_username_without_password() {
        let body = SourceBody::Git {
            url: "https://github.com/foo/bar".into(),
            revision: None,
            depth: None,
            username: Some("x-access-token".into()),
            password: None,
            git_connection_id: None,
        };
        assert!(body.validate().is_err());
    }

    #[test]
    fn validate_rejects_connection_id_plus_explicit_creds() {
        let body = SourceBody::Git {
            url: "https://github.com/foo/bar".into(),
            revision: None,
            depth: None,
            username: Some("x-access-token".into()),
            password: Some("ghp_xxx".into()),
            git_connection_id: Some(7),
        };
        assert!(body.validate().is_err());
    }

    #[test]
    fn validate_accepts_connection_id_alone() {
        let body = SourceBody::Git {
            url: "https://github.com/foo/bar".into(),
            revision: Some("main".into()),
            depth: Some(1),
            username: None,
            password: None,
            git_connection_id: Some(7),
        };
        assert!(body.validate().is_ok());
    }

    #[test]
    fn validate_accepts_paired_username_password() {
        let body = SourceBody::Git {
            url: "https://github.com/foo/bar".into(),
            revision: None,
            depth: None,
            username: Some("x-access-token".into()),
            password: Some("ghp_xxx".into()),
            git_connection_id: None,
        };
        assert!(body.validate().is_ok());
    }
}
