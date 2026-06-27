//! Multi-turn, streaming chat types for the AI foundation (ADR-023).
//!
//! Where [`crate::AiService::complete`] is a single request→response,
//! [`crate::AiService::chat_stream`] takes a replayed message history and streams
//! the assistant's reply token-by-token — the substrate for persistent,
//! resumable debugging conversations.

use std::pin::Pin;

use futures::Stream;
use serde::{Deserialize, Serialize};

use crate::service::AiError;

/// Who authored a turn. Mirrors the OpenAI-compatible roles the gateway accepts.
pub const ROLE_SYSTEM: &str = "system";
pub const ROLE_USER: &str = "user";
pub const ROLE_ASSISTANT: &str = "assistant";

/// One turn of a conversation. Deliberately flat (`role` + text `content`) so it
/// is provider-agnostic and trivially persisted as an `ai_messages` row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    /// `"system"` | `"user"` | `"assistant"`.
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: ROLE_SYSTEM.into(),
            content: content.into(),
        }
    }
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: ROLE_USER.into(),
            content: content.into(),
        }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: ROLE_ASSISTANT.into(),
            content: content.into(),
        }
    }
}

/// A multi-turn streaming request. The caller supplies the *full* replayed
/// history (our DB is the source of truth — see ADR-023); the provider is
/// stateless.
#[derive(Debug, Clone, Default)]
pub struct ChatTurnRequest {
    /// Short tag for logging / usage attribution, e.g. `"deploy.debug_chat"`.
    pub purpose: String,
    /// Governance + usage scope.
    pub project_id: Option<i32>,
    /// Full conversation history, oldest first (system prompt usually first).
    pub messages: Vec<ChatMessage>,
    /// Override the configured default model.
    pub model: Option<String>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
}

/// A stream of assistant text deltas. Each `Ok(String)` is an incremental chunk
/// to append; the stream ends when the reply is complete. Errors are terminal.
pub type TokenStream = Pin<Box<dyn Stream<Item = Result<String, AiError>> + Send>>;
