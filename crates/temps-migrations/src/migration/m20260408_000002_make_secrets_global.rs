use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        // Drop the project-scoped unique constraint
        db.execute_unprepared(
            "ALTER TABLE project_secrets DROP CONSTRAINT IF EXISTS project_secrets_project_id_name_key",
        )
        .await?;

        // Drop project_id column (secrets are now global)
        db.execute_unprepared("ALTER TABLE project_secrets DROP COLUMN IF EXISTS project_id")
            .await?;

        // Rename table to agent_secrets
        db.execute_unprepared("ALTER TABLE project_secrets RENAME TO agent_secrets")
            .await?;

        // Add global unique constraint on name
        db.execute_unprepared(
            "ALTER TABLE agent_secrets ADD CONSTRAINT agent_secrets_name_key UNIQUE (name)",
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        // Drop the global unique constraint
        db.execute_unprepared(
            "ALTER TABLE agent_secrets DROP CONSTRAINT IF EXISTS agent_secrets_name_key",
        )
        .await?;

        // Rename back
        db.execute_unprepared("ALTER TABLE agent_secrets RENAME TO project_secrets")
            .await?;

        // Re-add project_id column (nullable since we can't recover the original values)
        db.execute_unprepared(
            "ALTER TABLE project_secrets ADD COLUMN project_id INTEGER REFERENCES projects(id) ON DELETE CASCADE",
        )
        .await?;

        // Re-add original unique constraint
        db.execute_unprepared(
            "ALTER TABLE project_secrets ADD CONSTRAINT project_secrets_project_id_name_key UNIQUE (project_id, name)",
        )
        .await?;

        Ok(())
    }
}
