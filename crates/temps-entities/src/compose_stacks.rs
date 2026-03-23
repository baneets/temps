use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};
use temps_core::DBDateTime;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "compose_stacks")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub name: String,
    pub description: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub compose_content: String,
    #[sea_orm(column_type = "Text", nullable)]
    pub env_content: Option<String>,
    pub node_id: Option<i32>,
    pub state: String,
    #[sea_orm(column_type = "String(StringLen::N(512))", nullable)]
    pub repo_url: Option<String>,
    #[sea_orm(column_type = "String(StringLen::N(255))", nullable)]
    pub repo_branch: Option<String>,
    #[sea_orm(column_type = "String(StringLen::N(512))", nullable)]
    pub repo_compose_path: Option<String>,
    #[sea_orm(column_type = "Text", nullable)]
    pub repo_access_token: Option<String>,
    pub last_synced_at: Option<DBDateTime>,
    pub created_at: DBDateTime,
    pub updated_at: DBDateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::compose_stack_routes::Entity")]
    Routes,
}

impl Related<super::compose_stack_routes::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Routes.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
