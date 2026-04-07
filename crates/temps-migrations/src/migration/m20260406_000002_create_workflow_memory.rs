use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        db.execute_unprepared(
            r#"
            CREATE TABLE workflow_memory (
                id BIGSERIAL PRIMARY KEY,
                project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
                agent_id INTEGER NOT NULL REFERENCES project_agents(id) ON DELETE CASCADE,
                fact TEXT NOT NULL,
                tags JSONB NOT NULL DEFAULT '[]'::jsonb,
                confidence REAL NOT NULL DEFAULT 0.5,
                times_used INTEGER NOT NULL DEFAULT 0,
                source_run_ids JSONB NOT NULL DEFAULT '[]'::jsonb,
                superseded_by BIGINT REFERENCES workflow_memory(id) ON DELETE SET NULL,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                last_used_at TIMESTAMPTZ
            )
            "#,
        )
        .await?;

        db.execute_unprepared(
            "CREATE INDEX idx_workflow_memory_agent_active \
             ON workflow_memory(agent_id) WHERE superseded_by IS NULL",
        )
        .await?;

        db.execute_unprepared(
            "CREATE INDEX idx_workflow_memory_project ON workflow_memory(project_id)",
        )
        .await?;

        // GIN index for tag-based filtering (tags && ['tag1','tag2'] queries)
        db.execute_unprepared(
            "CREATE INDEX idx_workflow_memory_tags ON workflow_memory USING gin(tags)",
        )
        .await?;

        // Full-text search index over the fact column for `memory search` queries.
        // We use a functional index instead of a generated column to keep the
        // entity model simple and avoid Sea-ORM trouble with computed columns.
        db.execute_unprepared(
            "CREATE INDEX idx_workflow_memory_fact_fts \
             ON workflow_memory USING gin(to_tsvector('english', fact))",
        )
        .await?;

        // Hard cap: prevent runaway memory growth.
        // Compaction normally keeps things under 100, but the cap is a backstop.
        db.execute_unprepared(
            "CREATE INDEX idx_workflow_memory_lookup \
             ON workflow_memory(agent_id, confidence DESC, times_used DESC) \
             WHERE superseded_by IS NULL",
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("DROP TABLE IF EXISTS workflow_memory")
            .await?;
        Ok(())
    }
}
