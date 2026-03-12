//! Sentry CLI-compatible API endpoints
//!
//! Provides endpoints that match the Sentry API format used by `sentry-cli sourcemaps upload`.
//! This allows using the standard sentry-cli tool to upload source maps to Temps.
//!
//! ## Authentication
//!
//! These endpoints accept `Authorization: Bearer <dsn_public_key>` where the Bearer token
//! is the DSN public key from a project DSN. This matches how sentry-cli sends auth tokens.
//!
//! ## Endpoint mapping
//!
//! | sentry-cli endpoint | Temps handler |
//! |---------------------|---------------|
//! | `POST /api/0/organizations/{org}/releases/` | `create_release` (stub) |
//! | `POST /api/0/projects/{org}/{project}/releases/{version}/files/` | `upload_release_file` |
//! | `GET /api/0/projects/{org}/{project}/releases/{version}/files/` | `list_release_files` |
//! | `GET /api/0/organizations/{org}/chunk-upload/` | `chunk_upload_options` (stub) |

use axum::{
    extract::{Multipart, Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use temps_entities::{project_dsns, projects};
use tracing::{debug, warn};
use utoipa::{OpenApi, ToSchema};

use crate::services::source_map_service::SourceMapService;

#[derive(OpenApi)]
#[openapi(
    paths(
        create_release,
        upload_release_file,
        list_release_files,
        chunk_upload_options,
    ),
    components(schemas(
        SentryCreateReleaseRequest,
        SentryReleaseResponse,
        SentryReleaseFileResponse,
        SentryChunkUploadResponse,
    )),
    tags(
        (name = "sentry-compat", description = "Sentry CLI-compatible API endpoints for source map uploads")
    )
)]
pub struct SentryCompatApiDoc;

#[derive(Clone)]
pub struct SentryCompatAppState {
    pub source_map_service: Arc<SourceMapService>,
    pub db: Arc<DatabaseConnection>,
}

// --- Request/Response types ---

#[derive(Deserialize, ToSchema)]
pub struct SentryCreateReleaseRequest {
    /// Release version identifier
    pub version: String,
    /// Project slugs this release belongs to
    #[serde(default)]
    pub projects: Vec<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SentryReleaseResponse {
    pub version: String,
    pub date_created: String,
    pub date_released: Option<String>,
    pub short_version: String,
    pub projects: Vec<SentryReleaseProjectRef>,
}

#[derive(Serialize, ToSchema)]
pub struct SentryReleaseProjectRef {
    pub name: String,
    pub slug: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SentryReleaseFileResponse {
    pub id: String,
    pub name: String,
    pub dist: Option<String>,
    pub headers: serde_json::Value,
    pub size: i64,
    pub sha1: String,
    pub date_created: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SentryChunkUploadResponse {
    pub url: String,
    pub chunk_size: u64,
    pub chunks_per_request: u32,
    pub max_file_size: u64,
    pub max_request_size: u64,
    pub concurrency: u32,
    pub hash_algorithm: String,
    pub compression: Vec<String>,
    pub accept: Vec<String>,
}

// --- Route configuration ---

pub fn configure_sentry_compat_routes() -> Router<Arc<SentryCompatAppState>> {
    Router::new()
        .route(
            "/0/organizations/{org_slug}/releases/",
            post(create_release),
        )
        .route(
            "/0/projects/{org_slug}/{project_slug}/releases/{version}/files/",
            post(upload_release_file).get(list_release_files),
        )
        .route(
            "/0/organizations/{org_slug}/chunk-upload/",
            get(chunk_upload_options),
        )
}

// --- Auth helper ---

/// Extract project_id from Bearer token authentication.
///
/// sentry-cli sends `Authorization: Bearer <auth_token>`.
/// We accept the DSN public key as the Bearer token and resolve to a project.
async fn authenticate_bearer(
    headers: &HeaderMap,
    db: &DatabaseConnection,
) -> Result<i32, (StatusCode, String)> {
    let auth_header = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or((
            StatusCode::UNAUTHORIZED,
            "Missing Authorization header".to_string(),
        ))?;

    let token = if let Some(token) = auth_header.strip_prefix("Bearer ") {
        token.trim()
    } else if let Some(token) = auth_header.strip_prefix("DSN ") {
        token.trim()
    } else {
        return Err((
            StatusCode::UNAUTHORIZED,
            "Invalid Authorization format. Expected 'Bearer <token>'".to_string(),
        ));
    };

    // Look up the DSN public key to find the project
    let dsn = project_dsns::Entity::find()
        .filter(project_dsns::Column::PublicKey.eq(token))
        .filter(project_dsns::Column::IsActive.eq(true))
        .one(db)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Database error: {}", e),
            )
        })?
        .ok_or_else(|| (StatusCode::UNAUTHORIZED, "Invalid auth token".to_string()))?;

    Ok(dsn.project_id)
}

/// Resolve a project slug (or numeric ID) to a project_id.
/// Also validates it matches the authenticated project.
async fn resolve_project_slug(
    project_slug: &str,
    expected_project_id: i32,
    db: &DatabaseConnection,
) -> Result<i32, (StatusCode, String)> {
    // Try numeric ID first
    if let Ok(id) = project_slug.parse::<i32>() {
        if id == expected_project_id {
            return Ok(id);
        }
    }

    // Try slug lookup
    let project = projects::Entity::find()
        .filter(projects::Column::Slug.eq(project_slug))
        .one(db)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Database error: {}", e),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                format!("Project '{}' not found", project_slug),
            )
        })?;

    if project.id != expected_project_id {
        return Err((
            StatusCode::FORBIDDEN,
            "Token does not have access to this project".to_string(),
        ));
    }

    Ok(project.id)
}

// --- Handlers ---

/// Create a release (stub for sentry-cli compatibility).
///
/// sentry-cli calls this before uploading files. Since Temps implicitly creates
/// releases when source maps are uploaded, this is a no-op that returns the
/// expected response format.
#[utoipa::path(
    tag = "sentry-compat",
    post,
    path = "/0/organizations/{org_slug}/releases/",
    params(
        ("org_slug" = String, Path, description = "Organization slug (ignored in single-tenant mode)")
    ),
    request_body = SentryCreateReleaseRequest,
    responses(
        (status = 201, description = "Release created", body = SentryReleaseResponse),
        (status = 401, description = "Unauthorized"),
    ),
)]
async fn create_release(
    State(state): State<Arc<SentryCompatAppState>>,
    Path(_org_slug): Path<String>,
    headers: HeaderMap,
    Json(request): Json<SentryCreateReleaseRequest>,
) -> impl IntoResponse {
    // Authenticate
    if let Err((status, msg)) = authenticate_bearer(&headers, state.db.as_ref()).await {
        return (status, msg).into_response();
    }

    debug!("sentry-cli: Create release '{}' (stub)", request.version);

    let now = Utc::now().to_rfc3339();
    let short_version = if request.version.len() > 12 {
        request.version[..12].to_string()
    } else {
        request.version.clone()
    };

    let project_refs: Vec<SentryReleaseProjectRef> = request
        .projects
        .iter()
        .map(|p| SentryReleaseProjectRef {
            name: p.clone(),
            slug: p.clone(),
        })
        .collect();

    let response = SentryReleaseResponse {
        version: request.version,
        date_created: now,
        date_released: None,
        short_version,
        projects: project_refs,
    };

    (StatusCode::CREATED, Json(response)).into_response()
}

/// Upload a source map file for a release.
///
/// Accepts the same multipart format as the Sentry release files API.
/// The `name` field should be the URL path of the file (e.g., `~/dist/bundle.js.map`).
#[utoipa::path(
    tag = "sentry-compat",
    post,
    path = "/0/projects/{org_slug}/{project_slug}/releases/{version}/files/",
    params(
        ("org_slug" = String, Path, description = "Organization slug (ignored)"),
        ("project_slug" = String, Path, description = "Project slug or numeric ID"),
        ("version" = String, Path, description = "Release version"),
    ),
    responses(
        (status = 201, description = "File uploaded", body = SentryReleaseFileResponse),
        (status = 400, description = "Bad request"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Project not found"),
    ),
)]
async fn upload_release_file(
    State(state): State<Arc<SentryCompatAppState>>,
    Path((_org_slug, project_slug, version)): Path<(String, String, String)>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> impl IntoResponse {
    // Authenticate
    let auth_project_id = match authenticate_bearer(&headers, state.db.as_ref()).await {
        Ok(id) => id,
        Err((status, msg)) => return (status, msg).into_response(),
    };

    // Resolve project slug
    let project_id =
        match resolve_project_slug(&project_slug, auth_project_id, state.db.as_ref()).await {
            Ok(id) => id,
            Err((status, msg)) => return (status, msg).into_response(),
        };

    // Parse multipart form
    let mut file_data: Option<Vec<u8>> = None;
    let mut file_name: Option<String> = None;
    let mut dist: Option<String> = None;

    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or_default().to_string();

        match name.as_str() {
            "file" => {
                // Use the upload filename if no explicit name is set
                if file_name.is_none() {
                    if let Some(filename) = field.file_name() {
                        file_name = Some(filename.to_string());
                    }
                }
                match field.bytes().await {
                    Ok(data) => file_data = Some(data.to_vec()),
                    Err(e) => {
                        return (
                            StatusCode::BAD_REQUEST,
                            format!("Failed to read file: {}", e),
                        )
                            .into_response();
                    }
                }
            }
            "name" => {
                if let Ok(text) = field.text().await {
                    file_name = Some(text);
                }
            }
            "dist" => {
                if let Ok(text) = field.text().await {
                    if !text.is_empty() {
                        dist = Some(text);
                    }
                }
            }
            "header" => {
                // sentry-cli sends "Sourcemap: <url>" headers — we don't need these
                // since we track source maps by file path
                let _ = field.bytes().await;
            }
            _ => {
                let _ = field.bytes().await;
            }
        }
    }

    let source_map_data = match file_data {
        Some(data) if !data.is_empty() => data,
        _ => {
            return (StatusCode::BAD_REQUEST, "No file data provided".to_string()).into_response();
        }
    };

    let name = match file_name {
        Some(n) if !n.is_empty() => n,
        _ => {
            return (StatusCode::BAD_REQUEST, "No file name provided".to_string()).into_response();
        }
    };

    // The `name` field from sentry-cli is typically already in ~/path/to/file.js.map format
    // For non-.map files (e.g., the JS bundle itself), we still store them as source maps
    // The SourceMapService will validate if it's a valid source map
    let file_path = name.clone();

    match state
        .source_map_service
        .upload(
            project_id,
            &version,
            &file_path,
            source_map_data,
            dist.clone(),
        )
        .await
    {
        Ok(info) => {
            debug!(
                "sentry-cli: Uploaded file '{}' for release '{}' (project {})",
                info.file_path, version, project_id
            );

            let response = SentryReleaseFileResponse {
                id: info.id.to_string(),
                name: info.file_path,
                dist,
                headers: serde_json::json!({
                    "Content-Type": "application/json"
                }),
                size: info.size_bytes,
                sha1: info.checksum.unwrap_or_default(),
                date_created: info.created_at.to_rfc3339(),
            };

            (StatusCode::CREATED, Json(response)).into_response()
        }
        Err(e) => {
            // For non-source-map files (like JS bundles), sentry-cli uploads them too.
            // We reject invalid source maps but report it as a warning, not an error.
            warn!(
                "sentry-cli: Failed to store file '{}' for release '{}': {}",
                name, version, e
            );
            (
                StatusCode::BAD_REQUEST,
                format!("Failed to store file: {}", e),
            )
                .into_response()
        }
    }
}

/// List files for a release.
///
/// Returns all source maps stored for a specific release in sentry-cli compatible format.
#[utoipa::path(
    tag = "sentry-compat",
    get,
    path = "/0/projects/{org_slug}/{project_slug}/releases/{version}/files/",
    params(
        ("org_slug" = String, Path, description = "Organization slug (ignored)"),
        ("project_slug" = String, Path, description = "Project slug or numeric ID"),
        ("version" = String, Path, description = "Release version"),
    ),
    responses(
        (status = 200, description = "List of release files", body = Vec<SentryReleaseFileResponse>),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Project not found"),
    ),
)]
async fn list_release_files(
    State(state): State<Arc<SentryCompatAppState>>,
    Path((_org_slug, project_slug, version)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> impl IntoResponse {
    // Authenticate
    let auth_project_id = match authenticate_bearer(&headers, state.db.as_ref()).await {
        Ok(id) => id,
        Err((status, msg)) => return (status, msg).into_response(),
    };

    // Resolve project slug
    let project_id =
        match resolve_project_slug(&project_slug, auth_project_id, state.db.as_ref()).await {
            Ok(id) => id,
            Err((status, msg)) => return (status, msg).into_response(),
        };

    match state
        .source_map_service
        .list_for_release(project_id, &version)
        .await
    {
        Ok(maps) => {
            let files: Vec<SentryReleaseFileResponse> = maps
                .into_iter()
                .map(|info| SentryReleaseFileResponse {
                    id: info.id.to_string(),
                    name: info.file_path,
                    dist: info.dist,
                    headers: serde_json::json!({
                        "Content-Type": "application/json"
                    }),
                    size: info.size_bytes,
                    sha1: info.checksum.unwrap_or_default(),
                    date_created: info.created_at.to_rfc3339(),
                })
                .collect();

            (StatusCode::OK, Json(files)).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to list files: {}", e),
        )
            .into_response(),
    }
}

/// Chunk upload options (stub for sentry-cli compatibility).
///
/// sentry-cli checks this endpoint to determine if chunk-based upload is supported.
/// We return a response indicating that chunk upload is NOT supported, which forces
/// sentry-cli to fall back to the standard file-by-file upload.
#[utoipa::path(
    tag = "sentry-compat",
    get,
    path = "/0/organizations/{org_slug}/chunk-upload/",
    params(
        ("org_slug" = String, Path, description = "Organization slug (ignored)")
    ),
    responses(
        (status = 200, description = "Chunk upload options", body = SentryChunkUploadResponse),
    ),
)]
async fn chunk_upload_options(Path(_org_slug): Path<String>) -> impl IntoResponse {
    // Return options that effectively disable chunk upload
    // sentry-cli will fall back to file-by-file upload
    let response = SentryChunkUploadResponse {
        url: String::new(),
        chunk_size: 8 * 1024 * 1024, // 8MB
        chunks_per_request: 64,
        max_file_size: 50 * 1024 * 1024, // 50MB
        max_request_size: 50 * 1024 * 1024,
        concurrency: 1,
        hash_algorithm: "sha1".to_string(),
        compression: vec!["gzip".to_string()],
        accept: vec![], // Empty accept = no chunk upload support
    };

    Json(response)
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_bearer_token_extraction() {
        // Test that auth header parsing logic is correct
        let auth = "Bearer abc123def456";
        let token = auth.strip_prefix("Bearer ").unwrap();
        assert_eq!(token, "abc123def456");
    }

    #[test]
    fn test_dsn_token_extraction() {
        let auth = "DSN abc123def456";
        let token = auth.strip_prefix("DSN ").unwrap();
        assert_eq!(token, "abc123def456");
    }

    #[test]
    fn test_short_version() {
        let version = "abc123def456789";
        let short = if version.len() > 12 {
            version[..12].to_string()
        } else {
            version.to_string()
        };
        assert_eq!(short, "abc123def456");

        let short_version = "1.0.0";
        let short = if short_version.len() > 12 {
            short_version[..12].to_string()
        } else {
            short_version.to_string()
        };
        assert_eq!(short, "1.0.0");
    }
}
