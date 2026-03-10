use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};
use temps_core::DBDateTime;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "ai_usage_logs")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub timestamp: DBDateTime,
    pub user_id: Option<i32>,
    pub provider: String,
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub latency_ms: i32,
    /// Estimated cost in microcents (1/10000 of a cent)
    pub estimated_cost_microcents: i64,
    /// HTTP status code returned to the caller
    pub status: i16,
    /// Whether this was a streaming request
    pub is_streaming: bool,
    /// Whether the caller used their own API key (BYOK) instead of the system key
    pub is_byok: bool,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
