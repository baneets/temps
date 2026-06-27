//! ADR-022: the gateway-backed implementation of the general [`AiService`]
//! foundation.
//!
//! Wraps [`GatewayService`] so every internal AI call inherits provider-key
//! resolution, model routing, and per-scope rate/cost governance. Structured
//! output rides the gateway's existing `response_format` plumbing. Best-effort:
//! returns [`AiError`] rather than panicking; callers add the timeout.

use std::sync::Arc;

use async_trait::async_trait;
use futures_util::StreamExt;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use tracing::debug;

use temps_ai::{AiError, AiRequest, AiResponse, AiService, ChatTurnRequest, TokenStream};

use crate::services::{ByokOverride, GatewayService};
use crate::types::{
    ChatCompletionChunk, ChatCompletionRequest, ChatCompletionResponse, MessageContent,
};

/// Gateway-backed [`AiService`] (ADR-022).
pub struct GatewayAiService {
    gateway: Arc<GatewayService>,
    db: Arc<DatabaseConnection>,
}

impl GatewayAiService {
    pub fn new(gateway: Arc<GatewayService>, db: Arc<DatabaseConnection>) -> Self {
        Self { gateway, db }
    }

    /// Resolve the model to use: an explicit per-call `model`, else the first
    /// entry of `allowed_models` for a `project:{id}` config if present, else the
    /// instance-scope config. `None` when nothing names a concrete model.
    async fn resolve_model(
        &self,
        project_id: Option<i32>,
        explicit: Option<&str>,
    ) -> Option<String> {
        if let Some(m) = explicit {
            if !m.is_empty() {
                return Some(m.to_string());
            }
        }
        let mut scopes: Vec<String> = Vec::new();
        if let Some(pid) = project_id {
            scopes.push(format!("project:{pid}"));
        }
        scopes.push("instance".to_string());

        let rows = temps_entities::ai_gateway_config::Entity::find()
            .filter(temps_entities::ai_gateway_config::Column::Scope.is_in(scopes.clone()))
            .all(self.db.as_ref())
            .await
            .ok()?;
        for scope in scopes {
            if let Some(row) = rows.iter().find(|r| r.scope == scope) {
                if let Some(model) = first_model(row.allowed_models.as_ref()) {
                    return Some(model);
                }
            }
        }

        // Fallback: no allow-list configured (the common case — there is no UI for
        // it). Use a sensible default model for the first active provider key, so
        // AI works as soon as a key is added, with no separate allow-list step.
        let key = temps_entities::ai_provider_keys::Entity::find()
            .filter(temps_entities::ai_provider_keys::Column::IsActive.eq(true))
            .one(self.db.as_ref())
            .await
            .ok()??;
        default_model_for_provider(&key.provider)
    }
}

/// A safe, low-cost default chat model per provider, used when no allow-list
/// names one. The prefix routes back to the provider via `route_model_to_provider`.
fn default_model_for_provider(provider: &str) -> Option<String> {
    let model = match provider {
        "openai" => "gpt-4o-mini",
        "anthropic" => "claude-3-5-haiku-latest",
        "gemini" => "gemini-1.5-flash",
        "xai" => "grok-2-latest",
        _ => return None,
    };
    Some(model.to_string())
}

/// First model id from an `allowed_models` JSON array (`["a","b"]` -> "a").
/// `None`/non-array/empty -> `None` (NULL means "all allowed", which names no
/// specific model to use for internal calls).
fn first_model(allowed_models: Option<&serde_json::Value>) -> Option<String> {
    allowed_models?
        .as_array()?
        .iter()
        .find_map(|v| v.as_str())
        .map(str::to_string)
}

/// Build the OpenAI-style `response_format` for a requested JSON Schema. Gemini
/// maps this to JSON mode; OpenAI-compatible providers honour `json_schema`.
fn response_format_for(schema: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "type": "json_schema",
        "json_schema": { "name": "temps_response", "schema": schema, "strict": true }
    })
}

/// Pull the assistant text out of the first choice.
fn first_text(resp: &ChatCompletionResponse) -> Option<String> {
    let choice = resp.choices.first()?;
    match choice.message.content.as_ref()? {
        MessageContent::Text(s) => Some(s.clone()),
        MessageContent::Parts(_) => None,
    }
}

#[async_trait]
impl AiService for GatewayAiService {
    async fn is_available(&self) -> bool {
        self.resolve_model(None, None).await.is_some()
    }

    async fn complete(&self, request: AiRequest) -> Result<AiResponse, AiError> {
        let model = self
            .resolve_model(request.project_id, request.model.as_deref())
            .await
            .ok_or_else(|| AiError::NoModel {
                purpose: request.purpose.clone(),
            })?;

        let mut messages = Vec::new();
        if let Some(system) = &request.system {
            messages.push(serde_json::json!({"role": "system", "content": system}));
        }
        messages.push(serde_json::json!({"role": "user", "content": request.prompt}));

        let mut body = serde_json::json!({
            "model": model,
            "messages": messages,
        });
        if let Some(mt) = request.max_tokens {
            body["max_tokens"] = serde_json::json!(mt);
        }
        if let Some(t) = request.temperature {
            body["temperature"] = serde_json::json!(t);
        }
        let wants_json = request.response_schema.is_some();
        if let Some(schema) = &request.response_schema {
            body["response_format"] = response_format_for(schema);
        }

        let chat_req: ChatCompletionRequest =
            serde_json::from_value(body).map_err(|e| AiError::Provider {
                purpose: request.purpose.clone(),
                reason: format!("malformed request: {e}"),
            })?;

        let (resp, _cred) = self
            .gateway
            .chat_completion(&chat_req, &ByokOverride::default())
            .await
            .map_err(|e| {
                debug!(error = %e, purpose = request.purpose, model, "AI completion failed");
                AiError::Provider {
                    purpose: request.purpose.clone(),
                    reason: e.to_string(),
                }
            })?;

        let text = first_text(&resp).unwrap_or_default();
        let json = if wants_json {
            temps_ai::extract_json_block(&text)
        } else {
            None
        };
        Ok(AiResponse {
            text: text.trim().to_string(),
            json,
            model,
        })
    }

    async fn chat_stream(&self, request: ChatTurnRequest) -> Result<TokenStream, AiError> {
        let model = self
            .resolve_model(request.project_id, request.model.as_deref())
            .await
            .ok_or_else(|| AiError::NoModel {
                purpose: request.purpose.clone(),
            })?;

        let messages: Vec<serde_json::Value> = request
            .messages
            .iter()
            .map(|m| serde_json::json!({ "role": m.role, "content": m.content }))
            .collect();
        let mut body = serde_json::json!({ "model": model, "messages": messages, "stream": true });
        if let Some(mt) = request.max_tokens {
            body["max_tokens"] = serde_json::json!(mt);
        }
        if let Some(t) = request.temperature {
            body["temperature"] = serde_json::json!(t);
        }
        let chat_req: ChatCompletionRequest =
            serde_json::from_value(body).map_err(|e| AiError::Provider {
                purpose: request.purpose.clone(),
                reason: format!("malformed request: {e}"),
            })?;

        let purpose = request.purpose.clone();
        let (byte_stream, _cred) = self
            .gateway
            .chat_completion_stream(&chat_req, &ByokOverride::default())
            .await
            .map_err(|e| AiError::Provider {
                purpose: purpose.clone(),
                reason: e.to_string(),
            })?;

        // Parse the gateway's OpenAI-format SSE byte stream into assistant text
        // deltas. `data:` lines may split across byte chunks, so buffer to line
        // boundaries; `data: [DONE]` terminates.
        let token_stream = async_stream::stream! {
            let mut byte_stream = byte_stream;
            let mut buf = String::new();
            while let Some(item) = byte_stream.next().await {
                match item {
                    Ok(bytes) => {
                        buf.push_str(&String::from_utf8_lossy(&bytes));
                        while let Some(nl) = buf.find('\n') {
                            let line: String = buf.drain(..=nl).collect();
                            let line = line.trim();
                            let Some(data) = line.strip_prefix("data:") else { continue };
                            let data = data.trim();
                            if data == "[DONE]" {
                                return;
                            }
                            if let Ok(chunk) = serde_json::from_str::<ChatCompletionChunk>(data) {
                                if let Some(content) = chunk
                                    .choices
                                    .first()
                                    .and_then(|c| c.delta.content.as_ref())
                                {
                                    if !content.is_empty() {
                                        yield Ok(content.clone());
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        yield Err(AiError::Provider {
                            purpose: purpose.clone(),
                            reason: e.to_string(),
                        });
                        return;
                    }
                }
            }
        };
        Ok(Box::pin(token_stream))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_first_model() {
        assert_eq!(
            first_model(Some(&serde_json::json!(["gpt-4o-mini", "x"]))),
            Some("gpt-4o-mini".to_string())
        );
        assert_eq!(first_model(Some(&serde_json::json!([]))), None);
        assert_eq!(first_model(None), None);
    }

    #[test]
    fn test_response_format_for() {
        let rf = response_format_for(&serde_json::json!({"type": "object"}));
        assert_eq!(rf["type"], "json_schema");
        assert_eq!(rf["json_schema"]["schema"]["type"], "object");
        assert_eq!(rf["json_schema"]["strict"], true);
    }
}
