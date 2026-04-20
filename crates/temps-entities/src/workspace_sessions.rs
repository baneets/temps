use async_trait::async_trait;
use sea_orm::entity::prelude::*;
use sea_orm::{ActiveValue::Set, ConnectionTrait, DbErr};
use serde::{Deserialize, Serialize};
use temps_core::DBDateTime;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "workspace_sessions")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    /// Opaque external identifier (`wss_<16hex>`). Embedded in preview
    /// hostnames in place of the sequential `id` so URLs can't be
    /// enumerated. API routes still key off `id` — this is a display-only
    /// identifier.
    #[sea_orm(unique)]
    pub public_id: String,
    pub project_id: i32,
    pub user_id: i32,
    /// Optional user-provided title. When null the UI shows "Session #{id}".
    pub title: Option<String>,
    /// "active", "idle", "closed"
    pub status: String,
    /// Docker container ID for this session
    pub sandbox_container_id: Option<String>,
    /// Filesystem path to cloned repo inside container
    pub work_dir: Option<String>,
    /// Git branch created for this session's changes. If `base_branch_name`
    /// is also set, this branch is created locally off `base_branch_name`
    /// during sandbox initialization (it does not need to exist on the remote).
    pub branch_name: Option<String>,
    /// Optional base branch to fork the session's branch from. When set,
    /// the sandbox clones `base_branch_name` from the remote and then
    /// creates `branch_name` as a local branch off it.
    pub base_branch_name: Option<String>,
    /// AI provider used: "claude_cli", "codex_cli", "opencode"
    pub ai_provider: String,
    /// AI model used
    pub ai_model: Option<String>,
    /// Cumulative token usage
    pub tokens_input: i32,
    pub tokens_output: i32,
    pub estimated_cost_cents: i32,
    /// Number of files modified in this session
    pub files_changed: i32,
    /// Session metadata (initial context, sandbox config, etc.)
    #[sea_orm(column_type = "JsonBinary")]
    pub metadata: Option<serde_json::Value>,
    /// JSON array of skill slugs to inject into the sandbox at session start.
    /// Resolved from `project_skill_definitions` (falls back to global).
    #[sea_orm(column_type = "JsonBinary")]
    pub skills_config: Option<serde_json::Value>,
    /// JSON array of MCP server slugs to inject into the sandbox at session
    /// start. Resolved from `project_mcp_definitions` (falls back to global).
    /// Deep-merged into `/home/temps/.claude.json` (user-level config, kept
    /// out of the bind-mounted `/workspace` repo to avoid leaking resolved
    /// secrets into PR diffs).
    #[sea_orm(column_type = "JsonBinary")]
    pub mcp_servers_config: Option<serde_json::Value>,
    /// Argon2 hash of the per-session preview password. Enforced by the
    /// host-side Pingora before forwarding to the preview gateway. The
    /// plaintext is only returned to the caller ONCE at session creation
    /// (or on regenerate) and is never stored.
    pub preview_password_hash: Option<String>,
    /// Last 4 chars of the plaintext password, surfaced in the UI so users
    /// can tell which password they're looking at after the show-once
    /// reveal is gone. Not sensitive on its own.
    pub preview_password_hint: Option<String>,
    /// Per-session idle timeout in minutes. When null, the server-wide
    /// default (60min) applies. Sessions idle longer than this are marked
    /// closed and have their sandbox torn down by the periodic sweeper.
    pub idle_timeout_minutes: Option<i32>,
    /// CPU limit in milli-cpus (e.g. 2000 = 2 vCPU cores). Stored as
    /// integer to satisfy `Eq` on the entity. When null the server-wide
    /// default applies.
    pub cpu_milli: Option<i32>,
    /// Memory limit in MB. When null the server-wide default applies.
    pub memory_limit_mb: Option<i32>,
    /// PID limit. When null the server-wide default applies.
    pub pids_limit: Option<i32>,
    pub last_activity_at: DBDateTime,
    pub started_at: DBDateTime,
    pub closed_at: Option<DBDateTime>,
    pub created_at: DBDateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::projects::Entity",
        from = "Column::ProjectId",
        to = "super::projects::Column::Id",
        on_delete = "Cascade"
    )]
    Project,
    #[sea_orm(
        belongs_to = "super::users::Entity",
        from = "Column::UserId",
        to = "super::users::Column::Id",
        on_delete = "Cascade"
    )]
    User,
    #[sea_orm(has_many = "super::workspace_messages::Entity")]
    Messages,
}

impl Related<super::projects::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Project.def()
    }
}

impl Related<super::users::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::User.def()
    }
}

impl Related<super::workspace_messages::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Messages.def()
    }
}

#[async_trait]
impl ActiveModelBehavior for ActiveModel {
    async fn before_save<C>(mut self, _db: &C, insert: bool) -> Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        let now = chrono::Utc::now();
        if insert {
            if self.created_at.is_not_set() {
                self.created_at = Set(now);
            }
            if self.started_at.is_not_set() {
                self.started_at = Set(now);
            }
            if self.last_activity_at.is_not_set() {
                self.last_activity_at = Set(now);
            }
        }
        Ok(self)
    }
}
