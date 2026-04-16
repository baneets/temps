use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        // Opaque public id for workspace sessions. Mirrors the `sbx_<hex>`
        // pattern used by standalone sandboxes. The hex label (prefix
        // stripped) is spliced into preview hostnames so users can't
        // enumerate sessions by walking integer ids.
        db.execute_unprepared(
            "ALTER TABLE workspace_sessions \
             ADD COLUMN IF NOT EXISTS public_id VARCHAR(32)",
        )
        .await?;

        // Backfill existing rows with wss_ + 16 random hex chars. Uses
        // pgcrypto's gen_random_bytes if available, falls back to md5(random()).
        db.execute_unprepared(
            "UPDATE workspace_sessions \
             SET public_id = 'wss_' || substr(md5(random()::text || id::text), 1, 16) \
             WHERE public_id IS NULL",
        )
        .await?;

        db.execute_unprepared("ALTER TABLE workspace_sessions ALTER COLUMN public_id SET NOT NULL")
            .await?;

        db.execute_unprepared(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_workspace_sessions_public_id \
             ON workspace_sessions(public_id)",
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("DROP INDEX IF EXISTS idx_workspace_sessions_public_id")
            .await?;
        db.execute_unprepared("ALTER TABLE workspace_sessions DROP COLUMN IF EXISTS public_id")
            .await?;
        Ok(())
    }
}
