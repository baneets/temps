//! Log metadata service: manages log_chunks and log_events in the database
//!
//! Provides CRUD operations for chunk metadata and bulk insert for indexable log events.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, DatabaseConnection, EntityTrait, Order, QueryFilter,
    QueryOrder, QuerySelect, Set,
};
use tracing::debug;
use uuid::Uuid;

use crate::error::LogAggregatorError;
use crate::types::{ChunkMeta, LogLevel, LogLine};

/// Service for managing log metadata in the database.
///
/// Handles:
/// - Inserting chunk metadata after flush
/// - Inserting ERROR/WARN log events for fast indexed search
/// - Querying chunk metadata for search and retention
pub struct LogMetadataService {
    db: Arc<DatabaseConnection>,
}

impl LogMetadataService {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }

    /// Insert chunk metadata after a successful flush.
    pub async fn insert_chunk_meta(&self, meta: &ChunkMeta) -> Result<(), LogAggregatorError> {
        let model = temps_entities::log_chunks::ActiveModel {
            id: Set(meta.id),
            project_id: Set(meta.project_id),
            env: Set(meta.env.clone()),
            service: Set(meta.service.clone()),
            container_id: Set(meta.container_id.clone()),
            deploy_id: Set(meta.deploy_id),
            started_at: Set(meta.started_at),
            ended_at: Set(meta.ended_at),
            storage_key: Set(meta.storage_key.clone()),
            line_count: Set(meta.line_count),
            compressed_size_bytes: Set(meta.compressed_size_bytes),
            has_errors: Set(meta.has_errors),
            line_offsets: Set(meta.line_offsets.clone()),
        };

        model.insert(self.db.as_ref()).await?;

        debug!(
            chunk_id = %meta.id,
            project_id = %meta.project_id,
            service = meta.service,
            lines = meta.line_count,
            "Inserted chunk metadata"
        );

        Ok(())
    }

    /// Insert indexable (ERROR/WARN) log events for fast search.
    ///
    /// Each event references its chunk_id and line_offset for context retrieval.
    pub async fn insert_log_events(
        &self,
        chunk_id: Uuid,
        _lines: &[LogLine],
        line_offsets: &[(usize, &LogLine)],
    ) -> Result<u64, LogAggregatorError> {
        if line_offsets.is_empty() {
            return Ok(0);
        }

        let mut count = 0u64;
        for (offset, line) in line_offsets {
            if !line.level.is_indexable() {
                continue;
            }

            let model = temps_entities::log_events::ActiveModel {
                time: Set(line.ts),
                project_id: Set(line.project_id),
                service: Set(line.service.clone()),
                env: Set(line.env.clone()),
                level: Set(line.level.to_string()),
                message: Set(line.msg.clone()),
                fields: Set(line.fields.clone()),
                chunk_id: Set(chunk_id),
                line_offset: Set(*offset as i32),
                deploy_id: Set(line.deploy_id),
            };

            model.insert(self.db.as_ref()).await?;
            count += 1;
        }

        debug!(
            chunk_id = %chunk_id,
            count = count,
            "Inserted log events"
        );

        Ok(count)
    }

    /// Convenience method: insert log_events for indexable (ERROR/WARN) lines from a flushed chunk.
    ///
    /// Filters lines to those with indexable levels, computes their offset, and inserts into DB.
    /// Errors are logged but not propagated — this is used in fire-and-forget callbacks.
    pub async fn insert_log_events_from_lines(&self, chunk_id: Uuid, lines: &[LogLine]) {
        let line_offsets: Vec<(usize, &LogLine)> = lines
            .iter()
            .enumerate()
            .filter(|(_, l)| l.level.is_indexable())
            .collect();

        if line_offsets.is_empty() {
            return;
        }

        if let Err(e) = self.insert_log_events(chunk_id, lines, &line_offsets).await {
            tracing::error!(
                chunk_id = %chunk_id,
                error = %e,
                "Failed to insert log_events"
            );
        }
    }

    /// Query chunk metadata for a project within a time range.
    pub async fn find_chunks(
        &self,
        project_id: Uuid,
        service: Option<&str>,
        start_time: DateTime<Utc>,
        end_time: DateTime<Utc>,
    ) -> Result<Vec<ChunkMeta>, LogAggregatorError> {
        let mut condition = Condition::all()
            .add(temps_entities::log_chunks::Column::ProjectId.eq(project_id))
            .add(temps_entities::log_chunks::Column::StartedAt.lte(end_time))
            .add(temps_entities::log_chunks::Column::EndedAt.gte(start_time));

        if let Some(svc) = service {
            condition =
                condition.add(temps_entities::log_chunks::Column::Service.eq(svc.to_string()));
        }

        let chunks = temps_entities::log_chunks::Entity::find()
            .filter(condition)
            .order_by(temps_entities::log_chunks::Column::StartedAt, Order::Desc)
            .all(self.db.as_ref())
            .await?;

        Ok(chunks
            .into_iter()
            .map(|m| ChunkMeta {
                id: m.id,
                project_id: m.project_id,
                env: m.env,
                service: m.service,
                container_id: m.container_id,
                deploy_id: m.deploy_id,
                started_at: m.started_at,
                ended_at: m.ended_at,
                storage_key: m.storage_key,
                line_count: m.line_count,
                compressed_size_bytes: m.compressed_size_bytes,
                has_errors: m.has_errors,
                line_offsets: m.line_offsets,
            })
            .collect())
    }

    /// Query log_events (ERROR/WARN) from the TimescaleDB hypertable.
    pub async fn query_log_events(
        &self,
        project_id: Uuid,
        start_time: DateTime<Utc>,
        end_time: DateTime<Utc>,
        levels: &[LogLevel],
        services: &[String],
        deploy_id: Option<Uuid>,
        limit: u64,
    ) -> Result<Vec<temps_entities::log_events::Model>, LogAggregatorError> {
        let mut condition = Condition::all()
            .add(temps_entities::log_events::Column::ProjectId.eq(project_id))
            .add(temps_entities::log_events::Column::Time.gte(start_time))
            .add(temps_entities::log_events::Column::Time.lte(end_time));

        if !levels.is_empty() {
            let level_strings: Vec<String> = levels.iter().map(|l| l.to_string()).collect();
            condition =
                condition.add(temps_entities::log_events::Column::Level.is_in(level_strings));
        }

        if !services.is_empty() {
            condition =
                condition.add(temps_entities::log_events::Column::Service.is_in(services.to_vec()));
        }

        if let Some(deploy) = deploy_id {
            condition = condition.add(temps_entities::log_events::Column::DeployId.eq(deploy));
        }

        let events = temps_entities::log_events::Entity::find()
            .filter(condition)
            .order_by(temps_entities::log_events::Column::Time, Order::Desc)
            .limit(limit)
            .all(self.db.as_ref())
            .await?;

        Ok(events)
    }

    /// Find chunks older than a given timestamp for retention cleanup.
    pub async fn find_expired_chunks(
        &self,
        project_id: Uuid,
        before: DateTime<Utc>,
    ) -> Result<Vec<ChunkMeta>, LogAggregatorError> {
        let chunks = temps_entities::log_chunks::Entity::find()
            .filter(
                Condition::all()
                    .add(temps_entities::log_chunks::Column::ProjectId.eq(project_id))
                    .add(temps_entities::log_chunks::Column::EndedAt.lt(before)),
            )
            .all(self.db.as_ref())
            .await?;

        Ok(chunks
            .into_iter()
            .map(|m| ChunkMeta {
                id: m.id,
                project_id: m.project_id,
                env: m.env,
                service: m.service,
                container_id: m.container_id,
                deploy_id: m.deploy_id,
                started_at: m.started_at,
                ended_at: m.ended_at,
                storage_key: m.storage_key,
                line_count: m.line_count,
                compressed_size_bytes: m.compressed_size_bytes,
                has_errors: m.has_errors,
                line_offsets: m.line_offsets,
            })
            .collect())
    }

    /// Delete a chunk metadata row by ID.
    pub async fn delete_chunk_meta(&self, chunk_id: Uuid) -> Result<(), LogAggregatorError> {
        temps_entities::log_chunks::Entity::delete_by_id(chunk_id)
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    /// List all distinct project IDs that have log_chunks.
    ///
    /// Used by the retention scheduler to enumerate projects for cleanup.
    pub async fn list_distinct_projects(&self) -> Result<Vec<Uuid>, LogAggregatorError> {
        use sea_orm::FromQueryResult;

        #[derive(FromQueryResult)]
        struct ProjectIdResult {
            project_id: Uuid,
        }

        let results = temps_entities::log_chunks::Entity::find()
            .select_only()
            .column(temps_entities::log_chunks::Column::ProjectId)
            .group_by(temps_entities::log_chunks::Column::ProjectId)
            .into_model::<ProjectIdResult>()
            .all(self.db.as_ref())
            .await?;

        Ok(results.into_iter().map(|r| r.project_id).collect())
    }

    /// Get a single chunk metadata by ID.
    pub async fn get_chunk_meta(
        &self,
        chunk_id: Uuid,
    ) -> Result<Option<ChunkMeta>, LogAggregatorError> {
        let chunk = temps_entities::log_chunks::Entity::find_by_id(chunk_id)
            .one(self.db.as_ref())
            .await?;

        Ok(chunk.map(|m| ChunkMeta {
            id: m.id,
            project_id: m.project_id,
            env: m.env,
            service: m.service,
            container_id: m.container_id,
            deploy_id: m.deploy_id,
            started_at: m.started_at,
            ended_at: m.ended_at,
            storage_key: m.storage_key,
            line_count: m.line_count,
            compressed_size_bytes: m.compressed_size_bytes,
            has_errors: m.has_errors,
            line_offsets: m.line_offsets,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use temps_database::test_utils::TestDatabase;

    /// Insert a chunk_meta directly into the DB for testing.
    async fn insert_test_chunk(
        service: &LogMetadataService,
        project_id: Uuid,
        svc: &str,
        env: &str,
    ) -> ChunkMeta {
        let chunk = ChunkMeta {
            id: Uuid::new_v4(),
            project_id,
            env: env.to_string(),
            service: svc.to_string(),
            container_id: "test-container".to_string(),
            deploy_id: None,
            started_at: Utc::now() - Duration::minutes(5),
            ended_at: Utc::now(),
            storage_key: format!("test/{}/{}", project_id, Uuid::new_v4()),
            line_count: 10,
            compressed_size_bytes: 512,
            has_errors: false,
            line_offsets: vec![0],
        };
        service.insert_chunk_meta(&chunk).await.unwrap();
        chunk
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_insert_log_events_from_lines_filters_indexable() {
        let db = match TestDatabase::with_migrations().await {
            Ok(db) => db,
            Err(_) => {
                println!("Docker/DB not available, skipping test");
                return;
            }
        };

        let service = LogMetadataService::new(db.connection_arc());
        let project_id = Uuid::new_v4();
        let chunk = insert_test_chunk(&service, project_id, "api", "prod").await;

        // Create lines: 2 ERROR, 1 WARN (indexable), 5 INFO, 2 DEBUG (not indexable)
        let lines: Vec<LogLine> = vec![
            LogLine {
                ts: Utc::now(),
                stream: crate::types::LogStream::Stdout,
                level: LogLevel::Error,
                msg: "Error one".to_string(),
                fields: None,
                container_id: "test-container".to_string(),
                service: "api".to_string(),
                env: "prod".to_string(),
                project_id,
                deploy_id: None,
            },
            LogLine {
                ts: Utc::now(),
                stream: crate::types::LogStream::Stdout,
                level: LogLevel::Info,
                msg: "Info one".to_string(),
                fields: None,
                container_id: "test-container".to_string(),
                service: "api".to_string(),
                env: "prod".to_string(),
                project_id,
                deploy_id: None,
            },
            LogLine {
                ts: Utc::now(),
                stream: crate::types::LogStream::Stdout,
                level: LogLevel::Warn,
                msg: "Warning one".to_string(),
                fields: None,
                container_id: "test-container".to_string(),
                service: "api".to_string(),
                env: "prod".to_string(),
                project_id,
                deploy_id: None,
            },
            LogLine {
                ts: Utc::now(),
                stream: crate::types::LogStream::Stdout,
                level: LogLevel::Info,
                msg: "Info two".to_string(),
                fields: None,
                container_id: "test-container".to_string(),
                service: "api".to_string(),
                env: "prod".to_string(),
                project_id,
                deploy_id: None,
            },
            LogLine {
                ts: Utc::now(),
                stream: crate::types::LogStream::Stdout,
                level: LogLevel::Error,
                msg: "Error two".to_string(),
                fields: None,
                container_id: "test-container".to_string(),
                service: "api".to_string(),
                env: "prod".to_string(),
                project_id,
                deploy_id: None,
            },
        ];

        // This should only insert ERROR and WARN lines (3 out of 5)
        service.insert_log_events_from_lines(chunk.id, &lines).await;

        // Query log_events for this chunk
        let events = service
            .query_log_events(
                project_id,
                Utc::now() - Duration::hours(1),
                Utc::now() + Duration::hours(1),
                &[], // all levels
                &[],
                None,
                100,
            )
            .await
            .unwrap();

        // Only ERROR and WARN should have been inserted
        assert_eq!(
            events.len(),
            3,
            "Expected 3 indexable events (2 ERROR + 1 WARN), got {}",
            events.len()
        );

        let levels: Vec<String> = events.iter().map(|e| e.level.clone()).collect();
        let error_count = levels.iter().filter(|l| l.as_str() == "ERROR").count();
        let warn_count = levels.iter().filter(|l| l.as_str() == "WARN").count();
        assert_eq!(error_count, 2);
        assert_eq!(warn_count, 1);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_insert_log_events_from_lines_skips_all_info() {
        let db = match TestDatabase::with_migrations().await {
            Ok(db) => db,
            Err(_) => {
                println!("Docker/DB not available, skipping test");
                return;
            }
        };

        let service = LogMetadataService::new(db.connection_arc());
        let project_id = Uuid::new_v4();
        let chunk = insert_test_chunk(&service, project_id, "web", "staging").await;

        // All INFO lines — nothing should be inserted
        let lines: Vec<LogLine> = (0..5)
            .map(|i| LogLine {
                ts: Utc::now(),
                stream: crate::types::LogStream::Stdout,
                level: LogLevel::Info,
                msg: format!("Info message {}", i),
                fields: None,
                container_id: "test-container".to_string(),
                service: "web".to_string(),
                env: "staging".to_string(),
                project_id,
                deploy_id: None,
            })
            .collect();

        service.insert_log_events_from_lines(chunk.id, &lines).await;

        let events = service
            .query_log_events(
                project_id,
                Utc::now() - Duration::hours(1),
                Utc::now() + Duration::hours(1),
                &[],
                &[],
                None,
                100,
            )
            .await
            .unwrap();

        assert_eq!(
            events.len(),
            0,
            "Expected 0 events for all-INFO lines, got {}",
            events.len()
        );
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_list_distinct_projects() {
        let db = match TestDatabase::with_migrations().await {
            Ok(db) => db,
            Err(_) => {
                println!("Docker/DB not available, skipping test");
                return;
            }
        };

        let service = LogMetadataService::new(db.connection_arc());

        let project_a = Uuid::new_v4();
        let project_b = Uuid::new_v4();

        // Insert chunks for project A (2 chunks) and project B (1 chunk)
        insert_test_chunk(&service, project_a, "api", "prod").await;
        insert_test_chunk(&service, project_a, "worker", "prod").await;
        insert_test_chunk(&service, project_b, "web", "staging").await;

        let projects = service.list_distinct_projects().await.unwrap();

        assert!(projects.contains(&project_a), "Should contain project_a");
        assert!(projects.contains(&project_b), "Should contain project_b");
        // Even though project_a has 2 chunks, it should only appear once
        assert_eq!(
            projects.iter().filter(|p| **p == project_a).count(),
            1,
            "project_a should appear exactly once"
        );
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_list_distinct_projects_empty() {
        let db = match TestDatabase::with_migrations().await {
            Ok(db) => db,
            Err(_) => {
                println!("Docker/DB not available, skipping test");
                return;
            }
        };

        let service = LogMetadataService::new(db.connection_arc());
        let projects = service.list_distinct_projects().await.unwrap();
        assert!(
            projects.is_empty(),
            "Should return empty list when no chunks exist"
        );
    }
}
