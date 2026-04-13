use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            "ALTER TABLE workspace_sessions ADD COLUMN IF NOT EXISTS skills_config JSONB",
        )
        .await?;
        db.execute_unprepared(
            "ALTER TABLE workspace_sessions ADD COLUMN IF NOT EXISTS mcp_servers_config JSONB",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            "ALTER TABLE workspace_sessions DROP COLUMN IF EXISTS mcp_servers_config",
        )
        .await?;
        db.execute_unprepared("ALTER TABLE workspace_sessions DROP COLUMN IF EXISTS skills_config")
            .await?;
        Ok(())
    }
}
