use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        // Add archive column for skill directories (tar.gz stored as BYTEA).
        // Simple single-file skills keep using `content`; directory-based skills
        // store their tar.gz here and `content` holds the extracted SKILL.md text.
        db.execute_unprepared(
            "ALTER TABLE project_skill_definitions ADD COLUMN IF NOT EXISTS archive BYTEA DEFAULT NULL",
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            "ALTER TABLE project_skill_definitions DROP COLUMN IF EXISTS archive",
        )
        .await?;
        Ok(())
    }
}
