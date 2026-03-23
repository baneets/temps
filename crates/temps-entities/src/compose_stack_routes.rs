use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};
use temps_core::DBDateTime;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "compose_stack_routes")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub stack_id: i32,
    #[sea_orm(column_type = "String(StringLen::N(255))")]
    pub domain: String,
    pub target_port: i32,
    #[sea_orm(column_type = "String(StringLen::N(255))", nullable)]
    pub service_name: Option<String>,
    pub enabled: bool,
    pub created_at: DBDateTime,
    pub updated_at: DBDateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::compose_stacks::Entity",
        from = "Column::StackId",
        to = "super::compose_stacks::Column::Id"
    )]
    ComposeStack,
}

impl Related<super::compose_stacks::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::ComposeStack.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
