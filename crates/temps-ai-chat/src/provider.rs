//! Per-context seeding for conversations.
//!
//! A `ConversationContextProvider` turns an entity reference (`context_type` +
//! `context_id`) into the AI framing for a new chat. Registered one per
//! `context_type`; the deployment provider (in `temps-deployments`) is the
//! first, seeding from a build/deploy failure diagnosis.

use async_trait::async_trait;

/// The seed for a new conversation.
#[derive(Debug, Clone, Default)]
pub struct ConversationSeed {
    /// System framing: the situation + relevant facts (logs, status). Stored as
    /// the conversation's first `system` message and replayed every turn.
    pub system: String,
    /// Optional first assistant turn shown on open (e.g. the rendered diagnosis),
    /// so the chat starts already explaining the problem.
    pub first_assistant: Option<String>,
    /// Display title for the conversation.
    pub title: Option<String>,
    /// Provenance refs (log_ids, status) recorded on the conversation row.
    pub metadata: Option<serde_json::Value>,
}

/// Builds the AI context for one kind of entity.
#[async_trait]
pub trait ConversationContextProvider: Send + Sync {
    /// The `context_type` this provider handles, e.g. `"deployment"`.
    fn context_type(&self) -> &'static str;

    /// Finer-grained authorization for this context (the route already enforces
    /// project-level access). Default allow.
    async fn authorize(&self, _project_id: i32, _context_id: &str) -> bool {
        true
    }

    /// Build the seed for a new conversation. `None` if the entity can't be found
    /// or has no usable context (e.g. a deployment that didn't fail).
    async fn seed(&self, project_id: i32, context_id: &str) -> Option<ConversationSeed>;
}
