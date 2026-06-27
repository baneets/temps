//! ADR-022: the gateway-backed implementation of the general [`AiService`]
//! foundation.
//!
//! Wraps [`GatewayService`] so every internal AI call inherits provider-key
//! resolution, model routing, and per-scope rate/cost governance. Structured
//! output rides the gateway's existing `response_format` plumbing. Best-effort:
//! returns [`AiError`] rather than panicking; callers add the timeout.

use std::sync::Arc;

use async_trait::async_trait;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use tracing::debug;

use temps_core::ai::{AiError, AiRequest, AiResponse, AiService};

use crate::services::{ByokOverride, GatewayService};
use crate::types::{ChatCompletionRequest, ChatCompletionResponse, MessageContent};

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
        None
    }
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
            temps_core::ai::extract_json_block(&text)
        } else {
            None
        };
        Ok(AiResponse {
            text: text.trim().to_string(),
            json,
            model,
        })
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
