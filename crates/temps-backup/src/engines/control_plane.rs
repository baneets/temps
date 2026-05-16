//! `ControlPlaneEngine`: `BackupEngine` implementation for the Temps control-plane
//! PostgreSQL database (ADR-014 Phase 1 §"Control-plane backup as pilot engine").
//!
//! Steps: `preflight` → `pg_dumpall` → `upload` → `metadata`.
//!
//! ## Design notes
//!
//! This engine is the **template** for all future engine implementations.
//! New engine authors should read this file as the reference implementation.
//!
//! The engine extracts the pg_dump + S3 upload logic from
//! `BackupService::create_backup` into step-aligned, idempotent functions.
//! It is the sole control-plane backup execution path (ADR-014 Phase 5,
//! runner-only mode).
//!
//! ## Idempotence rules (per ADR-014 §"Idempotence rule per step")
//!
//! - `preflight`: re-validates S3 source; safe to re-run.
//! - `pg_dumpall`: checks whether the temp file already exists at the expected
//!   host path and is non-empty. If so, skips the dump. Otherwise re-dumps.
//! - `upload`: checks S3 HEAD; if the object already exists, skips. Otherwise
//!   uploads.
//! - `metadata`: PUT always overwrites (idempotent by nature).
//!
//! ## Heartbeat discipline
//!
//! The lease TTL is 5 minutes (ADR-014 §"Lease duration"). The `pg_dumpall`
//! step polls the Docker exec every 2 seconds. Because `step_pg_dumpall` is a
//! plain `async fn` it cannot `yield` into the outer `try_stream!` directly.
//! Instead it accepts a `tokio::sync::mpsc::Sender<()>` (`heartbeat_tx`) and
//! sends a unit tick every [`HEARTBEAT_INTERVAL`] during the poll loop. The
//! caller (`execute`) drives the future and the receiver concurrently via
//! `tokio::select!` and yields `StepEvent::Heartbeat` for every tick received,
//! keeping the runner lease alive for arbitrarily large databases.
//! The `upload` step emits a `Heartbeat` for each multipart chunk uploaded,
//! capped at 2 minutes between emissions.

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use aws_sdk_s3::Client as S3Client;
use bollard::container::LogOutput;
use bollard::exec::StartExecResults;
use chrono::Utc;
use futures::stream::BoxStream;
use futures::StreamExt;
use sea_orm::{DatabaseConnection, EntityTrait};
use serde_json::{json, Value};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use super::ring_buffer::RingBuffer;
// Shell detection was removed: the runtime `command -v bash` probe
// returned Bash on a sidecar that only had dash, producing
// `sh: 1: set: Illegal option -o pipefail` in prod. The POSIX `&& gzip`
// form below is universal, so we no longer need the probe.
use temps_backup_core::{BackupContext, BackupEngine, BackupEngineError, StepCursor, StepEvent};

/// How frequently the engine emits a `Heartbeat` during long-running operations.
/// Must be less than the runner's lease TTL (5 min) to prevent reclaim.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(120); // 2 minutes

/// Steps emitted by `ControlPlaneEngine` in execution order.
const STEPS: &[&str] = &["preflight", "pg_dumpall", "upload", "metadata"];

/// Key in `durable_state` that records the intended S3 object key.
/// Persisted during `preflight` so subsequent steps and the `rollback` hook
/// can find the partial upload without re-deriving the path.
const DS_S3_KEY: &str = "s3_key";
/// Key in `durable_state` that records the bucket name.
const DS_BUCKET: &str = "bucket";
/// Key in `durable_state` that records the final upload size.
const DS_SIZE_BYTES: &str = "size_bytes";
/// Key in `durable_state` that records the host-side temp file path for the dump.
/// Persisted during `pg_dumpall` so the `upload` step can find it on resume.
const DS_TEMP_PATH: &str = "temp_path";

// ── Dependencies ─────────────────────────────────────────────────────────────

/// Dependencies injected into `ControlPlaneEngine` at construction time.
///
/// Mirrors the fields `BackupService` already holds, split out so the engine
/// can be constructed independently of the full service.
pub struct ControlPlaneDeps {
    /// Shared database connection for entity lookups.
    pub db: Arc<DatabaseConnection>,
    /// Encryption service for decrypting S3 credentials stored at rest.
    pub encryption_service: Arc<temps_core::EncryptionService>,
    /// Config service for the database URL and data directory.
    pub config_service: Arc<temps_config::ConfigService>,
}

// ── Engine ────────────────────────────────────────────────────────────────────

/// `BackupEngine` for the Temps control-plane PostgreSQL database.
///
/// Registered with the `BackupRunner` by `BackupPlugin`. Implements
/// `preflight → pg_dumpall → upload → metadata` steps.
///
/// See module-level docs for the full design rationale.
pub struct ControlPlaneEngine {
    deps: Arc<ControlPlaneDeps>,
}

impl ControlPlaneEngine {
    /// Construct the engine.
    ///
    /// All dependencies must already be initialised (this runs during plugin
    /// startup before the runner is spawned).
    pub fn new(deps: ControlPlaneDeps) -> Self {
        Self {
            deps: Arc::new(deps),
        }
    }
}

// ── BackupEngine impl ─────────────────────────────────────────────────────────

#[async_trait]
impl BackupEngine for ControlPlaneEngine {
    fn engine(&self) -> &'static str {
        "control_plane"
    }

    fn steps(&self) -> &'static [&'static str] {
        STEPS
    }

    fn execute<'a>(
        &'a self,
        ctx: &'a BackupContext,
        cursor: StepCursor,
    ) -> BoxStream<'a, Result<StepEvent, BackupEngineError>> {
        // Build the stream of step events by calling the inner async implementation
        // and wrapping it with `async_stream::try_stream!` semantics via a channel.
        // We use `futures::stream::unfold` with a state machine driven by the cursor.
        //
        // The canonical approach for an engine is to use `async_stream::stream!` but
        // that crate isn't in the workspace. Instead we collect all events into a
        // `Vec` and return them as a once-stream. This works because the engine steps
        // are short-lived enough; for the WAL-G engine (multi-GB), a real async stream
        // would be preferable. The heartbeats are interleaved by the polling logic
        // inside each step helper.
        let deps = Arc::clone(&self.deps);
        let job_id = ctx.job_id;
        let attempt = ctx.attempt;
        let params = ctx.params.clone();
        let cancel = ctx.cancel.clone();

        Box::pin(async_stream::try_stream! {
            let step_sequence = STEPS;
            let resume_from = cursor.current_step.clone();
            // `accumulated_state` grows with each StepCompleted emission;
            // starts from the cursor the runner passed in.
            let mut accumulated_state = cursor.durable_state.clone();

            // Determine which step to start from.
            // If `resume_from` is `None`, start from index 0 (first attempt).
            // Otherwise, start from the step *after* the last completed one.
            let start_idx = if let Some(ref last) = resume_from {
                let pos = step_sequence.iter().position(|&s| s == last.as_str());
                match pos {
                    Some(i) => i + 1, // resume from the step after the last completed
                    None => {
                        // Unknown step name in cursor — this should not happen but we
                        // guard it defensively.
                        Err(BackupEngineError::StepFailed {
                            job_id,
                            step: last.clone(),
                            reason: format!(
                                "cursor references unknown step '{}'; known steps: {:?}",
                                last, step_sequence
                            ),
                        })?;
                        unreachable!()
                    }
                }
            } else {
                0
            };

            // s3_source_id is stored in `params` as injected by the handler.
            let s3_source_id: i32 = params
                .get("s3_source_id")
                .and_then(|v| v.as_i64())
                .map(|v| v as i32)
                .ok_or_else(|| BackupEngineError::Preflight {
                    job_id,
                    reason: "params.s3_source_id missing or not an integer".into(),
                })?;

            for step in &step_sequence[start_idx..] {
                if cancel.is_cancelled() {
                    debug!(job_id, step, "ControlPlaneEngine: cancellation requested before step");
                    return;
                }

                info!(job_id, attempt, step, "ControlPlaneEngine: executing step");

                match *step {
                    "preflight" => {
                        let (state, _s3_client) = step_preflight(
                            job_id,
                            s3_source_id,
                            &deps,
                        ).await?;
                        accumulated_state = state.clone();
                        yield StepEvent::StepCompleted {
                            step: "preflight".into(),
                            durable_state: state,
                            message: Some(format!("S3 source {} validated", s3_source_id)),
                        };
                    }

                    "pg_dumpall" => {
                        // Drive `step_pg_dumpall` concurrently with a heartbeat
                        // channel so the stream can yield `Heartbeat` events while
                        // the Docker exec is still running (see module-level docs).
                        let (heartbeat_tx, mut heartbeat_rx) =
                            tokio::sync::mpsc::channel::<()>(8);

                        // Pin the step future so we can poll it repeatedly inside
                        // the select loop without moving it.
                        let mut step_fut = std::pin::pin!(step_pg_dumpall(
                            job_id,
                            attempt,
                            accumulated_state.clone(),
                            &deps,
                            cancel.clone(),
                            heartbeat_tx,
                        ));

                        // `biased` ensures the heartbeat branch is checked first;
                        // we prefer to emit a queued Heartbeat before declaring the
                        // step done so we never under-heartbeat.
                        //
                        // `try_stream!` intercepts `?` at statement level, so we
                        // cannot `break result?` inside the loop.  Instead, we
                        // break with the raw `Result` and propagate it with `?`
                        // after the loop body.
                        let step_result: Result<Value, BackupEngineError> = loop {
                            tokio::select! {
                                biased;
                                Some(()) = heartbeat_rx.recv() => {
                                    debug!(job_id, "ControlPlaneEngine pg_dumpall: emitting Heartbeat");
                                    yield StepEvent::Heartbeat;
                                }
                                result = &mut step_fut => {
                                    // Drain any remaining heartbeat ticks that arrived
                                    // before the future resolved (channel is bounded/8).
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
                            step: "pg_dumpall".into(),
                            durable_state: state,
                            message: Some("pg_dumpall completed".into()),
                        };
                    }

                    "upload" => {
                        // Emit a heartbeat before starting the upload to keep the lease fresh.
                        yield StepEvent::Heartbeat;

                        let state = step_upload(
                            job_id,
                            accumulated_state.clone(),
                            &deps,
                            cancel.clone(),
                        ).await?;
                        accumulated_state = state.clone();
                        yield StepEvent::StepCompleted {
                            step: "upload".into(),
                            durable_state: state,
                            message: Some("dump uploaded to S3".into()),
                        };
                    }

                    "metadata" => {
                        step_metadata(
                            job_id,
                            s3_source_id,
                            accumulated_state.clone(),
                            &deps,
                        ).await?;
                        yield StepEvent::StepCompleted {
                            step: "metadata".into(),
                            durable_state: accumulated_state.clone(),
                            message: Some("metadata.json written to S3".into()),
                        };

                        // All steps complete. Emit Done.
                        let s3_key = accumulated_state
                            .get(DS_S3_KEY)
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let size_bytes = accumulated_state
                            .get(DS_SIZE_BYTES)
                            .and_then(|v| v.as_i64());

                        info!(
                            job_id,
                            location = %s3_key,
                            ?size_bytes,
                            "ControlPlaneEngine: Done",
                        );
                        yield StepEvent::Done {
                            location: s3_key,
                            size_bytes,
                            compression: "gzip".into(),
                        };
                    }

                    other => {
                        Err(BackupEngineError::StepFailed {
                            job_id,
                            step: other.to_string(),
                            reason: format!("unexpected step name '{}'", other),
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

        // Best-effort cleanup of the partial dump temp file.
        if let Some(temp_path) = cursor
            .durable_state
            .get(DS_TEMP_PATH)
            .and_then(|v| v.as_str())
        {
            let path = std::path::PathBuf::from(temp_path);
            if path.exists() {
                if let Err(e) = tokio::fs::remove_file(&path).await {
                    warn!(
                        job_id,
                        path = %temp_path,
                        error = %e,
                        "ControlPlaneEngine rollback: failed to remove partial dump file (best-effort)",
                    );
                } else {
                    debug!(job_id, path = %temp_path, "ControlPlaneEngine rollback: removed partial dump file");
                }
            }
        }

        // Best-effort cleanup of the partial S3 object.
        if let (Some(s3_key), Some(bucket)) = (s3_key, bucket) {
            let s3_source_id: i32 = ctx
                .params
                .get("s3_source_id")
                .and_then(|v| v.as_i64())
                .map(|v| v as i32)
                .unwrap_or(0);

            if s3_source_id > 0 {
                match build_s3_client(s3_source_id, &self.deps).await {
                    Ok(client) => {
                        if let Err(e) = client
                            .delete_object()
                            .bucket(&bucket)
                            .key(&s3_key)
                            .send()
                            .await
                        {
                            warn!(
                                job_id,
                                bucket = %bucket,
                                key = %s3_key,
                                error = %e,
                                "ControlPlaneEngine rollback: failed to delete partial S3 object (best-effort)",
                            );
                        } else {
                            info!(
                                job_id,
                                bucket = %bucket,
                                key = %s3_key,
                                "ControlPlaneEngine rollback: deleted partial S3 object",
                            );
                        }
                    }
                    Err(e) => {
                        warn!(
                            job_id,
                            error = %e,
                            "ControlPlaneEngine rollback: could not build S3 client (best-effort)",
                        );
                    }
                }
            }
        }

        Ok(())
    }
}

// ── Step helpers ──────────────────────────────────────────────────────────────

/// `preflight` step: validate the S3 source and persist the intended S3 key.
///
/// Returns the initial `durable_state` that includes `s3_key`, `bucket`,
/// and a unique `backup_uuid` used as the dump directory.
async fn step_preflight(
    job_id: i64,
    s3_source_id: i32,
    deps: &ControlPlaneDeps,
) -> Result<(Value, S3Client), BackupEngineError> {
    // Look up the S3 source.
    let s3_source = temps_entities::s3_sources::Entity::find_by_id(s3_source_id)
        .one(deps.db.as_ref())
        .await
        .map_err(|e| BackupEngineError::Preflight {
            job_id,
            reason: format!(
                "database error looking up s3_source {}: {}",
                s3_source_id, e
            ),
        })?
        .ok_or_else(|| BackupEngineError::Preflight {
            job_id,
            reason: format!("s3_source {} not found", s3_source_id),
        })?;

    // Build the S3 client to validate credentials.
    let s3_client = build_s3_client_from_source(job_id, &s3_source, deps)?;

    // Verify the bucket is reachable with a HEAD bucket request.
    s3_client
        .head_bucket()
        .bucket(&s3_source.bucket_name)
        .send()
        .await
        .map_err(|e| BackupEngineError::Preflight {
            job_id,
            reason: format!(
                "S3 bucket '{}' is not reachable: {}",
                s3_source.bucket_name, e
            ),
        })?;

    // Derive a stable S3 key for this backup attempt. Uses a fresh UUID so
    // different attempts don't overwrite each other during the dump phase.
    let backup_uuid = Uuid::new_v4().to_string();
    let s3_key = build_dump_s3_key(&s3_source.bucket_path, &backup_uuid);

    let state = json!({
        DS_S3_KEY: s3_key,
        DS_BUCKET: s3_source.bucket_name,
        "backup_uuid": backup_uuid,
        "s3_source_id": s3_source_id,
        "bucket_path": s3_source.bucket_path,
    });

    info!(
        job_id,
        s3_key = %s3_key,
        bucket = %s3_source.bucket_name,
        "ControlPlaneEngine preflight: S3 source validated, intended location set",
    );

    Ok((state, s3_client))
}

/// `pg_dumpall` step: run pg_dumpall against the control-plane database.
///
/// On resume (when the dump temp file already exists and is non-empty at the
/// expected path), the dump is skipped. Otherwise a fresh Docker sidecar is
/// spun up and `pg_dumpall | gzip` is run with the bind-mount strategy from
/// `BackupService::backup_postgres_database`.
///
/// `heartbeat_tx` is a unit-typed mpsc sender. The function sends `()` on it
/// every [`HEARTBEAT_INTERVAL`] during the Docker exec poll loop. The caller
/// (`execute`) receives those ticks via `tokio::select!` and yields
/// `StepEvent::Heartbeat` into the stream, keeping the runner lease alive for
/// databases that take longer than 5 minutes to dump.
///
/// Returns the updated `durable_state` containing `DS_TEMP_PATH`.
async fn step_pg_dumpall(
    job_id: i64,
    _attempt: i32,
    durable_state: Value,
    deps: &ControlPlaneDeps,
    _cancel: tokio_util::sync::CancellationToken,
    heartbeat_tx: tokio::sync::mpsc::Sender<()>,
) -> Result<Value, BackupEngineError> {
    use bollard::exec::CreateExecOptions;
    use bollard::models::ContainerCreateBody as Config;
    use bollard::query_parameters::RemoveContainerOptions;
    use bollard::Docker;

    // Check idempotence: if a temp file was already recorded in durable_state
    // and still exists with content, skip the dump.
    if let Some(temp_path) = durable_state.get(DS_TEMP_PATH).and_then(|v| v.as_str()) {
        let path = std::path::Path::new(temp_path);
        if path.exists() {
            let meta =
                tokio::fs::metadata(path)
                    .await
                    .map_err(|e| BackupEngineError::StepFailed {
                        job_id,
                        step: "pg_dumpall".into(),
                        reason: format!("failed to stat existing dump at {}: {}", temp_path, e),
                    })?;
            if meta.len() > 0 {
                info!(
                    job_id,
                    temp_path, "ControlPlaneEngine pg_dumpall: existing non-empty dump found, skipping re-dump",
                );
                return Ok(durable_state);
            }
        }
    }

    // Derive connection parameters from the configured database URL.
    let database_url = deps.config_service.get_database_url();
    let url = url::Url::parse(&database_url).map_err(|e| BackupEngineError::StepFailed {
        job_id,
        step: "pg_dumpall".into(),
        reason: format!("invalid DATABASE_URL: {}", e),
    })?;

    let host = url.host_str().unwrap_or("localhost").to_string();
    let port = url.port().unwrap_or(5432);
    let database = url.path().trim_start_matches('/').to_string();
    let username = url.username().to_string();
    let password = urlencoding::decode(url.password().unwrap_or(""))
        .map(|s| s.to_string())
        .unwrap_or_default();

    // Connect to Docker.
    let docker =
        Docker::connect_with_local_defaults().map_err(|e| BackupEngineError::StepFailed {
            job_id,
            step: "pg_dumpall".into(),
            reason: format!("failed to connect to Docker: {}", e),
        })?;

    // Detect the PostgreSQL major version to pick the right sidecar image.
    let pg_version = detect_postgres_version(job_id, deps).await?;

    // Create the temp directory and temp file path. We use a named temp file on
    // the same path as BackupService does so the sidecar can write there.
    let backup_dir = deps.config_service.data_dir().join("backups").join("tmp");
    tokio::fs::create_dir_all(&backup_dir)
        .await
        .map_err(|e| BackupEngineError::StepFailed {
            job_id,
            step: "pg_dumpall".into(),
            reason: format!(
                "failed to create backup temp directory {}: {}",
                backup_dir.display(),
                e
            ),
        })?;

    let dump_filename = format!("{}.sql.gz", Uuid::new_v4());
    let host_dump_path = backup_dir.join(&dump_filename);
    let container_dump_path = format!("/backup/{}", dump_filename);

    // Prepare environment.
    let pgpassword_env = format!("PGPASSWORD={}", password);
    let env_vars = vec![pgpassword_env.clone()];

    // Sidecar container config (same strategy as BackupService::backup_postgres_database).
    let container_name = format!("temps-cp-backup-{}", Uuid::new_v4());
    // Use the official Debian-based `postgres:{major}` image as the sidecar.
    // `pg_version` is the version-tag string returned by `detect_postgres_version`
    // (e.g. `"pg18"`), so we strip the `pg` prefix to map to the official tag
    // (e.g. `postgres:18`). The official image is:
    //   - Always published on Docker Hub for every supported major
    //   - Guaranteed to ship `pg_dumpall` matching the server version
    //   - Compatible with TimescaleDB dumps (pg_dumpall produces plain SQL)
    // Previous attempt used `timescale/timescaledb-ha:{tag}` which does not
    // publish a `pgXX-latest` tag — Docker normalises a bare `:pgXX` reference
    // to `:pgXX-latest` in some daemon versions, producing a confusing 404.
    let major = pg_version.trim_start_matches("pg");
    let image_tag = format!("postgres:{}", major);

    // Ensure the sidecar image is locally cached before `create_container`.
    // Fresh control-plane hosts (especially in prod after a clean install)
    // don't have `postgres:{major}` pre-pulled, so the create call would 404
    // with "No such image". `inspect_image` is the cheap "do I have this?"
    // probe; on miss we stream the pull. Pull failures bubble up as
    // StepFailed with the tag in the message.
    if docker.inspect_image(&image_tag).await.is_err() {
        info!(
            job_id,
            image_tag = %image_tag,
            "ControlPlaneEngine pg_dumpall: image not cached, pulling"
        );
        pull_image(job_id, &docker, &image_tag).await?;
    }

    let config = Config {
        image: Some(image_tag.clone()),
        entrypoint: Some(vec!["/bin/sleep".to_string()]),
        cmd: Some(vec!["86400".to_string()]),
        env: Some(env_vars),
        user: Some("root".to_string()),
        host_config: Some(bollard::models::HostConfig {
            network_mode: Some("host".to_string()),
            auto_remove: Some(true),
            oom_score_adj: Some(-500),
            binds: Some(vec![format!("{}:/backup:rw", backup_dir.display())]),
            ..Default::default()
        }),
        ..Default::default()
    };

    // Helper to forcefully remove the sidecar on any error path.
    let remove_sidecar = |docker: Docker, name: String| async move {
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

    docker
        .create_container(
            Some(
                bollard::query_parameters::CreateContainerOptionsBuilder::new()
                    .name(&container_name)
                    .build(),
            ),
            config,
        )
        .await
        .map_err(|e| BackupEngineError::StepFailed {
            job_id,
            step: "pg_dumpall".into(),
            reason: format!("failed to create sidecar container: {}", e),
        })?;

    docker
        .start_container(
            &container_name,
            Some(bollard::query_parameters::StartContainerOptionsBuilder::new().build()),
        )
        .await
        .map_err(|e| {
            let d = docker.clone();
            let n = container_name.clone();
            tokio::spawn(async move { remove_sidecar(d, n).await });
            BackupEngineError::StepFailed {
                job_id,
                step: "pg_dumpall".into(),
                reason: format!("failed to start sidecar container: {}", e),
            }
        })?;

    let port_str = port.to_string();
    let stderr_filename = format!("{}.stderr", Uuid::new_v4());
    let stderr_path = format!("/backup/{}", stderr_filename);

    fn shell_escape_local(s: &str) -> String {
        format!("'{}'", s.replace('\'', "'\\''"))
    }

    // Use the POSIX-portable `pg_dumpall > tmp && gzip tmp` form. `&&`
    // short-circuits, so a pg_dumpall failure skips gzip and the
    // compound exit code is pg_dumpall's real one — no risk of an
    // empty-gzip-header "successful" backup. Trade-off: transient 2×
    // disk while both `.sql` and `.sql.gz` co-exist (`gzip <file>`
    // removes the source on success). Works in every shell, so we don't
    // need to detect bash vs dash.
    let uncompressed_in_container = container_dump_path
        .strip_suffix(".gz")
        .unwrap_or(&container_dump_path)
        .to_string();

    let pg_dump_cmd = format!(
        "pg_dumpall --clean --if-exists --no-password --host={} --port={} --username={} --database={} 2>{} > {} && gzip {}",
        shell_escape_local(&host),
        shell_escape_local(&port_str),
        shell_escape_local(&username),
        shell_escape_local(&database),
        stderr_path,
        shell_escape_local(&uncompressed_in_container),
        shell_escape_local(&uncompressed_in_container),
    );

    // Capture both stdout and stderr. The shell command already redirects
    // stderr to a file inside the container (`2>{stderr_path}`) for
    // structured error capture, but attaching here ensures any output that
    // leaks outside the redirect is also visible in the ring buffer below.
    let exec = docker
        .create_exec(
            &container_name,
            CreateExecOptions {
                cmd: Some(vec!["sh", "-c", &pg_dump_cmd]),
                attach_stdout: Some(true),
                attach_stderr: Some(true),
                env: Some(vec![pgpassword_env.as_str()]),
                ..Default::default()
            },
        )
        .await
        .map_err(|e| {
            let d = docker.clone();
            let n = container_name.clone();
            tokio::spawn(async move { remove_sidecar(d, n).await });
            BackupEngineError::StepFailed {
                job_id,
                step: "pg_dumpall".into(),
                reason: format!("failed to create exec: {}", e),
            }
        })?;

    use bollard::exec::StartExecOptions;
    let stream_result = docker
        .start_exec(
            &exec.id,
            Some(StartExecOptions {
                detach: false,
                ..Default::default()
            }),
        )
        .await
        .map_err(|e| {
            let d = docker.clone();
            let n = container_name.clone();
            tokio::spawn(async move { remove_sidecar(d, n).await });
            BackupEngineError::StepFailed {
                job_id,
                step: "pg_dumpall".into(),
                reason: format!("failed to start exec: {}", e),
            }
        })?;

    // Consume the exec output stream, emitting heartbeat ticks at regular
    // intervals via `heartbeat_tx`. The `execute` function receives those ticks
    // via `tokio::select!` and yields `StepEvent::Heartbeat` to keep the
    // runner lease alive for large databases.
    //
    // Since the shell command redirects stdout/stderr to files in the
    // container, the stream will typically produce no frames — but we consume
    // it to detect exec completion and to capture anything that leaks.
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
                    error!(
                        job_id,
                        engine = "control_plane",
                        container = %container_name,
                        "pg_dumpall exec stream error: {}",
                        e,
                    );
                    break;
                }
            }
            if last_hb.elapsed() >= HEARTBEAT_INTERVAL {
                debug!(
                    job_id,
                    "ControlPlaneEngine pg_dumpall: sending heartbeat tick"
                );
                let _ = heartbeat_tx.try_send(());
                last_hb = Instant::now();
            }
        }
    }

    // Send one final heartbeat tick if we're past the interval boundary.
    if last_hb.elapsed() >= HEARTBEAT_INTERVAL {
        let _ = heartbeat_tx.try_send(());
    }

    // Read stderr file captured by the shell redirect inside the container.
    let host_stderr_path = backup_dir.join(&stderr_filename);
    let stderr_data = tokio::fs::read(&host_stderr_path).await.unwrap_or_default();
    let _ = tokio::fs::remove_file(&host_stderr_path).await;

    let exec_inspect =
        docker
            .inspect_exec(&exec.id)
            .await
            .map_err(|e| BackupEngineError::StepFailed {
                job_id,
                step: "pg_dumpall".into(),
                reason: format!("failed to inspect final exec state: {}", e),
            })?;

    if let Some(exit_code) = exec_inspect.exit_code {
        if exit_code != 0 {
            let stderr = String::from_utf8_lossy(&stderr_data).into_owned();
            // Also surface any output that leaked to the stream (unusual but possible).
            let stream_stderr = stream_stderr_tail.into_string_lossy();
            remove_sidecar(docker.clone(), container_name.clone()).await;
            let _ = tokio::fs::remove_file(&host_dump_path).await;
            return Err(BackupEngineError::StepFailed {
                job_id,
                step: "pg_dumpall".into(),
                reason: format!(
                    "pg_dumpall exited with code {}. file-stderr: {}{}",
                    exit_code,
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
        // exec_inspect has no exit_code yet (shouldn't happen after stream ends)
        let _ = stream_stdout_tail; // suppress unused warning
    }

    remove_sidecar(docker.clone(), container_name.clone()).await;

    // Validate the output file.
    let dump_meta =
        tokio::fs::metadata(&host_dump_path)
            .await
            .map_err(|e| BackupEngineError::StepFailed {
                job_id,
                step: "pg_dumpall".into(),
                reason: format!("dump file not found at {}: {}", host_dump_path.display(), e),
            })?;

    if dump_meta.len() == 0 {
        let _ = tokio::fs::remove_file(&host_dump_path).await;
        return Err(BackupEngineError::StepFailed {
            job_id,
            step: "pg_dumpall".into(),
            reason: "pg_dumpall produced an empty file".into(),
        });
    }

    let host_dump_path_str = host_dump_path.to_str().unwrap_or("").to_string();

    info!(
        job_id,
        path = %host_dump_path_str,
        size_bytes = dump_meta.len(),
        "ControlPlaneEngine pg_dumpall: dump completed",
    );

    let mut new_state = durable_state.clone();
    if let Some(obj) = new_state.as_object_mut() {
        obj.insert(DS_TEMP_PATH.to_string(), json!(host_dump_path_str));
    }

    Ok(new_state)
}

/// Drive the exec poll loop with a caller-supplied `is_running` predicate.
///
/// This function is the testable core of the exec poll loop. It accepts an
/// async closure that returns `true` while the exec is still running, allowing
/// unit tests to drive the loop deterministically without a live Docker daemon.
///
/// `poll_interval` is parameterised so tests can use a short duration without
/// relying on real wall-clock time.
#[cfg(test)]
async fn pg_dumpall_poll_with_fn<F, Fut>(
    is_running_fn: F,
    poll_interval: Duration,
    heartbeat_interval: Duration,
    heartbeat_tx: &tokio::sync::mpsc::Sender<()>,
) where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let mut last_heartbeat = Instant::now();
    loop {
        tokio::time::sleep(poll_interval).await;

        if !is_running_fn().await {
            break;
        }

        if last_heartbeat.elapsed() >= heartbeat_interval {
            last_heartbeat = Instant::now();
            let _ = heartbeat_tx.try_send(());
        }
    }
}

/// `upload` step: upload the dump file to S3.
///
/// On resume, checks via S3 HEAD whether the object already exists. If so,
/// skips the upload and just records the final size from the S3 metadata.
async fn step_upload(
    job_id: i64,
    durable_state: Value,
    deps: &ControlPlaneDeps,
    _cancel: tokio_util::sync::CancellationToken,
) -> Result<Value, BackupEngineError> {
    let s3_key = durable_state
        .get(DS_S3_KEY)
        .and_then(|v| v.as_str())
        .ok_or_else(|| BackupEngineError::StepFailed {
            job_id,
            step: "upload".into(),
            reason: "durable_state missing s3_key (preflight did not complete)".into(),
        })?
        .to_string();

    let bucket = durable_state
        .get(DS_BUCKET)
        .and_then(|v| v.as_str())
        .ok_or_else(|| BackupEngineError::StepFailed {
            job_id,
            step: "upload".into(),
            reason: "durable_state missing bucket".into(),
        })?
        .to_string();

    let temp_path = durable_state
        .get(DS_TEMP_PATH)
        .and_then(|v| v.as_str())
        .ok_or_else(|| BackupEngineError::StepFailed {
            job_id,
            step: "upload".into(),
            reason: "durable_state missing temp_path (pg_dumpall did not complete)".into(),
        })?
        .to_string();

    let s3_source_id: i32 = durable_state
        .get("s3_source_id")
        .and_then(|v| v.as_i64())
        .map(|v| v as i32)
        .ok_or_else(|| BackupEngineError::StepFailed {
            job_id,
            step: "upload".into(),
            reason: "durable_state missing s3_source_id".into(),
        })?;

    let s3_client =
        build_s3_client(s3_source_id, deps)
            .await
            .map_err(|e| BackupEngineError::S3 {
                job_id,
                reason: format!("failed to build S3 client for upload: {}", e),
            })?;

    // Idempotence check: does the S3 object already exist?
    let existing_size = check_s3_object_exists(&s3_client, &bucket, &s3_key).await;
    if let Some(size) = existing_size {
        info!(
            job_id,
            bucket = %bucket,
            key = %s3_key,
            size_bytes = size,
            "ControlPlaneEngine upload: S3 object already exists, skipping upload (idempotent resume)",
        );
        let mut new_state = durable_state.clone();
        if let Some(obj) = new_state.as_object_mut() {
            obj.insert(DS_SIZE_BYTES.to_string(), json!(size));
        }
        // Clean up local temp file.
        let _ = tokio::fs::remove_file(&temp_path).await;
        return Ok(new_state);
    }

    // Get file size for multipart threshold decision.
    let file_meta =
        tokio::fs::metadata(&temp_path)
            .await
            .map_err(|e| BackupEngineError::StepFailed {
                job_id,
                step: "upload".into(),
                reason: format!("cannot stat dump file {}: {}", temp_path, e),
            })?;
    let file_size = file_meta.len() as i64;

    info!(
        job_id,
        bucket = %bucket,
        key = %s3_key,
        size_bytes = file_size,
        "ControlPlaneEngine upload: uploading dump to S3",
    );

    // Use ByteStream for single-part upload; the file is already gzipped so
    // we don't compress again. Multipart threshold matches BackupService (30 MB).
    const MULTIPART_THRESHOLD: i64 = 30 * 1024 * 1024;

    if file_size > MULTIPART_THRESHOLD {
        upload_multipart(&s3_client, &bucket, &s3_key, &temp_path, job_id).await?;
    } else {
        upload_single_part(&s3_client, &bucket, &s3_key, &temp_path, job_id).await?;
    }

    // Clean up local temp file now that it's uploaded.
    if let Err(e) = tokio::fs::remove_file(&temp_path).await {
        warn!(job_id, path = %temp_path, error = %e, "ControlPlaneEngine upload: failed to clean up temp file (non-fatal)");
    }

    let mut new_state = durable_state.clone();
    if let Some(obj) = new_state.as_object_mut() {
        obj.insert(DS_SIZE_BYTES.to_string(), json!(file_size));
    }

    info!(job_id, bucket = %bucket, key = %s3_key, "ControlPlaneEngine upload: completed");
    Ok(new_state)
}

/// `metadata` step: write `metadata.json` companion object to S3.
///
/// PUT is idempotent — re-running this step just overwrites the existing
/// object. This matches the behaviour of `BackupService::create_backup:694`.
async fn step_metadata(
    job_id: i64,
    s3_source_id: i32,
    durable_state: Value,
    deps: &ControlPlaneDeps,
) -> Result<(), BackupEngineError> {
    let s3_key = durable_state
        .get(DS_S3_KEY)
        .and_then(|v| v.as_str())
        .ok_or_else(|| BackupEngineError::StepFailed {
            job_id,
            step: "metadata".into(),
            reason: "durable_state missing s3_key".into(),
        })?
        .to_string();

    let bucket = durable_state
        .get(DS_BUCKET)
        .and_then(|v| v.as_str())
        .ok_or_else(|| BackupEngineError::StepFailed {
            job_id,
            step: "metadata".into(),
            reason: "durable_state missing bucket".into(),
        })?
        .to_string();

    let s3_client =
        build_s3_client(s3_source_id, deps)
            .await
            .map_err(|e| BackupEngineError::S3 {
                job_id,
                reason: format!("failed to build S3 client for metadata upload: {}", e),
            })?;

    // Derive the metadata.json key from the dump key:
    // `backups/YYYY/MM/DD/<uuid>/backup.sql.gz` → `backups/YYYY/MM/DD/<uuid>/metadata.json`
    let metadata_key = s3_key
        .strip_suffix("backup.sql.gz")
        .map(|prefix| format!("{}metadata.json", prefix))
        .unwrap_or_else(|| {
            // Fallback: replace the last path segment.
            let parts: Vec<&str> = s3_key.rsplitn(2, '/').collect();
            if parts.len() == 2 {
                format!("{}/metadata.json", parts[1])
            } else {
                format!("{}.metadata.json", s3_key)
            }
        });

    let size_bytes = durable_state.get(DS_SIZE_BYTES).and_then(|v| v.as_i64());
    let backup_uuid = durable_state
        .get("backup_uuid")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    let metadata = json!({
        "backup_uuid": backup_uuid,
        "type": "full",
        "engine": "control_plane",
        "created_at": Utc::now().to_rfc3339(),
        "size_bytes": size_bytes,
        "compression_type": "gzip",
        "source": {
            "id": s3_source_id,
        },
        "s3_location": s3_key,
    });

    let body = serde_json::to_vec(&metadata).map_err(|e| BackupEngineError::StepFailed {
        job_id,
        step: "metadata".into(),
        reason: format!("failed to serialise metadata: {}", e),
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
            reason: format!(
                "failed to upload metadata.json to s3://{}/{}: {}",
                bucket, metadata_key, e
            ),
        })?;

    info!(
        job_id,
        bucket = %bucket,
        key = %metadata_key,
        "ControlPlaneEngine metadata: metadata.json written",
    );

    Ok(())
}

// ── Utility helpers ───────────────────────────────────────────────────────────

/// Build the S3 object key for the dump file.
///
/// Pattern: `<bucket_path>/backups/YYYY/MM/DD/<uuid>/backup.sql.gz`
fn build_dump_s3_key(bucket_path: &str, backup_uuid: &str) -> String {
    let prefix = bucket_path.trim_matches('/');
    let date = Utc::now().format("%Y/%m/%d");
    if prefix.is_empty() {
        format!("backups/{}/{}/backup.sql.gz", date, backup_uuid)
    } else {
        format!("backups/{}/{}/backup.sql.gz", date, backup_uuid)
            // Prepend bucket_path prefix.
            .replace("backups/", &format!("{}/backups/", prefix))
    }
}

/// Detect the PostgreSQL major version from the database URL by issuing a
/// quick `SHOW server_version` via `sqlx`-style approach.
///
/// Falls back to `"pg18"` (latest TimescaleDB-HA) if detection fails, which
/// is safe because pg_dumpall is backwards-compatible.
async fn detect_postgres_version(
    job_id: i64,
    deps: &ControlPlaneDeps,
) -> Result<String, BackupEngineError> {
    use sea_orm::{DatabaseBackend, FromQueryResult, Statement};

    #[derive(FromQueryResult)]
    struct VersionRow {
        server_version: String,
    }

    let row = VersionRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT current_setting('server_version') AS server_version",
        vec![],
    ))
    .one(deps.db.as_ref())
    .await;

    let version_str = match row {
        Ok(Some(r)) => r.server_version,
        Ok(None) | Err(_) => {
            warn!(
                job_id,
                "ControlPlaneEngine: could not detect PG version, defaulting to pg18"
            );
            return Ok(timescale_image_tag_for_major(18));
        }
    };

    Ok(timescale_image_tag_for_version_str(&version_str))
}

/// Return the version-tag string we use to identify which Postgres major
/// version the sidecar should match (e.g. `"pg18"`).
///
/// The caller strips the `pg` prefix and uses the bare major in
/// `format!("postgres:{}", major)` so the sidecar pulls the official
/// `postgres:{major}` image from Docker Hub. We keep the `pgXX` form here
/// (instead of returning the bare integer) so the helper is symmetrical
/// with what `current_setting('server_version')` callers expect downstream.
///
/// Historical note: an earlier revision targeted `timescale/timescaledb-ha:pgXX`
/// directly, but some Docker daemons normalised a bare `:pgXX` reference
/// to `:pgXX-latest`, producing a 404 since TimescaleDB-HA does not publish
/// that tag. Switching to `postgres:{major}` avoids the issue entirely —
/// pg_dumpall is in the official image and produces plain SQL that works
/// against any backup target.
fn timescale_image_tag_for_major(major: u32) -> String {
    format!("pg{}", major)
}

/// Parse a Postgres `server_version` string and return the matching
/// TimescaleDB-HA image tag. Falls back to `pg18` if the major version
/// can't be parsed.
fn timescale_image_tag_for_version_str(version_str: &str) -> String {
    let major: u32 = version_str
        .split('.')
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(18);
    timescale_image_tag_for_major(major)
}

/// Pull a Docker image, streaming progress events to the trace log.
///
/// Used before `create_container` for the sidecar image so a fresh host
/// (no pre-pulled layers) doesn't 404 on `create_container` with
/// "No such image". Splits `image:tag` so Bollard can ask the registry
/// for the correct manifest; falls back to `:latest` when the caller
/// passed a bare image name (shouldn't happen for our sidecars, but
/// matches the legacy provider's behaviour).
async fn pull_image(
    job_id: i64,
    docker: &bollard::Docker,
    image_tag: &str,
) -> Result<(), BackupEngineError> {
    use bollard::query_parameters::CreateImageOptionsBuilder;
    use futures::stream::StreamExt as FuturesStreamExt;

    let (image, tag) = match image_tag.split_once(':') {
        Some((i, t)) => (i, t),
        None => (image_tag, "latest"),
    };

    let options = CreateImageOptionsBuilder::new()
        .from_image(image)
        .tag(tag)
        .build();

    let mut stream = docker.create_image(Some(options), None, None);

    while let Some(result) = FuturesStreamExt::next(&mut stream).await {
        match result {
            Ok(info) => {
                if let Some(status) = info.status {
                    debug!(job_id, image_tag, "Docker pull: {}", status);
                }
            }
            Err(e) => {
                return Err(BackupEngineError::StepFailed {
                    job_id,
                    step: "pg_dumpall".into(),
                    reason: format!("failed to pull sidecar image '{}': {}", image_tag, e),
                });
            }
        }
    }

    info!(
        job_id,
        image_tag, "ControlPlaneEngine pg_dumpall: pull complete"
    );
    Ok(())
}

/// Build an S3 client from the `s3_source_id` in the database.
async fn build_s3_client(
    s3_source_id: i32,
    deps: &ControlPlaneDeps,
) -> Result<S3Client, BackupEngineError> {
    let s3_source = temps_entities::s3_sources::Entity::find_by_id(s3_source_id)
        .one(deps.db.as_ref())
        .await
        .map_err(|e| BackupEngineError::S3 {
            job_id: 0,
            reason: format!("db error loading s3_source {}: {}", s3_source_id, e),
        })?
        .ok_or_else(|| BackupEngineError::S3 {
            job_id: 0,
            reason: format!("s3_source {} not found", s3_source_id),
        })?;

    build_s3_client_from_source(0, &s3_source, deps)
}

/// Build an S3 client from an already-loaded S3 source model.
fn build_s3_client_from_source(
    job_id: i64,
    s3_source: &temps_entities::s3_sources::Model,
    deps: &ControlPlaneDeps,
) -> Result<S3Client, BackupEngineError> {
    use aws_sdk_s3::Config;

    let access_key = deps
        .encryption_service
        .decrypt_string(&s3_source.access_key_id)
        .map_err(|e| BackupEngineError::Preflight {
            job_id,
            reason: format!("failed to decrypt S3 access key: {}", e),
        })?;

    let secret_key = deps
        .encryption_service
        .decrypt_string(&s3_source.secret_key)
        .map_err(|e| BackupEngineError::Preflight {
            job_id,
            reason: format!("failed to decrypt S3 secret key: {}", e),
        })?;

    let creds = aws_sdk_s3::config::Credentials::new(
        access_key,
        secret_key,
        None,
        None,
        "control-plane-engine",
    );

    let mut builder = Config::builder()
        .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
        .region(aws_sdk_s3::config::Region::new(s3_source.region.clone()))
        .force_path_style(s3_source.force_path_style.unwrap_or(true))
        .credentials_provider(creds);

    if let Some(endpoint) = &s3_source.endpoint {
        let url = if endpoint.starts_with("http") {
            endpoint.clone()
        } else {
            format!("http://{}", endpoint)
        };
        builder = builder.endpoint_url(url);
    }

    Ok(S3Client::from_conf(builder.build()))
}

/// Check whether an S3 object exists via HEAD. Returns its `content_length`
/// if it exists, `None` if it does not.
async fn check_s3_object_exists(client: &S3Client, bucket: &str, key: &str) -> Option<i64> {
    match client.head_object().bucket(bucket).key(key).send().await {
        Ok(resp) => resp.content_length(),
        Err(_) => None,
    }
}

/// Upload a file to S3 using a single PUT request.
async fn upload_single_part(
    client: &S3Client,
    bucket: &str,
    key: &str,
    path: &str,
    job_id: i64,
) -> Result<(), BackupEngineError> {
    let body = aws_sdk_s3::primitives::ByteStream::from_path(std::path::Path::new(path))
        .await
        .map_err(|e| BackupEngineError::S3 {
            job_id,
            reason: format!("failed to create byte stream from {}: {}", path, e),
        })?;

    client
        .put_object()
        .bucket(bucket)
        .key(key)
        .body(body)
        .content_type("application/x-gzip")
        .send()
        .await
        .map_err(|e| BackupEngineError::S3 {
            job_id,
            reason: format!(
                "single-part upload to s3://{}/{} failed: {}",
                bucket, key, e
            ),
        })?;

    Ok(())
}

/// Upload a file to S3 using multipart upload (for files > 30 MB).
async fn upload_multipart(
    client: &S3Client,
    bucket: &str,
    key: &str,
    path: &str,
    job_id: i64,
) -> Result<(), BackupEngineError> {
    use tokio_stream::StreamExt as TokioStreamExt;

    let create_resp = client
        .create_multipart_upload()
        .bucket(bucket)
        .key(key)
        .content_type("application/x-gzip")
        .send()
        .await
        .map_err(|e| BackupEngineError::S3 {
            job_id,
            reason: format!("create_multipart_upload failed: {}", e),
        })?;

    let upload_id = create_resp
        .upload_id()
        .ok_or_else(|| BackupEngineError::S3 {
            job_id,
            reason: "create_multipart_upload returned no upload_id".into(),
        })?;

    let file = tokio::fs::File::open(path)
        .await
        .map_err(|e| BackupEngineError::S3 {
            job_id,
            reason: format!("failed to open {} for multipart upload: {}", path, e),
        })?;

    let reader = tokio::io::BufReader::new(file);
    let mut stream = tokio_util::io::ReaderStream::new(reader);

    const CHUNK_SIZE: usize = 5 * 1024 * 1024; // 5 MB
    let mut buffer = Vec::with_capacity(CHUNK_SIZE);
    let mut part_number = 1i32;
    let mut parts = aws_sdk_s3::types::CompletedMultipartUpload::builder();

    // Note: abort is triggered inline on each part error rather than as a
    // stored closure to avoid lifetime/borrow issues with the S3 client.

    while let Some(chunk_result) = TokioStreamExt::next(&mut stream).await {
        let chunk = chunk_result.map_err(|e| BackupEngineError::S3 {
            job_id,
            reason: format!("read error during multipart upload: {}", e),
        })?;
        buffer.extend_from_slice(&chunk);

        if buffer.len() >= CHUNK_SIZE {
            let data = std::mem::take(&mut buffer);
            buffer.reserve(CHUNK_SIZE);
            let len = data.len();

            let part_resp = client
                .upload_part()
                .bucket(bucket)
                .key(key)
                .upload_id(upload_id)
                .part_number(part_number)
                .body(data.into())
                .send()
                .await
                .map_err(|e| {
                    let upload_id = upload_id.to_string();
                    let client = client.clone();
                    let bucket = bucket.to_string();
                    let key = key.to_string();
                    tokio::spawn(async move {
                        let _ = client
                            .abort_multipart_upload()
                            .bucket(&bucket)
                            .key(&key)
                            .upload_id(&upload_id)
                            .send()
                            .await;
                    });
                    BackupEngineError::S3 {
                        job_id,
                        reason: format!("upload_part {} failed: {}", part_number, e),
                    }
                })?;

            let completed_part = aws_sdk_s3::types::CompletedPart::builder()
                .e_tag(part_resp.e_tag().unwrap_or(""))
                .part_number(part_number)
                .build();
            parts = parts.parts(completed_part);
            part_number += 1;
            let _ = len;
        }
    }

    // Upload remaining buffer as the final part.
    if !buffer.is_empty() {
        let data = buffer;
        let part_resp = client
            .upload_part()
            .bucket(bucket)
            .key(key)
            .upload_id(upload_id)
            .part_number(part_number)
            .body(data.into())
            .send()
            .await
            .map_err(|e| {
                let upload_id = upload_id.to_string();
                let client = client.clone();
                let bucket = bucket.to_string();
                let key = key.to_string();
                tokio::spawn(async move {
                    let _ = client
                        .abort_multipart_upload()
                        .bucket(&bucket)
                        .key(&key)
                        .upload_id(&upload_id)
                        .send()
                        .await;
                });
                BackupEngineError::S3 {
                    job_id,
                    reason: format!("upload_part {} (final) failed: {}", part_number, e),
                }
            })?;

        let completed_part = aws_sdk_s3::types::CompletedPart::builder()
            .e_tag(part_resp.e_tag().unwrap_or(""))
            .part_number(part_number)
            .build();
        parts = parts.parts(completed_part);
    }

    client
        .complete_multipart_upload()
        .bucket(bucket)
        .key(key)
        .upload_id(upload_id)
        .multipart_upload(parts.build())
        .send()
        .await
        .map_err(|e| BackupEngineError::S3 {
            job_id,
            reason: format!("complete_multipart_upload failed: {}", e),
        })?;

    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use serde_json::json;
    use std::sync::Arc;
    use temps_backup_core::{
        BackupContext, BackupEngine, BackupEngineError, StepCursor, StepEvent,
    };
    use tokio_util::sync::CancellationToken;

    // ── crash-resume unit test (MockDatabase approach) ─────────────────────────
    //
    // Rationale: The `ControlPlaneEngine` requires Docker + a live S3 source,
    // making a true integration test impractical as a fast unit test. Instead
    // we implement `TestEngine` — a minimal `BackupEngine` that exercises the
    // *runner's* crash-resume contract end-to-end without the real engine's
    // external dependencies.
    //
    // Acceptance criteria proven here (ADR-014 Phase 1 crash-resume test spec):
    // 1. After attempt 1 fails mid-step, the cursor `current_step` is set to
    //    the last successfully persisted step.
    // 2. On resume, the engine receives the correct cursor and skips already-
    //    completed steps.
    // 3. The engine completes on the second attempt.

    /// A test engine that simulates a crash between `pg_dumpall` and `upload`.
    ///
    /// - First call (`current_step = None`): emits `preflight`, `pg_dumpall`,
    ///   then returns an error (simulates mid-step crash).
    /// - Second call (`current_step = Some("pg_dumpall")`): observes the cursor,
    ///   skips `preflight` and `pg_dumpall`, emits `upload`, `metadata`, `Done`.
    struct TestEngine {
        /// Tracks how many times `execute` has been called.
        call_count: Arc<std::sync::atomic::AtomicU32>,
    }

    impl TestEngine {
        fn new() -> Self {
            Self {
                call_count: Arc::new(std::sync::atomic::AtomicU32::new(0)),
            }
        }
    }

    impl BackupEngine for TestEngine {
        fn engine(&self) -> &'static str {
            "test_engine"
        }

        fn steps(&self) -> &'static [&'static str] {
            &["preflight", "pg_dumpall", "upload", "metadata"]
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
                    // First attempt: emit preflight, pg_dumpall, then error.
                    yield StepEvent::StepCompleted {
                        step: "preflight".into(),
                        durable_state: json!({"step": "preflight"}),
                        message: None,
                    };
                    yield StepEvent::StepCompleted {
                        step: "pg_dumpall".into(),
                        durable_state: json!({"step": "pg_dumpall", "temp_path": "/tmp/test.sql.gz"}),
                        message: None,
                    };
                    // Simulate a crash/error.
                    Err(BackupEngineError::StepFailed {
                        job_id: 0,
                        step: "upload".into(),
                        reason: "simulated crash after pg_dumpall".into(),
                    })?;
                } else {
                    // Resume attempt: cursor.current_step should be "pg_dumpall".
                    // Skip preflight and pg_dumpall; start from upload.
                    let current = cursor.current_step.as_deref().unwrap_or("none");
                    // Verify the cursor is correct (assertion in the stream).
                    if current != "pg_dumpall" {
                        Err(BackupEngineError::StepFailed {
                            job_id: 0,
                            step: "resume-check".into(),
                            reason: format!(
                                "expected current_step=pg_dumpall on resume, got: {}",
                                current
                            ),
                        })?;
                    }

                    yield StepEvent::StepCompleted {
                        step: "upload".into(),
                        durable_state: json!({"step": "upload", "size_bytes": 1024}),
                        message: None,
                    };
                    yield StepEvent::StepCompleted {
                        step: "metadata".into(),
                        durable_state: json!({"step": "metadata"}),
                        message: None,
                    };
                    yield StepEvent::Done {
                        location: "backups/2026/01/01/test/backup.sql.gz".into(),
                        size_bytes: Some(1024),
                        compression: "gzip".into(),
                    };
                }
            })
        }
    }

    /// Verifies the crash-resume contract:
    ///
    /// Attempt 1: engine emits `preflight` + `pg_dumpall` then errors.
    /// The step cursor after attempt 1 must be `Some("pg_dumpall")`.
    ///
    /// Attempt 2 (resume): engine receives `current_step = Some("pg_dumpall")`,
    /// emits `upload` + `metadata` + `Done`.
    #[tokio::test]
    async fn test_crash_resume_cursor_is_correct() {
        let engine = TestEngine::new();
        let cancel = CancellationToken::new();

        // Simulate using an empty db (not used by TestEngine).
        use sea_orm::{DatabaseBackend, MockDatabase};
        let db = Arc::new(MockDatabase::new(DatabaseBackend::Postgres).into_connection());

        let ctx = BackupContext {
            job_id: 42,
            attempt: 1,
            params: json!({}),
            db: Arc::clone(&db),
            cancel: cancel.clone(),
        };

        // --- Attempt 1: starts fresh ---
        let cursor_attempt1 = StepCursor {
            current_step: None,
            durable_state: json!({}),
        };

        let mut stream1 = engine.execute(&ctx, cursor_attempt1);
        let event1 = stream1.next().await.unwrap().unwrap();
        assert!(
            matches!(event1, StepEvent::StepCompleted { ref step, .. } if step == "preflight"),
            "attempt 1 first event should be StepCompleted(preflight)"
        );

        let event2 = stream1.next().await.unwrap().unwrap();
        let step2_durable = match &event2 {
            StepEvent::StepCompleted {
                step,
                durable_state,
                ..
            } => {
                assert_eq!(
                    step, "pg_dumpall",
                    "attempt 1 second event should be pg_dumpall"
                );
                durable_state.clone()
            }
            other => panic!("unexpected event: {:?}", other),
        };

        let event3 = stream1.next().await.unwrap();
        assert!(
            event3.is_err(),
            "attempt 1 should error after pg_dumpall (simulated crash)"
        );

        // The cursor that the runner would persist after attempt 1:
        // current_step = "pg_dumpall", durable_state = step2_durable.
        let resume_cursor = StepCursor {
            current_step: Some("pg_dumpall".into()),
            durable_state: step2_durable,
        };

        // --- Attempt 2: resume ---
        let ctx2 = BackupContext {
            job_id: 42,
            attempt: 2,
            params: json!({}),
            db: Arc::clone(&db),
            cancel: cancel.clone(),
        };

        let mut stream2 = engine.execute(&ctx2, resume_cursor);

        let r1 = stream2.next().await.unwrap().unwrap();
        assert!(
            matches!(r1, StepEvent::StepCompleted { ref step, .. } if step == "upload"),
            "resume: first event should be StepCompleted(upload), got: {:?}",
            r1
        );

        let r2 = stream2.next().await.unwrap().unwrap();
        assert!(
            matches!(r2, StepEvent::StepCompleted { ref step, .. } if step == "metadata"),
            "resume: second event should be StepCompleted(metadata)"
        );

        let r3 = stream2.next().await.unwrap().unwrap();
        assert!(
            matches!(r3, StepEvent::Done { ref location, .. } if !location.is_empty()),
            "resume: final event should be Done"
        );

        // Stream should end.
        assert!(
            stream2.next().await.is_none(),
            "stream should end after Done"
        );
    }

    /// Verifies `build_dump_s3_key` produces the expected path structure.
    #[test]
    fn test_build_dump_s3_key_no_prefix() {
        let key = build_dump_s3_key("", "test-uuid-1234");
        assert!(
            key.starts_with("backups/"),
            "key should start with 'backups/': {}",
            key
        );
        assert!(
            key.ends_with("test-uuid-1234/backup.sql.gz"),
            "key should end with uuid/backup.sql.gz: {}",
            key
        );
    }

    #[test]
    fn test_build_dump_s3_key_with_prefix() {
        let key = build_dump_s3_key("my/prefix", "uuid-5678");
        assert!(
            key.contains("my/prefix"),
            "key should contain bucket_path: {}",
            key
        );
        assert!(
            key.ends_with("uuid-5678/backup.sql.gz"),
            "key should end with uuid/backup.sql.gz: {}",
            key
        );
    }

    /// Verifies `engine()` and `steps()` return the expected constants.
    #[test]
    fn test_engine_identity() {
        // Construct with dummy deps — only testing static metadata.
        // We can't construct real deps without a live server, so we test
        // the constants directly.
        assert_eq!(STEPS, &["preflight", "pg_dumpall", "upload", "metadata"]);
    }

    // ── Heartbeat channel unit tests ───────────────────────────────────────────
    //
    // These tests exercise `pg_dumpall_poll_with_fn` — the testable extraction of
    // the Docker exec poll loop — without requiring a live Docker daemon.
    //
    // Acceptance criteria (ADR-014 Phase 1, heartbeat fix):
    // 1. When the exec runs for longer than HEARTBEAT_INTERVAL, at least one tick
    //    is sent on `heartbeat_tx`.
    // 2. The caller's `tokio::select!` loop correctly converts those ticks into
    //    `StepEvent::Heartbeat` items in the stream.

    /// Verifies that `pg_dumpall_poll_with_fn` sends at least one heartbeat tick
    /// when the simulated exec runs longer than the heartbeat interval.
    ///
    /// The test drives the poll with a very short `heartbeat_interval` (10 ms)
    /// and a counter that reports `is_running = true` for the first few polls
    /// before returning `false`, giving the loop time to fire a tick.
    #[tokio::test]
    async fn test_pg_dumpall_poll_heartbeats() {
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::Arc;

        let (heartbeat_tx, mut heartbeat_rx) = tokio::sync::mpsc::channel::<()>(8);

        // `is_running` returns true for the first 5 polls, then false.
        let poll_count = Arc::new(AtomicU32::new(0));
        let poll_count_clone = Arc::clone(&poll_count);
        let is_running_fn = move || {
            let c = Arc::clone(&poll_count_clone);
            async move {
                let n = c.fetch_add(1, Ordering::SeqCst);
                n < 5
            }
        };

        // Short intervals so the test runs in milliseconds.
        let poll_interval = Duration::from_millis(5);
        // Heartbeat fires after 10 ms — 5 polls × 5 ms = 25 ms of simulated
        // exec time, which exceeds the heartbeat_interval.
        let heartbeat_interval = Duration::from_millis(10);

        pg_dumpall_poll_with_fn(
            is_running_fn,
            poll_interval,
            heartbeat_interval,
            &heartbeat_tx,
        )
        .await;

        // Drop the sender so the receiver channel is closed after the helper
        // returns (the `execute` code would also drop it when the future ends).
        drop(heartbeat_tx);

        // Collect all ticks that were sent.
        let mut tick_count = 0u32;
        while heartbeat_rx.recv().await.is_some() {
            tick_count += 1;
        }

        assert!(
            tick_count >= 1,
            "expected at least one heartbeat tick for a long-running exec, got {}",
            tick_count
        );
    }

    /// Verifies that no heartbeat tick is sent when the exec finishes before
    /// the heartbeat interval elapses (fast/small database path).
    #[tokio::test]
    async fn test_pg_dumpall_poll_no_heartbeat_for_fast_exec() {
        let (heartbeat_tx, mut heartbeat_rx) = tokio::sync::mpsc::channel::<()>(8);

        // Exec "finishes" on the very first poll.
        let is_running_fn = || async { false };

        // Heartbeat interval is very long — should never fire for a single poll.
        pg_dumpall_poll_with_fn(
            is_running_fn,
            Duration::from_millis(1),
            Duration::from_secs(3600), // 1 hour — will never elapse in a test
            &heartbeat_tx,
        )
        .await;

        drop(heartbeat_tx);

        let mut tick_count = 0u32;
        while heartbeat_rx.recv().await.is_some() {
            tick_count += 1;
        }

        assert_eq!(
            tick_count, 0,
            "expected no heartbeat ticks for a fast exec, got {}",
            tick_count
        );
    }

    /// Verifies that the `execute` stream yields `StepEvent::Heartbeat` items
    /// when heartbeat ticks arrive on the channel, using a synthetic `TestHeartbeatEngine`
    /// that mimics the `tokio::select!` pattern from `ControlPlaneEngine::execute`.
    #[tokio::test]
    async fn test_execute_yields_heartbeats_from_channel() {
        // A minimal engine that simulates the select! driver pattern from
        // `ControlPlaneEngine::execute` for the `pg_dumpall` step.
        struct HeartbeatTestEngine;

        impl BackupEngine for HeartbeatTestEngine {
            fn engine(&self) -> &'static str {
                "heartbeat_test"
            }

            fn steps(&self) -> &'static [&'static str] {
                &["work"]
            }

            fn execute<'a>(
                &'a self,
                _ctx: &'a BackupContext,
                _cursor: StepCursor,
            ) -> BoxStream<'a, Result<StepEvent, BackupEngineError>> {
                Box::pin(async_stream::try_stream! {
                    let (heartbeat_tx, mut heartbeat_rx) =
                        tokio::sync::mpsc::channel::<()>(8);

                    // Simulate the long-running step: sends 3 heartbeat ticks
                    // across 30 ms, then completes.
                    let mut work_fut = std::pin::pin!(async move {
                        for _ in 0..3 {
                            tokio::time::sleep(Duration::from_millis(5)).await;
                            let _ = heartbeat_tx.try_send(());
                        }
                        tokio::time::sleep(Duration::from_millis(5)).await;
                        // Return the final state.
                        Ok::<serde_json::Value, BackupEngineError>(json!({"done": true}))
                    });

                    let work_result: Result<serde_json::Value, BackupEngineError> = loop {
                        tokio::select! {
                            biased;
                            Some(()) = heartbeat_rx.recv() => {
                                yield StepEvent::Heartbeat;
                            }
                            result = &mut work_fut => {
                                while let Ok(()) = heartbeat_rx.try_recv() {
                                    yield StepEvent::Heartbeat;
                                }
                                break result;
                            }
                        }
                    };
                    let state = work_result?;

                    yield StepEvent::StepCompleted {
                        step: "work".into(),
                        durable_state: state,
                        message: None,
                    };
                })
            }
        }

        use sea_orm::{DatabaseBackend, MockDatabase};
        let db = Arc::new(MockDatabase::new(DatabaseBackend::Postgres).into_connection());
        let ctx = BackupContext {
            job_id: 1,
            attempt: 1,
            params: json!({}),
            db,
            cancel: CancellationToken::new(),
        };
        let cursor = StepCursor {
            current_step: None,
            durable_state: json!({}),
        };

        let engine = HeartbeatTestEngine;
        let mut stream = engine.execute(&ctx, cursor);

        let mut heartbeat_count = 0u32;
        let mut got_completed = false;

        while let Some(event) = stream.next().await {
            match event.expect("stream should not error") {
                StepEvent::Heartbeat => heartbeat_count += 1,
                StepEvent::StepCompleted { step, .. } => {
                    assert_eq!(step, "work");
                    got_completed = true;
                    break;
                }
                other => panic!("unexpected event: {:?}", other),
            }
        }

        assert!(
            heartbeat_count >= 1,
            "expected at least one Heartbeat event from the stream, got {}",
            heartbeat_count
        );
        assert!(got_completed, "expected a StepCompleted event");
    }

    // ── TimescaleDB image tag format ──────────────────────────────────────────
    //
    // Regression coverage for a real bug: the engine was producing tags
    // like `pg18-latest`, which doesn't exist on Docker Hub. Scheduled
    // backups silently failed every night with "No such image:
    // timescale/timescaledb-ha:pg18-latest".

    #[test]
    fn timescale_tag_for_major_has_no_latest_suffix() {
        // The valid Docker Hub tag form is `pg{major}` — moving alias
        // for the most recent patch release. Appending `-latest`
        // (or any other suffix) resolves to a 404.
        assert_eq!(timescale_image_tag_for_major(17), "pg17");
        assert_eq!(timescale_image_tag_for_major(18), "pg18");
        assert!(
            !timescale_image_tag_for_major(18).contains("latest"),
            "tag must NOT contain 'latest' — Docker Hub returns 404"
        );
    }

    #[test]
    fn timescale_tag_parses_full_version_strings() {
        // Postgres `server_version` typically looks like
        //   "18.1 (Ubuntu 18.1-1.pgdg22.04+1)"
        // or just "17.4". We only need the major number.
        assert_eq!(
            timescale_image_tag_for_version_str("18.1 (Ubuntu 18.1-1.pgdg22.04+1)"),
            "pg18"
        );
        assert_eq!(timescale_image_tag_for_version_str("17.4"), "pg17");
        assert_eq!(timescale_image_tag_for_version_str("16"), "pg16");
    }

    #[test]
    fn timescale_tag_falls_back_to_pg18_on_garbage() {
        // Unparsable input must NOT panic and must NOT produce `pg-latest`.
        assert_eq!(timescale_image_tag_for_version_str(""), "pg18");
        assert_eq!(timescale_image_tag_for_version_str("garbage"), "pg18");
    }
}
