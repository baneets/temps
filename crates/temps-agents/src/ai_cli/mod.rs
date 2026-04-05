pub mod claude;
pub mod codex;
pub mod opencode;

use async_trait::async_trait;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use crate::error::AgentError;

/// Callback invoked for each line of AI CLI output (for real-time streaming)
pub type OnEventCallback =
    Arc<dyn Fn(String) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

pub struct AiRunConfig {
    pub work_dir: PathBuf,
    pub prompt: String,
    pub api_key: String,
    pub max_turns: i32,
    pub timeout: Duration,
    /// Optional callback for streaming each line of output in real-time
    pub on_event: Option<OnEventCallback>,
}

pub struct AiRunResult {
    pub output: String,
    pub exit_code: i32,
    pub tokens_input: Option<i32>,
    pub tokens_output: Option<i32>,
    pub model: Option<String>,
    /// If the provider knows which files it changed, list them here.
    /// If `None`, the executor will detect changes via `git diff`.
    pub changed_files: Option<Vec<String>>,
}

/// Status of the AI CLI tool on this server.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AiCliStatus {
    pub provider: String,
    pub installed: bool,
    pub version: Option<String>,
    pub authenticated: bool,
    pub auth_method: Option<String>,
    pub email: Option<String>,
    pub subscription_type: Option<String>,
    /// Instructions for the user if not installed or not authenticated.
    pub setup_hint: Option<String>,
}

#[async_trait]
pub trait AiCliProvider: Send + Sync {
    fn name(&self) -> &str;
    async fn check_installed(&self) -> bool;
    async fn get_status(&self) -> AiCliStatus;
    async fn run(&self, config: AiRunConfig) -> Result<AiRunResult, AgentError>;
    /// Continue an existing conversation in the same work directory.
    /// Uses `--continue` to resume the most recent session.
    async fn continue_conversation(&self, config: AiRunConfig) -> Result<AiRunResult, AgentError>;
}

/// Create an AI CLI provider by name
pub fn create_provider(name: &str) -> Option<Box<dyn AiCliProvider>> {
    match name {
        "claude_cli" => Some(Box::new(claude::ClaudeCliProvider)),
        "codex_cli" => Some(Box::new(codex::CodexCliProvider)),
        "opencode" => Some(Box::new(opencode::OpenCodeCliProvider)),
        _ => None,
    }
}

/// All supported provider names.
pub const PROVIDER_NAMES: &[(&str, &str)] = &[
    ("claude_cli", "Claude Code"),
    ("opencode", "OpenCode"),
    ("codex_cli", "Codex"),
];
