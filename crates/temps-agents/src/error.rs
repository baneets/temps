use thiserror::Error;

#[derive(Error, Debug)]
pub enum AgentError {
    #[error("Autopilot config not found for project {project_id}")]
    ConfigNotFound { project_id: i32 },

    #[error("Agent '{slug}' not found in project {project_id}")]
    AgentNotFound { project_id: i32, slug: String },

    #[error("Autopilot run {run_id} not found")]
    RunNotFound { run_id: i32 },

    #[error("Project {project_id} not found")]
    ProjectNotFound { project_id: i32 },

    #[error("Daily budget exceeded for project {project_id}: spent {spent_cents} of {daily_limit_cents} cents")]
    BudgetExceeded {
        project_id: i32,
        daily_limit_cents: i32,
        spent_cents: i32,
    },

    #[error("Cooldown active for project {project_id}: {minutes_remaining} minutes remaining")]
    CooldownActive {
        project_id: i32,
        minutes_remaining: i32,
    },

    #[error("AI CLI '{provider}' is not installed or not found in PATH")]
    AiCliNotInstalled { provider: String },

    #[error("AI CLI '{provider}' failed with exit code {exit_code}: {stderr}")]
    AiCliFailed {
        provider: String,
        exit_code: i32,
        stderr: String,
    },

    #[error("AI CLI '{provider}' timed out after {timeout_secs} seconds")]
    AiCliTimeout { provider: String, timeout_secs: u64 },

    #[error("Git operation failed: {message}")]
    GitError { message: String },

    #[error("Encryption error: {message}")]
    EncryptionError { message: String },

    #[error("Validation error: {message}")]
    Validation { message: String },

    #[error("Database error: {0}")]
    Database(sea_orm::DbErr),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Sandbox creation failed for run {run_id} ({provider}): {reason}")]
    SandboxCreationFailed {
        run_id: i32,
        provider: String,
        reason: String,
    },

    #[error("Sandbox not found for run {run_id}")]
    SandboxNotFound { run_id: i32 },

    #[error("Sandbox exec failed for run {run_id} in sandbox {sandbox_id}: {reason}")]
    SandboxExecFailed {
        run_id: i32,
        sandbox_id: String,
        reason: String,
    },

    #[error("Sandbox provider '{provider}' unavailable: {reason}")]
    SandboxProviderUnavailable { provider: String, reason: String },
}

impl From<sea_orm::DbErr> for AgentError {
    fn from(error: sea_orm::DbErr) -> Self {
        match &error {
            sea_orm::DbErr::RecordNotFound(_) => AgentError::RunNotFound { run_id: 0 },
            sea_orm::DbErr::RecordNotInserted => AgentError::Validation {
                message: format!("Duplicate record or constraint violation: {}", error),
            },
            _ => AgentError::Database(error),
        }
    }
}
