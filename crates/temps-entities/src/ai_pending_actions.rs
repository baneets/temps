//! A proposed AI write-action awaiting human confirmation.
//!
//! When the AI SRE proposes a mutation (e.g. "redeploy this deployment"), a row
//! is inserted here with `status = "proposed"`. The operator reviews the
//! [`summary`] and [`params`], then approves or rejects. On approval the
//! orchestrator sets `status = "executing"`, replays [`params`] against the
//! identified operation, and records the outcome in [`result`] / [`error`].
//!
//! Status lifecycle:
//!   `proposed` → `approved` → `executing` → `executed`
//!                            ↘ `failed`
//!              → `rejected`
//!              → `expired`   (background sweeper for stale proposed rows)

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};
use temps_core::DBDateTime;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "ai_pending_actions")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    /// URL-safe opaque id used in the API (e.g. prefixed nanoid).
    pub public_id: String,
    /// The conversation that proposed this action.
    pub conversation_id: i64,
    /// The assistant message that contains the proposal (set after the message
    /// persists; `None` for actions proposed outside a message context).
    pub message_id: Option<i64>,
    /// The project this action targets.
    pub project_id: i32,
    /// Machine-readable operation identifier, e.g. `"redeploy_deployment"`.
    pub operation_id: String,
    /// HTTP method the operation will use: `"POST"`, `"PUT"`, `"PATCH"`, or `"DELETE"`.
    pub method: String,
    /// Human-readable one-liner shown in the confirmation UI.
    pub summary: String,
    /// Validated flat params replayed verbatim at execute time.
    pub params: serde_json::Value,
    /// Advisory permission name, e.g. `"DeploymentsCreate"` — checked at confirm time.
    pub required_permission: Option<String>,
    /// Current lifecycle status (see module-level docs).
    pub status: String,
    /// Execution response body recorded on success.
    pub result: Option<serde_json::Value>,
    /// Failure detail recorded when `status = "failed"`.
    pub error: Option<String>,
    /// User who initiated the chat session that produced this action.
    pub created_by: Option<i32>,
    /// User who confirmed (approved or rejected) this action.
    pub confirmed_by: Option<i32>,
    pub created_at: DBDateTime,
    pub confirmed_at: Option<DBDateTime>,
    pub executed_at: Option<DBDateTime>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
