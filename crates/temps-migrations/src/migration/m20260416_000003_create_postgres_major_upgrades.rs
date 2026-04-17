use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        db.execute_unprepared(
            r#"
            CREATE TABLE IF NOT EXISTS postgres_major_upgrades (
                id SERIAL PRIMARY KEY,
                service_id INTEGER NOT NULL
                    REFERENCES external_services(id) ON DELETE CASCADE,
                from_version VARCHAR(16) NOT NULL,
                to_version VARCHAR(16) NOT NULL,
                from_image VARCHAR(512) NOT NULL,
                to_image VARCHAR(512) NOT NULL,
                status VARCHAR(20) NOT NULL DEFAULT 'pending',
                phase VARCHAR(32) NOT NULL DEFAULT 'pre_backup',
                pre_upgrade_backup_id INTEGER
                    REFERENCES backups(id) ON DELETE SET NULL,
                log_id VARCHAR(64) NOT NULL,
                rollback_volume_name VARCHAR(255),
                rollback_volume_expires_at TIMESTAMPTZ,
                error_message TEXT,
                attempt INTEGER NOT NULL DEFAULT 1,
                started_at TIMESTAMPTZ,
                finished_at TIMESTAMPTZ,
                created_by INTEGER NOT NULL REFERENCES users(id),
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )
            "#,
        )
        .await?;

        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS idx_pg_major_upgrades_service_id \
             ON postgres_major_upgrades(service_id)",
        )
        .await?;

        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS idx_pg_major_upgrades_status \
             ON postgres_major_upgrades(status)",
        )
        .await?;

        // Per-service lock: at most one in-flight upgrade per service.
        // Enforced at the database level so two concurrent API calls can't
        // both spawn an orchestrator.
        db.execute_unprepared(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_pg_major_upgrades_service_active \
             ON postgres_major_upgrades(service_id) \
             WHERE status IN ('pending', 'running')",
        )
        .await?;

        // Sweep index: find completed upgrades whose rollback volume has expired.
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS idx_pg_major_upgrades_rollback_expiry \
             ON postgres_major_upgrades(rollback_volume_expires_at) \
             WHERE rollback_volume_name IS NOT NULL AND status = 'completed'",
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("DROP TABLE IF EXISTS postgres_major_upgrades")
            .await?;
        Ok(())
    }
}
