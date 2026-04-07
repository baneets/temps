use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            "ALTER TABLE workspace_sessions \
             ADD COLUMN IF NOT EXISTS cpu_milli INTEGER, \
             ADD COLUMN IF NOT EXISTS memory_limit_mb INTEGER, \
             ADD COLUMN IF NOT EXISTS pids_limit INTEGER",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            "ALTER TABLE workspace_sessions \
             DROP COLUMN IF EXISTS cpu_milli, \
             DROP COLUMN IF EXISTS memory_limit_mb, \
             DROP COLUMN IF EXISTS pids_limit",
        )
        .await?;
        Ok(())
    }
}
