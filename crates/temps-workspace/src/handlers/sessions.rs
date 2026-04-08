use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Extension, Path, Query, State,
    },
    http::StatusCode,
    response::sse::{Event, KeepAlive, Sse},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use bollard::exec::StartExecResults;
use bytes::Bytes;
use futures::stream::Stream;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use utoipa::ToSchema;

use temps_auth::{permission_guard, RequireAuth};
use temps_core::problemdetails::{self, Problem};
use temps_core::{AuditContext, AuditOperation, RequestMetadata};
use temps_entities::{workspace_messages, workspace_sessions};

use crate::error::WorkspaceError;
use crate::handlers::WorkspaceAppState;
use crate::services::workspace_service::{
    CreateSessionRequest, CreatedSession, SendMessageRequest, UpdateSessionFields,
};

/// Ports the UI should offer as "open this in a preview URL" quick links.
/// We don't know what the user's dev server is actually listening on, so we
/// surface the common ones. The UI also lets users type an arbitrary port.
const COMMON_PREVIEW_PORTS: &[u16] = &[3000, 3001, 4200, 5000, 5173, 8000, 8080, 8081, 8888];

#[derive(Debug, Serialize, ToSchema)]
pub struct PreviewPortUrl {
    pub port: u16,
    pub url: String,
}

/// Preview URL components derived from platform settings. Protocol and
/// (optional) external port come from `external_url`; the host suffix
/// comes from `preview_domain`. This way self-hosted setups running on a
/// non-standard port (e.g. `http://192.168.1.10:8080`) get correct URLs
/// instead of being silently upgraded to `https://...:443`.
#[derive(Clone)]
pub struct PreviewUrlParts {
    pub protocol: String,
    pub domain: String,
    /// `Some(port)` when external_url specifies a non-standard port; `None`
    /// for default 80/443.
    pub port: Option<u16>,
}

impl PreviewUrlParts {
    fn host_for(&self, session_id: i32, port: u16) -> String {
        let host = format!("ws-{}-{}.{}", session_id, port, self.domain);
        match self.port {
            Some(external_port) => format!("{}:{}", host, external_port),
            None => host,
        }
    }

    fn host_template(&self, session_id: i32) -> String {
        let host = format!("ws-{}-{{port}}.{}", session_id, self.domain);
        match self.port {
            Some(external_port) => format!("{}:{}", host, external_port),
            None => host,
        }
    }
}

fn build_preview_urls(session_id: i32, parts: &PreviewUrlParts) -> Vec<PreviewPortUrl> {
    COMMON_PREVIEW_PORTS
        .iter()
        .map(|p| PreviewPortUrl {
            port: *p,
            url: format!("{}://{}", parts.protocol, parts.host_for(session_id, *p)),
        })
        .collect()
}

/// Build the URL *template* — the UI substitutes `{port}` client-side so
/// users can also enter arbitrary ports.
fn build_preview_url_template(session_id: i32, parts: &PreviewUrlParts) -> String {
    format!("{}://{}", parts.protocol, parts.host_template(session_id))
}

// ── Error → Problem conversion ──────────────────────────────────────────────

impl From<WorkspaceError> for Problem {
    fn from(error: WorkspaceError) -> Self {
        match error {
            WorkspaceError::SessionNotFound { .. } => problemdetails::new(StatusCode::NOT_FOUND)
                .with_title("Workspace Session Not Found")
                .with_detail(error.to_string()),
            WorkspaceError::SessionNotActive { .. } => problemdetails::new(StatusCode::CONFLICT)
                .with_title("Session Not Active")
                .with_detail(error.to_string()),
            WorkspaceError::ProjectNotFound { .. } => problemdetails::new(StatusCode::NOT_FOUND)
                .with_title("Project Not Found")
                .with_detail(error.to_string()),
            WorkspaceError::SandboxCreationFailed { .. } => {
                problemdetails::new(StatusCode::INTERNAL_SERVER_ERROR)
                    .with_title("Sandbox Creation Failed")
                    .with_detail(error.to_string())
            }
            WorkspaceError::SandboxNotAvailable { .. } => {
                problemdetails::new(StatusCode::SERVICE_UNAVAILABLE)
                    .with_title("Sandbox Not Available")
                    .with_detail(error.to_string())
            }
            WorkspaceError::AiCliFailed { .. } => {
                problemdetails::new(StatusCode::INTERNAL_SERVER_ERROR)
                    .with_title("AI CLI Failed")
                    .with_detail(error.to_string())
            }
            WorkspaceError::AiCliTimeout { .. } => problemdetails::new(StatusCode::GATEWAY_TIMEOUT)
                .with_title("AI CLI Timeout")
                .with_detail(error.to_string()),
            WorkspaceError::Validation { .. } => problemdetails::new(StatusCode::BAD_REQUEST)
                .with_title("Validation Error")
                .with_detail(error.to_string()),
            WorkspaceError::PasswordHashFailed { .. } => {
                problemdetails::new(StatusCode::INTERNAL_SERVER_ERROR)
                    .with_title("Preview Password Error")
                    .with_detail(error.to_string())
            }
            WorkspaceError::MemoryNotFound { .. } => problemdetails::new(StatusCode::NOT_FOUND)
                .with_title("Memory Fact Not Found")
                .with_detail(error.to_string()),
            WorkspaceError::WorkflowNotFound { .. } => problemdetails::new(StatusCode::NOT_FOUND)
                .with_title("Workflow Not Found")
                .with_detail(error.to_string()),
            WorkspaceError::Database(_) => problemdetails::new(StatusCode::INTERNAL_SERVER_ERROR)
                .with_title("Internal Server Error")
                .with_detail(error.to_string()),
            WorkspaceError::Io(_) => problemdetails::new(StatusCode::INTERNAL_SERVER_ERROR)
                .with_title("Internal Server Error")
                .with_detail(error.to_string()),
            WorkspaceError::Agent(_) => problemdetails::new(StatusCode::INTERNAL_SERVER_ERROR)
                .with_title("Agent Error")
                .with_detail(error.to_string()),
        }
    }
}

// ── Request/Response DTOs ───────────────────────────────────────────────────

#[derive(Debug, Deserialize, ToSchema)]
pub struct StartSessionRequest {
    pub ai_provider: Option<String>,
    /// Branch to check out in the workspace sandbox. Defaults to the project's main branch.
    /// If `base_branch_name` is also set, this is the *new* branch to be created
    /// locally off `base_branch_name`.
    pub branch_name: Option<String>,
    /// Optional: when set, the sandbox clones `base_branch_name` from the
    /// remote and then creates `branch_name` as a new local branch on top of
    /// it. Use this to start a session "off main" without touching the remote.
    pub base_branch_name: Option<String>,
    pub metadata: Option<serde_json::Value>,
}

/// Body for `PATCH /projects/{project_id}/workspace/sessions/{session_id}`.
/// All fields are optional — only provided fields are updated.
#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateSessionBody {
    /// Per-session idle timeout in minutes. Send `null` to clear the
    /// override (fall back to server default). Omit to leave unchanged.
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub idle_timeout_minutes: Option<Option<i32>>,
    /// User-provided title. Send `null` to clear (fall back to "Session #{id}").
    /// Omit to leave unchanged.
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub title: Option<Option<String>>,
    /// CPU limit in vCPU cores (e.g. 2.0). `null` clears the override.
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub cpu_limit: Option<Option<f32>>,
    /// Memory limit in MB. `null` clears the override.
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub memory_limit_mb: Option<Option<i32>>,
    /// PID limit. `null` clears the override.
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub pids_limit: Option<Option<i32>>,
}

// Allow distinguishing "field absent" (leave unchanged) from "field is
// null" (clear the override) in JSON bodies.
fn deserialize_double_option<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SendMessageBody {
    pub content: String,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct PaginationParams {
    pub page: Option<u64>,
    pub page_size: Option<u64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SessionResponse {
    pub id: i32,
    pub project_id: i32,
    pub user_id: i32,
    pub status: String,
    pub ai_provider: String,
    pub ai_model: Option<String>,
    pub tokens_input: i32,
    pub tokens_output: i32,
    pub estimated_cost_cents: i32,
    pub files_changed: i32,
    pub branch_name: Option<String>,
    pub base_branch_name: Option<String>,
    pub started_at: String,
    pub last_activity_at: String,
    pub closed_at: Option<String>,
    pub sandbox_container_id: Option<String>,
    /// Last 4 chars of the current preview password (for UI disambiguation).
    /// Never contains the full password.
    pub preview_password_hint: Option<String>,
    /// Plaintext preview password — populated ONLY in the response to
    /// `POST /sessions` (creation) and `POST /sessions/:id/preview-password/regenerate`.
    /// Always `None` on list / get / update responses.
    pub preview_password: Option<String>,
    /// Pre-built URLs for common dev-server ports.
    pub preview_urls: Vec<PreviewPortUrl>,
    /// URL template with `{port}` placeholder, so the UI can substitute
    /// arbitrary ports.
    pub preview_url_template: String,
    /// Per-session idle timeout override in minutes. `None` means the
    /// server-wide default applies (currently 60).
    pub idle_timeout_minutes: Option<i32>,
    /// User-provided session title. `None` → UI falls back to `Session #{id}`.
    pub title: Option<String>,
    /// CPU limit in vCPU cores. `None` → server default applies.
    pub cpu_limit: Option<f32>,
    /// Memory limit in MB. `None` → server default applies.
    pub memory_limit_mb: Option<i32>,
    /// PID limit. `None` → server default applies.
    pub pids_limit: Option<i32>,
}

impl SessionResponse {
    pub fn from_model(s: workspace_sessions::Model, parts: &PreviewUrlParts) -> Self {
        let id = s.id;
        Self {
            id,
            project_id: s.project_id,
            user_id: s.user_id,
            status: s.status,
            ai_provider: s.ai_provider,
            ai_model: s.ai_model,
            tokens_input: s.tokens_input,
            tokens_output: s.tokens_output,
            estimated_cost_cents: s.estimated_cost_cents,
            files_changed: s.files_changed,
            branch_name: s.branch_name,
            base_branch_name: s.base_branch_name,
            started_at: s.started_at.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            last_activity_at: s.last_activity_at.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            closed_at: s
                .closed_at
                .map(|t| t.format("%Y-%m-%dT%H:%M:%SZ").to_string()),
            sandbox_container_id: s.sandbox_container_id,
            preview_password_hint: s.preview_password_hint,
            preview_password: None,
            preview_urls: build_preview_urls(id, parts),
            preview_url_template: build_preview_url_template(id, parts),
            idle_timeout_minutes: s.idle_timeout_minutes,
            title: s.title,
            cpu_limit: s.cpu_milli.map(|m| m as f32 / 1000.0),
            memory_limit_mb: s.memory_limit_mb,
            pids_limit: s.pids_limit,
        }
    }

    pub fn from_created(created: CreatedSession, parts: &PreviewUrlParts) -> Self {
        let mut resp = Self::from_model(created.session, parts);
        resp.preview_password = Some(created.preview_password_plaintext);
        resp
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MessageResponse {
    pub id: i64,
    pub session_id: i32,
    pub role: String,
    pub content: String,
    pub metadata: Option<serde_json::Value>,
    pub created_at: String,
}

impl From<workspace_messages::Model> for MessageResponse {
    fn from(m: workspace_messages::Model) -> Self {
        Self {
            id: m.id,
            session_id: m.session_id,
            role: m.role,
            content: m.content,
            metadata: m.metadata,
            created_at: m.created_at.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SessionWithMessagesResponse {
    pub session: SessionResponse,
    pub messages: Vec<MessageResponse>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SessionListResponse {
    pub sessions: Vec<SessionResponse>,
    pub total: u64,
    pub page: u64,
    pub page_size: u64,
}

// ── Routes ──────────────────────────────────────────────────────────────────

pub fn routes() -> Router<Arc<WorkspaceAppState>> {
    Router::new()
        .route(
            "/projects/{project_id}/workspace/sessions",
            get(list_sessions).post(start_session),
        )
        .route(
            "/projects/{project_id}/workspace/sessions/{session_id}",
            get(get_session)
                .patch(update_session)
                .delete(delete_session),
        )
        .route(
            "/projects/{project_id}/workspace/sessions/{session_id}/messages",
            post(send_message),
        )
        .route(
            "/projects/{project_id}/workspace/sessions/{session_id}/cancel",
            post(cancel_run),
        )
        .route(
            "/projects/{project_id}/workspace/sessions/{session_id}/stream",
            get(stream_messages),
        )
        .route(
            "/projects/{project_id}/workspace/sessions/{session_id}/close",
            post(close_session),
        )
        .route(
            "/projects/{project_id}/workspace/sessions/{session_id}/reopen",
            post(reopen_session),
        )
        .route(
            "/projects/{project_id}/workspace/sessions/{session_id}/preview-password/regenerate",
            post(regenerate_preview_password),
        )
        .route(
            "/projects/{project_id}/workspace/sessions/{session_id}/sandbox/stop",
            post(stop_sandbox),
        )
        .route(
            "/projects/{project_id}/workspace/sessions/{session_id}/sandbox/start",
            post(start_sandbox),
        )
        .route(
            "/projects/{project_id}/workspace/sessions/{session_id}/sandbox/restart",
            post(restart_sandbox),
        )
        .route(
            "/projects/{project_id}/workspace/sessions/{session_id}/sandbox/refresh",
            post(refresh_sandbox),
        )
        .route(
            "/projects/{project_id}/workspace/sessions/{session_id}/sandbox/stats",
            get(sandbox_stats),
        )
        .route(
            "/projects/{project_id}/workspace/sessions/{session_id}/terminal",
            get(session_terminal_ws),
        )
        .route(
            "/projects/{project_id}/workspace/sessions/{session_id}/terminal/paste-image",
            post(session_terminal_paste_image),
        )
        .route(
            "/projects/{project_id}/workspace/sessions/{session_id}/terminal/tabs",
            get(list_terminal_tabs),
        )
        .route(
            "/projects/{project_id}/workspace/sessions/{session_id}/terminal/tabs/{tab_id}",
            axum::routing::delete(delete_terminal_tab),
        )
}

// ── Terminal image paste ────────────────────────────────────────────────────
//
// xterm.js doesn't forward image clipboard data over the PTY (terminals carry
// text, not binary). To make Cmd+V on a screenshot work, the frontend POSTs
// the image bytes here, we drop the file into the sandbox's bind-mounted work
// dir on the host, and return the path as seen from *inside* the container.
// The frontend then types the path into the PTY — Claude CLI reads it as an
// image attachment.
//
// We write through the bind mount (`<host_work_dir>/.temps/pastes/…`) instead
// of going through `docker upload_to_container` because the bind mount is
// literally a shared directory: a `fs::write` on the host is visible inside
// the container immediately, with no tar building, no Docker API round-trip,
// and no 33% base64-in-JSON inflation once we later move to a streaming body.
// This collapses the paste latency from ~hundreds of ms to one syscall.
#[derive(Debug, Deserialize, ToSchema)]
struct PasteImageRequest {
    /// base64-encoded image bytes (no data: prefix)
    data: String,
    /// MIME type, used to pick the file extension. Defaults to "image/png".
    #[serde(default)]
    mime: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
struct PasteImageResponse {
    /// Path inside the sandbox where the image was written.
    path: String,
}

async fn session_terminal_paste_image(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<WorkspaceAppState>>,
    Path((project_id, session_id)): Path<(i32, i32)>,
    Json(req): Json<PasteImageRequest>,
) -> Result<impl IntoResponse, Problem> {
    use base64::{engine::general_purpose::STANDARD, Engine as _};

    permission_guard!(auth, ProjectsWrite);

    let session = app_state
        .workspace_service
        .get_session(session_id)
        .await
        .map_err(Problem::from)?;
    if session.project_id != project_id {
        return Err(problemdetails::new(StatusCode::NOT_FOUND)
            .with_title("Session Not Found")
            .with_detail(format!(
                "Session {session_id} does not belong to project {project_id}"
            )));
    }

    // Resolve the sandbox handle so we can see the bind-mounted host work dir.
    // A session with no live handle means the sandbox is gone — can't paste.
    let handle = app_state
        .session_manager
        .get_handle(session_id)
        .await
        .ok_or_else(|| {
            problemdetails::new(StatusCode::CONFLICT)
                .with_title("No Sandbox Container")
                .with_detail("Session has no running sandbox to paste into")
        })?;
    let host_work_dir = app_state
        .session_manager
        .get_host_work_dir(session_id)
        .await
        .ok_or_else(|| {
            // Recovered-from-disk session: host path wasn't persisted across
            // the server restart. Easiest recovery is asking the user to
            // send any message, which refreshes the sandbox tracking.
            problemdetails::new(StatusCode::CONFLICT)
                .with_title("Sandbox Not Fully Attached")
                .with_detail(
                    "This session was recovered after a server restart and its \
                     host work dir isn't tracked. Send a message first to \
                     refresh the sandbox, then retry the paste.",
                )
        })?;

    let bytes = STANDARD.decode(req.data.as_bytes()).map_err(|e| {
        problemdetails::new(StatusCode::BAD_REQUEST)
            .with_title("Invalid Image Data")
            .with_detail(format!("base64 decode failed: {e}"))
    })?;

    if bytes.len() > 25 * 1024 * 1024 {
        return Err(problemdetails::new(StatusCode::PAYLOAD_TOO_LARGE)
            .with_title("Image Too Large")
            .with_detail("Pasted images must be ≤ 25 MiB"));
    }

    // Sniff the actual file bytes first — clipboard MIME types are unreliable
    // (Safari sends empty, some apps send image/tiff or weird vendor types).
    // Magic-byte detection beats trusting the header.
    let sniffed_ext = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("gif")
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("webp")
    } else {
        None
    };
    let mime_ext = match req
        .mime
        .as_deref()
        .map(|s| {
            s.split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase()
        })
        .as_deref()
    {
        Some("image/png") => Some("png"),
        Some("image/jpeg") | Some("image/jpg") => Some("jpg"),
        Some("image/gif") => Some("gif"),
        Some("image/webp") => Some("webp"),
        _ => None,
    };
    // Prefer the sniffed extension; fall back to MIME; default to png so
    // Claude's bracketed-paste image detection still triggers.
    let ext = sniffed_ext.or(mime_ext).unwrap_or("png");
    let filename = format!("paste-{}.{ext}", uuid::Uuid::new_v4());

    // Write via the bind mount: host_work_dir is mounted at work_dir (typically
    // /workspace) inside the container, so a plain fs::write here is visible
    // to Claude CLI instantly. We scope pastes under a hidden `.temps/pastes/`
    // subdir so they don't pollute the user's repo listing.
    let host_dir = host_work_dir.join(".temps").join("pastes");
    tokio::fs::create_dir_all(&host_dir).await.map_err(|e| {
        problemdetails::new(StatusCode::INTERNAL_SERVER_ERROR)
            .with_title("Paste Dir Create Failed")
            .with_detail(format!("Failed to create {}: {e}", host_dir.display()))
    })?;
    let host_path = host_dir.join(&filename);
    tokio::fs::write(&host_path, &bytes).await.map_err(|e| {
        problemdetails::new(StatusCode::INTERNAL_SERVER_ERROR)
            .with_title("Paste Write Failed")
            .with_detail(format!(
                "Failed to write image to {}: {e}",
                host_path.display()
            ))
    })?;

    // Container-visible path: join the sandbox's work_dir (e.g. /workspace)
    // with the same relative path we just wrote on the host.
    let container_path = handle
        .work_dir
        .join(".temps")
        .join("pastes")
        .join(&filename)
        .to_string_lossy()
        .into_owned();

    Ok((
        StatusCode::OK,
        Json(PasteImageResponse {
            path: container_path,
        }),
    ))
}

// ── Terminal tabs (list / delete) ───────────────────────────────────────────
//
// Each browser terminal tab corresponds to one tmux session inside the
// container, named `temps-{kind}-{tab_id}`. These endpoints let the frontend
// rehydrate the tab bar on reload (so closing and reopening the browser
// surfaces previously-running tabs) and clean up tabs the user explicitly
// closes.

#[derive(Debug, Serialize, ToSchema)]
struct TerminalTab {
    /// `claude` or `shell` — drives which command runs in a fresh tab.
    kind: String,
    /// Stable id chosen by the client. Combined with `kind` to form the tmux
    /// session name.
    id: String,
    /// Number of tmux clients currently attached to this session. 0 means
    /// the tab is alive but no browser is viewing it.
    attached_clients: u32,
}

#[derive(Debug, Serialize, ToSchema)]
struct TerminalTabsResponse {
    tabs: Vec<TerminalTab>,
}

async fn list_terminal_tabs(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<WorkspaceAppState>>,
    Path((project_id, session_id)): Path<(i32, i32)>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, ProjectsRead);

    let (docker, container_id) =
        resolve_terminal_container(&app_state, project_id, session_id).await?;

    // tmux list-sessions output: "name: <N> windows (created ...) [80x24] (attached)"
    // We just need the names that match our prefix and the attached count.
    let output = exec_capture(
        &docker,
        &container_id,
        vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            "tmux list-sessions -F '#{session_name} #{session_attached}' 2>/dev/null || true"
                .to_string(),
        ],
    )
    .await
    .map_err(|e| {
        problemdetails::new(StatusCode::INTERNAL_SERVER_ERROR)
            .with_title("Failed to list terminal tabs")
            .with_detail(e)
    })?;

    let mut tabs = Vec::new();
    for line in output.lines() {
        let mut parts = line.split_ascii_whitespace();
        let Some(name) = parts.next() else { continue };
        let attached: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        // We only own sessions named `temps-{kind}-{id}` where kind ∈ {claude,shell}.
        let Some(rest) = name.strip_prefix("temps-") else {
            continue;
        };
        let (kind, id) = match rest.split_once('-') {
            Some((k, i)) if (k == "claude" || k == "shell") && !i.is_empty() => (k, i),
            _ => continue,
        };
        tabs.push(TerminalTab {
            kind: kind.to_string(),
            id: id.to_string(),
            attached_clients: attached,
        });
    }
    // Stable order: claude tabs first, then shells, then by id within each kind.
    tabs.sort_by(|a, b| a.kind.cmp(&b.kind).then_with(|| a.id.cmp(&b.id)));

    Ok((StatusCode::OK, Json(TerminalTabsResponse { tabs })))
}

async fn delete_terminal_tab(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<WorkspaceAppState>>,
    Path((project_id, session_id, tab_id)): Path<(i32, i32, String)>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, ProjectsWrite);

    // tab_id is "{kind}-{id}", e.g. "shell-abc123" or "claude-main".
    let (kind, id) = tab_id.split_once('-').ok_or_else(|| {
        problemdetails::new(StatusCode::BAD_REQUEST)
            .with_title("Invalid tab id")
            .with_detail("Expected format: {kind}-{id}, e.g. shell-abc")
    })?;
    if kind != "claude" && kind != "shell" {
        return Err(problemdetails::new(StatusCode::BAD_REQUEST)
            .with_title("Invalid tab kind")
            .with_detail("Tab kind must be 'claude' or 'shell'"));
    }
    if !is_safe_tab_id(id) {
        return Err(problemdetails::new(StatusCode::BAD_REQUEST)
            .with_title("Invalid tab id")
            .with_detail("Tab id must be alphanumeric (with - or _), max 32 chars"));
    }

    let (docker, container_id) =
        resolve_terminal_container(&app_state, project_id, session_id).await?;

    let session_name = format!("temps-{}-{}", kind, id);
    let _ = exec_capture(
        &docker,
        &container_id,
        vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            format!("tmux kill-session -t {} 2>/dev/null || true", session_name),
        ],
    )
    .await;

    Ok(StatusCode::NO_CONTENT)
}

/// Shared lookup: validates the workspace session belongs to the project,
/// gets the docker handle, and resolves the live container id. Returns 4xx
/// problems on every failure mode the terminal endpoints care about.
async fn resolve_terminal_container(
    app_state: &Arc<WorkspaceAppState>,
    project_id: i32,
    session_id: i32,
) -> Result<(Arc<bollard::Docker>, String), Problem> {
    let session = app_state
        .workspace_service
        .get_session(session_id)
        .await
        .map_err(Problem::from)?;
    if session.project_id != project_id {
        return Err(problemdetails::new(StatusCode::NOT_FOUND)
            .with_title("Session Not Found")
            .with_detail(format!(
                "Session {session_id} does not belong to project {project_id}"
            )));
    }
    let docker = app_state.docker.clone().ok_or_else(|| {
        problemdetails::new(StatusCode::SERVICE_UNAVAILABLE)
            .with_title("Docker Unavailable")
            .with_detail("Terminal tabs require the Docker sandbox provider")
    })?;
    let container_id = match app_state.session_manager.get_handle(session_id).await {
        Some(h) => h.sandbox_id,
        None => session.sandbox_container_id.clone().ok_or_else(|| {
            problemdetails::new(StatusCode::CONFLICT)
                .with_title("No Sandbox Container")
                .with_detail("Session has no running sandbox container yet")
        })?,
    };
    Ok((docker, container_id))
}

/// Run a one-shot command in the container and capture stdout. Used by the
/// tab list/delete endpoints. Errors are returned as plain strings — callers
/// wrap them into Problems with the right status code.
async fn exec_capture(
    docker: &Arc<bollard::Docker>,
    container_id: &str,
    cmd: Vec<String>,
) -> Result<String, String> {
    use bollard::exec::StartExecResults;
    let exec = docker
        .create_exec(
            container_id,
            bollard::models::ExecConfig {
                attach_stdout: Some(true),
                attach_stderr: Some(true),
                tty: Some(false),
                user: Some("temps".to_string()),
                cmd: Some(cmd),
                ..Default::default()
            },
        )
        .await
        .map_err(|e| format!("create_exec failed: {e}"))?;
    let start_opts = bollard::exec::StartExecOptions {
        detach: false,
        tty: false,
        ..Default::default()
    };
    let mut output = match docker
        .start_exec(&exec.id, Some(start_opts))
        .await
        .map_err(|e| format!("start_exec failed: {e}"))?
    {
        StartExecResults::Attached { output, .. } => output,
        StartExecResults::Detached => return Err("exec started detached".to_string()),
    };
    let mut buf = String::new();
    while let Some(chunk) = output.next().await {
        match chunk {
            Ok(bollard::container::LogOutput::StdOut { message })
            | Ok(bollard::container::LogOutput::StdErr { message })
            | Ok(bollard::container::LogOutput::Console { message }) => {
                buf.push_str(&String::from_utf8_lossy(&message));
            }
            Ok(_) => {}
            Err(e) => return Err(format!("read output: {e}")),
        }
    }
    Ok(buf)
}

// ── Terminal WebSocket ──────────────────────────────────────────────────────
//
// Raw PTY attached to the session's sandbox container. This is the replacement
// for the chat-message abstraction: instead of parsing stream-json and
// rebuilding the CLI's UI in React, we open `tmux new -A -s temps <cli>` over
// a websocket and pipe bytes to xterm.js. The AI CLI owns its own state,
// slash commands, interactive prompts, MCP approvals — all of it Just Works
// because the binary is running in a real TTY.
//
// Protocol (same as container_exec.rs):
//   client → server: binary frames are raw stdin bytes
//                    text frames {"type":"resize","cols":N,"rows":N} resize PTY
//   server → client: binary frames are raw PTY output for xterm.js
//                    text {"type":"exit","code":N} marks end of session

#[derive(Deserialize)]
struct TerminalControl {
    r#type: String,
    cols: Option<u16>,
    rows: Option<u16>,
    data: Option<String>,
}

/// Query string for the terminal websocket. Selects which tmux session inside
/// the container to attach to (so a workspace can have multiple independent
/// terminal tabs — one running claude, others running raw shells).
///
/// `kind` defaults to `claude` (run the AI CLI). `kind=shell` opens a bash.
/// `tab` is a stable identifier so reopening the same tab re-attaches to the
/// same tmux session inside the container — `temps-{kind}-{tab}`.
#[derive(Deserialize, Default)]
struct TerminalQuery {
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    tab: Option<String>,
}

/// Audit event emitted when a user successfully attaches a terminal websocket
/// to a workspace sandbox. This is the first thing the WS handler logs after
/// auth + ownership checks pass — i.e. once we're committed to upgrading the
/// connection. The detach event is logged from inside `handle_session_terminal`
/// when the loop exits, so an attach without a matching detach in the audit
/// log indicates a server crash mid-session.
#[derive(Debug, Clone, Serialize)]
struct WorkspaceTerminalAttachAudit {
    context: AuditContext,
    project_id: i32,
    session_id: i32,
    /// `claude` (AI CLI tab) or `shell` (raw bash). Mirrors the `kind` query param.
    kind: String,
    /// Logical tab id within the session, e.g. `main`. Combined with `kind`
    /// it identifies the dtach socket the user is attaching to.
    tab_id: String,
    /// First 12 chars of the Docker container id. Useful for cross-referencing
    /// with `docker logs` and host-level audit trails.
    container_id_prefix: String,
}

impl AuditOperation for WorkspaceTerminalAttachAudit {
    fn operation_type(&self) -> String {
        "WORKSPACE_TERMINAL_ATTACHED".to_string()
    }
    fn user_id(&self) -> i32 {
        self.context.user_id
    }
    fn ip_address(&self) -> Option<String> {
        self.context.ip_address.clone()
    }
    fn user_agent(&self) -> &str {
        &self.context.user_agent
    }
    fn serialize(&self) -> temps_core::anyhow::Result<String> {
        serde_json::to_string(self)
            .map_err(|e| temps_core::anyhow::anyhow!("Failed to serialize audit: {}", e))
    }
}

/// Audit event emitted when the terminal websocket loop exits — either
/// because the client disconnected, the idle timeout fired, or the PTY
/// command exited. Pairs with the `WORKSPACE_TERMINAL_ATTACHED` event so
/// auditors can compute session durations.
#[derive(Debug, Clone, Serialize)]
struct WorkspaceTerminalDetachAudit {
    context: AuditContext,
    project_id: i32,
    session_id: i32,
    kind: String,
    tab_id: String,
    container_id_prefix: String,
    /// Wall-clock seconds the websocket stayed open. Best-effort: measured
    /// from the moment the WS upgraded, not from auth time.
    duration_secs: u64,
}

impl AuditOperation for WorkspaceTerminalDetachAudit {
    fn operation_type(&self) -> String {
        "WORKSPACE_TERMINAL_DETACHED".to_string()
    }
    fn user_id(&self) -> i32 {
        self.context.user_id
    }
    fn ip_address(&self) -> Option<String> {
        self.context.ip_address.clone()
    }
    fn user_agent(&self) -> &str {
        &self.context.user_agent
    }
    fn serialize(&self) -> temps_core::anyhow::Result<String> {
        serde_json::to_string(self)
            .map_err(|e| temps_core::anyhow::anyhow!("Failed to serialize audit: {}", e))
    }
}

async fn session_terminal_ws(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<WorkspaceAppState>>,
    Extension(metadata): Extension<RequestMetadata>,
    Path((project_id, session_id)): Path<(i32, i32)>,
    Query(query): Query<TerminalQuery>,
    ws: WebSocketUpgrade,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, ProjectsWrite);

    // Verify session exists and belongs to this project.
    let session = app_state.workspace_service.get_session(session_id).await?;
    if session.project_id != project_id {
        return Err(problemdetails::new(StatusCode::NOT_FOUND)
            .with_title("Workspace Session Not Found")
            .with_detail(format!(
                "Session {} does not belong to project {}",
                session_id, project_id
            )));
    }
    if session.status == "closed" {
        return Err(problemdetails::new(StatusCode::CONFLICT)
            .with_title("Session Closed")
            .with_detail(format!(
                "Workspace session {} is closed — reopen it before attaching a terminal",
                session_id
            )));
    }

    let Some(docker) = app_state.docker.clone() else {
        return Err(problemdetails::new(StatusCode::SERVICE_UNAVAILABLE)
            .with_title("Terminal Unavailable")
            .with_detail(
                "This Temps instance is not running a Docker-backed sandbox provider, \
                 so interactive terminals are not available."
                    .to_string(),
            ));
    };

    // Resolve container_id. Prefer the live in-memory handle (most accurate),
    // fall back to the cached DB value on server-restart adoption.
    let container_id = match app_state.session_manager.get_handle(session_id).await {
        Some(h) => h.sandbox_id,
        None => session.sandbox_container_id.clone().ok_or_else(|| {
            problemdetails::new(StatusCode::CONFLICT)
                .with_title("Sandbox Not Ready")
                .with_detail(format!(
                    "Session {} has no live sandbox container yet. Send a chat \
                     message or refresh the sandbox first to provision one.",
                    session_id
                ))
        })?,
    };

    let ai_provider = session.ai_provider.clone();

    // Resolve tab kind + id. Defaults: kind=claude, tab=main. The tmux session
    // name baked from these is what makes multi-tab work — same {kind,tab} →
    // same tmux session → re-attach; different → independent terminal.
    let kind = query
        .kind
        .as_deref()
        .map(|k| k.to_ascii_lowercase())
        .unwrap_or_else(|| "claude".to_string());
    let kind = match kind.as_str() {
        "shell" => "shell".to_string(),
        // Anything else collapses to claude — keeps the API forgiving.
        _ => "claude".to_string(),
    };
    let tab_id = query
        .tab
        .filter(|t| is_safe_tab_id(t))
        .unwrap_or_else(|| "main".to_string());
    let tmux_session_name = format!("temps-{}-{}", kind, tab_id);

    let container_prefix = container_id[..container_id.len().min(12)].to_string();
    tracing::info!(
        "Workspace terminal requested: session={} project={} user={} container={} tmux={}",
        session_id,
        project_id,
        auth.user_id(),
        container_prefix,
        tmux_session_name
    );

    // Audit the attach BEFORE upgrading. We log the failure but never fail
    // the request — losing an audit row should not lock a developer out of
    // their terminal. The matching detach event is emitted from inside
    // `handle_session_terminal` once the loop exits.
    let audit_context = AuditContext {
        user_id: auth.user_id(),
        ip_address: Some(metadata.ip_address.clone()),
        user_agent: metadata.user_agent.clone(),
    };
    let attach_audit = WorkspaceTerminalAttachAudit {
        context: audit_context.clone(),
        project_id,
        session_id,
        kind: kind.clone(),
        tab_id: tab_id.clone(),
        container_id_prefix: container_prefix.clone(),
    };
    if let Err(e) = app_state
        .audit_service
        .create_audit_log(&attach_audit)
        .await
    {
        tracing::error!(
            "Failed to write WORKSPACE_TERMINAL_ATTACHED audit for session {}: {}",
            session_id,
            e
        );
    }

    let state_for_task = app_state.clone();
    // Cap per-frame and per-message sizes so an authenticated user can't
    // flood the PTY stdin with 64 MiB axum-default frames. 1 MiB / frame
    // comfortably covers multi-megabyte pastes; 4 MiB per aggregated
    // message covers the fragmented-upload case. Anything larger is
    // rejected by axum before we ever read it.
    let ws = ws
        .max_frame_size(1024 * 1024)
        .max_message_size(4 * 1024 * 1024);
    Ok(ws.on_upgrade(move |socket| {
        handle_session_terminal(
            socket,
            docker,
            container_id,
            session_id,
            project_id,
            ai_provider,
            kind,
            tab_id,
            tmux_session_name,
            container_prefix,
            audit_context,
            state_for_task,
        )
    }))
}

/// Validates a user-supplied tab id so it's safe to interpolate into a tmux
/// session name and shell command. We accept lowercase alphanumerics and `-`,
/// up to 32 chars. Anything else falls back to "main".
fn is_safe_tab_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 32
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// The CLI command to run inside the tmux session for a given provider.
/// tmux `new -A -s temps` attaches to the existing session if it exists,
/// otherwise creates one running the given command. Browser refreshes and
/// websocket reconnects re-attach to the same tmux session transparently,
/// so the CLI's scrollback + state survive disconnects.
fn tmux_cli_for_provider(provider: &str) -> &'static str {
    match provider {
        "codex_cli" => "codex",
        "opencode" => "opencode",
        // Default to claude for "claude_cli" and anything else.
        _ => "claude",
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_session_terminal(
    socket: WebSocket,
    docker: Arc<bollard::Docker>,
    container_id: String,
    session_id: i32,
    project_id: i32,
    ai_provider: String,
    kind: String,
    tab_id: String,
    tmux_session_name: String,
    container_id_prefix: String,
    audit_context: AuditContext,
    app_state: Arc<WorkspaceAppState>,
) {
    use futures::SinkExt;
    let attach_started = std::time::Instant::now();

    let cli = tmux_cli_for_provider(&ai_provider);

    // The "inner" command tmux runs when creating a new session. For claude
    // tabs, launch the AI CLI; for shell tabs, just drop into bash. Either
    // way, the fallback chain (`|| exec bash`) keeps the tmux session alive
    // if the launched program exits, so the user can recover.
    // Workspace sandboxes are isolated, ephemeral, and dedicated to a single
    // user — exactly the threat model `--dangerously-skip-permissions` was
    // designed for. Without it, the user has to approve every file edit and
    // shell command interactively, which is a non-starter inside a tmux pane
    // streamed over a websocket. The chat-mode path
    // (`session_manager::build_cli_cmd`) already passes this flag
    // unconditionally, so the terminal path now matches.
    //
    // The flag only applies to `claude` (other CLIs have their own
    // equivalents — codex uses `--approval-mode full-auto` baked into
    // `build_cli_cmd`, opencode has no concept of approvals).
    let cli_args = if cli == "claude" {
        " --dangerously-skip-permissions"
    } else {
        ""
    };
    // Terminal session supervision uses `dtach` instead of tmux. Why:
    //
    //   tmux-via-`docker exec` was unreliable across reconnects. Each exec
    //   spawned a fresh tmux client, and when the websocket dropped the
    //   exec's controlling sh would die — sometimes taking the tmux server
    //   with it. A new reconnect would find no server, run `new-session`,
    //   and spawn a *second* claude process that had no knowledge of the
    //   first one's background shells. We saw up to 5 claudes in a single
    //   container. The fundamental problem: tmux's server lifetime was
    //   entangled with the exec stream's lifetime.
    //
    //   `dtach` fixes this by design. It double-forks a "master" on first
    //   attach (`-A`) that owns the PTY and the child program, then the
    //   attach client just proxies bytes over a Unix socket. When the
    //   client dies (websocket closes), the master is untouched — it keeps
    //   running until the program inside exits or the container stops.
    //   Reconnect runs `dtach -A <same.sock>` which finds the existing
    //   master and re-attaches. Claude is launched exactly once per
    //   sandbox lifetime because the shell command that *creates* the
    //   master only runs on the very first `-A` call.
    //
    //   Flags:
    //     -A  = attach existing, or create new (the key flag)
    //     -E  = disable detach character (no `Ctrl-\` swallowing — we want
    //           every keystroke to reach the child program)
    //     -z  = ignore suspend, pass ^Z through
    //     -r winch = on attach, send SIGWINCH to child so its TUI redraws
    //                at the new client's size. This is how scrollback is
    //                "recovered" on reconnect — claude/codex/opencode all
    //                redraw their full TUI state on SIGWINCH.
    //
    // The socket lives in /run/temps-pty/{kind}-{tab}.sock. That directory
    // is created in the sandbox Dockerfile with `temps:temps` ownership.
    //
    // Stale-socket hygiene: if the container was stopped and restarted,
    // the old master process is gone but the socket file persists in the
    // writable layer. We detect this by comparing /proc/1's mtime
    // (effectively the container's boot time) against a marker file —
    // if they don't match, we wipe the directory and start fresh. This
    // runs once per container boot; subsequent exec invocations in the
    // same boot are a no-op.
    let sock_path = format!("/run/temps-pty/{}.sock", tmux_session_name);
    // `inner_cmd` is the script dtach runs ONCE on master creation. After the
    // CLI exits (or if it fails to launch), we fall through to bash so the
    // user still has a live shell inside the dtach master — they can fix
    // whatever went wrong and relaunch manually without losing the tab.
    //
    // Note we don't use `exec` on the CLI invocations here — if claude fails
    // with a non-zero exit we want the `||` chain to fire. Only the final
    // `exec bash` replaces the shell (there's nothing after it).
    let inner_cmd = match kind.as_str() {
        "shell" => "exec bash".to_string(),
        _ => format!(
            "cd /workspace && {{ {cli}{args} --continue 2>/dev/null || {cli}{args}; }}; exec bash",
            cli = cli,
            args = cli_args,
        ),
    };

    // PATH hardening: the dockerfile sets `ENV PATH=/home/temps/.local/bin:...`,
    // but older cached sandbox images (or containers created before that fix
    // landed) have `~/.local/bin` missing from their frozen env. An interactive
    // shell picks it up via `~/.bashrc`, but the non-interactive `sh -c` dtach
    // runs does not source .bashrc — so `claude` would appear missing even
    // though it's installed at /home/temps/.local/bin/claude. Prepending the
    // known install directories here is cheap insurance and works regardless
    // of what the container image baked in.
    let exec_script = format!(
        r#"export PATH=/home/temps/.local/bin:/usr/local/bun/bin:$PATH; \
. ~/.env 2>/dev/null; \
BOOT_ID=$(stat -c %Y /proc/1 2>/dev/null || echo unknown); \
if [ "$(cat /run/temps-pty/.boot 2>/dev/null)" != "$BOOT_ID" ]; then \
  rm -f /run/temps-pty/*.sock 2>/dev/null; \
  echo "$BOOT_ID" > /run/temps-pty/.boot 2>/dev/null; \
fi; \
if command -v dtach >/dev/null 2>&1; then \
  exec dtach -A {sock} -E -z -r winch /bin/sh -c 'export PATH=/home/temps/.local/bin:/usr/local/bun/bin:$PATH; . ~/.env 2>/dev/null; cd /workspace && {inner}'; \
else \
  cd /workspace && {inner}; \
fi"#,
        sock = sock_path,
        inner = inner_cmd,
    );

    let exec_config = bollard::models::ExecConfig {
        attach_stdin: Some(true),
        attach_stdout: Some(true),
        attach_stderr: Some(true),
        tty: Some(true),
        user: Some("temps".to_string()),
        working_dir: Some("/workspace".to_string()),
        env: Some(vec!["TERM=xterm-256color".to_string()]),
        cmd: Some(vec!["/bin/sh".to_string(), "-c".to_string(), exec_script]),
        ..Default::default()
    };

    let exec = match docker.create_exec(&container_id, exec_config).await {
        Ok(e) => e,
        Err(e) => {
            tracing::error!(
                "Failed to create terminal exec for session {}: {}",
                session_id,
                e
            );
            return;
        }
    };
    let exec_id = exec.id.clone();

    let start_opts = bollard::exec::StartExecOptions {
        detach: false,
        tty: true,
        ..Default::default()
    };

    let (mut pty_output, mut pty_input) = match docker.start_exec(&exec_id, Some(start_opts)).await
    {
        Ok(StartExecResults::Attached { output, input }) => (output, input),
        Ok(StartExecResults::Detached) => {
            tracing::error!(
                "Terminal exec for session {} started detached unexpectedly",
                session_id
            );
            return;
        }
        Err(e) => {
            tracing::error!(
                "Failed to start terminal exec for session {}: {}",
                session_id,
                e
            );
            return;
        }
    };

    let (ws_sender, mut ws_receiver) = socket.split();
    // Shared across PTY-output task AND the keepalive ping task so both can
    // push frames to the same websocket. A websocket sink is single-writer,
    // so the mutex just serializes access — contention is trivial in practice
    // (ping every 20s vs PTY writes that are already batched).
    let ws_sender = Arc::new(tokio::sync::Mutex::new(ws_sender));

    // Keepalive: send a WebSocket Ping frame every 20s. Pingora's upstream
    // read_timeout is 1h for WS upgrades (see temps-proxy), but middleboxes
    // between the client and Pingora (mobile NATs, corporate proxies) still
    // drop idle TCP after ~60s. The ping keeps every hop warm and also lets
    // us notice a dead client early so the docker exec is torn down
    // promptly instead of lingering until the next user keystroke.
    let ping_sender = ws_sender.clone();
    let ping_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(20));
        // Skip the immediate tick — we've just opened the socket.
        interval.tick().await;
        loop {
            interval.tick().await;
            let mut guard = ping_sender.lock().await;
            if guard.send(Message::Ping(Vec::new().into())).await.is_err() {
                break;
            }
        }
    });

    // PTY → websocket
    let exec_id_out = exec_id.clone();
    let docker_out = docker.clone();
    let output_sender = ws_sender.clone();
    let heartbeat_state = app_state.clone();
    let output_task = tokio::spawn(async move {
        // Throttled activity heartbeat: whenever the PTY emits output we
        // know the sandbox is alive and (most likely) an agent is working,
        // even if the user hasn't pressed a key. Bumping last_activity_at
        // keeps the idle reaper from closing background runs. At most one
        // UPDATE per 60s to avoid hammering the DB during chatty TUIs.
        let heartbeat_interval = Duration::from_secs(60);
        let mut last_heartbeat = std::time::Instant::now()
            .checked_sub(heartbeat_interval)
            .unwrap_or_else(std::time::Instant::now);
        while let Some(chunk) = pty_output.next().await {
            let bytes: Bytes = match chunk {
                Ok(bollard::container::LogOutput::StdOut { message }) => message,
                Ok(bollard::container::LogOutput::StdErr { message }) => message,
                Ok(bollard::container::LogOutput::Console { message }) => message,
                Ok(_) => continue,
                Err(e) => {
                    tracing::debug!(
                        "Terminal pty stream error for session {}: {}",
                        session_id,
                        e
                    );
                    break;
                }
            };
            if last_heartbeat.elapsed() >= heartbeat_interval {
                last_heartbeat = std::time::Instant::now();
                let svc = heartbeat_state.workspace_service.clone();
                tokio::spawn(async move {
                    if let Err(e) = svc.touch_activity(session_id).await {
                        tracing::debug!("touch_activity failed for session {}: {}", session_id, e);
                    }
                });
            }
            let mut guard = output_sender.lock().await;
            if guard
                .send(Message::Binary(bytes.to_vec().into()))
                .await
                .is_err()
            {
                break;
            }
        }

        let exit_code = docker_out
            .inspect_exec(&exec_id_out)
            .await
            .ok()
            .and_then(|i| i.exit_code)
            .unwrap_or(-1);
        let exit_msg = format!(r#"{{"type":"exit","code":{}}}"#, exit_code);
        let mut guard = output_sender.lock().await;
        let _ = guard.send(Message::Text(exit_msg.into())).await;
        let _ = guard.close().await;
    });

    // websocket → PTY (+ resize control messages)
    // Idle timeout is intentionally generous: a tmux-attached terminal where
    // the user is reading CLI output may sit silent for a long time.
    let idle_timeout = tokio::time::Duration::from_secs(60 * 60);

    // Token-bucket rate limit on stdin: 2 MiB/s sustained, 8 MiB burst.
    // Large pastes (up to 8 MiB instantly) pass through unthrottled; a
    // sustained flood gets the excess frames dropped rather than
    // disconnected, so pathological input never wedges the PTY or
    // saturates the Docker API stream.
    const RATE_BYTES_PER_SEC: u64 = 2 * 1024 * 1024;
    const BUCKET_CAPACITY: u64 = 8 * 1024 * 1024;
    let mut bucket_tokens: u64 = BUCKET_CAPACITY;
    let mut last_refill = std::time::Instant::now();
    let refill_bucket = |tokens: &mut u64, last: &mut std::time::Instant| {
        let now = std::time::Instant::now();
        let elapsed = now.duration_since(*last).as_secs_f64();
        if elapsed > 0.0 {
            let add = (elapsed * RATE_BYTES_PER_SEC as f64) as u64;
            *tokens = (*tokens).saturating_add(add).min(BUCKET_CAPACITY);
            *last = now;
        }
    };
    loop {
        let msg = tokio::time::timeout(idle_timeout, ws_receiver.next()).await;
        match msg {
            Ok(Some(Ok(Message::Binary(data)))) => {
                refill_bucket(&mut bucket_tokens, &mut last_refill);
                let needed = data.len() as u64;
                if needed > bucket_tokens {
                    tracing::warn!(
                        "Terminal stdin rate limit exceeded for session {} ({} bytes, {} available) — dropping frame",
                        session_id,
                        needed,
                        bucket_tokens
                    );
                    continue;
                }
                bucket_tokens -= needed;
                if pty_input.write_all(&data).await.is_err() {
                    break;
                }
                if pty_input.flush().await.is_err() {
                    break;
                }
            }
            Ok(Some(Ok(Message::Text(text)))) => {
                if let Ok(ctrl) = serde_json::from_str::<TerminalControl>(&text) {
                    match ctrl.r#type.as_str() {
                        "resize" => {
                            if let (Some(cols), Some(rows)) = (ctrl.cols, ctrl.rows) {
                                let opts = bollard::exec::ResizeExecOptions {
                                    width: cols,
                                    height: rows,
                                };
                                if let Err(e) = docker.resize_exec(&exec_id, opts).await {
                                    tracing::debug!(
                                        "resize_exec failed for session {}: {}",
                                        session_id,
                                        e
                                    );
                                }
                            }
                        }
                        "input" => {
                            if let Some(data) = ctrl.data {
                                refill_bucket(&mut bucket_tokens, &mut last_refill);
                                let needed = data.len() as u64;
                                if needed > bucket_tokens {
                                    tracing::warn!(
                                        "Terminal stdin rate limit exceeded for session {} ({} bytes, {} available) — dropping input",
                                        session_id,
                                        needed,
                                        bucket_tokens
                                    );
                                } else {
                                    bucket_tokens -= needed;
                                    if pty_input.write_all(data.as_bytes()).await.is_err() {
                                        break;
                                    }
                                    let _ = pty_input.flush().await;
                                }
                            }
                        }
                        _ => {}
                    }
                } else if pty_input.write_all(text.as_bytes()).await.is_err() {
                    break;
                }
            }
            Ok(Some(Ok(Message::Close(_)))) | Ok(None) => {
                tracing::debug!("Terminal closed by client for session {}", session_id);
                break;
            }
            Err(_) => {
                tracing::info!(
                    "Terminal idle timeout for session {} — closing websocket",
                    session_id
                );
                break;
            }
            _ => {}
        }
    }

    output_task.abort();
    ping_task.abort();

    let duration_secs = attach_started.elapsed().as_secs();
    tracing::info!(
        "Terminal session ended for workspace session {} after {}s",
        session_id,
        duration_secs
    );

    // Emit the matching detach audit. Best-effort: a missing detach row
    // (with the attach row present) tells an auditor "server crashed mid-
    // session" which is itself useful signal.
    let detach_audit = WorkspaceTerminalDetachAudit {
        context: audit_context,
        project_id,
        session_id,
        kind,
        tab_id,
        container_id_prefix,
        duration_secs,
    };
    if let Err(e) = app_state
        .audit_service
        .create_audit_log(&detach_audit)
        .await
    {
        tracing::error!(
            "Failed to write WORKSPACE_TERMINAL_DETACHED audit for session {}: {}",
            session_id,
            e
        );
    }
}

async fn refresh_sandbox(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<WorkspaceAppState>>,
    Path((_project_id, session_id)): Path<(i32, i32)>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, ProjectsWrite);
    if let Some(executor) = app_state.message_executor.clone() {
        executor.refresh_sandbox(session_id).await?;
    } else {
        return Err(WorkspaceError::SandboxNotAvailable { session_id }.into());
    }
    tracing::info!(
        "Workspace sandbox {} refreshed by user {}",
        session_id,
        auth.user_id()
    );
    Ok(StatusCode::NO_CONTENT)
}

/// Live resource-usage snapshot for a session's sandbox container.
///
/// All "used" fields are instantaneous readings from Docker's cgroup stats
/// (one-shot `stats` call, no streaming). Limits are what the sandbox was
/// created with. CPU limit is in vCPU cores; `cpu_used_cores` is the
/// fractional cores the container is currently consuming (e.g. 0.42 =
/// 42% of one core = 21% of a 2-core limit).
#[derive(Debug, Serialize, ToSchema)]
struct SandboxStatsResponse {
    container_id: String,
    /// CPU cores currently consumed (0.0 → cpu_limit_cores).
    cpu_used_cores: f64,
    /// CPU limit the container was created with, in vCPU cores.
    cpu_limit_cores: f64,
    /// Percent of the container's CPU budget currently in use (0–100).
    cpu_percent: f64,
    /// RAM currently consumed, in bytes (RSS-equivalent — Docker's
    /// `usage - cache` when available, otherwise raw `usage`).
    memory_used_bytes: u64,
    /// Hard memory limit the container was created with, in bytes.
    memory_limit_bytes: u64,
    /// Percent of the container's RAM budget currently in use (0–100).
    memory_percent: f64,
}

async fn sandbox_stats(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<WorkspaceAppState>>,
    Path((project_id, session_id)): Path<(i32, i32)>,
) -> Result<impl IntoResponse, Problem> {
    use bollard::query_parameters::StatsOptions;
    use futures::TryStreamExt;

    permission_guard!(auth, ProjectsRead);

    // Cheap sanity check that the session belongs to the project.
    let session = app_state.workspace_service.get_session(session_id).await?;
    if session.project_id != project_id {
        return Err(problemdetails::new(StatusCode::NOT_FOUND)
            .with_title("Session Not Found")
            .with_detail(format!(
                "Session {session_id} does not belong to project {project_id}"
            )));
    }

    let Some(docker) = app_state.docker.clone() else {
        return Err(problemdetails::new(StatusCode::SERVICE_UNAVAILABLE)
            .with_title("Stats Unavailable")
            .with_detail(
                "This Temps instance has no Docker-backed sandbox provider.".to_string(),
            ));
    };

    // Prefer the live in-memory handle for the freshest container id;
    // fall back to the DB cache for sessions adopted after a restart.
    let container_id = match app_state.session_manager.get_handle(session_id).await {
        Some(h) => h.sandbox_id,
        None => session.sandbox_container_id.clone().ok_or_else(|| {
            problemdetails::new(StatusCode::CONFLICT)
                .with_title("No Sandbox Container")
                .with_detail(format!(
                    "Session {session_id} has no live sandbox to query stats for."
                ))
        })?,
    };

    // one_shot=true → Docker returns a single snapshot immediately and
    // closes the stream. This is much cheaper than stream=true (which
    // opens a 1s polling loop server-side). The trade-off: CPU delta is
    // computed against precpu_stats from ~0ms ago, so the number is a
    // short-window average rather than a smoothed rate. Good enough for
    // a UI badge.
    let mut stream = docker.stats(
        &container_id,
        Some(StatsOptions {
            stream: false,
            one_shot: true,
        }),
    );
    let stats = stream
        .try_next()
        .await
        .map_err(|e| {
            problemdetails::new(StatusCode::INTERNAL_SERVER_ERROR)
                .with_title("Stats Read Failed")
                .with_detail(format!("docker stats for {container_id}: {e}"))
        })?
        .ok_or_else(|| {
            problemdetails::new(StatusCode::INTERNAL_SERVER_ERROR)
                .with_title("Stats Empty")
                .with_detail(format!("docker stats returned no data for {container_id}"))
        })?;

    // --- CPU -------------------------------------------------------------
    // Standard Docker formula: (cpu_delta / system_delta) * online_cpus.
    // online_cpus is the number the container can actually schedule on,
    // which equals the nano_cpus limit we set at create time (rounded up).
    let cur_cpu = stats
        .cpu_stats
        .as_ref()
        .and_then(|c| c.cpu_usage.as_ref())
        .and_then(|u| u.total_usage);
    let cur_sys = stats.cpu_stats.as_ref().and_then(|c| c.system_cpu_usage);
    let pre_cpu = stats
        .precpu_stats
        .as_ref()
        .and_then(|c| c.cpu_usage.as_ref())
        .and_then(|u| u.total_usage);
    let pre_sys = stats.precpu_stats.as_ref().and_then(|c| c.system_cpu_usage);
    let online_cpus = stats
        .cpu_stats
        .as_ref()
        .and_then(|c| c.online_cpus)
        .unwrap_or(1) as f64;

    let cpu_used_cores = match (cur_cpu, cur_sys, pre_cpu, pre_sys) {
        (Some(cc), Some(cs), Some(pc), Some(ps)) => {
            let cpu_delta = cc.saturating_sub(pc) as f64;
            let sys_delta = cs.saturating_sub(ps) as f64;
            if sys_delta > 0.0 {
                (cpu_delta / sys_delta) * online_cpus
            } else {
                0.0
            }
        }
        _ => 0.0,
    };

    // --- Memory ----------------------------------------------------------
    // Docker's `memory_stats.usage` on cgroup v2 reports `memory.current`,
    // which already excludes reclaimable page cache — good enough for a
    // UI badge without the v1/v2 stats-map gymnastics. Users on cgroup v1
    // hosts will see slightly inflated numbers (cache counted toward
    // usage); acceptable trade-off for simplicity.
    let mem = stats.memory_stats.as_ref();
    let memory_used_bytes = mem.and_then(|m| m.usage).unwrap_or(0);
    let memory_limit_bytes = mem.and_then(|m| m.limit).unwrap_or(0);

    // Resolve the CPU limit we set at create time. The DB row stores it
    // as integer milli-cpus (for Eq compatibility). Fall back to
    // online_cpus if nothing is set.
    let cpu_limit_cores = session
        .cpu_milli
        .map(|m| m as f64 / 1000.0)
        .unwrap_or(online_cpus.max(1.0));

    let cpu_percent = if cpu_limit_cores > 0.0 {
        ((cpu_used_cores / cpu_limit_cores) * 100.0).clamp(0.0, 100.0)
    } else {
        0.0
    };
    let memory_percent = if memory_limit_bytes > 0 {
        ((memory_used_bytes as f64 / memory_limit_bytes as f64) * 100.0).clamp(0.0, 100.0)
    } else {
        0.0
    };

    Ok(axum::Json(SandboxStatsResponse {
        container_id,
        cpu_used_cores,
        cpu_limit_cores,
        cpu_percent,
        memory_used_bytes,
        memory_limit_bytes,
        memory_percent,
    }))
}

async fn stop_sandbox(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<WorkspaceAppState>>,
    Path((_project_id, session_id)): Path<(i32, i32)>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, ProjectsWrite);
    app_state.session_manager.stop_sandbox(session_id).await?;
    tracing::info!(
        "Workspace sandbox {} stopped by user {}",
        session_id,
        auth.user_id()
    );
    Ok(StatusCode::NO_CONTENT)
}

async fn start_sandbox(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<WorkspaceAppState>>,
    Path((_project_id, session_id)): Path<(i32, i32)>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, ProjectsWrite);
    app_state.session_manager.start_sandbox(session_id).await?;
    tracing::info!(
        "Workspace sandbox {} started by user {}",
        session_id,
        auth.user_id()
    );
    Ok(StatusCode::NO_CONTENT)
}

async fn restart_sandbox(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<WorkspaceAppState>>,
    Path((_project_id, session_id)): Path<(i32, i32)>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, ProjectsWrite);
    app_state
        .session_manager
        .restart_sandbox(session_id)
        .await?;
    tracing::info!(
        "Workspace sandbox {} restarted by user {}",
        session_id,
        auth.user_id()
    );
    Ok(StatusCode::NO_CONTENT)
}

// ── Handlers ────────────────────────────────────────────────────────────────

async fn start_session(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<WorkspaceAppState>>,
    Path(project_id): Path<i32>,
    Json(request): Json<StartSessionRequest>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, ProjectsWrite);

    let mut created = app_state
        .workspace_service
        .create_session(CreateSessionRequest {
            project_id,
            user_id: auth.user_id(),
            ai_provider: request
                .ai_provider
                .unwrap_or_else(|| "claude_cli".to_string()),
            branch_name: request.branch_name,
            base_branch_name: request.base_branch_name,
            metadata: request.metadata,
        })
        .await?;

    // Eagerly provision the sandbox so the terminal tab is immediately
    // usable. See reopen_session for rationale.
    if let Some(executor) = app_state.message_executor.clone() {
        let session_id = created.session.id;
        if let Err(e) = executor.ensure_sandbox(session_id).await {
            tracing::warn!(
                "start_session: ensure_sandbox failed for session {}: {} — \
                 UI will fall back to lazy provisioning on first message",
                session_id,
                e
            );
        } else if let Ok(refreshed) = app_state.workspace_service.get_session(session_id).await {
            created.session = refreshed;
        }
    }

    tracing::info!(
        "Workspace session {} created for project {} by user {}",
        created.session.id,
        project_id,
        auth.user_id()
    );

    let preview_parts = preview_url_parts(&app_state).await;
    Ok((
        StatusCode::CREATED,
        Json(SessionResponse::from_created(created, &preview_parts)),
    ))
}

/// Load the preview URL parts (protocol, domain, optional port) from
/// platform settings, falling back to a local default if settings can't
/// be loaded. Never errors — a broken settings read should not break
/// session endpoints.
///
/// Protocol and (optional) port are derived from `external_url`. If
/// `external_url` is `http://1.2.3.4:8080`, all preview URLs are emitted
/// as `http://ws-{sid}-{port}.{domain}:8080` instead of being silently
/// upgraded to `https://...:443`.
async fn preview_url_parts(state: &WorkspaceAppState) -> PreviewUrlParts {
    match state.platform_config_service.get_settings().await {
        Ok(s) => {
            let (protocol, port) = if let Some(ref external_url) = s.external_url {
                if let Ok(parsed) = url::Url::parse(external_url) {
                    (parsed.scheme().to_string(), parsed.port())
                } else if external_url.starts_with("https://") {
                    ("https".to_string(), None)
                } else if external_url.starts_with("http://") {
                    ("http".to_string(), None)
                } else {
                    ("https".to_string(), None)
                }
            } else {
                ("https".to_string(), None)
            };

            let domain = if s.preview_domain.is_empty() {
                "localho.st".to_string()
            } else {
                s.preview_domain.trim_start_matches("*.").to_string()
            };

            // Don't append default ports — http://host:80 / https://host:443
            // would be ugly and unnecessary.
            let port = port.filter(|p| {
                !((protocol == "https" && *p == 443) || (protocol == "http" && *p == 80))
            });

            PreviewUrlParts {
                protocol,
                domain,
                port,
            }
        }
        Err(e) => {
            tracing::warn!(
                "failed to load platform settings for preview URL parts: {} — falling back to https://localho.st",
                e
            );
            PreviewUrlParts {
                protocol: "https".to_string(),
                domain: "localho.st".to_string(),
                port: None,
            }
        }
    }
}

async fn regenerate_preview_password(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<WorkspaceAppState>>,
    Path((_project_id, session_id)): Path<(i32, i32)>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, ProjectsWrite);

    let plaintext = app_state
        .workspace_service
        .regenerate_preview_password(session_id)
        .await?;
    let session = app_state.workspace_service.get_session(session_id).await?;
    let preview_parts = preview_url_parts(&app_state).await;
    let mut resp = SessionResponse::from_model(session, &preview_parts);
    resp.preview_password = Some(plaintext);
    tracing::info!(
        "Workspace session {} preview password regenerated by user {}",
        session_id,
        auth.user_id()
    );
    Ok((StatusCode::OK, Json(resp)))
}

async fn list_sessions(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<WorkspaceAppState>>,
    Path(project_id): Path<i32>,
    Query(params): Query<PaginationParams>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, ProjectsRead);

    let page = params.page.unwrap_or(1);
    let page_size = params.page_size.unwrap_or(20);

    let (sessions, total) = app_state
        .workspace_service
        .list_sessions(project_id, Some(page), Some(page_size))
        .await?;

    let preview_parts = preview_url_parts(&app_state).await;
    Ok(Json(SessionListResponse {
        sessions: sessions
            .into_iter()
            .map(|s| SessionResponse::from_model(s, &preview_parts))
            .collect(),
        total,
        page,
        page_size,
    }))
}

async fn get_session(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<WorkspaceAppState>>,
    Path((_project_id, session_id)): Path<(i32, i32)>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, ProjectsRead);

    let swm = app_state
        .workspace_service
        .get_session_with_messages(session_id)
        .await?;

    let preview_parts = preview_url_parts(&app_state).await;
    Ok(Json(SessionWithMessagesResponse {
        session: SessionResponse::from_model(swm.session, &preview_parts),
        messages: swm
            .messages
            .into_iter()
            .map(MessageResponse::from)
            .collect(),
    }))
}

async fn send_message(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<WorkspaceAppState>>,
    Path((_project_id, session_id)): Path<(i32, i32)>,
    Json(body): Json<SendMessageBody>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, ProjectsWrite);

    if body.content.trim().is_empty() {
        return Err(WorkspaceError::Validation {
            message: "Message content cannot be empty".to_string(),
        }
        .into());
    }

    // Save the user message. The drain loop reads pending user messages
    // from the DB, so persisting it here is what enqueues it.
    let message = app_state
        .workspace_service
        .append_message(SendMessageRequest {
            session_id,
            role: "user".to_string(),
            content: body.content,
            metadata: body.metadata,
        })
        .await?;

    // Kick the drain loop. If a loop is already running for this session,
    // this is a no-op (the loop will pick up the new message on its next
    // iteration). Otherwise it spawns a fresh loop. Errors from the loop
    // are surfaced to the chat as terminal system+assistant messages by
    // the loop itself, so we don't need to handle them here.
    if let Some(executor) = app_state.message_executor.clone() {
        if let Err(e) = executor.enqueue_run(session_id).await {
            tracing::error!(
                "Failed to enqueue drain run for session {}: {}",
                session_id,
                e
            );
        }
    } else {
        tracing::warn!(
            "Message saved to session {} but no MessageExecutor available — AI will not run",
            session_id
        );
    }

    Ok((StatusCode::ACCEPTED, Json(MessageResponse::from(message))))
}

/// Manual cancel/reset for a stuck session.
///
/// Does three things:
///   1. Fires the per-session cancellation token so the in-flight exec
///      future bails out on its next poll (via tokio::select).
///   2. Sends SIGTERM to any `claude` process in the sandbox, waits 2s,
///      then SIGKILL. This guarantees we don't leak a claude child even
///      if the exec stream is phantom-hung.
///   3. Marks the session dirty so the next message runs the jsonl repair
///      pass before invoking --continue.
///   4. Writes terminal `system` + `assistant` messages so the UI spinner
///      clears immediately.
async fn cancel_run(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<WorkspaceAppState>>,
    Path((_project_id, session_id)): Path<(i32, i32)>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, ProjectsWrite);

    // Verify the session exists.
    app_state.workspace_service.get_session(session_id).await?;

    // Trigger the full cancellation path if an executor is wired up.
    if let Some(executor) = app_state.message_executor.clone() {
        executor.cancel(session_id).await;
    }

    let text = "Run cancelled by user.".to_string();
    let _ = app_state
        .workspace_service
        .append_message(SendMessageRequest {
            session_id,
            role: "system".to_string(),
            content: text.clone(),
            metadata: None,
        })
        .await;
    let _ = app_state
        .workspace_service
        .append_message(SendMessageRequest {
            session_id,
            role: "assistant".to_string(),
            content: text,
            metadata: Some(serde_json::json!({
                "error": true,
                "error_kind": "cancelled",
            })),
        })
        .await;

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
pub struct StreamParams {
    pub after_id: Option<i64>,
}

async fn stream_messages(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<WorkspaceAppState>>,
    Path((_project_id, session_id)): Path<(i32, i32)>,
    Query(params): Query<StreamParams>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, Problem> {
    permission_guard!(auth, ProjectsRead);

    // Verify the session exists (errors here become Problem responses)
    app_state.workspace_service.get_session(session_id).await?;

    let workspace_service = app_state.workspace_service.clone();
    let start_after_id = params.after_id.unwrap_or(0);

    let stream = async_stream::stream! {
        let mut last_id = start_after_id;
        let mut terminal = false;
        let mut idle_count = 0;

        loop {
            // Poll for new messages
            match workspace_service.get_messages_after(session_id, last_id).await {
                Ok(messages) => {
                    if messages.is_empty() {
                        idle_count += 1;
                    } else {
                        idle_count = 0;
                        for msg in messages {
                            last_id = msg.id;
                            let response = MessageResponse::from(msg);
                            let json = serde_json::to_string(&response)
                                .unwrap_or_else(|_| "{}".to_string());
                            yield Ok(Event::default().event("message").data(json));
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("SSE poll error for session {}: {}", session_id, e);
                    idle_count += 1;
                }
            }

            // Check if session is terminal
            if let Ok(session) = workspace_service.get_session(session_id).await {
                if session.status == "closed" {
                    yield Ok(Event::default().event("status").data("closed"));
                    terminal = true;
                }
            }

            if terminal {
                break;
            }

            // Close the stream if idle for too long (5 minutes of no activity)
            if idle_count > 600 {
                yield Ok(Event::default().event("status").data("idle_timeout"));
                break;
            }

            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    };

    Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))))
}

async fn update_session(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<WorkspaceAppState>>,
    Path((project_id, session_id)): Path<(i32, i32)>,
    Json(body): Json<UpdateSessionBody>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, ProjectsWrite);

    // Validate the timeout value if one was provided (not cleared).
    if let Some(Some(minutes)) = body.idle_timeout_minutes {
        // 0 means "disabled" (never reap) — allow it explicitly.
        if !(0..=10_080).contains(&minutes) {
            return Err(WorkspaceError::Validation {
                message:
                    "idle_timeout_minutes must be 0 (disabled) or between 1 and 10080 (7 days)"
                        .to_string(),
            }
            .into());
        }
    }
    if let Some(Some(cpu)) = body.cpu_limit {
        if !cpu.is_finite() || !(0.25..=16.0).contains(&cpu) {
            return Err(WorkspaceError::Validation {
                message: "cpu_limit must be between 0.25 and 16 vCPUs".to_string(),
            }
            .into());
        }
    }
    if let Some(Some(mem)) = body.memory_limit_mb {
        if !(256..=32_768).contains(&mem) {
            return Err(WorkspaceError::Validation {
                message: "memory_limit_mb must be between 256 and 32768 MB".to_string(),
            }
            .into());
        }
    }
    if let Some(Some(pids)) = body.pids_limit {
        if !(64..=8192).contains(&pids) {
            return Err(WorkspaceError::Validation {
                message: "pids_limit must be between 64 and 8192".to_string(),
            }
            .into());
        }
    }
    // Normalize title: trim, treat empty as clear, cap at 200 chars.
    let normalized_title = body.title.map(|opt| {
        opt.and_then(|s| {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.chars().take(200).collect::<String>())
            }
        })
    });

    let resource_changed =
        body.cpu_limit.is_some() || body.memory_limit_mb.is_some() || body.pids_limit.is_some();

    let updated = app_state
        .workspace_service
        .update_session(
            session_id,
            UpdateSessionFields {
                idle_timeout_minutes: body.idle_timeout_minutes,
                title: normalized_title,
                cpu_limit: body.cpu_limit,
                memory_limit_mb: body.memory_limit_mb,
                pids_limit: body.pids_limit,
                ..Default::default()
            },
        )
        .await?;

    // Propagate CPU / memory / pids limits to the live container so the
    // cgroup matches the DB. Without this, the UI shows the new limit but
    // the sandbox still runs under the original creation-time limit — which
    // is how Next.js was getting OOM-killed at the old 2GB ceiling even
    // after the user bumped memory to 6GB in the UI.
    //
    // This is a *required* step when the session has a live container:
    // if the docker update fails we return 500 so the DB and cgroup can
    // never silently diverge.
    if resource_changed {
        let container_id = match app_state.session_manager.get_handle(session_id).await {
            Some(h) => Some(h.sandbox_id),
            None => updated.sandbox_container_id.clone(),
        };
        if let Some(cid) = container_id {
            let docker = app_state.docker.clone().ok_or_else(|| {
                problemdetails::new(StatusCode::SERVICE_UNAVAILABLE)
                    .with_title("Docker Unavailable")
                    .with_detail(
                        "Cannot apply new resource limits: this Temps instance has no \
                         Docker-backed sandbox provider."
                            .to_string(),
                    )
            })?;

            // Docker wants: memory in bytes, nano_cpus = cores * 1e9,
            // pids_limit as-is. Keep memory_swap == memory so swap stays
            // disabled (matches the creation-time HostConfig in
            // temps-agents/src/sandbox/docker.rs).
            let memory = updated.memory_limit_mb.map(|mb| mb as i64 * 1024 * 1024);
            let nano_cpus = updated.cpu_milli.map(|milli| (milli as i64) * 1_000_000);
            let pids = updated.pids_limit.map(|p| p as i64);

            let update_body = bollard::models::ContainerUpdateBody {
                memory,
                memory_swap: memory,
                nano_cpus,
                pids_limit: pids,
                ..Default::default()
            };
            docker
                .update_container(&cid, update_body)
                .await
                .map_err(|e| {
                    problemdetails::new(StatusCode::INTERNAL_SERVER_ERROR)
                        .with_title("Container Update Failed")
                        .with_detail(format!(
                            "Failed to apply new resource limits to container {cid} for session {session_id}: {e}"
                        ))
                })?;
            tracing::info!(
                "Updated live container {} for session {}: memory={:?} MB, cpu_milli={:?}, pids={:?}",
                cid,
                session_id,
                updated.memory_limit_mb,
                updated.cpu_milli,
                updated.pids_limit
            );
        }
    }

    let preview_parts = preview_url_parts(&app_state).await;
    tracing::info!(
        "Workspace session {} (project {}) updated by user {}",
        session_id,
        project_id,
        auth.user_id()
    );
    Ok(Json(SessionResponse::from_model(updated, &preview_parts)))
}

async fn close_session(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<WorkspaceAppState>>,
    Path((_project_id, session_id)): Path<(i32, i32)>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, ProjectsWrite);

    // Close the DB session
    app_state
        .workspace_service
        .close_session(session_id)
        .await?;

    // Release the sandbox container — but keep the per-session home
    // volume alive so reopening the session restores claude auth, shell
    // history, and tmux state. The volume is only purged on *delete*.
    if let Err(e) = app_state.session_manager.release(session_id, false).await {
        tracing::warn!(
            "Failed to release sandbox for session {}: {}",
            session_id,
            e
        );
    }

    tracing::info!(
        "Workspace session {} closed by user {}",
        session_id,
        auth.user_id()
    );

    Ok(StatusCode::NO_CONTENT)
}

async fn delete_session(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<WorkspaceAppState>>,
    Path((project_id, session_id)): Path<(i32, i32)>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, ProjectsWrite);

    // 1. Cancel any in-flight run so the executor stops touching this session.
    if let Some(executor) = &app_state.message_executor {
        executor.cancel(session_id).await;
    }

    // 2. Adopt the existing sandbox container if it isn't tracked in memory
    //    (e.g. after a server restart). Without this, `release` would be a
    //    no-op and the container would leak. Best-effort: log and continue
    //    if recovery fails — we still want to delete the DB row.
    if let Err(e) = app_state
        .session_manager
        .adopt_existing(session_id, project_id)
        .await
    {
        tracing::warn!(
            "Failed to adopt sandbox before deleting session {}: {}",
            session_id,
            e
        );
    }

    // 3. Release (destroy) the sandbox container AND its home volume —
    //    the session row is about to be gone, so nothing should survive.
    if let Err(e) = app_state.session_manager.release(session_id, true).await {
        tracing::warn!(
            "Failed to release sandbox while deleting session {}: {}",
            session_id,
            e
        );
    }

    // 4. Hard-delete the session row. Cascades to workspace_messages.
    app_state
        .workspace_service
        .delete_session(session_id)
        .await?;

    tracing::info!(
        "Workspace session {} deleted by user {}",
        session_id,
        auth.user_id()
    );

    Ok(StatusCode::NO_CONTENT)
}

async fn reopen_session(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<WorkspaceAppState>>,
    Path((_project_id, session_id)): Path<(i32, i32)>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, ProjectsWrite);

    app_state
        .workspace_service
        .reopen_session(session_id)
        .await?;

    // Eagerly provision the sandbox so the terminal tab has a live
    // container to attach to. Lazy provisioning-on-first-chat-message
    // breaks the terminal-first UX: the user clicks Terminal and sees
    // "sandbox not started" instead of a claude prompt.
    if let Some(executor) = app_state.message_executor.clone() {
        if let Err(e) = executor.ensure_sandbox(session_id).await {
            tracing::warn!(
                "reopen_session: ensure_sandbox failed for session {}: {} — \
                 UI will fall back to lazy provisioning on first message",
                session_id,
                e
            );
        }
    }

    // Re-fetch so the response carries the freshly-populated
    // `sandbox_container_id` (initialize_sandbox persists it).
    let session = app_state.workspace_service.get_session(session_id).await?;

    tracing::info!(
        "Workspace session {} reopened by user {}",
        session_id,
        auth.user_id()
    );

    let preview_parts = preview_url_parts(&app_state).await;
    Ok((
        StatusCode::OK,
        Json(SessionResponse::from_model(session, &preview_parts)),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_tab_id_accepts_alphanumeric_and_dashes() {
        assert!(is_safe_tab_id("main"));
        assert!(is_safe_tab_id("tab-1"));
        assert!(is_safe_tab_id("my_tab"));
        assert!(is_safe_tab_id("abc123"));
    }

    #[test]
    fn safe_tab_id_rejects_empty_and_overlong() {
        assert!(!is_safe_tab_id(""));
        assert!(!is_safe_tab_id(&"a".repeat(33)));
    }

    #[test]
    fn safe_tab_id_rejects_special_chars() {
        assert!(!is_safe_tab_id("tab;rm -rf"));
        assert!(!is_safe_tab_id("../escape"));
        assert!(!is_safe_tab_id("tab id")); // space
        assert!(!is_safe_tab_id("tab\n"));
    }

    #[test]
    fn tmux_cli_for_codex() {
        assert_eq!(tmux_cli_for_provider("codex_cli"), "codex");
    }

    #[test]
    fn tmux_cli_defaults_to_claude() {
        assert_eq!(tmux_cli_for_provider("claude_cli"), "claude");
        assert_eq!(tmux_cli_for_provider("anything_else"), "claude");
    }

    /// Token-bucket rate limiter correctness test.
    /// Re-implements the exact bucket logic from `handle_session_terminal` to
    /// verify the maths without needing a real WebSocket connection.
    #[test]
    fn terminal_rate_limiter_bucket_math() {
        const RATE_BYTES_PER_SEC: u64 = 2 * 1024 * 1024;
        const BUCKET_CAPACITY: u64 = 8 * 1024 * 1024;

        let mut tokens: u64 = BUCKET_CAPACITY;
        let mut last_refill = std::time::Instant::now();

        let refill = |tokens: &mut u64, last: &mut std::time::Instant| {
            let now = std::time::Instant::now();
            let elapsed = now.duration_since(*last).as_secs_f64();
            if elapsed > 0.0 {
                let add = (elapsed * RATE_BYTES_PER_SEC as f64) as u64;
                *tokens = (*tokens).saturating_add(add).min(BUCKET_CAPACITY);
                *last = now;
            }
        };

        // Bucket starts full
        assert_eq!(tokens, BUCKET_CAPACITY);

        // Consume exactly the full bucket
        let big_chunk = BUCKET_CAPACITY;
        assert!(big_chunk <= tokens);
        tokens -= big_chunk;
        assert_eq!(tokens, 0);

        // Immediately after, another message should be rejected
        refill(&mut tokens, &mut last_refill);
        let _small_msg: u64 = 100;
        // Tokens may be ~0 (tiny elapsed time) — should fail
        // Allow up to a small buffer for timing jitter
        assert!(
            tokens < 1024,
            "bucket should be nearly empty right after drain, got {}",
            tokens
        );

        // After 1 second, ~2 MiB should refill
        std::thread::sleep(std::time::Duration::from_millis(1010));
        refill(&mut tokens, &mut last_refill);
        assert!(
            tokens >= RATE_BYTES_PER_SEC - 100_000, // allow 100KB jitter
            "after 1s, bucket should have ~2 MiB, got {}",
            tokens
        );
        assert!(
            tokens <= BUCKET_CAPACITY,
            "bucket should never exceed capacity"
        );
    }
}
