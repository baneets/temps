use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        // Webhook ID: short non-secret identifier used in the URL path.
        db.execute_unprepared(
            "ALTER TABLE project_agents ADD COLUMN IF NOT EXISTS webhook_id VARCHAR DEFAULT NULL",
        )
        .await?;
        // Webhook token: secret credential sent via X-Webhook-Token header.
        db.execute_unprepared(
            "ALTER TABLE project_agents ADD COLUMN IF NOT EXISTS webhook_token VARCHAR DEFAULT NULL",
        )
        .await?;
        // Index for fast lookup by webhook_id
        db.execute_unprepared(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_project_agents_webhook_id ON project_agents (webhook_id) WHERE webhook_id IS NOT NULL",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("DROP INDEX IF EXISTS idx_project_agents_webhook_id")
            .await?;
        db.execute_unprepared("ALTER TABLE project_agents DROP COLUMN IF EXISTS webhook_id")
            .await?;
        db.execute_unprepared("ALTER TABLE project_agents DROP COLUMN IF EXISTS webhook_token")
            .await?;
        Ok(())
    }
}
