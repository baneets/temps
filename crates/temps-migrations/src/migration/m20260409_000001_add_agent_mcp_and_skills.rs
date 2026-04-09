use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        // MCP servers config: JSONB with Claude Code settings.json mcpServers format
        db.execute_unprepared(
            "ALTER TABLE project_agents ADD COLUMN mcp_servers_config JSONB DEFAULT NULL",
        )
        .await?;
        // Skills config: JSONB array of skill definitions
        db.execute_unprepared(
            "ALTER TABLE project_agents ADD COLUMN skills_config JSONB DEFAULT NULL",
        )
        .await?;
        // Tools config: JSONB array of tool definitions
        db.execute_unprepared(
            "ALTER TABLE project_agents ADD COLUMN tools_config JSONB DEFAULT NULL",
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
