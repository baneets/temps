use sea_orm_migration::prelude::*;

/// Adds support for ephemeral (CLI dry-run) workflow executions.
///
/// - `source`: where the run config came from. `committed` (default, from
///   project_agents) or `cli_ephemeral` (from a YAML uploaded once via the CLI).
/// - `ephemeral_yaml`: the full WorkflowYamlConfig as YAML text. Only set when
///   `source = 'cli_ephemeral'`. The executor parses this instead of looking up
///   project_agents.
/// - `config_id` becomes nullable so ephemeral runs (and historical autofixer
///   runs which used a `0` sentinel) don't need to fabricate a row in
///   project_agents.
///
/// The default value `'committed'` backfills every existing row, matching the
/// historical behavior. No data migration needed.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        db.execute_unprepared(
            "ALTER TABLE agent_runs \
             ADD COLUMN IF NOT EXISTS source VARCHAR NOT NULL DEFAULT 'committed'",
        )
        .await?;

        db.execute_unprepared(
            "ALTER TABLE agent_runs ADD COLUMN IF NOT EXISTS ephemeral_yaml TEXT",
        )
        .await?;

        db.execute_unprepared("ALTER TABLE agent_runs ALTER COLUMN config_id DROP NOT NULL")
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        // Restoring the NOT NULL constraint requires backfilling — leave any
        // historical NULLs as 0 so the down migration is non-destructive on a
        // database that's already exercised the new code path.
        db.execute_unprepared("UPDATE agent_runs SET config_id = 0 WHERE config_id IS NULL")
            .await?;
        db.execute_unprepared("ALTER TABLE agent_runs ALTER COLUMN config_id SET NOT NULL")
            .await?;

        db.execute_unprepared("ALTER TABLE agent_runs DROP COLUMN IF EXISTS ephemeral_yaml")
            .await?;
        db.execute_unprepared("ALTER TABLE agent_runs DROP COLUMN IF EXISTS source")
            .await?;

        Ok(())
    }
}
