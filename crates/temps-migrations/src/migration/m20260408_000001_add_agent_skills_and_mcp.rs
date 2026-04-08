use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        // Per-agent config repo: private repo containing .claude/ directory
        db.execute_unprepared(
            "ALTER TABLE project_agents ADD COLUMN config_repo_url VARCHAR DEFAULT NULL",
        )
        .await?;
        db.execute_unprepared(
            "ALTER TABLE project_agents ADD COLUMN config_repo_branch VARCHAR DEFAULT NULL",
        )
        .await?;
        // Project secrets: encrypted values injected into agent sandboxes
        db.execute_unprepared(
            "CREATE TABLE project_secrets (
                id SERIAL PRIMARY KEY,
                project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
                name VARCHAR NOT NULL,
                secret_type VARCHAR NOT NULL DEFAULT 'env',
                encrypted_value TEXT NOT NULL,
                mount_path VARCHAR DEFAULT NULL,
                description VARCHAR DEFAULT NULL,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                UNIQUE(project_id, name)
            )",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("DROP TABLE IF EXISTS project_secrets")
            .await?;
        db.execute_unprepared("ALTER TABLE project_agents DROP COLUMN IF EXISTS config_repo_url")
            .await?;
        db.execute_unprepared(
            "ALTER TABLE project_agents DROP COLUMN IF EXISTS config_repo_branch",
        )
        .await?;
        Ok(())
    }
}
