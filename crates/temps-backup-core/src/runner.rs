//! `BackupRunner`: poll-claim-dispatch-persist loop (ADR-014 §"Runner loop").
//!
//! Phase 0: the runner is fully wired up but has an empty engine registry.
//! It polls the queue every `config.poll_interval`, finds no claimable jobs,
//! and sleeps. No engines are dispatched. Phase 1 registers the first engine
//! (`ControlPlaneEngine`) and the runner begins dispatching.
//!
//! The runner is stateless with respect to the database — it can run on any
//! node that has a connection. Multiple runner instances can process jobs
//! concurrently; the claim query's `FOR UPDATE SKIP LOCKED` prevents
//! double-claiming.

use std::collections::HashMap;
use std::sync::Arc;

use futures::StreamExt;
use sea_orm::DatabaseConnection;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::config::RunnerConfig;
use crate::engine::{BackupContext, BackupEngine, BackupEngineError, StepCursor, StepEvent};
use crate::error::BackupRunnerError;
use crate::notifier::{BackupFailureContext, BackupFailureNotifier};
use crate::queue::{
    backoff_delay, claim_one_job, extend_lease, mark_job_completed, mark_job_failed,
    persist_step_completed, schedule_retry, BackupJobRow,
};

// ── EnqueueJobParams ──────────────────────────────────────────────────────────

/// Parameters for inserting a new `backup_jobs` row via `enqueue_job`.
///
/// Used by handlers and the scheduler in Phase 1+. Phase 0 provides the type
/// so callers can be written before the handler migration is done.
#[derive(Debug, Clone)]
pub struct EnqueueJobParams {
    /// FK to the parent `backups` row.
    pub backup_id: i32,
    /// Engine key (must match a registered `BackupEngine::engine()`).
    pub engine: String,
    /// `"control_plane"` or `"external_service"`.
    pub target_kind: String,
    /// `None` for control-plane; FK to `external_services.id` otherwise.
    pub target_id: Option<i32>,
    /// Engine-specific parameters (S3 bucket, compression, max_concurrent, etc.).
    pub params: serde_json::Value,
    /// Maximum retry count. Defaults to 3 when `None`.
    pub max_attempts: Option<i32>,
    /// Wall-clock timeout override for this job (seconds).
    ///
    /// Resolution order in `enqueue_job`:
    /// 1. This field (`Some(secs)`) — highest priority.
    /// 2. `backup_schedules.max_runtime_secs` — passed by the caller when
    ///    enqueueing a scheduled backup and the schedule has a custom limit.
    /// 3. `crate::timeouts::default_max_runtime_secs(engine)` — engine family
    ///    default (24 h for Postgres, 12 h for S3, 4 h for Redis/Mongo).
    ///
    /// `None` means "use schedule override or engine default." The resolved
    /// value is written into `backup_jobs.max_runtime_secs` at insert time
    /// so the runner never recomputes it at dispatch.
    pub max_runtime_secs: Option<i64>,
}

// ── BackupRunner ──────────────────────────────────────────────────────────────

/// The poll-claim-dispatch-persist loop (ADR-014 §"Runner loop").
///
/// Instantiated by `BackupPlugin::initialize_plugin_services` when
/// `TEMPS_BACKUP_RUNNER_ENABLED=true`. In Phase 0 the engine registry is empty
/// and the runner idles. Phase 1+ register engines via `register_engine`.
pub struct BackupRunner {
    db: Arc<DatabaseConnection>,
    config: RunnerConfig,
    /// Engines keyed by `BackupEngine::engine()`. Populated by `register_engine`.
    engines: HashMap<&'static str, Arc<dyn BackupEngine>>,
    /// Optional notification hook, fired on every terminal failure via a
    /// detached `tokio::spawn`. Set via [`BackupRunner::with_notifier`].
    notifier: Option<Arc<dyn BackupFailureNotifier>>,
}

impl BackupRunner {
    /// Create a runner with an empty engine registry and no failure notifier.
    ///
    /// Call `register_engine` for each engine implementation before calling
    /// `run_forever`. In Phase 0 no engines are registered.
    pub fn new(db: Arc<DatabaseConnection>, config: RunnerConfig) -> Self {
        Self {
            db,
            config,
            engines: HashMap::new(),
            notifier: None,
        }
    }

    /// Attach a failure notifier to this runner (builder pattern).
    ///
    /// When set, the runner fires [`BackupFailureNotifier::notify_failed`] in a
    /// detached `tokio::spawn` every time a job reaches the terminal `failed`
    /// state. Notification failures are logged internally and never surface to
    /// the queue write path.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let notifier: Arc<dyn BackupFailureNotifier> = Arc::new(MyNotifier::new(...));
    /// let runner = BackupRunner::new(db, config).with_notifier(notifier);
    /// ```
    pub fn with_notifier(mut self, notifier: Arc<dyn BackupFailureNotifier>) -> Self {
        self.notifier = Some(notifier);
        self
    }

    /// Register an engine implementation with the runner.
    ///
    /// The engine's `engine()` key must be unique. Duplicate keys silently
    /// overwrite the previous registration — callers should ensure each key is
    /// registered only once during plugin startup.
    pub fn register_engine(&mut self, engine: Arc<dyn BackupEngine>) {
        self.engines.insert(engine.engine(), engine);
    }

    /// Insert a new `backup_jobs` row and return its id.
    ///
    /// This is the primary entry point for handlers and the scheduler in
    /// Phase 1+. The row starts in `state='pending'` with `next_attempt_at=NOW()`,
    /// so the runner will claim it on its next poll.
    ///
    /// This function does NOT start execution — it only enqueues. The runner
    /// picks up the job asynchronously.
    ///
    /// ## Concurrency guard
    ///
    /// Before inserting, this method checks for an existing `backup_jobs` row
    /// with the same `(engine, target_kind, target_id)` whose `state` is
    /// `'pending'` or `'running'`. If such a row exists,
    /// `Err(BackupRunnerError::AlreadyInFlight)` is returned without inserting.
    ///
    /// This prevents two concurrent `wal-g backup-push` processes from fighting
    /// over `pg_backup_start` on the same Postgres cluster, which caused the
    /// three-concurrent-job deadlock in production (May 2026 incident).
    pub async fn enqueue_job(
        &self,
        db: &DatabaseConnection,
        params: EnqueueJobParams,
    ) -> Result<i64, BackupRunnerError> {
        use sea_orm::{DatabaseBackend, FromQueryResult, Statement, Value as SValue};

        #[derive(FromQueryResult)]
        struct InsertedId {
            id: i64,
        }

        #[derive(FromQueryResult)]
        struct ExistingId {
            id: i64,
        }

        // ── Concurrency guard ────────────────────────────────────────────────
        // Refuse to enqueue if there is already a pending or running job for
        // the same (engine, target_kind, target_id). For control-plane jobs,
        // target_id IS NULL — the IS NULL branch ensures those are also guarded.
        let guard_sql = r#"
SELECT id FROM backup_jobs
WHERE engine      = $1
  AND target_kind = $2
  AND (
        (target_id = $3 AND $3 IS NOT NULL)
     OR ($3 IS NULL AND target_id IS NULL)
  )
  AND state IN ('pending', 'running')
LIMIT 1
        "#;

        let existing = ExistingId::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            guard_sql,
            vec![
                SValue::from(params.engine.clone()),
                SValue::from(params.target_kind.clone()),
                SValue::from(params.target_id),
            ],
        ))
        .one(db)
        .await
        .map_err(|e| BackupRunnerError::Database {
            operation: "enqueue_job:concurrency_guard",
            source: e,
        })?;

        if let Some(row) = existing {
            return Err(BackupRunnerError::AlreadyInFlight {
                engine: params.engine,
                target_id: params.target_id,
                existing_job_id: row.id,
            });
        }

        // ── Insert ───────────────────────────────────────────────────────────
        // `params` column is JSONB. Binding via `SValue::Json(...)` so the
        // driver sends a JSON value, not a text-encoded string. Using
        // `SValue::from(String)` here produces a `text` arg and Postgres
        // rejects the INSERT with: `column "params" is of type jsonb but
        // expression is of type text`.
        let params_value = SValue::Json(Some(Box::new(params.params.clone())));

        let max_attempts = params.max_attempts.unwrap_or(3);

        // Resolve the wall-clock timeout using the three-tier precedence chain.
        // The resolved value is written into the row so the runner reads it
        // directly at dispatch time (no re-computation needed).
        let max_runtime_secs = crate::timeouts::resolve_max_runtime(
            params.max_runtime_secs,
            None, // schedule-level override is resolved by the caller before calling enqueue_job
            &params.engine,
        );

        let sql = r#"
INSERT INTO backup_jobs
    (backup_id, engine, target_kind, target_id, params, max_attempts, max_runtime_secs, next_attempt_at)
VALUES
    ($1, $2, $3, $4, $5, $6, $7, NOW())
RETURNING id
        "#;

        let row = InsertedId::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            sql,
            vec![
                SValue::from(params.backup_id),
                SValue::from(params.engine.clone()),
                SValue::from(params.target_kind),
                SValue::from(params.target_id),
                params_value,
                SValue::from(max_attempts),
                SValue::from(max_runtime_secs),
            ],
        ))
        .one(db)
        .await
        .map_err(|e| BackupRunnerError::Database {
            operation: "enqueue_job",
            source: e,
        })?
        .ok_or_else(|| BackupRunnerError::EnqueueFailed {
            backup_id: params.backup_id,
            engine: params.engine,
        })?;

        Ok(row.id)
    }

    /// Insert a `backup_jobs` row inside a caller-owned transaction.
    ///
    /// This is the transactional variant of [`enqueue_job`]. Use it when the
    /// caller is already inside a `db.begin()` transaction and needs the job
    /// insertion to be part of the same atomic unit (i.e., the parent `backups`
    /// row and the `backup_jobs` row must both commit or both roll back).
    ///
    /// The concurrency guard check runs within the same transaction, so it sees
    /// the full serialized state at transaction isolation level. If a concurrent
    /// in-flight job exists, the transaction is left open for the caller to roll
    /// back (this method returns `Err`; the caller drives the rollback).
    ///
    /// Unlike [`enqueue_job`], this method does **not** commit; the caller
    /// must call `txn.commit()` after all work succeeds.
    pub async fn enqueue_job_in_txn(
        &self,
        txn: &sea_orm::DatabaseTransaction,
        params: EnqueueJobParams,
    ) -> Result<i64, BackupRunnerError> {
        use sea_orm::{DatabaseBackend, FromQueryResult, Statement, Value as SValue};

        #[derive(FromQueryResult)]
        struct InsertedId {
            id: i64,
        }

        #[derive(FromQueryResult)]
        struct ExistingId {
            id: i64,
        }

        // ── Concurrency guard (within the caller's transaction) ───────────────
        let guard_sql = r#"
SELECT id FROM backup_jobs
WHERE engine      = $1
  AND target_kind = $2
  AND (
        (target_id = $3 AND $3 IS NOT NULL)
     OR ($3 IS NULL AND target_id IS NULL)
  )
  AND state IN ('pending', 'running')
LIMIT 1
        "#;

        let existing = ExistingId::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            guard_sql,
            vec![
                SValue::from(params.engine.clone()),
                SValue::from(params.target_kind.clone()),
                SValue::from(params.target_id),
            ],
        ))
        .one(txn)
        .await
        .map_err(|e| BackupRunnerError::Database {
            operation: "enqueue_job_in_txn:concurrency_guard",
            source: e,
        })?;

        if let Some(row) = existing {
            return Err(BackupRunnerError::AlreadyInFlight {
                engine: params.engine,
                target_id: params.target_id,
                existing_job_id: row.id,
            });
        }

        // ── Insert (JSONB params via SValue::Json) ───────────────────────────
        let params_value = SValue::Json(Some(Box::new(params.params.clone())));
        let max_attempts = params.max_attempts.unwrap_or(3);

        // Resolve the wall-clock timeout. The caller has already folded the
        // schedule-level override into `params.max_runtime_secs` if applicable
        // (see `enqueue_scheduled_backup` in temps-backup). Here we apply the
        // engine-default fallback for any remaining None.
        let max_runtime_secs = crate::timeouts::resolve_max_runtime(
            params.max_runtime_secs,
            None, // schedule-level override pre-resolved by caller
            &params.engine,
        );

        let sql = r#"
INSERT INTO backup_jobs
    (backup_id, engine, target_kind, target_id, params, max_attempts, max_runtime_secs, next_attempt_at)
VALUES
    ($1, $2, $3, $4, $5, $6, $7, NOW())
RETURNING id
        "#;

        let row = InsertedId::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            sql,
            vec![
                SValue::from(params.backup_id),
                SValue::from(params.engine.clone()),
                SValue::from(params.target_kind),
                SValue::from(params.target_id),
                params_value,
                SValue::from(max_attempts),
                SValue::from(max_runtime_secs),
            ],
        ))
        .one(txn)
        .await
        .map_err(|e| BackupRunnerError::Database {
            operation: "enqueue_job_in_txn",
            source: e,
        })?
        .ok_or_else(|| BackupRunnerError::EnqueueFailed {
            backup_id: params.backup_id,
            engine: params.engine,
        })?;

        Ok(row.id)
    }

    /// Fire the failure notifier for a terminal job failure, if one is configured.
    ///
    /// Spawns a detached `tokio::spawn` so notification I/O (SMTP, webhook)
    /// never delays the queue write that already succeeded.  If no notifier is
    /// set this is a no-op.
    fn fire_failure_notification(self: &Arc<Self>, ctx: BackupFailureContext) {
        if let Some(notifier) = &self.notifier {
            let n = Arc::clone(notifier);
            tokio::spawn(async move {
                n.notify_failed(ctx).await;
            });
        }
    }

    /// Start the poll loop.
    ///
    /// Runs forever until the `cancel` token is fired. Designed to be called
    /// as `tokio::spawn(Arc::clone(&runner).run_forever(cancel))`.
    ///
    /// The loop:
    /// 1. Claims one job.
    /// 2. Spawns a task to dispatch it to the registered engine.
    /// 3. Sleeps for `config.poll_interval` if the queue was empty.
    ///
    /// In Phase 0 no engines are registered, so step 2 never fires and the
    /// loop logs nothing alarming — it simply finds no claimable jobs.
    pub async fn run_forever(self: Arc<Self>, cancel: CancellationToken) {
        info!(
            instance_id = %self.config.instance_id,
            poll_interval_secs = self.config.poll_interval.as_secs(),
            max_concurrent = self.config.max_concurrent,
            registered_engines = self.engines.len(),
            "BackupRunner started",
        );

        let mut interval = tokio::time::interval(self.config.poll_interval);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    info!(
                        instance_id = %self.config.instance_id,
                        "BackupRunner received cancellation, shutting down",
                    );
                    return;
                }
                _ = interval.tick() => {
                    let runner = Arc::clone(&self);
                    if let Err(e) = runner.poll_once().await {
                        error!(
                            instance_id = %self.config.instance_id,
                            error = %e,
                            "BackupRunner poll_once failed; will retry next tick",
                        );
                    }
                }
            }
        }
    }

    /// Execute a single poll-and-dispatch cycle.
    ///
    /// Extracted so tests can drive the loop directly without needing timers.
    /// `pub` so integration tests in `tests/` can call it without spawning
    /// a full `run_forever` loop.
    pub async fn poll_once(self: Arc<Self>) -> Result<(), BackupRunnerError> {
        let lease_ttl = self.config.lease_ttl.as_secs() as i64;
        let row = claim_one_job(self.db.as_ref(), &self.config.instance_id, lease_ttl).await?;

        let row = match row {
            Some(r) => r,
            None => {
                debug!(instance_id = %self.config.instance_id, "BackupRunner: queue empty");
                return Ok(());
            }
        };

        info!(
            job_id = row.id,
            engine = %row.engine,
            attempt = row.attempts,
            instance_id = %self.config.instance_id,
            "BackupRunner claimed job",
        );

        // Look up the engine. If not registered (Phase 0: empty registry), fail
        // the job immediately so it doesn't spin.
        let engine = match self.engines.get(row.engine.as_str()) {
            Some(e) => Arc::clone(e),
            None => {
                let registered = self.engines.keys().copied().collect::<Vec<_>>().join(", ");
                error!(
                    job_id = row.id,
                    engine = %row.engine,
                    registered_engines = %registered,
                    "No engine registered for claimed job; failing immediately",
                );
                // Fail the job so it does not retry forever.
                if let Some(token) = row.claim_token {
                    let err_msg = format!(
                        "No engine registered for key '{}'. Registered: [{}]",
                        row.engine, registered
                    );
                    let _ =
                        mark_job_failed(self.db.as_ref(), row.id, token, row.backup_id, &err_msg)
                            .await;
                    self.fire_failure_notification(BackupFailureContext {
                        job_id: row.id,
                        backup_id: row.backup_id,
                        engine: row.engine.clone(),
                        attempts: row.attempts,
                        max_attempts: row.max_attempts,
                        error_message: err_msg,
                        failed_at: Utc::now(),
                    });
                }
                return Ok(());
            }
        };

        let runner = Arc::clone(&self);
        tokio::spawn(async move {
            runner.dispatch(row, engine).await;
        });

        Ok(())
    }

    /// Dispatch a claimed job to its engine and advance the job through the
    /// runner loop (ADR-014 §"Runner loop" pseudocode).
    ///
    /// A per-job wall-clock timeout is read from `row.max_runtime_secs` (baked
    /// in at enqueue time via `crate::timeouts::resolve_max_runtime`). Jobs
    /// that exceed this ceiling are immediately marked failed with a descriptive
    /// timeout message and their `CancellationToken` is fired so cooperative
    /// engines can abort.
    ///
    /// A floor of 60 seconds is applied so a zero or corrupt DB value never
    /// instantly fails every job.
    async fn dispatch(self: Arc<Self>, row: BackupJobRow, engine: Arc<dyn BackupEngine>) {
        let job_id = row.id;
        let backup_id = row.backup_id;
        let attempt = row.attempts;
        let lease_ttl = self.config.lease_ttl.as_secs() as i64;

        let claim_token = match row.claim_token {
            Some(t) => t,
            None => {
                error!(job_id, "Claimed job has no claim_token — this is a bug");
                return;
            }
        };

        let cursor = StepCursor {
            current_step: row.step.clone(),
            durable_state: row.step_state.clone(),
        };

        let job_cancel = CancellationToken::new();
        let ctx = BackupContext {
            job_id,
            attempt,
            params: row.params.clone(),
            db: Arc::clone(&self.db),
            cancel: job_cancel.clone(),
        };

        let mut stream = engine.execute(&ctx, cursor.clone());

        // Per-job wall-clock deadline (ADR-014 hardening fix #3).
        // Read the resolved timeout from the row (baked in at enqueue time).
        // Apply a 60-second floor so a corrupt or zero value never instantly
        // fails the job. Pin the sleep future so we can poll it inside the
        // select loop without recreating it on every iteration.
        let max_runtime = std::time::Duration::from_secs(row.max_runtime_secs.max(60) as u64);
        let work_deadline = tokio::time::sleep(max_runtime);
        tokio::pin!(work_deadline);

        loop {
            let event = tokio::select! {
                biased;

                // Check the wall-clock deadline first. If it fires, fail the job
                // immediately and cancel the engine's CancellationToken so
                // cooperative steps abort at their next checkpoint.
                () = &mut work_deadline => {
                    let timeout_secs = max_runtime.as_secs();
                    let hours = timeout_secs / 3600;
                    let minutes = (timeout_secs % 3600) / 60;
                    let human = if hours > 0 {
                        format!("{}h {}m", hours, minutes)
                    } else {
                        format!("{}m", minutes)
                    };
                    let msg = format!(
                        "Job exceeded wall-clock timeout of {} seconds ({}); \
                         automatically failed to prevent indefinite execution.",
                        timeout_secs,
                        human,
                    );
                    error!(job_id, attempt, timeout_secs, %msg, "BackupRunner: job timeout");
                    job_cancel.cancel();
                    let _ = mark_job_failed(
                        self.db.as_ref(),
                        job_id,
                        claim_token,
                        backup_id,
                        &msg,
                    )
                    .await;
                    self.fire_failure_notification(BackupFailureContext {
                        job_id,
                        backup_id,
                        engine: row.engine.clone(),
                        attempts: attempt,
                        max_attempts: row.max_attempts,
                        error_message: msg,
                        failed_at: Utc::now(),
                    });
                    return;
                }

                event = stream.next() => event,
            };

            match event {
                None => {
                    // Stream ended without a `Done` event — treat as failure.
                    warn!(job_id, attempt, "Engine stream ended without Done event");
                    if attempt >= row.max_attempts {
                        let _ = engine.rollback(&ctx, cursor.clone()).await;
                        let _ = mark_job_failed(
                            self.db.as_ref(),
                            job_id,
                            claim_token,
                            backup_id,
                            "Engine stream ended without Done event",
                        )
                        .await;
                        self.fire_failure_notification(BackupFailureContext {
                            job_id,
                            backup_id,
                            engine: row.engine.clone(),
                            attempts: attempt,
                            max_attempts: row.max_attempts,
                            error_message: "Engine stream ended without Done event".to_string(),
                            failed_at: Utc::now(),
                        });
                    } else {
                        let delay = backoff_delay(attempt);
                        let next_at = Utc::now() + delay;
                        let _ = schedule_retry(
                            self.db.as_ref(),
                            job_id,
                            claim_token,
                            next_at,
                            backup_id,
                            "Engine stream ended without Done event",
                        )
                        .await;
                    }
                    return;
                }

                Some(Err(engine_err)) => {
                    error!(
                        job_id,
                        attempt,
                        error = %engine_err,
                        "Engine returned error",
                    );

                    // Permanent failures bypass retry — surface immediately to
                    // the UI. All current `BackupEngineError` variants represent
                    // permanent failures:
                    //
                    // - `Preflight`: user config is broken (S3 unreachable,
                    //   bucket missing, container not found). Retry won't fix.
                    // - `StepFailed`: engine step blew up mid-execution (e.g.
                    //   sidecar container died, exec returned non-zero, dump
                    //   file invalid). These are *not* transient SDK errors —
                    //   they're deterministic failures that will repeat.
                    // - `Unsupported`: engine key was wrong. Retry won't fix.
                    // - `Io` / `S3`: lower-level failures bubble through the
                    //   engine; we treat them as permanent too since retrying
                    //   with the same config will produce the same error.
                    //
                    // The runner's job is to track state and orchestrate, not
                    // to second-guess engine errors. If real transient retry
                    // is needed for SDK blips, the engine should catch and
                    // retry internally rather than bubble up a
                    // `BackupEngineError`.
                    //
                    // Prior behavior treated `StepFailed` as transient, which
                    // produced confusing "Pending" rows with a failure banner
                    // because schedule_retry kept `state='pending'` for 31
                    // minutes before giving up (verified in prod for s3_mirror
                    // job 16 on 2026-05-14).
                    let is_permanent = matches!(
                        engine_err,
                        BackupEngineError::Preflight { .. }
                            | BackupEngineError::Unsupported { .. }
                            | BackupEngineError::StepFailed { .. }
                            | BackupEngineError::Io(_)
                            | BackupEngineError::S3 { .. }
                    );

                    if is_permanent || attempt >= row.max_attempts {
                        let err_msg = engine_err.to_string();
                        let _ = engine.rollback(&ctx, cursor.clone()).await;
                        let _ = mark_job_failed(
                            self.db.as_ref(),
                            job_id,
                            claim_token,
                            backup_id,
                            &err_msg,
                        )
                        .await;
                        self.fire_failure_notification(BackupFailureContext {
                            job_id,
                            backup_id,
                            engine: row.engine.clone(),
                            attempts: attempt,
                            max_attempts: row.max_attempts,
                            error_message: err_msg,
                            failed_at: Utc::now(),
                        });
                    } else {
                        let delay = backoff_delay(attempt);
                        let next_at = Utc::now() + delay;
                        let _ = schedule_retry(
                            self.db.as_ref(),
                            job_id,
                            claim_token,
                            next_at,
                            backup_id,
                            &engine_err.to_string(),
                        )
                        .await;
                    }
                    return;
                }

                Some(Ok(StepEvent::Heartbeat)) => {
                    debug!(job_id, attempt, "Engine heartbeat — extending lease");
                    if let Err(e) =
                        extend_lease(self.db.as_ref(), job_id, claim_token, lease_ttl).await
                    {
                        error!(job_id, error = %e, "Failed to extend lease on heartbeat; aborting");
                        job_cancel.cancel();
                        return;
                    }
                }

                Some(Ok(StepEvent::StepCompleted {
                    step,
                    durable_state,
                    message,
                })) => {
                    debug!(job_id, attempt, step = %step, "Engine completed step");
                    if let Err(e) = persist_step_completed(
                        self.db.as_ref(),
                        job_id,
                        claim_token,
                        attempt,
                        &step,
                        durable_state,
                        message.as_deref(),
                    )
                    .await
                    {
                        error!(
                            job_id,
                            step = %step,
                            error = %e,
                            "Failed to persist step; aborting (step will be re-run on next attempt)",
                        );
                        job_cancel.cancel();
                        return;
                    }
                }

                Some(Ok(StepEvent::Done {
                    location,
                    size_bytes,
                    compression,
                })) => {
                    info!(
                        job_id,
                        backup_id,
                        location = %location,
                        "Engine done — marking job completed",
                    );
                    if let Err(e) = mark_job_completed(
                        self.db.as_ref(),
                        job_id,
                        claim_token,
                        backup_id,
                        &location,
                        size_bytes,
                        &compression,
                    )
                    .await
                    {
                        error!(job_id, error = %e, "Failed to mark job completed");
                    }
                    return;
                }
            }
        }
    }
}

use chrono::Utc;

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::{DatabaseBackend, MockDatabase};
    use std::collections::BTreeMap;
    use tokio_util::sync::CancellationToken;

    // ── run_forever cancels cleanly ───────────────────────────────────────────

    #[tokio::test]
    async fn test_run_forever_cancels_cleanly() {
        use sea_orm::Value as SVal;

        // Empty BTreeMap rows simulate no claimable jobs.
        let empty: Vec<BTreeMap<String, SVal>> = vec![];
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![empty.clone()])
            .append_query_results(vec![empty.clone()])
            .append_query_results(vec![empty.clone()])
            .append_query_results(vec![empty])
            .into_connection();

        let config = RunnerConfig {
            poll_interval: std::time::Duration::from_millis(10),
            ..Default::default()
        };
        let runner = Arc::new(BackupRunner::new(Arc::new(db), config));
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();

        let handle = tokio::spawn(runner.run_forever(cancel.clone()));

        // Fire cancellation after a short delay.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        cancel_clone.cancel();

        // Should complete without panicking or hanging.
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;

        assert!(
            result.is_ok(),
            "run_forever should complete after cancellation"
        );
    }

    // ── enqueue_job ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_enqueue_job_returns_id() {
        use sea_orm::Value as SVal;

        // The concurrency guard SELECT runs first (returns empty → no in-flight job),
        // then the INSERT RETURNING runs and returns id=99.
        let empty: Vec<BTreeMap<String, SVal>> = vec![];

        let mut insert_row: BTreeMap<String, SVal> = BTreeMap::new();
        insert_row.insert("id".to_string(), SVal::BigInt(Some(99)));

        let db = Arc::new(
            MockDatabase::new(DatabaseBackend::Postgres)
                // query 1: guard SELECT — empty means no in-flight job
                .append_query_results(vec![empty])
                // query 2: INSERT RETURNING id
                .append_query_results(vec![vec![insert_row]])
                .into_connection(),
        );

        let config = RunnerConfig::default();
        let runner = BackupRunner::new(Arc::clone(&db), config);

        let params = EnqueueJobParams {
            backup_id: 7,
            engine: "redis".to_string(),
            target_kind: "external_service".to_string(),
            target_id: Some(3),
            params: serde_json::json!({}),
            max_attempts: None,
            max_runtime_secs: None,
        };

        let result = runner.enqueue_job(db.as_ref(), params).await;

        assert!(result.is_ok(), "enqueue_job should succeed: {:?}", result);
        assert_eq!(result.unwrap(), 99);
    }

    /// `enqueue_job` must resolve and write the per-job `max_runtime_secs`
    /// onto the row at INSERT time. Without this, the runner's
    /// `dispatch()` would read whatever default the DB column carries
    /// instead of honoring caller overrides.
    ///
    /// We assert by inspecting the transaction log: the second statement
    /// must be the INSERT, and its parameter list must contain `7200_i64`
    /// (the explicit caller override) — not the postgres_walg default
    /// `86_400` and not `0`.
    #[tokio::test]
    async fn test_enqueue_job_writes_resolved_max_runtime() {
        use sea_orm::Value as SVal;

        let empty: Vec<BTreeMap<String, SVal>> = vec![];
        let mut insert_row: BTreeMap<String, SVal> = BTreeMap::new();
        insert_row.insert("id".to_string(), SVal::BigInt(Some(101)));

        let db = Arc::new(
            MockDatabase::new(DatabaseBackend::Postgres)
                .append_query_results(vec![empty])
                .append_query_results(vec![vec![insert_row]])
                .into_connection(),
        );

        let runner = BackupRunner::new(Arc::clone(&db), RunnerConfig::default());

        let params = EnqueueJobParams {
            backup_id: 1,
            engine: "postgres_walg".to_string(),
            target_kind: "external_service".to_string(),
            target_id: Some(1),
            params: serde_json::json!({}),
            max_attempts: None,
            // Explicit override — must end up in the row regardless of engine default.
            max_runtime_secs: Some(7200),
        };

        let result = runner.enqueue_job(db.as_ref(), params).await;
        assert!(result.is_ok(), "enqueue must succeed: {:?}", result);

        // Inspect the transaction log to confirm the INSERT carried 7200.
        drop(runner);
        let inner = Arc::try_unwrap(db).expect("exclusive ownership");
        let log = inner.into_transaction_log();
        let insert_stmt = log
            .iter()
            .flat_map(|txn| txn.statements())
            .find(|s| s.sql.trim_start().to_uppercase().starts_with("INSERT"))
            .expect("INSERT statement must appear in the transaction log");
        let has_7200 = insert_stmt
            .values
            .as_ref()
            .map(|v| {
                v.0.iter()
                    .any(|val| matches!(val, sea_orm::Value::BigInt(Some(n)) if *n == 7200))
            })
            .unwrap_or(false);
        assert!(
            has_7200,
            "INSERT must bind max_runtime_secs=7200, got values: {:?}",
            insert_stmt.values
        );
    }

    /// When the caller doesn't override `max_runtime_secs`, `enqueue_job`
    /// must fall back to the engine-family default from
    /// `timeouts::default_max_runtime_secs`. For `redis` that's 4 hours
    /// (`14_400` seconds).
    #[tokio::test]
    async fn test_enqueue_job_uses_engine_default_when_no_override() {
        use sea_orm::Value as SVal;

        let empty: Vec<BTreeMap<String, SVal>> = vec![];
        let mut insert_row: BTreeMap<String, SVal> = BTreeMap::new();
        insert_row.insert("id".to_string(), SVal::BigInt(Some(102)));

        let db = Arc::new(
            MockDatabase::new(DatabaseBackend::Postgres)
                .append_query_results(vec![empty])
                .append_query_results(vec![vec![insert_row]])
                .into_connection(),
        );

        let runner = BackupRunner::new(Arc::clone(&db), RunnerConfig::default());

        let params = EnqueueJobParams {
            backup_id: 2,
            engine: "redis".to_string(),
            target_kind: "external_service".to_string(),
            target_id: Some(2),
            params: serde_json::json!({}),
            max_attempts: None,
            max_runtime_secs: None,
        };

        runner
            .enqueue_job(db.as_ref(), params)
            .await
            .expect("enqueue must succeed");

        drop(runner);
        let inner = Arc::try_unwrap(db).expect("exclusive ownership");
        let log = inner.into_transaction_log();
        let insert_stmt = log
            .iter()
            .flat_map(|txn| txn.statements())
            .find(|s| s.sql.trim_start().to_uppercase().starts_with("INSERT"))
            .expect("INSERT must appear");

        let expected = crate::timeouts::default_max_runtime_secs("redis");
        let has_default = insert_stmt
            .values
            .as_ref()
            .map(|v| {
                v.0.iter()
                    .any(|val| matches!(val, sea_orm::Value::BigInt(Some(n)) if *n == expected))
            })
            .unwrap_or(false);
        assert!(
            has_default,
            "INSERT must bind max_runtime_secs={} (redis default), got values: {:?}",
            expected, insert_stmt.values
        );
    }

    #[tokio::test]
    async fn test_enqueue_job_already_in_flight_returns_error() {
        use sea_orm::Value as SVal;

        // Guard SELECT returns an existing row — simulates an in-flight job.
        let mut existing_row: BTreeMap<String, SVal> = BTreeMap::new();
        existing_row.insert("id".to_string(), SVal::BigInt(Some(42)));

        let db = Arc::new(
            MockDatabase::new(DatabaseBackend::Postgres)
                // Guard SELECT finds an existing job → AlreadyInFlight
                .append_query_results(vec![vec![existing_row]])
                .into_connection(),
        );

        let config = RunnerConfig::default();
        let runner = BackupRunner::new(Arc::clone(&db), config);

        let params = EnqueueJobParams {
            backup_id: 99,
            engine: "redis".to_string(),
            target_kind: "external_service".to_string(),
            target_id: Some(5),
            params: serde_json::json!({}),
            max_attempts: None,
            max_runtime_secs: None,
        };

        let result = runner.enqueue_job(db.as_ref(), params).await;

        assert!(result.is_err(), "should reject duplicate enqueue");
        assert!(
            matches!(
                result.unwrap_err(),
                BackupRunnerError::AlreadyInFlight {
                    existing_job_id: 42,
                    ..
                }
            ),
            "expected AlreadyInFlight with existing_job_id=42"
        );
    }

    // ── Preflight error → immediate failure, no retry ─────────────────────────

    /// Bug 1 regression: verify that a `BackupEngineError::Preflight` error
    /// bypasses the retry path and goes straight to `mark_job_failed`, even
    /// when `attempts < max_attempts`.
    ///
    /// Before the fix, any engine error was retried up to `max_attempts` times.
    /// A Preflight failure (S3 unreachable, bucket missing) would silently wait
    /// 1+5+25=31 minutes before surfacing "Failed" to the user.
    ///
    /// The assertion: the MockDatabase should receive exactly two exec statements
    /// (the two UPDATEEs inside `mark_job_failed`'s transaction). If `schedule_retry`
    /// were called instead it would also produce two exec statements but with
    /// different SQL — we verify correctness by counting that the transaction log
    /// contains an UPDATE for `backup_jobs` setting `state='failed'`.
    #[tokio::test]
    async fn test_preflight_error_fails_immediately_without_retry() {
        use futures::stream;
        use sea_orm::MockExecResult;

        struct PreflightFailEngine;

        #[async_trait::async_trait]
        impl BackupEngine for PreflightFailEngine {
            fn engine(&self) -> &'static str {
                "test_preflight"
            }
            fn steps(&self) -> &'static [&'static str] {
                &["preflight"]
            }
            fn execute<'a>(
                &'a self,
                ctx: &'a BackupContext,
                _cursor: StepCursor,
            ) -> futures::stream::BoxStream<'a, Result<StepEvent, crate::engine::BackupEngineError>>
            {
                let job_id = ctx.job_id;
                Box::pin(stream::once(async move {
                    Err(crate::engine::BackupEngineError::Preflight {
                        job_id,
                        reason: "bucket not reachable: connection refused".to_string(),
                    })
                }))
            }
        }

        // mark_job_failed runs inside a transaction: BEGIN + UPDATE backup_jobs
        // + UPDATE backups + COMMIT.  The MockDatabase exec results are consumed
        // in the order issued; we supply two rows_affected=1 results for the two
        // UPDATEEs.
        let db = Arc::new(
            MockDatabase::new(DatabaseBackend::Postgres)
                .append_exec_results(vec![
                    MockExecResult {
                        last_insert_id: 0,
                        rows_affected: 1,
                    }, // UPDATE backup_jobs
                    MockExecResult {
                        last_insert_id: 0,
                        rows_affected: 1,
                    }, // UPDATE backups
                ])
                .into_connection(),
        );

        let config = crate::config::RunnerConfig::default();
        let runner = Arc::new(BackupRunner::new(Arc::clone(&db), config));

        // Construct a job row with attempt=1, max_attempts=3 to ensure the fix
        // fires even though attempts < max_attempts.
        let row = BackupJobRow {
            id: 11,
            backup_id: 5,
            engine: "test_preflight".to_string(),
            target_kind: "external_service".to_string(),
            target_id: Some(3),
            params: serde_json::Value::Object(Default::default()),
            state: "running".to_string(),
            step: None,
            step_state: serde_json::Value::Object(Default::default()),
            attempts: 1, // not at max — proves the permanent-failure path
            max_attempts: 3,
            claim_token: Some(uuid::Uuid::new_v4()),
            max_runtime_secs: 86_400, // 24 h — will not fire in the test
        };

        let engine: Arc<dyn BackupEngine> = Arc::new(PreflightFailEngine);

        // dispatch runs to completion synchronously in this test (no spawn needed).
        // Clone the Arc so the original `runner` reference can be dropped before
        // we call `Arc::try_unwrap` on `db`.
        Arc::clone(&runner).dispatch(row, engine).await;

        // `into_transaction_log` takes ownership of the DatabaseConnection, so we
        // must extract it from the Arc. Drop all other Arc clones first.
        drop(runner);
        let inner_db =
            Arc::try_unwrap(db).expect("should have exclusive ownership after dropping runner");

        // If schedule_retry had been called it would issue its own two exec
        // statements. Verify the transaction log contains exactly the two UPDATE
        // statements from mark_job_failed.
        let log = inner_db.into_transaction_log();
        let exec_count: usize = log
            .iter()
            .flat_map(|txn| txn.statements())
            .filter(|stmt| stmt.sql.trim().to_uppercase().starts_with("UPDATE"))
            .count();

        // mark_job_failed produces exactly 2 UPDATEEs (jobs + backups).
        // We assert count == 2 to confirm the failure path fired.
        assert_eq!(
            exec_count, 2,
            "Preflight error must call mark_job_failed (2 UPDATEEs), got {} UPDATE statements",
            exec_count
        );
    }

    // ── BackupFailureNotifier is called on terminal failure ───────────────────

    /// Verify that a `BackupFailureNotifier` registered via `with_notifier` is
    /// invoked when `dispatch` reaches a terminal failure path.
    ///
    /// A `MockNotifier` captures the `BackupFailureContext` into a shared
    /// `Arc<Mutex<Option<BackupFailureContext>>>`. After `dispatch` completes
    /// we assert the captured context has the expected `engine` and
    /// `error_message` values.
    #[tokio::test]
    async fn test_notifier_called_on_terminal_failure() {
        use crate::notifier::{BackupFailureContext, BackupFailureNotifier};
        use futures::stream;
        use sea_orm::MockExecResult;
        use std::sync::Mutex;

        #[derive(Clone)]
        struct MockNotifier {
            captured: Arc<Mutex<Option<BackupFailureContext>>>,
        }

        #[async_trait::async_trait]
        impl BackupFailureNotifier for MockNotifier {
            async fn notify_failed(&self, ctx: BackupFailureContext) {
                let mut guard = self.captured.lock().unwrap();
                *guard = Some(ctx);
            }
        }

        struct PreflightFailEngine2;

        #[async_trait::async_trait]
        impl BackupEngine for PreflightFailEngine2 {
            fn engine(&self) -> &'static str {
                "test_notifier_engine"
            }
            fn steps(&self) -> &'static [&'static str] {
                &["preflight"]
            }
            fn execute<'a>(
                &'a self,
                ctx: &'a BackupContext,
                _cursor: StepCursor,
            ) -> futures::stream::BoxStream<'a, Result<StepEvent, crate::engine::BackupEngineError>>
            {
                let job_id = ctx.job_id;
                Box::pin(stream::once(async move {
                    Err(crate::engine::BackupEngineError::Preflight {
                        job_id,
                        reason: "s3 bucket unreachable".to_string(),
                    })
                }))
            }
        }

        let captured: Arc<Mutex<Option<BackupFailureContext>>> = Arc::new(Mutex::new(None));
        let notifier = MockNotifier {
            captured: Arc::clone(&captured),
        };

        let db = Arc::new(
            MockDatabase::new(DatabaseBackend::Postgres)
                .append_exec_results(vec![
                    MockExecResult {
                        last_insert_id: 0,
                        rows_affected: 1,
                    },
                    MockExecResult {
                        last_insert_id: 0,
                        rows_affected: 1,
                    },
                ])
                .into_connection(),
        );

        let config = crate::config::RunnerConfig::default();
        let runner =
            Arc::new(BackupRunner::new(Arc::clone(&db), config).with_notifier(Arc::new(notifier)));

        let row = BackupJobRow {
            id: 55,
            backup_id: 12,
            engine: "test_notifier_engine".to_string(),
            target_kind: "external_service".to_string(),
            target_id: Some(1),
            params: serde_json::Value::Object(Default::default()),
            state: "running".to_string(),
            step: None,
            step_state: serde_json::Value::Object(Default::default()),
            attempts: 1,
            max_attempts: 1, // at max_attempts so terminal failure fires
            claim_token: Some(uuid::Uuid::new_v4()),
            max_runtime_secs: 86_400,
        };

        let engine: Arc<dyn BackupEngine> = Arc::new(PreflightFailEngine2);
        Arc::clone(&runner).dispatch(row, engine).await;

        // Give the spawned notifier task a moment to execute.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let ctx_opt = captured.lock().unwrap().take();
        assert!(
            ctx_opt.is_some(),
            "MockNotifier must be called when the job reaches terminal failure",
        );
        let ctx = ctx_opt.unwrap();
        assert_eq!(ctx.job_id, 55, "job_id must match");
        assert_eq!(ctx.backup_id, 12, "backup_id must match");
        assert_eq!(ctx.engine, "test_notifier_engine", "engine must match");
        assert!(
            ctx.error_message.contains("s3 bucket unreachable"),
            "error_message must contain the engine error: got '{}'",
            ctx.error_message,
        );
    }
}
