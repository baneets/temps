//! HTTP surface for AI debugging conversations (ADR-023).
//!
//! `GET/POST /projects/{project_id}/ai/conversations` (find / get-or-create),
//! `GET .../{public_id}` (history), `POST .../{public_id}/messages` (SSE stream
//! of the assistant reply), `POST .../{public_id}/archive`. All gated on the
//! per-project `ai_debug_chat_enabled` toggle + AI being configured.

use std::convert::Infallible;
use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    response::sse::{Event, KeepAlive, Sse},
    routing::{get, post},
    Json, Router,
};
use futures::stream::Stream;
use futures_util::StreamExt;
use sea_orm::{DatabaseConnection, EntityTrait};
use serde::{Deserialize, Serialize};
use utoipa::{OpenApi, ToSchema};

use temps_auth::{permission_guard, RequireAuth};
use temps_core::problemdetails::{self, Problem};
use temps_entities::{ai_conversations, ai_messages};

use crate::{ChatError, ConversationService};

/// Shared state for the chat routes.
pub struct AppState {
    pub service: Arc<ConversationService>,
    pub db: Arc<DatabaseConnection>,
}

// --- DTOs --------------------------------------------------------------------

#[derive(Debug, Serialize, ToSchema)]
pub struct ConversationResponse {
    pub public_id: String,
    pub context_type: String,
    pub context_id: String,
    pub title: Option<String>,
    pub status: String,
    pub created_at: String,
    pub last_activity_at: String,
}

impl From<ai_conversations::Model> for ConversationResponse {
    fn from(m: ai_conversations::Model) -> Self {
        Self {
            public_id: m.public_id,
            context_type: m.context_type,
            context_id: m.context_id,
            title: m.title,
            status: m.status,
            created_at: m.created_at.to_rfc3339(),
            last_activity_at: m.last_activity_at.to_rfc3339(),
        }
    }
}

/// A conversation in the unified cross-project switcher: carries the project it
/// belongs to (name/slug) so the UI can show where the chat was started and
/// link back to the source.
#[derive(Debug, Serialize, ToSchema)]
pub struct GlobalConversationResponse {
    pub public_id: String,
    pub project_id: i32,
    pub project_name: Option<String>,
    pub project_slug: Option<String>,
    pub context_type: String,
    pub context_id: String,
    pub title: Option<String>,
    pub status: String,
    pub created_at: String,
    pub last_activity_at: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MessageResponse {
    pub role: String,
    pub content: String,
    pub created_at: String,
}

impl From<ai_messages::Model> for MessageResponse {
    fn from(m: ai_messages::Model) -> Self {
        Self {
            role: m.role,
            content: m.content,
            created_at: m.created_at.to_rfc3339(),
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ConversationDetailResponse {
    #[serde(flatten)]
    pub conversation: ConversationResponse,
    /// Turns oldest-first. The `system` seed message is omitted (internal).
    pub messages: Vec<MessageResponse>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateConversationRequest {
    /// e.g. `"deployment"`.
    pub context_type: String,
    /// The entity id (ints stringified).
    pub context_id: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct FindConversationQuery {
    pub context_type: String,
    pub context_id: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SendMessageRequest {
    pub content: String,
}

// --- error mapping -----------------------------------------------------------

impl From<ChatError> for Problem {
    fn from(e: ChatError) -> Self {
        match e {
            ChatError::NotFound(_) => problemdetails::new(axum::http::StatusCode::NOT_FOUND)
                .with_title("Conversation Not Found")
                .with_detail(e.to_string()),
            ChatError::NoProvider(_) | ChatError::ContextUnavailable => {
                problemdetails::new(axum::http::StatusCode::NOT_FOUND)
                    .with_title("Context Not Available")
                    .with_detail(e.to_string())
            }
            ChatError::AiUnavailable => problemdetails::new(axum::http::StatusCode::CONFLICT)
                .with_title("AI Not Configured")
                .with_detail(e.to_string()),
            ChatError::Db(_) | ChatError::Ai(_) => {
                problemdetails::new(axum::http::StatusCode::INTERNAL_SERVER_ERROR)
                    .with_title("Internal Server Error")
                    .with_detail(e.to_string())
            }
        }
    }
}

/// Gate: the project must have opted into AI debug chat AND AI must be configured.
async fn ensure_enabled(state: &AppState, project_id: i32) -> Result<(), Problem> {
    let project = temps_entities::projects::Entity::find_by_id(project_id)
        .one(state.db.as_ref())
        .await
        .map_err(|e| {
            problemdetails::new(axum::http::StatusCode::INTERNAL_SERVER_ERROR)
                .with_detail(e.to_string())
        })?;
    let enabled = matches!(project.and_then(|p| p.ai_debug_chat_enabled), Some(true));
    if !enabled {
        return Err(problemdetails::new(axum::http::StatusCode::FORBIDDEN)
            .with_title("AI Debug Chat Disabled")
            .with_detail("Enable AI debug chat for this project to use it."));
    }
    if !state.service.ai_available().await {
        return Err(problemdetails::new(axum::http::StatusCode::CONFLICT)
            .with_title("AI Not Configured")
            .with_detail("Configure an AI provider to use debugging chat."));
    }
    Ok(())
}

// --- handlers ----------------------------------------------------------------

/// Find the existing chat for a context (returns `null` if none yet).
#[utoipa::path(
    get, tag = "AI Chat",
    path = "/projects/{project_id}/ai/conversations",
    params(("project_id" = i32, Path,), ("context_type" = String, Query,), ("context_id" = String, Query,)),
    responses((status = 200, body = Option<ConversationResponse>), (status = 401), (status = 403)),
    security(("bearer_auth" = []))
)]
pub async fn find_conversation(
    RequireAuth(auth): RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(project_id): Path<i32>,
    Query(q): Query<FindConversationQuery>,
) -> Result<Json<Option<ConversationResponse>>, Problem> {
    permission_guard!(auth, ProjectsRead);
    let found = state
        .service
        .find_by_context(project_id, &q.context_type, &q.context_id)
        .await?;
    Ok(Json(found.map(ConversationResponse::from)))
}

/// List every active conversation across all projects, most-recently-active
/// first, annotated with project name/slug. Powers the unified "all chats"
/// switcher in the AI assistant dock.
#[utoipa::path(
    get, tag = "AI Chat",
    path = "/ai/conversations",
    responses((status = 200, body = Vec<GlobalConversationResponse>), (status = 401), (status = 403)),
    security(("bearer_auth" = []))
)]
pub async fn list_all_conversations(
    RequireAuth(auth): RequireAuth,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<GlobalConversationResponse>>, Problem> {
    permission_guard!(auth, ProjectsRead);
    let items = state.service.list_all_conversations().await?;
    Ok(Json(
        items
            .into_iter()
            .map(|i| GlobalConversationResponse {
                public_id: i.conversation.public_id,
                project_id: i.conversation.project_id,
                project_name: i.project_name,
                project_slug: i.project_slug,
                context_type: i.conversation.context_type,
                context_id: i.conversation.context_id,
                title: i.conversation.title,
                status: i.conversation.status,
                created_at: i.conversation.created_at.to_rfc3339(),
                last_activity_at: i.conversation.last_activity_at.to_rfc3339(),
            })
            .collect(),
    ))
}

/// List all active conversations for a project, most-recently-active first.
/// Powers the conversation switcher in the AI assistant sidebar.
#[utoipa::path(
    get, tag = "AI Chat",
    path = "/projects/{project_id}/ai/conversations/list",
    params(("project_id" = i32, Path,)),
    responses((status = 200, body = Vec<ConversationResponse>), (status = 401), (status = 403)),
    security(("bearer_auth" = []))
)]
pub async fn list_conversations(
    RequireAuth(auth): RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(project_id): Path<i32>,
) -> Result<Json<Vec<ConversationResponse>>, Problem> {
    permission_guard!(auth, ProjectsRead);
    let conversations = state.service.list_conversations(project_id).await?;
    Ok(Json(
        conversations
            .into_iter()
            .map(ConversationResponse::from)
            .collect(),
    ))
}

/// Get-or-create the chat for a context (seeds it on first open).
#[utoipa::path(
    post, tag = "AI Chat",
    path = "/projects/{project_id}/ai/conversations",
    params(("project_id" = i32, Path,)),
    request_body = CreateConversationRequest,
    responses((status = 200, body = ConversationResponse), (status = 401), (status = 403), (status = 404)),
    security(("bearer_auth" = []))
)]
pub async fn create_conversation(
    RequireAuth(auth): RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(project_id): Path<i32>,
    Json(req): Json<CreateConversationRequest>,
) -> Result<Json<ConversationResponse>, Problem> {
    permission_guard!(auth, ProjectsRead);
    ensure_enabled(&state, project_id).await?;
    let conv = state
        .service
        .get_or_create(
            project_id,
            &req.context_type,
            &req.context_id,
            Some(auth.user_id()),
        )
        .await?;
    Ok(Json(ConversationResponse::from(conv)))
}

/// Full conversation history (excluding the internal system seed).
#[utoipa::path(
    get, tag = "AI Chat",
    path = "/projects/{project_id}/ai/conversations/{public_id}",
    params(("project_id" = i32, Path,), ("public_id" = String, Path,)),
    responses((status = 200, body = ConversationDetailResponse), (status = 401), (status = 403), (status = 404)),
    security(("bearer_auth" = []))
)]
pub async fn get_conversation(
    RequireAuth(auth): RequireAuth,
    State(state): State<Arc<AppState>>,
    Path((project_id, public_id)): Path<(i32, String)>,
) -> Result<Json<ConversationDetailResponse>, Problem> {
    permission_guard!(auth, ProjectsRead);
    let conv = state
        .service
        .get_by_public_id(project_id, &public_id)
        .await?;
    let messages = state
        .service
        .messages(conv.id)
        .await?
        .into_iter()
        .filter(|m| m.role != "system")
        .map(MessageResponse::from)
        .collect();
    Ok(Json(ConversationDetailResponse {
        conversation: ConversationResponse::from(conv),
        messages,
    }))
}

/// Send a user message; stream the assistant reply as Server-Sent Events.
#[utoipa::path(
    post, tag = "AI Chat",
    path = "/projects/{project_id}/ai/conversations/{public_id}/messages",
    params(("project_id" = i32, Path,), ("public_id" = String, Path,)),
    request_body = SendMessageRequest,
    responses((status = 200, description = "SSE stream of assistant text deltas", content_type = "text/event-stream"), (status = 401), (status = 403), (status = 404)),
    security(("bearer_auth" = []))
)]
pub async fn send_message(
    RequireAuth(auth): RequireAuth,
    State(state): State<Arc<AppState>>,
    Path((project_id, public_id)): Path<(i32, String)>,
    Json(req): Json<SendMessageRequest>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, Problem> {
    permission_guard!(auth, ProjectsRead);
    ensure_enabled(&state, project_id).await?;
    let conv = state
        .service
        .get_by_public_id(project_id, &public_id)
        .await?;
    let token_stream = state.service.send_message(&conv, &req.content).await?;

    let sse = token_stream.map(|item| {
        let event = match item {
            Ok(tok) => Event::default().data(tok),
            Err(e) => Event::default().event("error").data(e.to_string()),
        };
        Ok::<_, Infallible>(event)
    });
    Ok(Sse::new(sse).keep_alive(KeepAlive::default()))
}

/// Archive (soft-delete) a conversation.
#[utoipa::path(
    post, tag = "AI Chat",
    path = "/projects/{project_id}/ai/conversations/{public_id}/archive",
    params(("project_id" = i32, Path,), ("public_id" = String, Path,)),
    responses((status = 204), (status = 401), (status = 403), (status = 404)),
    security(("bearer_auth" = []))
)]
pub async fn archive_conversation(
    RequireAuth(auth): RequireAuth,
    State(state): State<Arc<AppState>>,
    Path((project_id, public_id)): Path<(i32, String)>,
) -> Result<axum::http::StatusCode, Problem> {
    permission_guard!(auth, ProjectsWrite);
    let conv = state
        .service
        .get_by_public_id(project_id, &public_id)
        .await?;
    state.service.archive(&conv).await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

pub fn configure_routes() -> Router<Arc<AppState>> {
    Router::new()
        // Unified cross-project switcher.
        .route("/ai/conversations", get(list_all_conversations))
        .route(
            "/projects/{project_id}/ai/conversations",
            get(find_conversation).post(create_conversation),
        )
        // Static `/list` registered before the `{public_id}` param route; matchit
        // prioritizes the literal segment so it can't be shadowed.
        .route(
            "/projects/{project_id}/ai/conversations/list",
            get(list_conversations),
        )
        .route(
            "/projects/{project_id}/ai/conversations/{public_id}",
            get(get_conversation),
        )
        .route(
            "/projects/{project_id}/ai/conversations/{public_id}/messages",
            post(send_message),
        )
        .route(
            "/projects/{project_id}/ai/conversations/{public_id}/archive",
            post(archive_conversation),
        )
}

#[derive(OpenApi)]
#[openapi(
    paths(
        find_conversation,
        list_conversations,
        list_all_conversations,
        create_conversation,
        get_conversation,
        send_message,
        archive_conversation,
    ),
    components(schemas(
        ConversationResponse,
        GlobalConversationResponse,
        MessageResponse,
        ConversationDetailResponse,
        CreateConversationRequest,
        SendMessageRequest,
    ))
)]
pub struct AiChatApiDoc;
