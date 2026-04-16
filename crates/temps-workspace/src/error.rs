use thiserror::Error;

#[derive(Error, Debug)]
pub enum WorkspaceError {
    #[error("Workspace session {session_id} not found")]
    SessionNotFound { session_id: i32 },

    #[error("Workspace session {session_id} is not active (status: {status})")]
    SessionNotActive { session_id: i32, status: String },

    #[error("Project {project_id} not found")]
    ProjectNotFound { project_id: i32 },

    #[error("Sandbox creation failed for session {session_id}: {reason}")]
    SandboxCreationFailed { session_id: i32, reason: String },

    #[error("Sandbox not available for session {session_id}")]
    SandboxNotAvailable { session_id: i32 },

    #[error("AI CLI execution failed for session {session_id}: {reason}")]
    AiCliFailed { session_id: i32, reason: String },

    #[error("AI CLI timed out for session {session_id} after {timeout_secs}s")]
    AiCliTimeout { session_id: i32, timeout_secs: u64 },

    #[error("Validation error: {message}")]
    Validation { message: String },

    #[error("Failed to hash preview password for session {session_id}: {reason}")]
    PasswordHashFailed { session_id: i32, reason: String },

    #[error("Memory fact {fact_id} not found in workflow {agent_id} (project {project_id})")]
    MemoryNotFound {
        project_id: i32,
        agent_id: i32,
        fact_id: i64,
    },

    #[error("Workflow {slug} not found in project {project_id}")]
    WorkflowNotFound { project_id: i32, slug: String },

    #[error("Database error: {0}")]
    Database(sea_orm::DbErr),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Agent error: {0}")]
    Agent(#[from] temps_agents::error::AgentError),
}

impl From<sea_orm::DbErr> for WorkspaceError {
    /// Generic `From` impl for the `?` operator — preserves the original
    /// error message. Callers that know which resource was missing (session,
    /// project, workflow, memory fact, etc.) MUST construct the correct
    /// typed `NotFound` variant themselves with the real ID, instead of
    /// relying on this fallback. Mapping every `RecordNotFound` to
    /// `SessionNotFound{session_id: 0}` here would mislabel missing
    /// projects, workflows, and memory rows as "session 0 not found".
    fn from(error: sea_orm::DbErr) -> Self {
        WorkspaceError::Database(error)
    }
}
