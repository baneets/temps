use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        // Use ADD COLUMN IF NOT EXISTS to be idempotent (columns may exist from dev iterations)
        db.execute_unprepared(
            "ALTER TABLE project_agents ADD COLUMN IF NOT EXISTS mcp_servers_config JSONB DEFAULT NULL",
        )
        .await?;
        db.execute_unprepared(
            "ALTER TABLE project_agents ADD COLUMN IF NOT EXISTS skills_config JSONB DEFAULT NULL",
        )
        .await?;
        db.execute_unprepared(
            "ALTER TABLE project_agents ADD COLUMN IF NOT EXISTS tools_config JSONB DEFAULT NULL",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            "ALTER TABLE project_agents DROP COLUMN IF EXISTS mcp_servers_config",
        )
        .await?;
        db.execute_unprepared("ALTER TABLE project_agents DROP COLUMN IF EXISTS skills_config")
            .await?;
        db.execute_unprepared("ALTER TABLE project_agents DROP COLUMN IF EXISTS tools_config")
            .await?;
        Ok(())
    }
}
