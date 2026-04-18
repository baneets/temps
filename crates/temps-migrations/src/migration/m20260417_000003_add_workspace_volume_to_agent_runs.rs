use sea_orm_migration::prelude::*;

/// Tracks the Docker volume that holds `/workspace` for each agent run.
///
/// Today the executor mounts a host tmpdir at `/workspace` and throws it
/// away when the container exits, so any work the AI did (including
/// committed-but-unpushed changes) is lost. Switching to a per-run named
/// volume (`temps-wfrun-{id}`) lets us re-mount the exact same filesystem
/// into a follow-up workspace sandbox so the user can finish a failed
/// push from where the agent left off.
///
/// Nullable because pre-migration runs had no associated volume.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE agent_runs ADD COLUMN IF NOT EXISTS workspace_volume TEXT",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("ALTER TABLE agent_runs DROP COLUMN IF EXISTS workspace_volume")
            .await?;
        Ok(())
    }
}
