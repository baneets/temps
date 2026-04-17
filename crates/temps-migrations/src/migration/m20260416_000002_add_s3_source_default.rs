use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        db.execute_unprepared(
            "ALTER TABLE s3_sources \
             ADD COLUMN IF NOT EXISTS is_default BOOLEAN NOT NULL DEFAULT FALSE",
        )
        .await?;

        db.execute_unprepared(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_s3_sources_single_default \
             ON s3_sources (is_default) WHERE is_default = TRUE",
        )
        .await?;

        db.execute_unprepared(
            "UPDATE s3_sources SET is_default = TRUE \
             WHERE id = (SELECT id FROM s3_sources ORDER BY id ASC LIMIT 1) \
             AND (SELECT COUNT(*) FROM s3_sources) = 1",
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("DROP INDEX IF EXISTS idx_s3_sources_single_default")
            .await?;
        db.execute_unprepared("ALTER TABLE s3_sources DROP COLUMN IF EXISTS is_default")
            .await?;
        Ok(())
    }
}
