//! The conversation service: create/find/history + streaming `send_message`.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

use chrono::Utc;
use futures::Stream;
use futures_util::StreamExt;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
};

use temps_ai::{AiService, ChatMessage, ChatTool, ChatTurnRequest};
use temps_entities::{ai_conversations, ai_messages};

use crate::provider::ConversationContextProvider;
use crate::ChatError;

/// A conversation plus its project's display info, for the unified switcher.
pub struct ConversationWithProject {
    pub conversation: ai_conversations::Model,
    pub project_name: Option<String>,
    pub project_slug: Option<String>,
}

/// Owns conversation persistence + AI turn streaming. Construct once with the
/// registered context providers; resolve via the plugin DI.
pub struct ConversationService {
    db: Arc<DatabaseConnection>,
    ai: Arc<dyn AiService>,
    providers: HashMap<&'static str, Arc<dyn ConversationContextProvider>>,
}

impl ConversationService {
    pub fn new(
        db: Arc<DatabaseConnection>,
        ai: Arc<dyn AiService>,
        providers: Vec<Arc<dyn ConversationContextProvider>>,
    ) -> Self {
        let providers = providers
            .into_iter()
            .map(|p| (p.context_type(), p))
            .collect();
        Self { db, ai, providers }
    }

    /// Is AI configured at all? (Capability gate; feature opt-in is checked at the handler.)
    pub async fn ai_available(&self) -> bool {
        self.ai.is_available().await
    }

    /// The active conversation for a context, if one exists.
    pub async fn find_by_context(
        &self,
        project_id: i32,
        context_type: &str,
        context_id: &str,
    ) -> Result<Option<ai_conversations::Model>, ChatError> {
        Ok(ai_conversations::Entity::find()
            .filter(ai_conversations::Column::ProjectId.eq(project_id))
            .filter(ai_conversations::Column::ContextType.eq(context_type))
            .filter(ai_conversations::Column::ContextId.eq(context_id))
            .filter(ai_conversations::Column::Status.eq("active"))
            .one(self.db.as_ref())
            .await?)
    }

    /// All active conversations for a project, most-recently-active first. Powers
    /// the conversation switcher.
    pub async fn list_conversations(
        &self,
        project_id: i32,
    ) -> Result<Vec<ai_conversations::Model>, ChatError> {
        Ok(ai_conversations::Entity::find()
            .filter(ai_conversations::Column::ProjectId.eq(project_id))
            .filter(ai_conversations::Column::Status.eq("active"))
            .order_by_desc(ai_conversations::Column::LastActivityAt)
            .all(self.db.as_ref())
            .await?)
    }

    /// All active conversations across every project, most-recently-active
    /// first, each annotated with its project's name/slug so the UI can show
    /// where the chat was started and link back to it. Powers the unified
    /// "all chats" switcher.
    pub async fn list_all_conversations(&self) -> Result<Vec<ConversationWithProject>, ChatError> {
        let convs = ai_conversations::Entity::find()
            .filter(ai_conversations::Column::Status.eq("active"))
            .order_by_desc(ai_conversations::Column::LastActivityAt)
            .all(self.db.as_ref())
            .await?;

        let mut ids: Vec<i32> = convs.iter().map(|c| c.project_id).collect();
        ids.sort_unstable();
        ids.dedup();
        let projects = if ids.is_empty() {
            Vec::new()
        } else {
            temps_entities::projects::Entity::find()
                .filter(temps_entities::projects::Column::Id.is_in(ids))
                .all(self.db.as_ref())
                .await?
        };
        let by_id: HashMap<i32, (String, String)> = projects
            .into_iter()
            .map(|p| (p.id, (p.name, p.slug)))
            .collect();

        Ok(convs
            .into_iter()
            .map(|c| {
                let info = by_id.get(&c.project_id).cloned();
                ConversationWithProject {
                    project_name: info.as_ref().map(|x| x.0.clone()),
                    project_slug: info.map(|x| x.1),
                    conversation: c,
                }
            })
            .collect())
    }

    /// A conversation by its public id, scoped to the project.
    pub async fn get_by_public_id(
        &self,
        project_id: i32,
        public_id: &str,
    ) -> Result<ai_conversations::Model, ChatError> {
        ai_conversations::Entity::find()
            .filter(ai_conversations::Column::ProjectId.eq(project_id))
            .filter(ai_conversations::Column::PublicId.eq(public_id))
            .one(self.db.as_ref())
            .await?
            .ok_or_else(|| ChatError::NotFound(public_id.to_string()))
    }

    /// All turns of a conversation, oldest first.
    pub async fn messages(
        &self,
        conversation_id: i64,
    ) -> Result<Vec<ai_messages::Model>, ChatError> {
        Ok(ai_messages::Entity::find()
            .filter(ai_messages::Column::ConversationId.eq(conversation_id))
            .order_by_asc(ai_messages::Column::CreatedAt)
            .order_by_asc(ai_messages::Column::Id)
            .all(self.db.as_ref())
            .await?)
    }

    /// Find or create the conversation for a context (idempotent per active
    /// context). On create, seeds via the provider: a `system` framing message
    /// plus an optional first `assistant` message (e.g. the diagnosis).
    pub async fn get_or_create(
        &self,
        project_id: i32,
        context_type: &str,
        context_id: &str,
        user_id: Option<i32>,
    ) -> Result<ai_conversations::Model, ChatError> {
        if let Some(existing) = self
            .find_by_context(project_id, context_type, context_id)
            .await?
        {
            return Ok(existing);
        }
        let provider = self
            .providers
            .get(context_type)
            .ok_or_else(|| ChatError::NoProvider(context_type.to_string()))?;
        if !provider.authorize(project_id, context_id).await {
            return Err(ChatError::ContextUnavailable);
        }
        let seed = provider
            .seed(project_id, context_id)
            .await
            .ok_or(ChatError::ContextUnavailable)?;

        let now = Utc::now();
        let conv = ai_conversations::ActiveModel {
            public_id: Set(uuid::Uuid::new_v4().simple().to_string()),
            project_id: Set(project_id),
            context_type: Set(context_type.to_string()),
            context_id: Set(context_id.to_string()),
            title: Set(seed.title.clone()),
            status: Set("active".to_string()),
            created_by: Set(user_id),
            metadata: Set(seed.metadata.clone()),
            created_at: Set(now),
            last_activity_at: Set(now),
            ..Default::default()
        }
        .insert(self.db.as_ref())
        .await?;

        self.insert_message(conv.id, "system", &seed.system, None)
            .await?;
        if let Some(first) = &seed.first_assistant {
            self.insert_message(conv.id, "assistant", first, None)
                .await?;
        }
        Ok(conv)
    }

    async fn insert_message(
        &self,
        conversation_id: i64,
        role: &str,
        content: &str,
        metadata: Option<serde_json::Value>,
    ) -> Result<ai_messages::Model, ChatError> {
        Ok(ai_messages::ActiveModel {
            conversation_id: Set(conversation_id),
            role: Set(role.to_string()),
            content: Set(content.to_string()),
            metadata: Set(metadata),
            created_at: Set(Utc::now()),
            ..Default::default()
        }
        .insert(self.db.as_ref())
        .await?)
    }

    async fn touch(&self, conversation_id: i64) {
        let am = ai_conversations::ActiveModel {
            id: Set(conversation_id),
            last_activity_at: Set(Utc::now()),
            ..Default::default()
        };
        let _ = am.update(self.db.as_ref()).await;
    }

    /// Append a user message and stream the assistant reply. Persists the user
    /// message up front and the assistant message when the stream completes
    /// (the `system` seed is already the first stored turn, so history replay is
    /// the full context). Errors before streaming starts return `Err`; errors
    /// mid-stream arrive as a stream item.
    pub async fn send_message(
        &self,
        conv: &ai_conversations::Model,
        user_text: &str,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<String, ChatError>> + Send>>, ChatError> {
        if !self.ai.is_available().await {
            return Err(ChatError::AiUnavailable);
        }
        self.insert_message(conv.id, "user", user_text, None)
            .await?;
        self.touch(conv.id).await;

        let history = self.messages(conv.id).await?;
        let mut messages: Vec<ChatMessage> = history
            .iter()
            .filter(|m| matches!(m.role.as_str(), "system" | "user" | "assistant"))
            .map(|m| ChatMessage {
                role: m.role.clone(),
                content: m.content.clone(),
                ..Default::default()
            })
            .collect();

        // Refresh the system framing with the provider's CURRENT context (logs,
        // job failures, live status) on every turn, so the model always reasons
        // over up-to-date evidence — not the snapshot captured when the chat was
        // first created (which may predate the logs entirely). Best-effort: if
        // the provider can no longer build context, keep the stored system seed.
        if let Some(provider) = self.providers.get(conv.context_type.as_str()) {
            if let Some(seed) = provider.seed(conv.project_id, &conv.context_id).await {
                match messages.iter_mut().find(|m| m.role == "system") {
                    Some(sys) => sys.content = seed.system,
                    None => messages.insert(0, ChatMessage::system(seed.system)),
                }
            }
        }

        // Agentic tool path: when this context exposes tools (e.g. a git-backed
        // deployment can read repo files), run a non-streaming tool loop and
        // return the final answer. Falls back to plain streaming if the model
        // can't do tools or the loop yields nothing.
        if let Some(provider) = self.providers.get(conv.context_type.as_str()).cloned() {
            let tools = provider.tools(conv.project_id, &conv.context_id).await;
            if !tools.is_empty() {
                if let Some(stream) = self.try_tool_loop(conv, &messages, &provider, tools).await {
                    return Ok(stream);
                }
            }
        }

        let req = ChatTurnRequest {
            purpose: format!("chat.{}", conv.context_type),
            project_id: Some(conv.project_id),
            messages,
            ..Default::default()
        };
        let mut token_stream = self
            .ai
            .chat_stream(req)
            .await
            .map_err(|e| ChatError::Ai(e.to_string()))?;

        let db = self.db.clone();
        let conv_id = conv.id;
        let out = async_stream::stream! {
            let mut acc = String::new();
            while let Some(item) = token_stream.next().await {
                match item {
                    Ok(tok) => {
                        acc.push_str(&tok);
                        yield Ok(tok);
                    }
                    Err(e) => {
                        yield Err(ChatError::Ai(e.to_string()));
                        break;
                    }
                }
            }
            // Persist the assistant turn once the reply is complete.
            if !acc.is_empty() {
                let am = ai_messages::ActiveModel {
                    conversation_id: Set(conv_id),
                    role: Set("assistant".to_string()),
                    content: Set(acc),
                    created_at: Set(Utc::now()),
                    ..Default::default()
                };
                let _ = am.insert(db.as_ref()).await;
            }
        };
        Ok(Box::pin(out))
    }

    /// One agentic tool loop: call the model with `tools`, execute any tool
    /// calls it makes via the provider, feed results back, and repeat until it
    /// answers in prose (or a round cap is hit). Returns a single-shot stream of
    /// the final answer (and persists it). `None` when the model can't do tools
    /// or never settled on an answer — the caller then falls back to plain
    /// streaming. Tool turns are intentionally *not* persisted: only the final
    /// user→assistant exchange is stored, so the next turn re-derives context.
    async fn try_tool_loop(
        &self,
        conv: &ai_conversations::Model,
        base_messages: &[ChatMessage],
        provider: &Arc<dyn ConversationContextProvider>,
        tools: Vec<ChatTool>,
    ) -> Option<Pin<Box<dyn Stream<Item = Result<String, ChatError>> + Send>>> {
        const MAX_ROUNDS: usize = 6;
        let mut messages = base_messages.to_vec();
        let mut final_text: Option<String> = None;

        for _ in 0..MAX_ROUNDS {
            let req = ChatTurnRequest {
                purpose: format!("chat.{}.tools", conv.context_type),
                project_id: Some(conv.project_id),
                messages: messages.clone(),
                tools: tools.clone(),
                ..Default::default()
            };
            // An error here (e.g. the model/provider can't do tools) aborts the
            // loop; the caller falls back to plain streaming.
            let resp = self.ai.chat(req).await.ok()?;
            if resp.tool_calls.is_empty() {
                final_text = resp.content;
                break;
            }
            messages.push(ChatMessage {
                role: "assistant".to_string(),
                content: resp.content.clone().unwrap_or_default(),
                tool_calls: Some(resp.tool_calls.clone()),
                tool_call_id: None,
            });
            for tc in &resp.tool_calls {
                let result = provider
                    .execute_tool(conv.project_id, &conv.context_id, &tc.name, &tc.arguments)
                    .await;
                messages.push(ChatMessage::tool(tc.id.clone(), result));
            }
        }

        let text = final_text.filter(|t| !t.is_empty())?;
        let db = self.db.clone();
        let conv_id = conv.id;
        let out = async_stream::stream! {
            yield Ok(text.clone());
            let am = ai_messages::ActiveModel {
                conversation_id: Set(conv_id),
                role: Set("assistant".to_string()),
                content: Set(text),
                created_at: Set(Utc::now()),
                ..Default::default()
            };
            let _ = am.insert(db.as_ref()).await;
        };
        Some(Box::pin(out))
    }

    /// Archive a conversation (soft delete).
    pub async fn archive(&self, conv: &ai_conversations::Model) -> Result<(), ChatError> {
        let am = ai_conversations::ActiveModel {
            id: Set(conv.id),
            status: Set("archived".to_string()),
            ..Default::default()
        };
        am.update(self.db.as_ref()).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Mutex;

    use async_trait::async_trait;
    use sea_orm::{DatabaseBackend, MockDatabase};

    use temps_ai::{AiError, AiRequest, AiResponse, ChatTurnResponse, TokenStream, ToolCall};

    /// A scripted `AiService`: each `chat()` call pops the next queued response
    /// (or error) so a test can drive the tool loop turn-by-turn, while counting
    /// how many times `chat()` was invoked.
    struct ScriptedAi {
        /// Front-to-back queue of responses for successive `chat()` calls.
        responses: Mutex<std::collections::VecDeque<Result<ChatTurnResponse, AiError>>>,
        chat_calls: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl ScriptedAi {
        fn new(responses: Vec<Result<ChatTurnResponse, AiError>>) -> Self {
            Self {
                responses: Mutex::new(responses.into_iter().collect()),
                chat_calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            }
        }
    }

    #[async_trait]
    impl AiService for ScriptedAi {
        async fn is_available(&self) -> bool {
            true
        }
        async fn complete(&self, _request: AiRequest) -> Result<AiResponse, AiError> {
            Err(AiError::NotAvailable)
        }
        async fn chat_stream(&self, _request: ChatTurnRequest) -> Result<TokenStream, AiError> {
            Err(AiError::NotAvailable)
        }
        async fn chat(&self, _request: ChatTurnRequest) -> Result<ChatTurnResponse, AiError> {
            self.chat_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            // When the script is exhausted, keep requesting tool calls so a
            // misbehaving loop would run forever — letting MAX_ROUNDS assert.
            self.responses
                .lock()
                .expect("scripted-ai lock")
                .pop_front()
                .unwrap_or_else(|| {
                    Ok(ChatTurnResponse {
                        content: None,
                        tool_calls: vec![ToolCall {
                            id: "loop".to_string(),
                            name: "echo".to_string(),
                            arguments: "{}".to_string(),
                        }],
                    })
                })
        }
    }

    /// A stub provider exposing a single `echo` tool, counting executions.
    struct StubProvider {
        tool_calls: Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait]
    impl ConversationContextProvider for StubProvider {
        fn context_type(&self) -> &'static str {
            "test"
        }
        async fn seed(
            &self,
            _project_id: i32,
            _context_id: &str,
        ) -> Option<crate::provider::ConversationSeed> {
            None
        }
        async fn tools(&self, _project_id: i32, _context_id: &str) -> Vec<ChatTool> {
            vec![ChatTool {
                name: "echo".to_string(),
                description: "Echoes its input.".to_string(),
                parameters: serde_json::json!({"type": "object", "properties": {}}),
            }]
        }
        async fn execute_tool(
            &self,
            _project_id: i32,
            _context_id: &str,
            _name: &str,
            _arguments: &str,
        ) -> String {
            self.tool_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            "tool result".to_string()
        }
    }

    fn test_conversation() -> ai_conversations::Model {
        let now = Utc::now();
        ai_conversations::Model {
            id: 1,
            public_id: "pub1".to_string(),
            project_id: 7,
            context_type: "test".to_string(),
            context_id: "42".to_string(),
            title: None,
            status: "active".to_string(),
            created_by: None,
            metadata: None,
            created_at: now,
            last_activity_at: now,
        }
    }

    fn assistant_msg_model() -> ai_messages::Model {
        ai_messages::Model {
            id: 1,
            conversation_id: 1,
            role: "assistant".to_string(),
            content: "final answer".to_string(),
            metadata: None,
            tokens_in: None,
            tokens_out: None,
            cost_microcents: None,
            created_at: Utc::now(),
        }
    }

    /// Build a service whose only DB interaction (the final assistant insert) is
    /// satisfied by one mocked query result, plus the `echo` tool list to drive
    /// the loop. The provider is passed directly to `try_tool_loop` per test.
    fn service_with(ai: Arc<ScriptedAi>) -> (ConversationService, Vec<ChatTool>) {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![vec![assistant_msg_model()]])
            .into_connection();
        let tools = vec![ChatTool {
            name: "echo".to_string(),
            description: "Echoes its input.".to_string(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        }];
        let svc = ConversationService {
            db: Arc::new(db),
            ai,
            providers: HashMap::new(),
        };
        (svc, tools)
    }

    async fn drain(
        stream: Pin<Box<dyn Stream<Item = Result<String, ChatError>> + Send>>,
    ) -> Vec<String> {
        let mut s = stream;
        let mut out = Vec::new();
        while let Some(item) = s.next().await {
            if let Ok(tok) = item {
                out.push(tok);
            }
        }
        out
    }

    // (a) model calls a tool, then returns prose -> tool executed, final text
    // streamed + persisted.
    #[tokio::test]
    async fn test_tool_loop_executes_tool_then_returns_prose() {
        let ai = Arc::new(ScriptedAi::new(vec![
            // Round 1: request a tool call.
            Ok(ChatTurnResponse {
                content: None,
                tool_calls: vec![ToolCall {
                    id: "c1".to_string(),
                    name: "echo".to_string(),
                    arguments: "{}".to_string(),
                }],
            }),
            // Round 2: settle on prose.
            Ok(ChatTurnResponse {
                content: Some("final answer".to_string()),
                tool_calls: vec![],
            }),
        ]));
        let provider = Arc::new(StubProvider {
            tool_calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        });
        let tool_count = provider.tool_calls.clone();
        let chat_count = ai.chat_calls.clone();
        let (svc, tools) = service_with(ai);

        let conv = test_conversation();
        let provider_dyn: Arc<dyn ConversationContextProvider> = provider;
        let stream = svc
            .try_tool_loop(&conv, &[], &provider_dyn, tools)
            .await
            .expect("loop should produce a final answer stream");
        let out = drain(stream).await;

        assert_eq!(out, vec!["final answer".to_string()]);
        assert_eq!(tool_count.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(chat_count.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    // (b) chat() errors on round 1 -> returns None (caller falls back).
    #[tokio::test]
    async fn test_tool_loop_chat_error_returns_none() {
        let ai = Arc::new(ScriptedAi::new(vec![Err(AiError::Provider {
            purpose: "chat.test.tools".to_string(),
            reason: "boom".to_string(),
        })]));
        let provider = Arc::new(StubProvider {
            tool_calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        });
        let (svc, tools) = service_with(ai);

        let conv = test_conversation();
        let provider_dyn: Arc<dyn ConversationContextProvider> = provider;
        let result = svc.try_tool_loop(&conv, &[], &provider_dyn, tools).await;
        assert!(result.is_none());
    }

    // (c) MAX_ROUNDS enforced: a model that always asks for a tool must never
    // exceed 6 chat() calls, and the loop yields None (no final prose).
    #[tokio::test]
    async fn test_tool_loop_enforces_max_rounds() {
        // Empty script: the fallback in ScriptedAi::chat always returns a tool
        // call, so the loop would spin forever without the round cap.
        let ai = Arc::new(ScriptedAi::new(vec![]));
        let provider = Arc::new(StubProvider {
            tool_calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        });
        let chat_count = ai.chat_calls.clone();
        let (svc, tools) = service_with(ai);

        let conv = test_conversation();
        let provider_dyn: Arc<dyn ConversationContextProvider> = provider;
        let result = svc.try_tool_loop(&conv, &[], &provider_dyn, tools).await;

        assert!(result.is_none(), "no final prose -> None");
        assert!(
            chat_count.load(std::sync::atomic::Ordering::SeqCst) <= 6,
            "chat() must not be called more than MAX_ROUNDS (6) times"
        );
    }

    // (d) empty final text -> None.
    #[tokio::test]
    async fn test_tool_loop_empty_final_text_returns_none() {
        let ai = Arc::new(ScriptedAi::new(vec![Ok(ChatTurnResponse {
            content: Some(String::new()),
            tool_calls: vec![],
        })]));
        let provider = Arc::new(StubProvider {
            tool_calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        });
        let (svc, tools) = service_with(ai);

        let conv = test_conversation();
        let provider_dyn: Arc<dyn ConversationContextProvider> = provider;
        let result = svc.try_tool_loop(&conv, &[], &provider_dyn, tools).await;
        assert!(result.is_none());
    }
}
