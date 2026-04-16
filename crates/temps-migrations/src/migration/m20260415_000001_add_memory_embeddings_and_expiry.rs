//! Add embeddings + expiry to workflow_memory.
//!
//! Two new columns:
//!
//! 1. `embedding` — optional BYTEA holding a raw little-endian f32 vector.
//!    We intentionally do NOT use the pgvector type here: pgvector is
//!    optional in our deploys (the bundled `timescale/timescaledb-ha:pg18`
//!    image ships it, but bare Postgres does not), and requiring it as a
//!    hard dependency for the core schema would break self-hosters on
//!    stock Postgres. Consumers that want vector similarity can add a
//!    view with `vector(embedding)` on top; the raw bytes are portable.
//!
//! 2. `expires_at` — optional TIMESTAMPTZ. When set, the fact is eligible
//!    for the compaction sweep (implemented outside the schema). Keeping
//!    expiry as a column rather than a separate `memory_ttl` table means
//!    the sweep is a single index-backed range query instead of a join.
//!
//! Both columns are nullable with no default, so existing rows are
//! unaffected and the migration is online-safe on big tables.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        db.execute_unprepared(
            r#"
            ALTER TABLE workflow_memory
                ADD COLUMN IF NOT EXISTS embedding BYTEA,
                ADD COLUMN IF NOT EXISTS expires_at TIMESTAMPTZ
            "#,
        )
        .await?;

        // Partial index for the compaction sweep: only non-null, non-
        // superseded rows with an expiry in the past matter. Indexing the
        // whole column would be wasteful since most rows never expire.
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS idx_workflow_memory_expires_at \
             ON workflow_memory(expires_at) \
             WHERE expires_at IS NOT NULL AND superseded_by IS NULL",
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        db.execute_unprepared("DROP INDEX IF EXISTS idx_workflow_memory_expires_at")
            .await?;
        db.execute_unprepared(
            "ALTER TABLE workflow_memory \
             DROP COLUMN IF EXISTS expires_at, \
             DROP COLUMN IF EXISTS embedding",
        )
        .await?;

        Ok(())
    }
}
