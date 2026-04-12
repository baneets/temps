use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        // Make project_id nullable on both tables to support global (platform-wide) definitions
        db.execute_unprepared(
            "ALTER TABLE project_skill_definitions ALTER COLUMN project_id DROP NOT NULL",
        )
        .await?;

        db.execute_unprepared(
            "ALTER TABLE project_mcp_definitions ALTER COLUMN project_id DROP NOT NULL",
        )
        .await?;

        // Add partial unique index for global entries (project_id IS NULL)
        db.execute_unprepared(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_global_skill_definitions_slug
             ON project_skill_definitions (slug)
             WHERE project_id IS NULL",
        )
        .await?;

        db.execute_unprepared(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_global_mcp_definitions_slug
             ON project_mcp_definitions (slug)
             WHERE project_id IS NULL",
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        db.execute_unprepared("DROP INDEX IF EXISTS idx_global_mcp_definitions_slug")
            .await?;
        db.execute_unprepared("DROP INDEX IF EXISTS idx_global_skill_definitions_slug")
            .await?;

        // Delete any global rows before making column NOT NULL again
        db.execute_unprepared("DELETE FROM project_skill_definitions WHERE project_id IS NULL")
            .await?;
        db.execute_unprepared("DELETE FROM project_mcp_definitions WHERE project_id IS NULL")
            .await?;

        db.execute_unprepared(
            "ALTER TABLE project_skill_definitions ALTER COLUMN project_id SET NOT NULL",
        )
        .await?;
        db.execute_unprepared(
            "ALTER TABLE project_mcp_definitions ALTER COLUMN project_id SET NOT NULL",
        )
        .await?;

        Ok(())
    }
}
