//! `PostgresPgDumpEngine`: `BackupEngine` for Postgres via pg_dump sidecar
//! (ADR-014 Phase 3 §"Postgres engines").
//!
//! Steps: `preflight` → `dump` → `upload` → `metadata`.
//!
//! ## Design notes
//!
//! Lifts the pg_dump sidecar logic from
//! `temps-providers/src/externalsvc/postgres.rs:2129` (`backup_to_s3_pgdump`
//! / `run_pg_dumpall_to_s3`). Used as the fallback engine when the Postgres
//! container does not have WAL-G installed.
//!
//! ## Heartbeat discipline
//!
//! The `dump` step polls the sidecar Docker exec and sends heartbeat ticks
//! using the same mpsc + select pattern as `control_plane.rs:213–254`.
//!
//! ## Idempotence
//!
//! - `preflight`: re-validates S3 source; safe to re-run.
//! - `dump`: checks for an existing non-empty temp file at `durable_state.temp_path`
//!   before re-running the sidecar.
//! - `upload`: S3 HEAD check before upload.
//! - `metadata`: PUT is always overwrite.

use std::sync::Arc;
use std::time::{Duration, Instant};

use aws_sdk_s3::Client as S3Client;
use bollard::container::LogOutput;
use bollard::exec::StartExecOptions;
use bollard::exec::StartExecResults;
use chrono::Utc;
use futures::stream::BoxStream;
use futures::StreamExt;
use sea_orm::{DatabaseConnection, EntityTrait};
use serde_json::{json, Value};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use super::ring_buffer::RingBuffer;
// Shell detection (`use super::shell::*`) was removed: the runtime probe
// proved unreliable in production (a sidecar with only `dash` was
// returning `Bash`, then the bash-pipefail command exploded with
// `sh: 1: set: Illegal option -o pipefail`). The POSIX two-stage form
// below works in every shell, so we no longer need to detect.
use temps_backup_core::{BackupContext, BackupEngine, BackupEngineError, StepCursor, StepEvent};
use temps_core::EncryptionService;

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(120);

const STEPS: &[&str] = &["preflight", "dump", "upload", "metadata"];

const DS_S3_KEY: &str = "s3_key";
const DS_BUCKET: &str = "bucket";
const DS_SIZE_BYTES: &str = "size_bytes";
const DS_TEMP_PATH: &str = "temp_path";

// ── Dependencies ─────────────────────────────────────────────────────────────

pub struct PostgresPgDumpDeps {
    pub db: Arc<DatabaseConnection>,
    pub encryption_service: Arc<EncryptionService>,
    pub docker: bollard::Docker,
}

// ── Engine ────────────────────────────────────────────────────────────────────

/// `BackupEngine` for Postgres external services using pg_dump sidecar.
///
/// Used when the Postgres container does not have WAL-G installed. Runs
/// `pg_dumpall | gzip` via a sidecar container and uploads to S3.
/// Reference: `postgres.rs:2135` (`backup_to_s3_pgdump`).
pub struct PostgresPgDumpEngine {
    deps: Arc<PostgresPgDumpDeps>,
}

impl PostgresPgDumpEngine {
    pub fn new(deps: PostgresPgDumpDeps) -> Self {
        Self {
            deps: Arc::new(deps),
        }
    }
}

#[async_trait::async_trait]
impl BackupEngine for PostgresPgDumpEngine {
    fn engine(&self) -> &'static str {
        "postgres_pgdump"
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
                        job_id,
                        step: last.clone(),
                        reason: format!("unknown step '{}'; known: {:?}", last, STEPS),
                    })?
            } else {
                0
            };

            let service_id: i32 = params.get("service_id").and_then(|v| v.as_i64()).map(|v| v as i32)
                .ok_or_else(|| BackupEngineError::Preflight { job_id, reason: "params.service_id missing".into() })?;
            let s3_source_id: i32 = params.get("s3_source_id").and_then(|v| v.as_i64()).map(|v| v as i32)
                .ok_or_else(|| BackupEngineError::Preflight { job_id, reason: "params.s3_source_id missing".into() })?;

            for step in &STEPS[start_idx..] {
                if cancel.is_cancelled() {
                    debug!(job_id, step, "PostgresPgDumpEngine: cancellation requested");
                    return;
                }
                info!(job_id, attempt, step, "PostgresPgDumpEngine: executing step");

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

                    "dump" => {
                        let (heartbeat_tx, mut heartbeat_rx) = tokio::sync::mpsc::channel::<()>(8);
                        let mut step_fut = std::pin::pin!(step_dump(
                            job_id, attempt, accumulated_state.clone(), Arc::clone(&deps), cancel.clone(), heartbeat_tx,
                        ));

                        let step_result: Result<Value, BackupEngineError> = loop {
                            tokio::select! {
                                biased;
                                Some(()) = heartbeat_rx.recv() => {
                                    debug!(job_id, "PostgresPgDumpEngine dump: Heartbeat");
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
                            step: "dump".into(),
                            durable_state: state,
                            message: Some("pg_dumpall completed".into()),
                        };
                    }

                    "upload" => {
                        yield StepEvent::Heartbeat;
                        let state = step_upload(job_id, accumulated_state.clone(), &deps, cancel.clone()).await?;
                        accumulated_state = state.clone();
                        yield StepEvent::StepCompleted {
                            step: "upload".into(),
                            durable_state: state,
                            message: Some("dump uploaded to S3".into()),
                        };
                    }

                    "metadata" => {
                        step_metadata(job_id, s3_source_id, accumulated_state.clone(), &deps).await?;
                        yield StepEvent::StepCompleted {
                            step: "metadata".into(),
                            durable_state: accumulated_state.clone(),
                            message: Some("metadata.json written".into()),
                        };

                        let s3_key = accumulated_state.get(DS_S3_KEY).and_then(|v| v.as_str()).unwrap_or("").to_string();
                        let size_bytes = accumulated_state.get(DS_SIZE_BYTES).and_then(|v| v.as_i64());
                        info!(job_id, location = %s3_key, ?size_bytes, "PostgresPgDumpEngine: Done");
                        yield StepEvent::Done { location: s3_key, size_bytes, compression: "gzip".into() };
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
        cursor: StepCursor,
    ) -> Result<(), BackupEngineError> {
        let job_id = ctx.job_id;
        if let Some(p) = cursor
            .durable_state
            .get(DS_TEMP_PATH)
            .and_then(|v| v.as_str())
        {
            let path = std::path::PathBuf::from(p);
            if path.exists() {
                if let Err(e) = tokio::fs::remove_file(&path).await {
                    warn!(job_id, path = %p, error = %e, "PostgresPgDumpEngine rollback: cleanup failed");
                }
            }
        }
        rollback_s3_object(job_id, ctx, &cursor, &self.deps).await;
        Ok(())
    }
}

// ── Step helpers ──────────────────────────────────────────────────────────────

async fn step_preflight(
    job_id: i64,
    service_id: i32,
    s3_source_id: i32,
    deps: &PostgresPgDumpDeps,
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

    let backup_uuid = Uuid::new_v4().to_string();
    let s3_key = build_s3_key(
        &s3_source.bucket_path,
        &service.name,
        &backup_uuid,
        "dump.sql.gz",
    );

    info!(job_id, %s3_key, bucket = %s3_source.bucket_name, "PostgresPgDumpEngine preflight: validated");

    Ok(json!({
        DS_S3_KEY: s3_key,
        DS_BUCKET: s3_source.bucket_name,
        "backup_uuid": backup_uuid,
        "s3_source_id": s3_source_id,
        "service_id": service_id,
        "service_name": service.name,
        "bucket_path": s3_source.bucket_path,
    }))
}

async fn step_dump(
    job_id: i64,
    _attempt: i32,
    durable_state: Value,
    deps: Arc<PostgresPgDumpDeps>,
    _cancel: tokio_util::sync::CancellationToken,
    heartbeat_tx: tokio::sync::mpsc::Sender<()>,
) -> Result<Value, BackupEngineError> {
    use bollard::exec::CreateExecOptions;
    use bollard::models::ContainerCreateBody as Config;
    use bollard::query_parameters::RemoveContainerOptions;

    error!(
        job_id,
        "PostgresPgDumpEngine dump: step_dump ENTRY (diagnostic)"
    );

    // Idempotence: if temp file already exists and is non-empty, skip re-dump.
    if let Some(temp_path) = durable_state.get(DS_TEMP_PATH).and_then(|v| v.as_str()) {
        let path = std::path::Path::new(temp_path);
        if path.exists() {
            let meta = tokio::fs::metadata(path).await.ok();
            if meta.map(|m| m.len() > 0).unwrap_or(false) {
                info!(
                    job_id,
                    temp_path, "PostgresPgDumpEngine dump: existing dump found, skipping"
                );
                return Ok(durable_state);
            }
        }
    }

    let service_id: i32 = durable_state
        .get("service_id")
        .and_then(|v| v.as_i64())
        .map(|v| v as i32)
        .ok_or_else(|| BackupEngineError::StepFailed {
            job_id,
            step: "dump".into(),
            reason: "missing service_id".into(),
        })?;

    let service = temps_entities::external_services::Entity::find_by_id(service_id)
        .one(deps.db.as_ref())
        .await
        .map_err(|e| BackupEngineError::StepFailed {
            job_id,
            step: "dump".into(),
            reason: format!("db error: {}", e),
        })?
        .ok_or_else(|| BackupEngineError::StepFailed {
            job_id,
            step: "dump".into(),
            reason: format!("service {} not found", service_id),
        })?;

    // Decrypt and read Postgres connection params from the service config.
    let config_json = deps
        .encryption_service
        .decrypt_string(service.config.as_deref().unwrap_or("{}"))
        .unwrap_or_else(|_| "{}".to_string());
    let pg_params = load_postgres_params(job_id, &config_json)?;

    // Create temp directory for the dump file.
    let backup_dir = std::env::temp_dir().join("temps-extpg-backup");
    tokio::fs::create_dir_all(&backup_dir)
        .await
        .map_err(|e| BackupEngineError::StepFailed {
            job_id,
            step: "dump".into(),
            reason: format!("create temp dir: {}", e),
        })?;

    let dump_filename = format!("{}.sql.gz", Uuid::new_v4());
    let host_dump_path = backup_dir.join(&dump_filename);
    let container_dump_path = format!("/backup/{}", dump_filename);
    let stderr_filename = format!("{}.stderr", Uuid::new_v4());
    let stderr_path_container = format!("/backup/{}", stderr_filename);
    let host_stderr_path = backup_dir.join(&stderr_filename);

    let sidecar_image = pg_params.docker_image.clone();
    let sidecar_name = format!("temps-ext-pg-backup-{}", Uuid::new_v4());
    let password_env = format!("PGPASSWORD={}", pg_params.password);

    let sidecar_config = Config {
        image: Some(sidecar_image.clone()),
        entrypoint: Some(vec!["/bin/sleep".to_string()]),
        cmd: Some(vec!["86400".to_string()]),
        env: Some(vec![password_env.clone()]),
        user: Some("root".to_string()),
        host_config: Some(bollard::models::HostConfig {
            oom_score_adj: Some(-500),
            binds: Some(vec![format!("{}:/backup:rw", backup_dir.display())]),
            ..Default::default()
        }),
        networking_config: Some(bollard::models::NetworkingConfig {
            endpoints_config: Some(std::collections::HashMap::from([(
                temps_core::NETWORK_NAME.to_string(),
                bollard::models::EndpointSettings::default(),
            )])),
        }),
        ..Default::default()
    };

    let remove_sidecar = |docker: bollard::Docker, name: String| async move {
        let _ = docker
            .remove_container(
                &name,
                Some(RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                }),
            )
            .await;
    };

    deps.docker
        .create_container(
            Some(
                bollard::query_parameters::CreateContainerOptionsBuilder::new()
                    .name(&sidecar_name)
                    .build(),
            ),
            sidecar_config,
        )
        .await
        .map_err(|e| BackupEngineError::StepFailed {
            job_id,
            step: "dump".into(),
            reason: format!("create sidecar: {}", e),
        })?;

    deps.docker
        .start_container(
            &sidecar_name,
            Some(bollard::query_parameters::StartContainerOptionsBuilder::new().build()),
        )
        .await
        .map_err(|e| {
            let d = deps.docker.clone();
            let n = sidecar_name.clone();
            tokio::spawn(async move { remove_sidecar(d, n).await });
            BackupEngineError::StepFailed {
                job_id,
                step: "dump".into(),
                reason: format!("start sidecar: {}", e),
            }
        })?;

    // The Postgres container's name matches the legacy provider's
    // `get_container_name()` (postgres.rs:269-271): `postgres-{service_name}`.
    let db_container = format!("postgres-{}", service.name);
    let port_str = "5432".to_string();
    fn shell_escape(s: &str) -> String {
        format!("'{}'", s.replace('\'', "'\\''"))
    }

    // Naive `pg_dumpall ... 2>file | gzip > out` masks failures: `gzip`
    // exits 0 on empty input, so the pipeline reports success even when
    // `pg_dumpall` errored. `set -o pipefail` would fix it but is a
    // bashism (`sh: 1: set: Illegal option -o pipefail` under `dash`).
    //
    // We use a POSIX-portable two-stage `&&` form that works in every
    // shell — `dash`, `ash`, `bash`, busybox: dump to an uncompressed
    // file first, then `gzip` it. `&&` short-circuits so a `pg_dumpall`
    // failure skips `gzip` and the compound exit code is `pg_dumpall`'s
    // real one. Trade-off: transient 2× disk usage while both `.sql`
    // and `.sql.gz` co-exist (`gzip <file>` removes the source on
    // success). The earlier shell-detection branch proved unreliable
    // in prod (probe returned Bash on a sidecar that had only dash),
    // so we removed it.
    let uncompressed_in_container = container_dump_path
        .strip_suffix(".gz")
        .unwrap_or(&container_dump_path)
        .to_string();

    let dump_cmd = format!(
        "pg_dumpall --clean --if-exists --no-password --host={} --port={} --username={} --database={} 2>{} > {} && gzip {}",
        shell_escape(&db_container),
        shell_escape(&port_str),
        shell_escape(&pg_params.username),
        shell_escape(&pg_params.database),
        stderr_path_container,
        shell_escape(&uncompressed_in_container),
        shell_escape(&uncompressed_in_container),
    );

    // Log the exact dump_cmd so we can reproduce any shell-parsing failure
    // outside the engine. Removing this once the dash-pipefail mystery is
    // diagnosed.
    info!(
        job_id,
        sidecar = %sidecar_name,
        image = %sidecar_image,
        dump_cmd = %dump_cmd,
        "PostgresPgDumpEngine dump: about to exec sh -c"
    );

    // Capture stdout + stderr from the exec stream (no `2>&1` in cmd — we
    // split the streams). The shell redirect (`2>{stderr_path}`) captures
    // errors inside the container; stream capture handles anything that leaks.
    let exec = deps
        .docker
        .create_exec(
            &sidecar_name,
            CreateExecOptions {
                cmd: Some(vec!["sh", "-c", &dump_cmd]),
                attach_stdout: Some(true),
                attach_stderr: Some(true),
                env: Some(vec![password_env.as_str()]),
                ..Default::default()
            },
        )
        .await
        .map_err(|e| {
            let d = deps.docker.clone();
            let n = sidecar_name.clone();
            tokio::spawn(async move { remove_sidecar(d, n).await });
            BackupEngineError::StepFailed {
                job_id,
                step: "dump".into(),
                reason: format!("create exec: {}", e),
            }
        })?;

    let stream_result = deps
        .docker
        .start_exec(
            &exec.id,
            Some(StartExecOptions {
                detach: false,
                ..Default::default()
            }),
        )
        .await
        .map_err(|e| {
            let d = deps.docker.clone();
            let n = sidecar_name.clone();
            tokio::spawn(async move { remove_sidecar(d, n).await });
            BackupEngineError::StepFailed {
                job_id,
                step: "dump".into(),
                reason: format!("start exec: {}", e),
            }
        })?;

    // Consume the stream, emitting heartbeats and capturing any leaked output.
    let mut stream_stdout_tail = RingBuffer::with_capacity(64 * 1024);
    let mut stream_stderr_tail = RingBuffer::with_capacity(64 * 1024);
    let mut last_hb = Instant::now();

    if let StartExecResults::Attached { mut output, .. } = stream_result {
        while let Some(item) = output.next().await {
            match item {
                Ok(LogOutput::StdOut { message }) => stream_stdout_tail.append(&message),
                Ok(LogOutput::StdErr { message }) => stream_stderr_tail.append(&message),
                Ok(_) => {}
                Err(e) => {
                    error!(job_id, engine = "postgres_pgdump", sidecar = %sidecar_name, "dump exec stream error: {}", e);
                    break;
                }
            }
            if last_hb.elapsed() >= HEARTBEAT_INTERVAL {
                debug!(job_id, "PostgresPgDumpEngine dump: sending heartbeat tick");
                let _ = heartbeat_tx.try_send(());
                last_hb = Instant::now();
            }
        }
    }

    let stream_stderr = stream_stderr_tail.into_string_lossy();
    // Suppress unused-variable warning for stdout tail when exec doesn't leak output.
    let _ = stream_stdout_tail;

    let stderr_data = tokio::fs::read(&host_stderr_path).await.unwrap_or_default();
    let _ = tokio::fs::remove_file(&host_stderr_path).await;

    let exec_inspect =
        deps.docker
            .inspect_exec(&exec.id)
            .await
            .map_err(|e| BackupEngineError::StepFailed {
                job_id,
                step: "dump".into(),
                reason: format!("inspect exec: {}", e),
            })?;

    if let Some(code) = exec_inspect.exit_code {
        if code != 0 {
            let stderr = String::from_utf8_lossy(&stderr_data).into_owned();
            remove_sidecar(deps.docker.clone(), sidecar_name.clone()).await;
            let _ = tokio::fs::remove_file(&host_dump_path).await;
            return Err(BackupEngineError::StepFailed {
                job_id,
                step: "dump".into(),
                reason: format!(
                    "pg_dumpall exited {}. file-stderr: {}{}",
                    code,
                    stderr,
                    if stream_stderr.trim().is_empty() {
                        String::new()
                    } else {
                        format!(". stream-stderr: {}", stream_stderr.trim())
                    },
                ),
            });
        }
    } else {
        let _ = stream_stderr; // only used in error path above
    }

    remove_sidecar(deps.docker.clone(), sidecar_name).await;

    let dump_meta =
        tokio::fs::metadata(&host_dump_path)
            .await
            .map_err(|e| BackupEngineError::StepFailed {
                job_id,
                step: "dump".into(),
                reason: format!("dump file not found: {}", e),
            })?;
    // A gzip header alone is ~20 bytes. A real pg_dumpall output (with at
    // minimum the boilerplate `CREATE ROLE` / `\connect` block) compresses to
    // hundreds of bytes. Treat anything under 100 bytes as a failed dump —
    // it almost certainly means pg_dumpall couldn't connect and `2>file |
    // gzip` swallowed the failure as empty-gzip-of-nothing. Include the
    // stderr we captured so the operator can see why.
    const MIN_PLAUSIBLE_DUMP_BYTES: u64 = 100;
    if dump_meta.len() < MIN_PLAUSIBLE_DUMP_BYTES {
        let stderr = String::from_utf8_lossy(&stderr_data).into_owned();
        let _ = tokio::fs::remove_file(&host_dump_path).await;
        return Err(BackupEngineError::StepFailed {
            job_id,
            step: "dump".into(),
            reason: format!(
                "pg_dumpall produced an implausibly small dump ({} bytes); pg_dumpall stderr: {}",
                dump_meta.len(),
                if stderr.trim().is_empty() {
                    "<empty>"
                } else {
                    stderr.trim()
                },
            ),
        });
    }

    let host_dump_str = host_dump_path.to_str().unwrap_or("").to_string();
    info!(job_id, path = %host_dump_str, size_bytes = dump_meta.len(), "PostgresPgDumpEngine dump: completed");
    // Surface pg_dumpall warnings (NOTICE lines, etc.) even on success so
    // operators can see them in the runtime log. stderr_data was already read
    // above; this is a non-fatal info-level surface.
    if !stderr_data.is_empty() {
        let stderr = String::from_utf8_lossy(&stderr_data);
        let trimmed = stderr.trim();
        if !trimmed.is_empty() {
            info!(job_id, "PostgresPgDumpEngine dump stderr: {}", trimmed);
        }
    }

    let mut new_state = durable_state.clone();
    if let Some(obj) = new_state.as_object_mut() {
        obj.insert(DS_TEMP_PATH.to_string(), json!(host_dump_str));
    }
    Ok(new_state)
}

async fn step_upload(
    job_id: i64,
    durable_state: Value,
    deps: &PostgresPgDumpDeps,
    _cancel: tokio_util::sync::CancellationToken,
) -> Result<Value, BackupEngineError> {
    let s3_key = durable_state
        .get(DS_S3_KEY)
        .and_then(|v| v.as_str())
        .ok_or_else(|| BackupEngineError::StepFailed {
            job_id,
            step: "upload".into(),
            reason: "missing s3_key".into(),
        })?
        .to_string();
    let bucket = durable_state
        .get(DS_BUCKET)
        .and_then(|v| v.as_str())
        .ok_or_else(|| BackupEngineError::StepFailed {
            job_id,
            step: "upload".into(),
            reason: "missing bucket".into(),
        })?
        .to_string();
    let temp_path = durable_state
        .get(DS_TEMP_PATH)
        .and_then(|v| v.as_str())
        .ok_or_else(|| BackupEngineError::StepFailed {
            job_id,
            step: "upload".into(),
            reason: "missing temp_path".into(),
        })?
        .to_string();
    let s3_source_id: i32 = durable_state
        .get("s3_source_id")
        .and_then(|v| v.as_i64())
        .map(|v| v as i32)
        .ok_or_else(|| BackupEngineError::StepFailed {
            job_id,
            step: "upload".into(),
            reason: "missing s3_source_id".into(),
        })?;

    let s3_client =
        build_s3_client(s3_source_id, deps)
            .await
            .map_err(|e| BackupEngineError::S3 {
                job_id,
                reason: format!("build S3 client: {}", e),
            })?;

    // Idempotence check.
    if let Some(size) = check_s3_object_exists(&s3_client, &bucket, &s3_key).await {
        info!(job_id, %bucket, %s3_key, "PostgresPgDumpEngine upload: S3 object exists, skipping");
        let _ = tokio::fs::remove_file(&temp_path).await;
        let mut ns = durable_state.clone();
        if let Some(o) = ns.as_object_mut() {
            o.insert(DS_SIZE_BYTES.to_string(), json!(size));
        }
        return Ok(ns);
    }

    let meta =
        tokio::fs::metadata(&temp_path)
            .await
            .map_err(|e| BackupEngineError::StepFailed {
                job_id,
                step: "upload".into(),
                reason: format!("stat {}: {}", temp_path, e),
            })?;
    let file_size = meta.len() as i64;

    let body = aws_sdk_s3::primitives::ByteStream::from_path(std::path::Path::new(&temp_path))
        .await
        .map_err(|e| BackupEngineError::S3 {
            job_id,
            reason: format!("byte stream: {}", e),
        })?;
    s3_client
        .put_object()
        .bucket(&bucket)
        .key(&s3_key)
        .body(body)
        .content_type("application/x-gzip")
        .send()
        .await
        .map_err(|e| BackupEngineError::S3 {
            job_id,
            reason: format!("upload {}: {}", s3_key, e),
        })?;

    if let Err(e) = tokio::fs::remove_file(&temp_path).await {
        warn!(job_id, path = %temp_path, error = %e, "PostgresPgDumpEngine upload: cleanup failed (non-fatal)");
    }

    let mut ns = durable_state.clone();
    if let Some(o) = ns.as_object_mut() {
        o.insert(DS_SIZE_BYTES.to_string(), json!(file_size));
    }
    info!(job_id, %bucket, %s3_key, "PostgresPgDumpEngine upload: completed");
    Ok(ns)
}

async fn step_metadata(
    job_id: i64,
    s3_source_id: i32,
    durable_state: Value,
    deps: &PostgresPgDumpDeps,
) -> Result<(), BackupEngineError> {
    let s3_key = durable_state
        .get(DS_S3_KEY)
        .and_then(|v| v.as_str())
        .ok_or_else(|| BackupEngineError::StepFailed {
            job_id,
            step: "metadata".into(),
            reason: "missing s3_key".into(),
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

    let metadata_key = derive_metadata_key(&s3_key);
    let body = serde_json::to_vec(&json!({
        "backup_uuid": durable_state.get("backup_uuid").and_then(|v| v.as_str()).unwrap_or("unknown"),
        "type": "full",
        "engine": "postgres_pgdump",
        "backup_tool": "pg_dumpall",
        "created_at": Utc::now().to_rfc3339(),
        "size_bytes": durable_state.get(DS_SIZE_BYTES).and_then(|v| v.as_i64()),
        "compression_type": "gzip",
        "source": { "id": s3_source_id },
        "s3_location": s3_key,
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

    info!(job_id, %bucket, key = %metadata_key, "PostgresPgDumpEngine metadata: written");
    Ok(())
}

// ── Utility helpers ───────────────────────────────────────────────────────────

struct PgParams {
    username: String,
    password: String,
    database: String,
    docker_image: String,
}

/// Extract Postgres connection parameters from the service's decrypted config JSON.
fn load_postgres_params(_job_id: i64, config_json: &str) -> Result<PgParams, BackupEngineError> {
    let params: serde_json::Value = serde_json::from_str(config_json).unwrap_or_else(|_| json!({}));

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
        .and_then(|v| v.as_str())
        .or_else(|| params.get("db_name").and_then(|v| v.as_str()))
        .unwrap_or("postgres")
        .to_string();
    let docker_image = params
        .get("docker_image")
        .and_then(|v| v.as_str())
        .unwrap_or("gotempsh/postgres-walg:18-bookworm")
        .to_string();

    Ok(PgParams {
        username,
        password,
        database,
        docker_image,
    })
}

fn build_s3_key(
    bucket_path: &str,
    service_name: &str,
    backup_uuid: &str,
    filename: &str,
) -> String {
    // Include the backup_uuid in the path so concurrent or same-day backups
    // of the same service write to distinct keys. Without this, the engine's
    // idempotent "S3 object already exists, skipping upload" check would
    // ignore every attempt after the first within a calendar day — silently
    // re-using yesterday's artifact as today's "backup."
    let prefix = bucket_path.trim_matches('/');
    let date = Utc::now().format("%Y/%m/%d");
    if prefix.is_empty() {
        format!(
            "external_services/postgres/{}/{}/{}/{}",
            service_name, date, backup_uuid, filename
        )
    } else {
        format!(
            "{}/external_services/postgres/{}/{}/{}/{}",
            prefix, service_name, date, backup_uuid, filename
        )
    }
}

fn derive_metadata_key(s3_key: &str) -> String {
    let parts: Vec<&str> = s3_key.rsplitn(2, '/').collect();
    if parts.len() == 2 {
        format!("{}/metadata.json", parts[1])
    } else {
        format!("{}.metadata.json", s3_key)
    }
}

async fn build_s3_client(
    s3_source_id: i32,
    deps: &PostgresPgDumpDeps,
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
    deps: &PostgresPgDumpDeps,
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
    let creds = aws_sdk_s3::config::Credentials::new(ak, sk, None, None, "postgres-pgdump-engine");
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

async fn check_s3_object_exists(client: &S3Client, bucket: &str, key: &str) -> Option<i64> {
    match client.head_object().bucket(bucket).key(key).send().await {
        Ok(r) => r.content_length(),
        Err(_) => None,
    }
}

async fn rollback_s3_object(
    job_id: i64,
    ctx: &BackupContext,
    cursor: &StepCursor,
    deps: &PostgresPgDumpDeps,
) {
    let s3_key = cursor
        .durable_state
        .get(DS_S3_KEY)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let bucket = cursor
        .durable_state
        .get(DS_BUCKET)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    if let (Some(key), Some(bkt)) = (s3_key, bucket) {
        let s3_source_id = ctx
            .params
            .get("s3_source_id")
            .and_then(|v| v.as_i64())
            .map(|v| v as i32)
            .unwrap_or(0);
        if s3_source_id > 0 {
            if let Ok(client) = build_s3_client(s3_source_id, deps).await {
                if let Err(e) = client.delete_object().bucket(&bkt).key(&key).send().await {
                    warn!(job_id, %bkt, %key, error = %e, "PostgresPgDumpEngine rollback: S3 delete failed (best-effort)");
                }
            }
        }
    }
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

    struct TestPgDumpEngine {
        call_count: Arc<std::sync::atomic::AtomicU32>,
    }

    impl TestPgDumpEngine {
        fn new() -> Self {
            Self {
                call_count: Arc::new(std::sync::atomic::AtomicU32::new(0)),
            }
        }
    }

    impl BackupEngine for TestPgDumpEngine {
        fn engine(&self) -> &'static str {
            "postgres_pgdump"
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
                    yield StepEvent::StepCompleted { step: "preflight".into(), durable_state: json!({"s3_key": "k", "bucket": "b"}), message: None };
                    yield StepEvent::StepCompleted { step: "dump".into(), durable_state: json!({"temp_path": "/tmp/d.sql.gz"}), message: None };
                    Err(BackupEngineError::StepFailed { job_id: 0, step: "upload".into(), reason: "crash".into() })?;
                } else {
                    let current = cursor.current_step.as_deref().unwrap_or("none");
                    if current != "dump" {
                        Err(BackupEngineError::StepFailed { job_id: 0, step: "resume-check".into(), reason: format!("expected dump, got {}", current) })?;
                    }
                    yield StepEvent::StepCompleted { step: "upload".into(), durable_state: json!({"size_bytes": 512}), message: None };
                    yield StepEvent::StepCompleted { step: "metadata".into(), durable_state: json!({}), message: None };
                    yield StepEvent::Done { location: "k".into(), size_bytes: Some(512), compression: "gzip".into() };
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
        let e = TestPgDumpEngine::new();
        assert_eq!(e.engine(), "postgres_pgdump");
    }

    #[test]
    fn test_steps_list() {
        let e = TestPgDumpEngine::new();
        assert_eq!(e.steps(), STEPS);
        assert_eq!(e.steps()[1], "dump");
    }

    #[tokio::test]
    async fn test_crash_resume_cursor_is_correct() {
        let engine = TestPgDumpEngine::new();
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
        assert_eq!(last.as_deref(), Some("dump"));

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
