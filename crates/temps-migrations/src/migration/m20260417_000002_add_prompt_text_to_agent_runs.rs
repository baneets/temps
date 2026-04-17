use sea_orm_migration::prelude::*;

/// Stores the final assembled prompt sent to the AI CLI on each run.
///
/// Today the executor builds this per-run (`build_trigger_context` + the
/// YAML prompt, with error-group fields interpolated) and throws it away
/// after handing it to the CLI. That makes debugging — "what did the AI
/// actually see?" — frustrating.
///
/// Nullable because pre-migration rows never captured it.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("ALTER TABLE agent_runs ADD COLUMN IF NOT EXISTS prompt_text TEXT")
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("ALTER TABLE agent_runs DROP COLUMN IF EXISTS prompt_text")
            .await?;
        Ok(())
    }
}
