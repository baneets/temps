//! Email events entity — tracks opens, clicks, bounces, complaints, deliveries

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};
use temps_core::DBDateTime;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "email_events")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub email_id: Uuid,
    pub event_type: String,
    pub provider_message_id: Option<String>,
    pub recipient: Option<String>,
    #[sea_orm(column_type = "JsonBinary", nullable)]
    pub metadata: Option<Json>,
    pub ip_address: Option<String>,
    pub user_agent: Option<String>,
    pub created_at: DBDateTime,
}

// No relations — email_id is a soft reference (no FK constraint)
#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
