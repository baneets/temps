//! The Temps AI foundation (ADR-022).
//!
//! A single, governed, provider-agnostic way for any crate to ask the configured
//! model for either free text or **typed, structured data**. The object-safe
//! [`AiService`] trait is registered + resolved through the plugin DI as
//! `Arc<dyn AiService>` (like [`crate::notifications::NotificationService`]); the
//! generic [`complete_typed`]/[`complete_text`] helpers in [`typed`] add the
//! ergonomic, schema-derived API on top.
//!
//! Everything here is best-effort: the trait returns [`Result`], the helpers
//! return [`Option`], and AI must never sit on a path that can block or fail a
//! core operation — callers wrap calls in a timeout.

pub mod typed;

pub use typed::{complete_text, complete_typed, extract_json_block};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// A single AI completion request. Construct with `..Default::default()` and set
/// only what you need:
///
/// ```ignore
/// let req = AiRequest { purpose: "alert.summary".into(), prompt, ..Default::default() };
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AiRequest {
    /// Short tag for logging, usage attribution, and per-purpose budgets,
    /// e.g. `"alert.summary"` or `"deploy.risk"`.
    pub purpose: String,
    /// Optional governance + usage scope (per-project budgets / allow-lists).
    pub project_id: Option<i32>,
    /// Optional system instruction.
    pub system: Option<String>,
    /// The user prompt.
    pub prompt: String,
    /// Override the configured default model for this call.
    pub model: Option<String>,
    /// Cap on response tokens (provider default when `None`).
    pub max_tokens: Option<u32>,
    /// Sampling temperature (provider default when `None`).
    pub temperature: Option<f32>,
    /// When set, the provider is asked to return JSON matching this JSON Schema.
    /// Usually populated by [`complete_typed`] from a Rust type rather than by
    /// hand.
    pub response_schema: Option<serde_json::Value>,
}

/// The result of a completion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiResponse {
    /// The assistant's text reply.
    pub text: String,
    /// Parsed JSON, when a schema was requested and the reply parsed as JSON.
    pub json: Option<serde_json::Value>,
    /// The model that actually served the request.
    pub model: String,
}

/// Why an AI call could not be completed. All variants are non-fatal — callers
/// fall back to non-AI behaviour.
#[derive(Debug, thiserror::Error)]
pub enum AiError {
    /// No provider key / usable model is configured. Check [`AiService::is_available`]
    /// first to avoid building a prompt that can't be served.
    #[error("AI is not configured (no provider key or usable model)")]
    NotAvailable,
    /// No model could be resolved for this request.
    #[error("no model configured for AI request '{purpose}'")]
    NoModel { purpose: String },
    /// The provider/gateway returned an error.
    #[error("AI provider error for '{purpose}': {reason}")]
    Provider { purpose: String, reason: String },
}

/// The governed AI capability. Object-safe so it can be registered and resolved
/// as `Arc<dyn AiService>` through the plugin DI.
///
/// Implementations route through the AI gateway, inheriting provider-key
/// resolution, model routing, and per-scope rate/cost governance. They are
/// best-effort: never panic, and never block beyond the work itself (the caller
/// adds the timeout).
#[async_trait]
pub trait AiService: Send + Sync {
    /// Cheap gate: is a provider key + usable model actually configured? Lets a
    /// caller skip prompt construction when AI is unavailable.
    async fn is_available(&self) -> bool;

    /// Low-level completion. Prefer the [`complete_text`] / [`complete_typed`]
    /// helpers for everyday use.
    async fn complete(&self, request: AiRequest) -> Result<AiResponse, AiError>;
}
