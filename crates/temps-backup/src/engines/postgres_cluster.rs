//! `PostgresClusterEngine`: `BackupEngine` for Postgres cluster (HA) topology
//! (ADR-014 Phase 3 §"Postgres engines").
//!
//! Steps: `find_primary` → `preflight` → `walg_push` → `record_lsn` → `metadata`.
//!
//! ## Design notes
//!
//! Extends `PostgresWalgEngine` with a `find_primary` step that locates the
//! primary member in a pg_auto_failover cluster. The primary member's container
//! name is stored in `durable_state` so subsequent steps can target it directly.
//!
//! Reference: `postgres_cluster.rs` cluster backup path and
//! `backup.rs:4413` (cluster topology routing in `backup_external_service`).
//!
//! ## Heartbeat discipline
//!
//! `walg_push` uses the mpsc + select pattern from `control_plane.rs:213–254`.
//!
//! ## Idempotence
//!
//! - `find_primary`: always re-runs (DB lookup is idempotent).
//! - `preflight`, `walg_push`, `record_lsn`, `metadata`: same as `PostgresWalgEngine`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use aws_sdk_s3::Client as S3Client;
use bollard::container::LogOutput;
use bollard::exec::StartExecResults;
use chrono::Utc;
use futures::stream::BoxStream;
use futures::StreamExt;
use sea_orm::{DatabaseConnection, EntityTrait};
use serde_json::{json, Value};
use tracing::{debug, error, info, warn};

use super::ring_buffer::RingBuffer;
use temps_backup_core::{BackupContext, BackupEngine, BackupEngineError, StepCursor, StepEvent};
use temps_core::EncryptionService;

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(120);

const STEPS: &[&str] = &[
    "find_primary",
    "preflight",
    "walg_push",
    "record_lsn",
    "metadata",
];

const DS_S3_KEY: &str = "s3_key";
const DS_BUCKET: &str = "bucket";
const DS_SIZE_BYTES: &str = "size_bytes";
const DS_WALG_PREFIX: &str = "walg_prefix";
const DS_LSN: &str = "lsn";
const DS_PRIMARY_CONTAINER: &str = "primary_container";

// ── Dependencies ─────────────────────────────────────────────────────────────

pub struct PostgresClusterDeps {
    pub db: Arc<DatabaseConnection>,
    pub encryption_service: Arc<EncryptionService>,
    pub docker: bollard::Docker,
}

// ── Engine ────────────────────────────────────────────────────────────────────

/// `BackupEngine` for Postgres cluster (pg_auto_failover) external services.
///
/// Adds a `find_primary` step before `preflight` to locate the current primary
/// in the cluster's `service_members` table.
/// Reference: `backup.rs:4413` (cluster WAL-G dispatch).
pub struct PostgresClusterEngine {
    deps: Arc<PostgresClusterDeps>,
}

impl PostgresClusterEngine {
    pub fn new(deps: PostgresClusterDeps) -> Self {
        Self {
            deps: Arc::new(deps),
        }
    }
}

#[async_trait::async_trait]
impl BackupEngine for PostgresClusterEngine {
    fn engine(&self) -> &'static str {
        "postgres_cluster"
    }
    fn steps(&self) -> &'static [&'static str] {
        STEPS
    }

    fn execute<'a>(
        &'a self,
        ctx: &'a BackupContext,
        cursor: StepCursor,
    ) -> BoxStream<'a, Result<StepEvent, BackupEngineError>> {
        let deps = Arc::clone(&self.deps);
        let job_id = ctx.job_id;
        let attempt = ctx.attempt;
        let params = ctx.params.clone();
        let cancel = ctx.cancel.clone();

        Box::pin(async_stream::try_stream! {
            let resume_from = cursor.current_step.clone();
            let mut accumulated_state = cursor.durable_state.clone();

            let start_idx = if let Some(ref last) = resume_from {
                STEPS.iter().position(|&s| s == last.as_str())
                    .map(|i| i + 1)
                    .ok_or_else(|| BackupEngineError::StepFailed {
                        job_id, step: last.clone(),
                        reason: format!("unknown step '{}'; known: {:?}", last, STEPS),
                    })?
            } else { 0 };

            let service_id: i32 = params.get("service_id").and_then(|v| v.as_i64()).map(|v| v as i32)
                .ok_or_else(|| BackupEngineError::Preflight { job_id, reason: "params.service_id missing".into() })?;
            let s3_source_id: i32 = params.get("s3_source_id").and_then(|v| v.as_i64()).map(|v| v as i32)
                .ok_or_else(|| BackupEngineError::Preflight { job_id, reason: "params.s3_source_id missing".into() })?;

            for step in &STEPS[start_idx..] {
                if cancel.is_cancelled() {
                    debug!(job_id, step, "PostgresClusterEngine: cancellation requested");
                    return;
                }
                info!(job_id, attempt, step, "PostgresClusterEngine: executing step");

                match *step {
                    "find_primary" => {
                        let state = step_find_primary(job_id, service_id, accumulated_state.clone(), &deps).await?;
                        accumulated_state = state.clone();
                        let primary = accumulated_state.get(DS_PRIMARY_CONTAINER).and_then(|v| v.as_str()).unwrap_or("unknown");
                        yield StepEvent::StepCompleted {
                            step: "find_primary".into(),
                            durable_state: state,
                            message: Some(format!("primary container: {}", primary)),
                        };
                    }

                    "preflight" => {
                        let state = step_preflight(job_id, service_id, s3_source_id, accumulated_state.clone(), &deps).await?;
                        accumulated_state = state.clone();
                        yield StepEvent::StepCompleted {
                            step: "preflight".into(),
                            durable_state: state,
                            message: Some(format!("service {} and S3 source {} validated", service_id, s3_source_id)),
                        };
                    }

                    "walg_push" => {
                        let (heartbeat_tx, mut heartbeat_rx) = tokio::sync::mpsc::channel::<()>(8);
                        let mut step_fut = std::pin::pin!(step_walg_push(
                            job_id, accumulated_state.clone(), Arc::clone(&deps), cancel.clone(), heartbeat_tx,
                        ));

                        let step_result: Result<Value, BackupEngineError> = loop {
                            tokio::select! {
                                biased;
                                Some(()) = heartbeat_rx.recv() => {
                                    debug!(job_id, "PostgresClusterEngine walg_push: Heartbeat");
                                    yield StepEvent::Heartbeat;
                                }
                                result = &mut step_fut => {
                                    while let Ok(()) = heartbeat_rx.try_recv() {
                                        yield StepEvent::Heartbeat;
                                    }
                                    break result;
                                }
                            }
                        };
                        let state = step_result?;
                        accumulated_state = state.clone();
                        yield StepEvent::StepCompleted {
                            step: "walg_push".into(),
                            durable_state: state,
                            message: Some("wal-g backup-push completed on primary".into()),
                        };
                    }

                    "record_lsn" => {
                        let state = step_record_lsn(job_id, accumulated_state.clone(), &deps).await?;
                        accumulated_state = state.clone();
                        yield StepEvent::StepCompleted {
                            step: "record_lsn".into(),
                            durable_state: state,
                            message: Some("LSN recorded from primary".into()),
                        };
                    }

                    "metadata" => {
                        step_metadata(job_id, s3_source_id, accumulated_state.clone(), &deps).await?;
                        yield StepEvent::StepCompleted {
                            step: "metadata".into(),
                            durable_state: accumulated_state.clone(),
                            message: Some("metadata.json written".into()),
                        };
                        let location = accumulated_state.get(DS_WALG_PREFIX).and_then(|v| v.as_str()).unwrap_or("").to_string();
                        let size_bytes = accumulated_state.get(DS_SIZE_BYTES).and_then(|v| v.as_i64());
                        info!(job_id, %location, ?size_bytes, "PostgresClusterEngine: Done");
                        yield StepEvent::Done { location, size_bytes, compression: "lz4".into() };
                    }

                    other => {
                        Err(BackupEngineError::StepFailed {
                            job_id, step: other.to_string(), reason: format!("unexpected step '{}'", other),
                        })?;
                    }
                }
            }
        })
    }

    async fn rollback(
        &self,
        ctx: &BackupContext,
        _cursor: StepCursor,
    ) -> Result<(), BackupEngineError> {
        info!(
            job_id = ctx.job_id,
            "PostgresClusterEngine rollback: WAL-G manages S3 retention"
        );
        Ok(())
    }
}

// ── Step helpers ──────────────────────────────────────────────────────────────

/// `find_primary` step: look up the primary member in `service_members`.
/// Reference: `backup.rs:4413`.
async fn step_find_primary(
    job_id: i64,
    service_id: i32,
    durable_state: Value,
    deps: &PostgresClusterDeps,
) -> Result<Value, BackupEngineError> {
    use sea_orm::{ColumnTrait, QueryFilter};

    // Query service_members for the primary.
    let primary_member = temps_entities::service_members::Entity::find()
        .filter(temps_entities::service_members::Column::ServiceId.eq(service_id))
        .filter(temps_entities::service_members::Column::Role.eq("primary"))
        .filter(temps_entities::service_members::Column::Status.eq("running"))
        .one(deps.db.as_ref())
        .await
        .map_err(|e| BackupEngineError::StepFailed {
            job_id,
            step: "find_primary".into(),
            reason: format!("db error: {}", e),
        })?
        .ok_or_else(|| BackupEngineError::StepFailed {
            job_id,
            step: "find_primary".into(),
            reason: format!(
                "no running primary found for cluster service {}",
                service_id
            ),
        })?;

    let primary_container = primary_member.container_name.clone();

    info!(
        job_id,
        service_id,
        container = %primary_container,
        ordinal = primary_member.ordinal,
        "PostgresClusterEngine find_primary: found",
    );

    let mut new_state = durable_state.clone();
    if let Some(obj) = new_state.as_object_mut() {
        obj.insert(DS_PRIMARY_CONTAINER.to_string(), json!(primary_container));
        obj.insert("service_id".to_string(), json!(service_id));
    }
    Ok(new_state)
}

async fn step_preflight(
    job_id: i64,
    service_id: i32,
    s3_source_id: i32,
    durable_state: Value,
    deps: &PostgresClusterDeps,
) -> Result<Value, BackupEngineError> {
    let service = temps_entities::external_services::Entity::find_by_id(service_id)
        .one(deps.db.as_ref())
        .await
        .map_err(|e| BackupEngineError::Preflight {
            job_id,
            reason: format!("db service {}: {}", service_id, e),
        })?
        .ok_or_else(|| BackupEngineError::Preflight {
            job_id,
            reason: format!("service {} not found", service_id),
        })?;

    let s3_source = temps_entities::s3_sources::Entity::find_by_id(s3_source_id)
        .one(deps.db.as_ref())
        .await
        .map_err(|e| BackupEngineError::Preflight {
            job_id,
            reason: format!("db s3_source {}: {}", s3_source_id, e),
        })?
        .ok_or_else(|| BackupEngineError::Preflight {
            job_id,
            reason: format!("s3_source {} not found", s3_source_id),
        })?;

    let s3_client = build_s3_client_from_source(job_id, &s3_source, deps)?;
    s3_client
        .head_bucket()
        .bucket(&s3_source.bucket_name)
        .send()
        .await
        .map_err(|e| BackupEngineError::Preflight {
            job_id,
            reason: format!("bucket not reachable: {}", e),
        })?;

    let subpath_root = format!("external_services/postgres/{}", service.name);
    let walg_prefix = format!(
        "s3://{}/{}/walg",
        s3_source.bucket_name,
        subpath_root.trim_matches('/')
    );
    let s3_list_prefix = format!("{}/walg/", subpath_root.trim_matches('/'));

    let mut new_state = durable_state.clone();
    if let Some(obj) = new_state.as_object_mut() {
        obj.insert(DS_S3_KEY.to_string(), json!(walg_prefix.clone()));
        obj.insert(DS_BUCKET.to_string(), json!(s3_source.bucket_name));
        obj.insert(DS_WALG_PREFIX.to_string(), json!(walg_prefix.clone()));
        obj.insert("s3_list_prefix".to_string(), json!(s3_list_prefix));
        obj.insert("s3_source_id".to_string(), json!(s3_source_id));
        obj.insert("service_name".to_string(), json!(service.name));
    }

    info!(job_id, %walg_prefix, "PostgresClusterEngine preflight: validated");
    Ok(new_state)
}

async fn step_walg_push(
    job_id: i64,
    durable_state: Value,
    deps: Arc<PostgresClusterDeps>,
    _cancel: tokio_util::sync::CancellationToken,
    heartbeat_tx: tokio::sync::mpsc::Sender<()>,
) -> Result<Value, BackupEngineError> {
    let primary_container = durable_state
        .get(DS_PRIMARY_CONTAINER)
        .and_then(|v| v.as_str())
        .ok_or_else(|| BackupEngineError::StepFailed {
            job_id,
            step: "walg_push".into(),
            reason: "missing primary_container (find_primary not done)".into(),
        })?
        .to_string();

    let walg_prefix = durable_state
        .get(DS_WALG_PREFIX)
        .and_then(|v| v.as_str())
        .ok_or_else(|| BackupEngineError::StepFailed {
            job_id,
            step: "walg_push".into(),
            reason: "missing walg_prefix".into(),
        })?
        .to_string();

    let s3_source_id: i32 = durable_state
        .get("s3_source_id")
        .and_then(|v| v.as_i64())
        .map(|v| v as i32)
        .ok_or_else(|| BackupEngineError::StepFailed {
            job_id,
            step: "walg_push".into(),
            reason: "missing s3_source_id".into(),
        })?;
    let service_id: i32 = durable_state
        .get("service_id")
        .and_then(|v| v.as_i64())
        .map(|v| v as i32)
        .ok_or_else(|| BackupEngineError::StepFailed {
            job_id,
            step: "walg_push".into(),
            reason: "missing service_id".into(),
        })?;

    let service = temps_entities::external_services::Entity::find_by_id(service_id)
        .one(deps.db.as_ref())
        .await
        .map_err(|e| BackupEngineError::StepFailed {
            job_id,
            step: "walg_push".into(),
            reason: format!("db service: {}", e),
        })?
        .ok_or_else(|| BackupEngineError::StepFailed {
            job_id,
            step: "walg_push".into(),
            reason: "service not found".into(),
        })?;

    let s3_source = temps_entities::s3_sources::Entity::find_by_id(s3_source_id)
        .one(deps.db.as_ref())
        .await
        .map_err(|e| BackupEngineError::StepFailed {
            job_id,
            step: "walg_push".into(),
            reason: format!("db s3_source: {}", e),
        })?
        .ok_or_else(|| BackupEngineError::StepFailed {
            job_id,
            step: "walg_push".into(),
            reason: "s3_source not found".into(),
        })?;

    let access_key = deps
        .encryption_service
        .decrypt_string(&s3_source.access_key_id)
        .map_err(|e| BackupEngineError::StepFailed {
            job_id,
            step: "walg_push".into(),
            reason: format!("decrypt ak: {}", e),
        })?;
    let secret_key = deps
        .encryption_service
        .decrypt_string(&s3_source.secret_key)
        .map_err(|e| BackupEngineError::StepFailed {
            job_id,
            step: "walg_push".into(),
            reason: format!("decrypt sk: {}", e),
        })?;

    let config_json = deps
        .encryption_service
        .decrypt_string(service.config.as_deref().unwrap_or("{}"))
        .unwrap_or_else(|_| "{}".to_string());
    let params: Value = serde_json::from_str(&config_json).unwrap_or_else(|_| json!({}));
    let username = params
        .get("username")
        .and_then(|v| v.as_str())
        .unwrap_or("postgres")
        .to_string();
    let password = params
        .get("password")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let database = params
        .get("database")
        .or_else(|| params.get("db_name"))
        .and_then(|v| v.as_str())
        .unwrap_or("postgres")
        .to_string();

    // WAL-G memory tuning — see comment on the same block in
    // `postgres_walg.rs` for the full rationale. Caps RSS at ~512 MiB peak
    // (UPLOAD_CONCURRENCY × TAR_SIZE_THRESHOLD) so the sidecar isn't
    // OOM-killed under the default 1 GiB container memory limit.
    let mut walg_env: Vec<String> = vec![
        format!("WALG_S3_PREFIX={}", walg_prefix),
        format!("AWS_ACCESS_KEY_ID={}", access_key),
        format!("AWS_SECRET_ACCESS_KEY={}", secret_key),
        format!("AWS_REGION={}", s3_source.region),
        format!("PGUSER={}", username),
        format!("PGPASSWORD={}", password),
        format!("PGDATABASE={}", database),
        "PGHOST=localhost".to_string(),
        "PGPORT=5432".to_string(),
        "WALG_UPLOAD_CONCURRENCY=4".to_string(),
        "WALG_UPLOAD_DISK_CONCURRENCY=1".to_string(),
        "WALG_UPLOAD_QUEUE=2".to_string(),
        "WALG_TAR_SIZE_THRESHOLD=134217728".to_string(),
    ];
    if let Some(ep) = &s3_source.endpoint {
        let url = if ep.starts_with("http") {
            ep.clone()
        } else {
            format!("http://{}", ep)
        };
        walg_env.push(format!("AWS_ENDPOINT={}", url));
    }
    if s3_source.force_path_style.unwrap_or(true) {
        walg_env.push("AWS_S3_FORCE_PATH_STYLE=true".to_string());
    }

    let env_refs: Vec<&str> = walg_env.iter().map(|s| s.as_str()).collect();
    // Capture stdout + stderr so failures are diagnosable (no `2>&1` in cmd).
    let exec = deps
        .docker
        .create_exec(
            &primary_container,
            bollard::exec::CreateExecOptions {
                cmd: Some(vec!["sh", "-c", "wal-g backup-push $PGDATA"]),
                attach_stdout: Some(true),
                attach_stderr: Some(true),
                env: Some(env_refs),
                ..Default::default()
            },
        )
        .await
        .map_err(|e| BackupEngineError::StepFailed {
            job_id,
            step: "walg_push".into(),
            reason: format!("create exec on {}: {}", primary_container, e),
        })?;

    let stream_result = deps
        .docker
        .start_exec(
            &exec.id,
            Some(bollard::exec::StartExecOptions {
                detach: false,
                ..Default::default()
            }),
        )
        .await
        .map_err(|e| BackupEngineError::StepFailed {
            job_id,
            step: "walg_push".into(),
            reason: format!("start exec: {}", e),
        })?;

    let mut stdout_tail = RingBuffer::with_capacity(64 * 1024);
    let mut stderr_tail = RingBuffer::with_capacity(64 * 1024);
    let mut last_hb = Instant::now();

    if let StartExecResults::Attached { mut output, .. } = stream_result {
        while let Some(item) = output.next().await {
            match item {
                Ok(LogOutput::StdOut { message }) => stdout_tail.append(&message),
                Ok(LogOutput::StdErr { message }) => stderr_tail.append(&message),
                Ok(_) => {}
                Err(e) => {
                    error!(job_id, engine = "postgres_cluster", container = %primary_container, "walg_push exec stream error: {}", e);
                    break;
                }
            }
            if last_hb.elapsed() >= HEARTBEAT_INTERVAL {
                let _ = heartbeat_tx.try_send(());
                last_hb = Instant::now();
            }
        }
    }

    let inspect =
        deps.docker
            .inspect_exec(&exec.id)
            .await
            .map_err(|e| BackupEngineError::StepFailed {
                job_id,
                step: "walg_push".into(),
                reason: format!("inspect exec: {}", e),
            })?;
    let exit_code = inspect.exit_code.unwrap_or(-1);
    let stdout = stdout_tail.into_string_lossy();
    let stderr = stderr_tail.into_string_lossy();

    if exit_code != 0 {
        return Err(BackupEngineError::StepFailed {
            job_id,
            step: "walg_push".into(),
            reason: format!(
                "wal-g backup-push exited with code {}. stderr: {}. stdout: {}",
                exit_code,
                if stderr.trim().is_empty() {
                    "<empty>"
                } else {
                    stderr.trim()
                },
                if stdout.trim().is_empty() {
                    "<empty>"
                } else {
                    stdout.trim()
                },
            ),
        });
    }

    if !stderr.trim().is_empty() {
        info!(
            job_id,
            engine = "postgres_cluster",
            container = %primary_container,
            "walg_push stderr (warnings): {}",
            stderr.trim(),
        );
    }

    let bucket = durable_state
        .get(DS_BUCKET)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let s3_list_prefix = durable_state
        .get("s3_list_prefix")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let s3_client = build_s3_client_from_source(job_id, &s3_source, &deps)?;
    let size_bytes = match list_total_s3_size(&s3_client, &bucket, &s3_list_prefix).await {
        Ok(n) => Some(n),
        Err(e) => {
            warn!(job_id, error = %e, "walg_push: could not compute size");
            None
        }
    };

    let mut new_state = durable_state.clone();
    if let Some(obj) = new_state.as_object_mut() {
        if let Some(sz) = size_bytes {
            obj.insert(DS_SIZE_BYTES.to_string(), json!(sz));
        }
    }

    info!(job_id, %primary_container, %walg_prefix, ?size_bytes, "PostgresClusterEngine walg_push: completed");
    Ok(new_state)
}

async fn step_record_lsn(
    job_id: i64,
    durable_state: Value,
    deps: &PostgresClusterDeps,
) -> Result<Value, BackupEngineError> {
    if durable_state.get(DS_LSN).is_some() {
        return Ok(durable_state);
    }

    let primary_container = durable_state
        .get(DS_PRIMARY_CONTAINER)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let service_id: i32 = durable_state
        .get("service_id")
        .and_then(|v| v.as_i64())
        .map(|v| v as i32)
        .unwrap_or(0);

    if service_id > 0 && !primary_container.is_empty() {
        let service = temps_entities::external_services::Entity::find_by_id(service_id)
            .one(deps.db.as_ref())
            .await
            .ok()
            .flatten();
        if let Some(svc) = service {
            let config_json = deps
                .encryption_service
                .decrypt_string(svc.config.as_deref().unwrap_or("{}"))
                .unwrap_or_else(|_| "{}".to_string());
            let params: Value = serde_json::from_str(&config_json).unwrap_or_else(|_| json!({}));
            let username = params
                .get("username")
                .and_then(|v| v.as_str())
                .unwrap_or("postgres")
                .to_string();
            let password = params
                .get("password")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let database = params
                .get("database")
                .or_else(|| params.get("db_name"))
                .and_then(|v| v.as_str())
                .unwrap_or("postgres")
                .to_string();

            let cmd = format!(
                "PGPASSWORD={} psql -U {} -d {} -t -c 'SELECT pg_current_wal_lsn()'",
                password, username, database
            );
            if let Ok(lsn) =
                run_command_in_container(job_id, &deps.docker, &primary_container, &cmd).await
            {
                let lsn = lsn.trim().to_string();
                let mut new_state = durable_state.clone();
                if let Some(obj) = new_state.as_object_mut() {
                    obj.insert(DS_LSN.to_string(), json!(lsn));
                }
                info!(job_id, %lsn, "PostgresClusterEngine record_lsn: recorded");
                return Ok(new_state);
            }
        }
    }

    Ok(durable_state)
}

async fn step_metadata(
    job_id: i64,
    s3_source_id: i32,
    durable_state: Value,
    deps: &PostgresClusterDeps,
) -> Result<(), BackupEngineError> {
    let walg_prefix = durable_state
        .get(DS_WALG_PREFIX)
        .and_then(|v| v.as_str())
        .ok_or_else(|| BackupEngineError::StepFailed {
            job_id,
            step: "metadata".into(),
            reason: "missing walg_prefix".into(),
        })?
        .to_string();
    let bucket = durable_state
        .get(DS_BUCKET)
        .and_then(|v| v.as_str())
        .ok_or_else(|| BackupEngineError::StepFailed {
            job_id,
            step: "metadata".into(),
            reason: "missing bucket".into(),
        })?
        .to_string();
    let s3_list_prefix = durable_state
        .get("s3_list_prefix")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let s3_client =
        build_s3_client(s3_source_id, deps)
            .await
            .map_err(|e| BackupEngineError::S3 {
                job_id,
                reason: format!("build S3 client: {}", e),
            })?;

    let metadata_key = format!("{}metadata.json", s3_list_prefix.trim_end_matches('/'));
    let body = serde_json::to_vec(&json!({
        "type": "full",
        "engine": "postgres_cluster",
        "backup_tool": "wal-g",
        "created_at": Utc::now().to_rfc3339(),
        "size_bytes": durable_state.get(DS_SIZE_BYTES).and_then(|v| v.as_i64()),
        "compression_type": "lz4",
        "lsn": durable_state.get(DS_LSN).and_then(|v| v.as_str()).unwrap_or(""),
        "primary_container": durable_state.get(DS_PRIMARY_CONTAINER).and_then(|v| v.as_str()).unwrap_or(""),
        "source": { "id": s3_source_id },
        "s3_location": walg_prefix,
    })).map_err(|e| BackupEngineError::StepFailed { job_id, step: "metadata".into(), reason: format!("serialize: {}", e) })?;

    s3_client
        .put_object()
        .bucket(&bucket)
        .key(&metadata_key)
        .body(body.into())
        .content_type("application/json")
        .send()
        .await
        .map_err(|e| BackupEngineError::S3 {
            job_id,
            reason: format!("upload metadata.json: {}", e),
        })?;

    info!(job_id, %bucket, key = %metadata_key, "PostgresClusterEngine metadata: written");
    Ok(())
}

// ── Utility helpers ───────────────────────────────────────────────────────────

async fn run_command_in_container(
    job_id: i64,
    docker: &bollard::Docker,
    container_name: &str,
    cmd: &str,
) -> Result<String, BackupEngineError> {
    use futures::StreamExt;

    let exec = docker
        .create_exec(
            container_name,
            bollard::exec::CreateExecOptions {
                cmd: Some(vec!["sh", "-c", cmd]),
                attach_stdout: Some(true),
                attach_stderr: Some(false),
                ..Default::default()
            },
        )
        .await
        .map_err(|e| BackupEngineError::StepFailed {
            job_id,
            step: "record_lsn".into(),
            reason: format!("create exec: {}", e),
        })?;

    let output =
        docker
            .start_exec(&exec.id, None)
            .await
            .map_err(|e| BackupEngineError::StepFailed {
                job_id,
                step: "record_lsn".into(),
                reason: format!("start exec: {}", e),
            })?;

    let mut result = String::new();
    if let bollard::exec::StartExecResults::Attached { mut output, .. } = output {
        while let Some(Ok(msg)) = output.next().await {
            if let bollard::container::LogOutput::StdOut { message } = msg {
                result.push_str(&String::from_utf8_lossy(&message));
            }
        }
    }
    Ok(result)
}

async fn build_s3_client(
    s3_source_id: i32,
    deps: &PostgresClusterDeps,
) -> Result<S3Client, BackupEngineError> {
    let src = temps_entities::s3_sources::Entity::find_by_id(s3_source_id)
        .one(deps.db.as_ref())
        .await
        .map_err(|e| BackupEngineError::S3 {
            job_id: 0,
            reason: format!("db: {}", e),
        })?
        .ok_or_else(|| BackupEngineError::S3 {
            job_id: 0,
            reason: format!("s3_source {} not found", s3_source_id),
        })?;
    build_s3_client_from_source(0, &src, deps)
}

fn build_s3_client_from_source(
    job_id: i64,
    s3_source: &temps_entities::s3_sources::Model,
    deps: &PostgresClusterDeps,
) -> Result<S3Client, BackupEngineError> {
    use aws_sdk_s3::Config;
    let ak = deps
        .encryption_service
        .decrypt_string(&s3_source.access_key_id)
        .map_err(|e| BackupEngineError::Preflight {
            job_id,
            reason: format!("decrypt ak: {}", e),
        })?;
    let sk = deps
        .encryption_service
        .decrypt_string(&s3_source.secret_key)
        .map_err(|e| BackupEngineError::Preflight {
            job_id,
            reason: format!("decrypt sk: {}", e),
        })?;
    let creds = aws_sdk_s3::config::Credentials::new(ak, sk, None, None, "postgres-cluster-engine");
    let mut b = Config::builder()
        .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
        .region(aws_sdk_s3::config::Region::new(s3_source.region.clone()))
        .force_path_style(s3_source.force_path_style.unwrap_or(true))
        .credentials_provider(creds);
    if let Some(ep) = &s3_source.endpoint {
        let url = if ep.starts_with("http") {
            ep.clone()
        } else {
            format!("http://{}", ep)
        };
        b = b.endpoint_url(url);
    }
    Ok(S3Client::from_conf(b.build()))
}

async fn list_total_s3_size(
    client: &S3Client,
    bucket: &str,
    prefix: &str,
) -> Result<i64, BackupEngineError> {
    let mut total: i64 = 0;
    let mut continuation: Option<String> = None;
    loop {
        let mut req = client.list_objects_v2().bucket(bucket).prefix(prefix);
        if let Some(tok) = continuation {
            req = req.continuation_token(tok);
        }
        let resp = req.send().await.map_err(|e| BackupEngineError::S3 {
            job_id: 0,
            reason: format!("list: {}", e),
        })?;
        for obj in resp.contents() {
            total += obj.size().unwrap_or(0);
        }
        if resp.is_truncated().unwrap_or(false) {
            continuation = resp.next_continuation_token().map(|s| s.to_string());
        } else {
            break;
        }
    }
    Ok(total)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use serde_json::json;
    use temps_backup_core::{
        BackupContext, BackupEngine, BackupEngineError, StepCursor, StepEvent,
    };
    use tokio_util::sync::CancellationToken;

    struct TestClusterEngine {
        call_count: Arc<std::sync::atomic::AtomicU32>,
    }

    impl TestClusterEngine {
        fn new() -> Self {
            Self {
                call_count: Arc::new(std::sync::atomic::AtomicU32::new(0)),
            }
        }
    }

    impl BackupEngine for TestClusterEngine {
        fn engine(&self) -> &'static str {
            "postgres_cluster"
        }
        fn steps(&self) -> &'static [&'static str] {
            STEPS
        }

        fn execute<'a>(
            &'a self,
            _ctx: &'a BackupContext,
            cursor: StepCursor,
        ) -> BoxStream<'a, Result<StepEvent, BackupEngineError>> {
            let call_n = self
                .call_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async_stream::try_stream! {
                if call_n == 0 {
                    yield StepEvent::StepCompleted { step: "find_primary".into(), durable_state: json!({"primary_container": "pg-primary"}), message: None };
                    yield StepEvent::StepCompleted { step: "preflight".into(), durable_state: json!({"walg_prefix": "s3://b/w", "bucket": "b"}), message: None };
                    yield StepEvent::StepCompleted { step: "walg_push".into(), durable_state: json!({"size_bytes": 4096}), message: None };
                    Err(BackupEngineError::StepFailed { job_id: 0, step: "record_lsn".into(), reason: "crash".into() })?;
                } else {
                    let current = cursor.current_step.as_deref().unwrap_or("none");
                    if current != "walg_push" {
                        Err(BackupEngineError::StepFailed { job_id: 0, step: "resume-check".into(), reason: format!("expected walg_push, got {}", current) })?;
                    }
                    yield StepEvent::StepCompleted { step: "record_lsn".into(), durable_state: json!({"lsn": "0/ABC"}), message: None };
                    yield StepEvent::StepCompleted { step: "metadata".into(), durable_state: json!({}), message: None };
                    yield StepEvent::Done { location: "s3://b/w".into(), size_bytes: Some(4096), compression: "lz4".into() };
                }
            })
        }
    }

    fn make_ctx() -> BackupContext {
        let db = sea_orm::MockDatabase::new(sea_orm::DatabaseBackend::Postgres).into_connection();
        BackupContext {
            job_id: 1,
            attempt: 1,
            params: json!({"service_id": 1, "s3_source_id": 1}),
            db: Arc::new(db),
            cancel: CancellationToken::new(),
        }
    }

    #[test]
    fn test_engine_key() {
        assert_eq!(TestClusterEngine::new().engine(), "postgres_cluster");
    }

    #[test]
    fn test_steps_list() {
        let e = TestClusterEngine::new();
        assert_eq!(e.steps()[0], "find_primary");
        assert_eq!(e.steps()[2], "walg_push");
    }

    #[tokio::test]
    async fn test_crash_resume_cursor_is_correct() {
        let engine = TestClusterEngine::new();
        let ctx = make_ctx();
        let mut stream = engine.execute(
            &ctx,
            StepCursor {
                current_step: None,
                durable_state: json!({}),
            },
        );
        let mut last = None;
        let mut errored = false;
        while let Some(ev) = stream.next().await {
            match ev {
                Ok(StepEvent::StepCompleted { ref step, .. }) => last = Some(step.clone()),
                Ok(_) => {}
                Err(_) => {
                    errored = true;
                    break;
                }
            }
        }
        assert!(errored);
        assert_eq!(last.as_deref(), Some("walg_push"));

        let mut stream2 = engine.execute(
            &ctx,
            StepCursor {
                current_step: last,
                durable_state: json!({}),
            },
        );
        let mut done = false;
        while let Some(ev) = stream2.next().await {
            match ev {
                Ok(StepEvent::Done { .. }) => done = true,
                Ok(_) => {}
                Err(e) => panic!("resume failed: {}", e),
            }
        }
        assert!(done);
    }
}
