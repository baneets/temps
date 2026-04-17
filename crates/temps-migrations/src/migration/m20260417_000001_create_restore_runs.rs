use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        // `restore_runs` tracks engine-agnostic restore operations across
        // Postgres, Redis, MongoDB, and S3/RustFS. Mirrors the phase-driven
        // pattern of `postgres_major_upgrades` so operators get a uniform
        // experience: resumable, observable, audited.
        db.execute_unprepared(
            r#"
            CREATE TABLE IF NOT EXISTS restore_runs (
                id SERIAL PRIMARY KEY,
                source_backup_id INTEGER NOT NULL
                    REFERENCES backups(id) ON DELETE RESTRICT,
                source_service_id INTEGER NOT NULL
                    REFERENCES external_services(id) ON DELETE CASCADE,
                target_service_id INTEGER
                    REFERENCES external_services(id) ON DELETE SET NULL,
                target_service_name VARCHAR(255),
                mode VARCHAR(32) NOT NULL,
                status VARCHAR(20) NOT NULL DEFAULT 'pending',
                phase VARCHAR(32) NOT NULL DEFAULT 'prepare',
                recovery_target JSONB,
                parameter_overrides JSONB NOT NULL DEFAULT '{}'::jsonb,
                resume_token JSONB,
                log_id VARCHAR(64) NOT NULL,
                error_message TEXT,
                attempt INTEGER NOT NULL DEFAULT 1,
                started_at TIMESTAMPTZ,
                finished_at TIMESTAMPTZ,
                created_by INTEGER NOT NULL REFERENCES users(id),
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                CONSTRAINT restore_runs_mode_check
                    CHECK (mode IN ('in_place', 'new_service', 'pitr')),
                CONSTRAINT restore_runs_status_check
                    CHECK (status IN ('pending', 'running', 'completed', 'failed', 'cancelled')),
                CONSTRAINT restore_runs_phase_check
                    CHECK (phase IN ('prepare', 'provision', 'restore', 'recover', 'verify', 'completed', 'failed'))
            )
            "#,
        )
        .await?;

        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS idx_restore_runs_source_service \
             ON restore_runs(source_service_id)",
        )
        .await?;

        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS idx_restore_runs_target_service \
             ON restore_runs(target_service_id) WHERE target_service_id IS NOT NULL",
        )
        .await?;

        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS idx_restore_runs_source_backup \
             ON restore_runs(source_backup_id)",
        )
        .await?;

        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS idx_restore_runs_status \
             ON restore_runs(status)",
        )
        .await?;

        // Per-service lock: at most one in-flight restore per source service.
        // Prevents two concurrent API calls from both spawning an orchestrator
        // on the same service. Separate restores on different services are fine.
        db.execute_unprepared(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_restore_runs_source_service_active \
             ON restore_runs(source_service_id) \
             WHERE status IN ('pending', 'running') AND mode = 'in_place'",
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("DROP TABLE IF EXISTS restore_runs")
            .await?;
        Ok(())
    }
}
