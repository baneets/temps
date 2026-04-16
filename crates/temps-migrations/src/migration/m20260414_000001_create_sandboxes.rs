use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        db.execute_unprepared(
            r#"
            CREATE TABLE IF NOT EXISTS sandboxes (
                id SERIAL PRIMARY KEY,
                public_id VARCHAR(64) NOT NULL UNIQUE,
                user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                name VARCHAR(255) NOT NULL,
                status VARCHAR(20) NOT NULL DEFAULT 'running',
                image VARCHAR(255),
                work_dir TEXT NOT NULL DEFAULT '/workspace',
                timeout_secs INTEGER NOT NULL DEFAULT 3600,
                metadata JSONB,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                last_activity_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                expires_at TIMESTAMPTZ NOT NULL DEFAULT (NOW() + INTERVAL '1 hour')
            )
            "#,
        )
        .await?;

        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS idx_sandboxes_user_id ON sandboxes(user_id)",
        )
        .await?;

        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS idx_sandboxes_status ON sandboxes(status)",
        )
        .await?;

        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS idx_sandboxes_expires_at ON sandboxes(expires_at) \
             WHERE status = 'running'",
        )
        .await?;

        // Offset the SERIAL sequence so standalone sandbox IDs never collide
        // with agent-run IDs. The underlying SandboxProvider names containers
        // `temps-sandbox-<id>` keyed on i32; agent runs and standalone
        // sandboxes share that namespace. Starting at 1_000_000 keeps the
        // two ID spaces disjoint in practice (agent_runs will not reach
        // 1M before this design is revisited) so the existing preview
        // gateway (ws-<id>-<port> → temps-sandbox-<id>:<port>) works for
        // both without a gateway change.
        db.execute_unprepared("ALTER SEQUENCE sandboxes_id_seq RESTART WITH 1000000")
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("DROP TABLE IF EXISTS sandboxes")
            .await?;
        Ok(())
    }
}
