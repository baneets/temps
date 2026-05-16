//! `PostgresWalgEngine`: `BackupEngine` for Postgres via WAL-G
//! (ADR-014 Phase 3 §"Postgres engines").
//!
//! Steps: `preflight` → `walg_push` → `record_lsn` → `metadata`.
//!
//! ## Design notes
//!
//! Lifts the WAL-G backup logic from
//! `temps-providers/src/externalsvc/postgres.rs:1952` (`backup_to_s3_walg`
//! / `run_walg_backup_push`). Used when the Postgres container has
//! `wal-g` installed.
//!
//! The `walg_push` step runs `wal-g backup-push $PGDATA` inside the running
//! container. Zero data flows through the Temps process. After success,
//! `record_lsn` queries `pg_current_wal_lsn()` so PITR can use it.
//!
//! ## Heartbeat discipline
//!
//! `walg_push` uses the mpsc + select pattern from `control_plane.rs:213–254`.
//!
//! ## Idempotence
//!
//! - `preflight`: re-validates S3 source; safe to re-run.
//! - `walg_push`: WAL-G is idempotent by design (overwrites existing base backup
//!   at the same WAL-G prefix). Always re-runs on resume.
//! - `record_lsn`: re-queries the database; result may differ but is acceptable.
//! - `metadata`: PUT is always overwrite.

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

const STEPS: &[&str] = &["preflight", "walg_push", "record_lsn", "metadata"];

const DS_S3_KEY: &str = "s3_key";
const DS_BUCKET: &str = "bucket";
const DS_SIZE_BYTES: &str = "size_bytes";
const DS_WALG_PREFIX: &str = "walg_prefix";
const DS_LSN: &str = "lsn";

// ── Dependencies ─────────────────────────────────────────────────────────────

pub struct PostgresWalgDeps {
    pub db: Arc<DatabaseConnection>,
    pub encryption_service: Arc<EncryptionService>,
    pub docker: bollard::Docker,
}

// ── Engine ────────────────────────────────────────────────────────────────────

/// `BackupEngine` for Postgres external services using WAL-G.
///
/// Requires WAL-G to be installed in the Postgres container
/// (image `gotempsh/postgres-walg:*`).
/// Reference: `postgres.rs:1952` (`backup_to_s3_walg`).
pub struct PostgresWalgEngine {
    deps: Arc<PostgresWalgDeps>,
}

impl PostgresWalgEngine {
    pub fn new(deps: PostgresWalgDeps) -> Self {
        Self {
            deps: Arc::new(deps),
        }
    }
}

#[async_trait::async_trait]
impl BackupEngine for PostgresWalgEngine {
    fn engine(&self) -> &'static str {
        "postgres_walg"
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
                    debug!(job_id, step, "PostgresWalgEngine: cancellation requested");
                    return;
                }
                info!(job_id, attempt, step, "PostgresWalgEngine: executing step");

                match *step {
                    "preflight" => {
                        let state = step_preflight(job_id, service_id, s3_source_id, &deps).await?;
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
                                    debug!(job_id, "PostgresWalgEngine walg_push: Heartbeat");
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
                            message: Some("wal-g backup-push completed".into()),
                        };
                    }

                    "record_lsn" => {
                        let state = step_record_lsn(job_id, service_id, accumulated_state.clone(), &deps).await?;
                        accumulated_state = state.clone();
                        yield StepEvent::StepCompleted {
                            step: "record_lsn".into(),
                            durable_state: state,
                            message: Some("LSN recorded".into()),
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
                        info!(job_id, %location, ?size_bytes, "PostgresWalgEngine: Done");
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
        // WAL-G manages its own S3 retention. Best-effort: nothing to clean up locally.
        info!(
            job_id = ctx.job_id,
            "PostgresWalgEngine rollback: no local cleanup needed (WAL-G manages S3)"
        );
        Ok(())
    }
}

// ── Step helpers ──────────────────────────────────────────────────────────────

async fn step_preflight(
    job_id: i64,
    service_id: i32,
    s3_source_id: i32,
    deps: &PostgresWalgDeps,
) -> Result<Value, BackupEngineError> {
    let service = temps_entities::external_services::Entity::find_by_id(service_id)
        .one(deps.db.as_ref())
        .await
        .map_err(|e| BackupEngineError::Preflight {
            job_id,
            reason: format!("db error service {}: {}", service_id, e),
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
            reason: format!("db error s3_source {}: {}", s3_source_id, e),
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
    // For listing size after backup.
    let s3_list_prefix = format!("{}/walg/", subpath_root.trim_matches('/'));

    info!(job_id, %walg_prefix, "PostgresWalgEngine preflight: validated");

    Ok(json!({
        DS_S3_KEY: walg_prefix.clone(),
        DS_BUCKET: s3_source.bucket_name,
        DS_WALG_PREFIX: walg_prefix,
        "s3_list_prefix": s3_list_prefix,
        "s3_source_id": s3_source_id,
        "service_id": service_id,
        "service_name": service.name,
    }))
}

async fn step_walg_push(
    job_id: i64,
    durable_state: Value,
    deps: Arc<PostgresWalgDeps>,
    _cancel: tokio_util::sync::CancellationToken,
    heartbeat_tx: tokio::sync::mpsc::Sender<()>,
) -> Result<Value, BackupEngineError> {
    let service_id: i32 = durable_state
        .get("service_id")
        .and_then(|v| v.as_i64())
        .map(|v| v as i32)
        .ok_or_else(|| BackupEngineError::StepFailed {
            job_id,
            step: "walg_push".into(),
            reason: "missing service_id".into(),
        })?;
    let s3_source_id: i32 = durable_state
        .get("s3_source_id")
        .and_then(|v| v.as_i64())
        .map(|v| v as i32)
        .ok_or_else(|| BackupEngineError::StepFailed {
            job_id,
            step: "walg_push".into(),
            reason: "missing s3_source_id".into(),
        })?;
    let walg_prefix = durable_state
        .get(DS_WALG_PREFIX)
        .and_then(|v| v.as_str())
        .ok_or_else(|| BackupEngineError::StepFailed {
            job_id,
            step: "walg_push".into(),
            reason: "missing walg_prefix".into(),
        })?
        .to_string();

    let service = temps_entities::external_services::Entity::find_by_id(service_id)
        .one(deps.db.as_ref())
        .await
        .map_err(|e| BackupEngineError::StepFailed {
            job_id,
            step: "walg_push".into(),
            reason: format!("db: {}", e),
        })?
        .ok_or_else(|| BackupEngineError::StepFailed {
            job_id,
            step: "walg_push".into(),
            reason: format!("service {} not found", service_id),
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
    let pg_params = load_postgres_params(job_id, &config_json)?;
    // Container naming matches temps-providers/src/externalsvc/postgres.rs:269-271.
    let container_name = format!("postgres-{}", service.name);

    // WAL-G memory tuning — without these, a base backup of a small DB can
    // burn 1–2 GB of RSS and get OOM-killed (exit 137) by Docker. WAL-G keeps
    // each in-flight tar bundle fully buffered in memory; the defaults
    // (`WALG_UPLOAD_CONCURRENCY=16`, `WALG_TAR_SIZE_THRESHOLD=1GB`) multiply
    // to ~16 GB peak. The values below cap RSS at roughly
    //   WALG_UPLOAD_CONCURRENCY * WALG_TAR_SIZE_THRESHOLD ≈ 4 × 128 MiB = 512 MiB
    // which is plenty for sub-100GB clusters and survives the default 1 GiB
    // container memory limit. Users with very large DBs can override these
    // by setting the env variables on the service or the worker.
    //
    // Numbers chosen:
    //   - UPLOAD_CONCURRENCY=4: still parallel enough to saturate a 1 Gbps
    //     uplink with small tars; default 16 is over-aggressive for small DBs.
    //   - UPLOAD_DISK_CONCURRENCY=1: serial disk read keeps the page cache
    //     hot and avoids thrashing on rotational disks.
    //   - TAR_SIZE_THRESHOLD=134217728 (128 MiB): smaller tars = smaller peak
    //     buffer per uploader. Trade-off: more S3 PUTs per backup, but each
    //     one is faster to retry.
    //   - UPLOAD_QUEUE=2: at most two tar bundles waiting for upload at any
    //     time. Default is larger and adds to peak memory unnecessarily.
    let mut walg_env: Vec<String> = vec![
        format!("WALG_S3_PREFIX={}", walg_prefix),
        format!("AWS_ACCESS_KEY_ID={}", access_key),
        format!("AWS_SECRET_ACCESS_KEY={}", secret_key),
        format!("AWS_REGION={}", s3_source.region),
        format!("PGUSER={}", pg_params.username),
        format!("PGPASSWORD={}", pg_params.password),
        format!("PGDATABASE={}", pg_params.database),
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
    // Capture both stdout and stderr so failures are diagnosable.
    // Note: no `2>&1` in cmd — we let Bollard route each stream separately.
    let exec = deps
        .docker
        .create_exec(
            &container_name,
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
            reason: format!("create exec: {}", e),
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

    // Stream output into bounded ring buffers and emit periodic heartbeats.
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
                    error!(job_id, engine = "postgres_walg", container = %container_name, "walg_push exec stream error: {}", e);
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

    // On success, surface any stderr warnings at INFO so operators see them.
    if !stderr.trim().is_empty() {
        info!(
            job_id,
            engine = "postgres_walg",
            container = %container_name,
            "walg_push stderr (warnings): {}",
            stderr.trim(),
        );
    }

    // Compute total size by listing WAL-G objects.
    let s3_list_prefix = durable_state
        .get("s3_list_prefix")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let bucket = durable_state
        .get(DS_BUCKET)
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

    info!(job_id, %walg_prefix, ?size_bytes, "PostgresWalgEngine walg_push: completed");
    Ok(new_state)
}

async fn step_record_lsn(
    job_id: i64,
    service_id: i32,
    durable_state: Value,
    deps: &PostgresWalgDeps,
) -> Result<Value, BackupEngineError> {
    // If LSN already recorded (idempotent resume), return as-is.
    if durable_state.get(DS_LSN).is_some() {
        info!(
            job_id,
            "PostgresWalgEngine record_lsn: LSN already in cursor, skipping"
        );
        return Ok(durable_state);
    }

    let service = temps_entities::external_services::Entity::find_by_id(service_id)
        .one(deps.db.as_ref())
        .await
        .map_err(|e| BackupEngineError::StepFailed {
            job_id,
            step: "record_lsn".into(),
            reason: format!("db: {}", e),
        })?
        .ok_or_else(|| BackupEngineError::StepFailed {
            job_id,
            step: "record_lsn".into(),
            reason: format!("service {} not found", service_id),
        })?;

    let config_json = deps
        .encryption_service
        .decrypt_string(service.config.as_deref().unwrap_or("{}"))
        .unwrap_or_else(|_| "{}".to_string());
    let pg_params = load_postgres_params(job_id, &config_json)?;
    // Container naming matches temps-providers/src/externalsvc/postgres.rs:269-271.
    let container_name = format!("postgres-{}", service.name);

    // Run pg_current_wal_lsn() inside the container via docker exec.
    let lsn = query_current_wal_lsn(job_id, &deps.docker, &container_name, &pg_params)
        .await
        .unwrap_or_else(|e| {
            warn!(job_id, error = %e, "record_lsn: could not query LSN (will record empty)");
            String::new()
        });

    let mut new_state = durable_state.clone();
    if let Some(obj) = new_state.as_object_mut() {
        obj.insert(DS_LSN.to_string(), json!(lsn));
    }
    info!(job_id, %lsn, "PostgresWalgEngine record_lsn: recorded");
    Ok(new_state)
}

async fn step_metadata(
    job_id: i64,
    s3_source_id: i32,
    durable_state: Value,
    deps: &PostgresWalgDeps,
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

    let s3_client =
        build_s3_client(s3_source_id, deps)
            .await
            .map_err(|e| BackupEngineError::S3 {
                job_id,
                reason: format!("build S3 client: {}", e),
            })?;

    // Store metadata at <list_prefix>metadata.json.
    let s3_list_prefix = durable_state
        .get("s3_list_prefix")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let metadata_key = format!("{}metadata.json", s3_list_prefix.trim_end_matches('/'));

    let body = serde_json::to_vec(&json!({
        "type": "full",
        "engine": "postgres_walg",
        "backup_tool": "wal-g",
        "created_at": Utc::now().to_rfc3339(),
        "size_bytes": durable_state.get(DS_SIZE_BYTES).and_then(|v| v.as_i64()),
        "compression_type": "lz4",
        "lsn": durable_state.get(DS_LSN).and_then(|v| v.as_str()).unwrap_or(""),
        "source": { "id": s3_source_id },
        "s3_location": walg_prefix,
    }))
    .map_err(|e| BackupEngineError::StepFailed {
        job_id,
        step: "metadata".into(),
        reason: format!("serialize: {}", e),
    })?;

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

    info!(job_id, %bucket, key = %metadata_key, "PostgresWalgEngine metadata: written");
    Ok(())
}

// ── Utility helpers ───────────────────────────────────────────────────────────

struct PgParams {
    username: String,
    password: String,
    database: String,
}

fn load_postgres_params(_job_id: i64, config_json: &str) -> Result<PgParams, BackupEngineError> {
    let params: Value = serde_json::from_str(config_json).unwrap_or_else(|_| json!({}));
    Ok(PgParams {
        username: params
            .get("username")
            .and_then(|v| v.as_str())
            .unwrap_or("postgres")
            .to_string(),
        password: params
            .get("password")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        database: params
            .get("database")
            .or_else(|| params.get("db_name"))
            .and_then(|v| v.as_str())
            .unwrap_or("postgres")
            .to_string(),
    })
}

async fn query_current_wal_lsn(
    job_id: i64,
    docker: &bollard::Docker,
    container_name: &str,
    pg_params: &PgParams,
) -> Result<String, BackupEngineError> {
    use futures::StreamExt;

    let cmd = format!(
        "PGPASSWORD={} psql -U {} -d {} -t -c 'SELECT pg_current_wal_lsn()'",
        pg_params.password, pg_params.username, pg_params.database
    );
    let exec = docker
        .create_exec(
            container_name,
            bollard::exec::CreateExecOptions {
                cmd: Some(vec!["sh", "-c", &cmd]),
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
    Ok(result.trim().to_string())
}

async fn build_s3_client(
    s3_source_id: i32,
    deps: &PostgresWalgDeps,
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
    deps: &PostgresWalgDeps,
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
    let creds = aws_sdk_s3::config::Credentials::new(ak, sk, None, None, "postgres-walg-engine");
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
            reason: format!("list objects: {}", e),
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

    struct TestWalgEngine {
        call_count: Arc<std::sync::atomic::AtomicU32>,
    }

    impl TestWalgEngine {
        fn new() -> Self {
            Self {
                call_count: Arc::new(std::sync::atomic::AtomicU32::new(0)),
            }
        }
    }

    impl BackupEngine for TestWalgEngine {
        fn engine(&self) -> &'static str {
            "postgres_walg"
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
                    yield StepEvent::StepCompleted { step: "preflight".into(), durable_state: json!({"walg_prefix": "s3://b/w", "bucket": "b"}), message: None };
                    yield StepEvent::StepCompleted { step: "walg_push".into(), durable_state: json!({"size_bytes": 2048}), message: None };
                    Err(BackupEngineError::StepFailed { job_id: 0, step: "record_lsn".into(), reason: "crash".into() })?;
                } else {
                    let current = cursor.current_step.as_deref().unwrap_or("none");
                    if current != "walg_push" {
                        Err(BackupEngineError::StepFailed { job_id: 0, step: "resume-check".into(), reason: format!("expected walg_push, got {}", current) })?;
                    }
                    yield StepEvent::StepCompleted { step: "record_lsn".into(), durable_state: json!({"lsn": "0/1234"}), message: None };
                    yield StepEvent::StepCompleted { step: "metadata".into(), durable_state: json!({}), message: None };
                    yield StepEvent::Done { location: "s3://b/w".into(), size_bytes: Some(2048), compression: "lz4".into() };
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
        assert_eq!(TestWalgEngine::new().engine(), "postgres_walg");
    }

    #[test]
    fn test_steps_list() {
        let e = TestWalgEngine::new();
        assert_eq!(e.steps(), STEPS);
        assert_eq!(e.steps()[1], "walg_push");
        assert_eq!(e.steps()[2], "record_lsn");
    }

    #[tokio::test]
    async fn test_crash_resume_cursor_is_correct() {
        let engine = TestWalgEngine::new();
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
