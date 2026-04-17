use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            "ALTER TABLE sandboxes \
             ADD COLUMN IF NOT EXISTS preview_password_hash TEXT, \
             ADD COLUMN IF NOT EXISTS preview_password_hint VARCHAR(8)",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            "ALTER TABLE sandboxes \
             DROP COLUMN IF EXISTS preview_password_hash, \
             DROP COLUMN IF EXISTS preview_password_hint",
        )
        .await?;
        Ok(())
    }
}
