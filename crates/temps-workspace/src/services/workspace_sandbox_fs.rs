//! Adapter that lets the shared `temps_agents::services::sandbox_injector`
//! write into a workspace session's sandbox. Binds a `WorkspaceSessionManager`
//! and `session_id` behind the `SandboxFs` trait so the injector can treat
//! workspaces and agent runs uniformly.

use std::sync::Arc;

use async_trait::async_trait;
use temps_agents::error::AgentError;
use temps_agents::services::sandbox_injector::SandboxFs;

use crate::services::session_manager::WorkspaceSessionManager;

pub struct WorkspaceSandboxFs {
    pub sm: Arc<WorkspaceSessionManager>,
    pub session_id: i32,
}

fn to_agent_err(session_id: i32, ctx: &str, e: impl std::fmt::Display) -> AgentError {
    AgentError::SandboxExecFailed {
        run_id: session_id,
        sandbox_id: String::new(),
        reason: format!("{}: {}", ctx, e),
    }
}

#[async_trait]
impl SandboxFs for WorkspaceSandboxFs {
    async fn exec(&self, cmd: Vec<String>) -> Result<(), AgentError> {
        self.sm
            .exec(self.session_id, cmd, std::collections::HashMap::new(), None)
            .await
            .map(|_| ())
            .map_err(|e| to_agent_err(self.session_id, "workspace exec", e))
    }

    async fn write_file(&self, path: &str, contents: &[u8], mode: u32) -> Result<(), AgentError> {
        self.sm
            .write_file(self.session_id, path, contents, mode)
            .await
            .map_err(|e| to_agent_err(self.session_id, "workspace write_file", e))
    }

    async fn read_file(&self, path: &str) -> Result<Vec<u8>, AgentError> {
        self.sm
            .read_file(self.session_id, path)
            .await
            .map_err(|e| to_agent_err(self.session_id, "workspace read_file", e))
    }

    async fn write_directory(
        &self,
        local_dir: &std::path::Path,
        target_path: &str,
    ) -> Result<(), AgentError> {
        self.sm
            .write_directory(self.session_id, local_dir, target_path)
            .await
            .map_err(|e| to_agent_err(self.session_id, "workspace write_directory", e))
    }
}
