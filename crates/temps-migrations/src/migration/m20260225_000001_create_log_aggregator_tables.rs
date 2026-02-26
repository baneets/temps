//! Migration to create log aggregator tables
//!
//! Creates:
//! - `log_chunks` table for chunk metadata
//! - `log_events` hypertable for indexed ERROR/WARN log events
//! - Appropriate indexes for efficient querying
//! - TimescaleDB retention and compression policies for log_events

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // ── log_chunks table ────────────────────────────────────────────
        manager
            .create_table(
                Table::create()
                    .table(LogChunks::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(LogChunks::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(LogChunks::ProjectId).uuid().not_null())
                    .col(ColumnDef::new(LogChunks::Env).string().not_null())
                    .col(ColumnDef::new(LogChunks::Service).string().not_null())
                    .col(ColumnDef::new(LogChunks::ContainerId).string().not_null())
                    .col(ColumnDef::new(LogChunks::DeployId).uuid().null())
                    .col(
                        ColumnDef::new(LogChunks::StartedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(LogChunks::EndedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(ColumnDef::new(LogChunks::StorageKey).string().not_null())
                    .col(ColumnDef::new(LogChunks::LineCount).integer().not_null())
                    .col(
                        ColumnDef::new(LogChunks::CompressedSizeBytes)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(LogChunks::HasErrors)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(
                        ColumnDef::new(LogChunks::LineOffsets)
                            .array(ColumnType::Integer)
                            .not_null()
                            .default(Expr::cust("ARRAY[]::INTEGER[]")),
                    )
                    .to_owned(),
            )
            .await?;

        // Primary lookup index: (project_id, service, started_at DESC)
        manager
            .create_index(
                Index::create()
                    .name("idx_log_chunks_project_service_time")
                    .table(LogChunks::Table)
                    .col(LogChunks::ProjectId)
                    .col(LogChunks::Service)
                    .col(LogChunks::StartedAt)
                    .to_owned(),
            )
            .await?;

        // Deploy-based lookup
        manager
            .create_index(
                Index::create()
                    .name("idx_log_chunks_deploy_id")
                    .table(LogChunks::Table)
                    .col(LogChunks::DeployId)
                    .to_owned(),
            )
            .await?;

        // ── log_events table ────────────────────────────────────────────
        // Note: TimescaleDB hypertable creation and policies are applied
        // via raw SQL since sea-orm-migration doesn't support TimescaleDB extensions
        manager
            .create_table(
                Table::create()
                    .table(LogEvents::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(LogEvents::Time)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(ColumnDef::new(LogEvents::ProjectId).uuid().not_null())
                    .col(ColumnDef::new(LogEvents::Service).string().not_null())
                    .col(ColumnDef::new(LogEvents::Env).string().not_null())
                    .col(ColumnDef::new(LogEvents::Level).string().not_null())
                    .col(ColumnDef::new(LogEvents::Message).text().not_null())
                    .col(ColumnDef::new(LogEvents::Fields).json_binary().null())
                    .col(ColumnDef::new(LogEvents::ChunkId).uuid().not_null())
                    .col(ColumnDef::new(LogEvents::LineOffset).integer().not_null())
                    .col(ColumnDef::new(LogEvents::DeployId).uuid().null())
                    .to_owned(),
            )
            .await?;

        // Convert to TimescaleDB hypertable
        let db = manager.get_connection();
        db.execute_unprepared(
            "SELECT create_hypertable('log_events', 'time', if_not_exists => TRUE)",
        )
        .await?;

        // Retention policy: 7 days
        db.execute_unprepared(
            "SELECT add_retention_policy('log_events', INTERVAL '7 days', if_not_exists => TRUE)",
        )
        .await?;

        // Compression policy: compress chunks older than 1 day
        db.execute_unprepared(
            "ALTER TABLE log_events SET (
                timescaledb.compress,
                timescaledb.compress_segmentby = 'project_id, service',
                timescaledb.compress_orderby = 'time DESC'
            )",
        )
        .await?;

        db.execute_unprepared(
            "SELECT add_compression_policy('log_events', INTERVAL '1 day', if_not_exists => TRUE)",
        )
        .await?;

        // Indexes for log_events
        manager
            .create_index(
                Index::create()
                    .name("idx_log_events_project_time_level")
                    .table(LogEvents::Table)
                    .col(LogEvents::ProjectId)
                    .col(LogEvents::Time)
                    .col(LogEvents::Level)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Drop indexes
        manager
            .drop_index(
                Index::drop()
                    .name("idx_log_events_project_time_level")
                    .table(LogEvents::Table)
                    .to_owned(),
            )
            .await?;

        // Remove TimescaleDB policies before dropping
        let db = manager.get_connection();
        let _ = db
            .execute_unprepared("SELECT remove_retention_policy('log_events', if_exists => TRUE)")
            .await;
        let _ = db
            .execute_unprepared("SELECT remove_compression_policy('log_events', if_exists => TRUE)")
            .await;

        // Drop tables
        manager
            .drop_table(Table::drop().table(LogEvents::Table).to_owned())
            .await?;

        manager
            .drop_index(
                Index::drop()
                    .name("idx_log_chunks_deploy_id")
                    .table(LogChunks::Table)
                    .to_owned(),
            )
            .await?;

        manager
            .drop_index(
                Index::drop()
                    .name("idx_log_chunks_project_service_time")
                    .table(LogChunks::Table)
                    .to_owned(),
            )
            .await?;

        manager
            .drop_table(Table::drop().table(LogChunks::Table).to_owned())
            .await?;

        Ok(())
    }
}

#[derive(DeriveIden)]
enum LogChunks {
    Table,
    Id,
    ProjectId,
    Env,
    Service,
    ContainerId,
    DeployId,
    StartedAt,
    EndedAt,
    StorageKey,
    LineCount,
    CompressedSizeBytes,
    HasErrors,
    LineOffsets,
}

#[derive(DeriveIden)]
enum LogEvents {
    Table,
    Time,
    ProjectId,
    Service,
    Env,
    Level,
    Message,
    Fields,
    ChunkId,
    LineOffset,
    DeployId,
}
