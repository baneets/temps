pub mod docker;
pub mod local;

use async_trait::async_trait;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use crate::ai_cli::OnEventCallback;
use crate::error::AgentError;

/// A handle to an active sandbox. Opaque to callers — the internal fields
/// are provider-specific (Docker container ID, Vercel sandbox ID, etc.).
#[derive(Debug, Clone)]
pub struct SandboxHandle {
    /// Provider-specific identifier (container ID, Vercel sandbox ID, etc.)
    pub sandbox_id: String,
    /// Human-readable name for logging (e.g. `temps-sandbox-42`)
    pub sandbox_name: String,
    /// Path to the repository inside the sandbox (e.g. `/workspace`)
    pub work_dir: PathBuf,
}

/// Configuration for creating a new sandbox.
pub struct SandboxCreateConfig {
    /// The agent run this sandbox belongs to
    pub run_id: i32,
    /// Path to the cloned repository on the host (for bind mount / upload)
    pub host_work_dir: PathBuf,
    /// Custom Docker image override (empty = use provider default)
    pub image: Option<String>,
    /// CPU limit in cores (e.g. 2.0)
    pub cpu_limit: Option<f64>,
    /// Memory limit in megabytes
    pub memory_limit_mb: Option<u64>,
    /// Network access: "full", "restricted", "none"
    pub network_mode: Option<String>,
    /// Environment variables to inject (ANTHROPIC_API_KEY, etc.)
    pub env_vars: HashMap<String, String>,
    /// Maximum time the sandbox should stay alive without activity
    pub idle_timeout: Duration,
}

/// Result of executing a command inside a sandbox.
pub struct SandboxExecResult {
    pub exit_code: i32,
    pub stdout: String,
}

/// Pluggable sandbox backend. Implementations provide container/VM isolation
/// for agent runs. The executor and autofixer interact only with this trait,
/// never with Docker or any specific backend directly.
#[async_trait]
pub trait SandboxProvider: Send + Sync {
    /// Create and start a new sandbox for a run.
    async fn create(&self, config: SandboxCreateConfig) -> Result<SandboxHandle, AgentError>;

    /// Execute a command inside an existing sandbox, streaming stdout via callback.
    async fn exec(
        &self,
        handle: &SandboxHandle,
        cmd: Vec<String>,
        env: HashMap<String, String>,
        on_output: Option<OnEventCallback>,
    ) -> Result<SandboxExecResult, AgentError>;

    /// Check if a sandbox is still alive and usable.
    async fn is_alive(&self, handle: &SandboxHandle) -> Result<bool, AgentError>;

    /// Destroy sandbox and clean up all resources.
    async fn destroy(&self, handle: &SandboxHandle) -> Result<(), AgentError>;

    /// Attempt to recover a sandbox after server restart (by naming convention).
    /// Returns `None` if no recoverable sandbox exists for this run.
    async fn recover(&self, run_id: i32) -> Result<Option<SandboxHandle>, AgentError>;

    /// Provider name for logging and error messages.
    fn name(&self) -> &str;

    /// Check if the sandbox backend is available (e.g. Docker daemon reachable).
    async fn is_available(&self) -> bool;

    /// Check if the sandbox image is built/available.
    /// Returns (is_ready, image_name).
    async fn image_status(&self) -> Result<(bool, String), AgentError>;
}
