//! SQL queue primitives for the backup execution queue (ADR-014).
//!
//! All functions operate at the SQL level — they do not touch business logic.
//! The `BackupRunner` (in `runner.rs`) calls these functions in the correct
//! order; nothing else should call them directly.
//!
//! The claim pattern mirrors `ch_fanout.rs:214–232` which uses
//! `FOR UPDATE SKIP LOCKED` to prevent double-claiming across runners.

use chrono::{Duration, Utc};
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DatabaseTransaction, FromQueryResult,
    Statement, TransactionTrait, Value as SValue,
};
use serde_json::Value;
use uuid::Uuid;

use crate::error::BackupRunnerError;

// ── Claim ─────────────────────────────────────────────────────────────────────

/// A minimal projection of `backup_jobs` returned by the claim query.
///
/// Only contains the fields the runner needs to dispatch to an engine. The full
/// entity (`temps_entities::backup_jobs::Model`) is heavier than necessary for
/// the hot path.
#[derive(Debug, Clone, FromQueryResult)]
pub struct BackupJobRow {
    pub id: i64,
    pub backup_id: i32,
    pub engine: String,
    pub target_kind: String,
    pub target_id: Option<i32>,
    pub params: serde_json::Value,
    pub state: String,
    pub step: Option<String>,
    pub step_state: serde_json::Value,
    pub attempts: i32,
    pub max_attempts: i32,
    pub claim_token: Option<uuid::Uuid>,
    /// Wall-clock timeout baked in at enqueue time (seconds).
    ///
    /// The runner reads this directly at dispatch time. A floor of 60 seconds
    /// is applied in `dispatch` so a corrupt or zero value never instantly fails
    /// the job.
    pub max_runtime_secs: i64,
}

/// Claim one job from the queue (ADR-014 §"Claim query").
///
/// Issues an atomic `UPDATE … RETURNING *` that:
/// 1. Finds the oldest pending job whose `next_attempt_at <= NOW()`.
/// 2. Also reclaims expired leases: `state='running' AND leased_until < NOW()`
///    (ADR-014 §"Lease duration" — replaces the boot-time reconcile sweep).
/// 3. Rotates `claim_token` to a fresh UUID so a stale runner cannot overwrite
///    a new owner's progress.
///
/// Returns `Ok(None)` when the queue is empty or all pending rows have
/// `next_attempt_at` in the future.
pub async fn claim_one_job(
    db: &DatabaseConnection,
    claimed_by: &str,
    lease_ttl_secs: i64,
) -> Result<Option<BackupJobRow>, BackupRunnerError> {
    // The claim query uses a correlated sub-SELECT with FOR UPDATE SKIP LOCKED
    // so two concurrent runners cannot claim the same row. SKIP LOCKED means
    // a runner that cannot acquire the lock immediately moves on rather than
    // waiting, which avoids head-of-line blocking.
    //
    // We claim two kinds of rows in a single WHERE-OR (the ADR-014 §"Claim
    // query" UNION-ALL form is not usable with FOR UPDATE — Postgres rejects
    // `FOR UPDATE is not allowed with UNION/INTERSECT/EXCEPT`):
    //   1. Pending rows ready to run (state='pending', next_attempt_at <= NOW()).
    //   2. Stale running rows whose lease expired (state='running',
    //      leased_until < NOW()). This is the lease-expiry reclaim that
    //      replaces boot-time reconcile.
    //
    // The OR variant is semantically equivalent and lets the row-level lock
    // attach cleanly. ORDER BY next_attempt_at NULLS FIRST keeps FIFO order
    // for pending rows; stale-running rows have a populated next_attempt_at
    // from their original insert.
    //
    // ADR-014 reference: §"Claim query" and §"Lease duration".
    let sql = r#"
UPDATE backup_jobs
SET
    state        = 'running',
    attempts     = attempts + 1,
    claim_token  = gen_random_uuid(),
    claimed_by   = $1,
    leased_until = NOW() + ($2 * interval '1 second'),
    started_at   = COALESCE(started_at, NOW()),
    updated_at   = NOW()
WHERE id = (
    SELECT id FROM backup_jobs
    WHERE (state = 'pending' AND next_attempt_at <= NOW())
       OR (state = 'running' AND leased_until < NOW())
    ORDER BY next_attempt_at NULLS FIRST
    LIMIT 1
    FOR UPDATE SKIP LOCKED
)
RETURNING
    id,
    backup_id,
    engine,
    target_kind,
    target_id,
    params,
    state,
    step,
    step_state,
    attempts,
    max_attempts,
    claim_token,
    max_runtime_secs
    "#;

    let row = BackupJobRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        vec![
            SValue::from(claimed_by.to_owned()),
            SValue::from(lease_ttl_secs),
        ],
    ))
    .one(db)
    .await
    .map_err(|e| BackupRunnerError::Database {
        operation: "claim_one_job",
        source: e,
    })?;

    Ok(row)
}

// ── Lease extension ───────────────────────────────────────────────────────────

/// Extend the lease on a claimed job (ADR-014 §"Lease duration").
///
/// Called by the runner on every `StepCompleted` and `Heartbeat` event.
/// The `claim_token` fencing check ensures a stale runner that was presumed
/// dead cannot extend the lease of a job it no longer owns.
///
/// Returns `Err(BackupRunnerError::LeaseLost)` if the UPDATE matched zero rows
/// (the job was reclaimed by another runner).
pub async fn extend_lease(
    db: &DatabaseConnection,
    job_id: i64,
    claim_token: Uuid,
    lease_ttl_secs: i64,
) -> Result<(), BackupRunnerError> {
    let sql = r#"
UPDATE backup_jobs
   SET leased_until = NOW() + ($1 * interval '1 second'),
       updated_at   = NOW()
 WHERE id = $2
   AND claim_token = $3
    "#;

    let result = db
        .execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            sql,
            vec![
                SValue::from(lease_ttl_secs),
                SValue::from(job_id),
                SValue::from(claim_token),
            ],
        ))
        .await
        .map_err(|e| BackupRunnerError::Database {
            operation: "extend_lease",
            source: e,
        })?;

    if result.rows_affected() == 0 {
        return Err(BackupRunnerError::LeaseLost { job_id });
    }

    Ok(())
}

// ── Step persistence ──────────────────────────────────────────────────────────

/// Persist a completed step atomically (ADR-014 §"Runner loop", `persist_step`).
///
/// Executes a transaction:
/// 1. UPDATE `backup_jobs` with the new step + durable_state + fresh lease.
///    The `claim_token` WHERE clause is a fencing token.
/// 2. INSERT an audit row into `backup_job_steps`.
///
/// If the UPDATE matches zero rows (claim_token mismatch), the transaction is
/// rolled back and `Err(BackupRunnerError::StepFenced)` is returned. This is
/// safe: the engine's `StepCompleted` event is idempotent by contract, so the
/// new owner will re-run the step and write its own row.
pub async fn persist_step_completed(
    db: &DatabaseConnection,
    job_id: i64,
    claim_token: Uuid,
    attempt: i32,
    step: &str,
    durable_state: Value,
    message: Option<&str>,
) -> Result<(), BackupRunnerError> {
    let txn: DatabaseTransaction = db.begin().await.map_err(|e| BackupRunnerError::Database {
        operation: "persist_step_completed:begin",
        source: e,
    })?;

    let update_sql = r#"
UPDATE backup_jobs
   SET step         = $1,
       step_state   = $2,
       leased_until = NOW() + interval '300 seconds',
       updated_at   = NOW()
 WHERE id           = $3
   AND claim_token  = $4
    "#;

    // `step_state` and `backup_job_steps.durable_state` are JSONB. Bind via
    // `SValue::Json(...)` so Postgres receives a JSON value, not a text
    // string. The same Value is reused for both the UPDATE and the audit
    // INSERT below.
    let durable_value = || SValue::Json(Some(Box::new(durable_state.clone())));

    let update_result = txn
        .execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            update_sql,
            vec![
                SValue::from(step.to_owned()),
                durable_value(),
                SValue::from(job_id),
                SValue::from(claim_token),
            ],
        ))
        .await
        .map_err(|e| BackupRunnerError::Database {
            operation: "persist_step_completed:update",
            source: e,
        })?;

    if update_result.rows_affected() == 0 {
        // Fencing token mismatch — roll back and report.
        txn.rollback()
            .await
            .map_err(|e| BackupRunnerError::Database {
                operation: "persist_step_completed:rollback",
                source: e,
            })?;
        return Err(BackupRunnerError::StepFenced {
            job_id,
            step: step.to_owned(),
            attempt,
        });
    }

    let insert_sql = r#"
INSERT INTO backup_job_steps
    (job_id, attempt, step, state, durable_state, message, occurred_at)
VALUES
    ($1, $2, $3, 'completed', $4, $5, NOW())
    "#;

    txn.execute(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        insert_sql,
        vec![
            SValue::from(job_id),
            SValue::from(attempt),
            SValue::from(step.to_owned()),
            durable_value(),
            SValue::from(message.map(|s| s.to_owned())),
        ],
    ))
    .await
    .map_err(|e| BackupRunnerError::Database {
        operation: "persist_step_completed:insert_step",
        source: e,
    })?;

    txn.commit()
        .await
        .map_err(|e| BackupRunnerError::Database {
            operation: "persist_step_completed:commit",
            source: e,
        })?;

    Ok(())
}

// ── Job completion ────────────────────────────────────────────────────────────

/// Mark a job as completed and propagate the result to the parent `backups` row
/// (ADR-014 §"Relationship to existing `backups`").
///
/// Both updates run in a single transaction so there is no window where
/// `backup_jobs.state='completed'` but `backups.state` is still `'running'`.
///
/// `claim_token` is checked as a fencing token on the `backup_jobs` UPDATE.
pub async fn mark_job_completed(
    db: &DatabaseConnection,
    job_id: i64,
    claim_token: Uuid,
    backup_id: i32,
    location: &str,
    size_bytes: Option<i64>,
    compression: &str,
) -> Result<(), BackupRunnerError> {
    let txn: DatabaseTransaction = db.begin().await.map_err(|e| BackupRunnerError::Database {
        operation: "mark_job_completed:begin",
        source: e,
    })?;

    let job_sql = r#"
UPDATE backup_jobs
   SET state       = 'completed',
       finished_at = NOW(),
       updated_at  = NOW()
 WHERE id          = $1
   AND claim_token = $2
    "#;

    txn.execute(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        job_sql,
        vec![SValue::from(job_id), SValue::from(claim_token)],
    ))
    .await
    .map_err(|e| BackupRunnerError::Database {
        operation: "mark_job_completed:update_job",
        source: e,
    })?;

    let backup_sql = r#"
UPDATE backups
   SET state            = 'completed',
       s3_location      = $1,
       size_bytes       = $2,
       compression_type = $3,
       finished_at      = NOW()
 WHERE id = $4
    "#;

    let result = txn
        .execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            backup_sql,
            vec![
                SValue::from(location.to_owned()),
                SValue::from(size_bytes),
                SValue::from(compression.to_owned()),
                SValue::from(backup_id),
            ],
        ))
        .await
        .map_err(|e| BackupRunnerError::Database {
            operation: "mark_job_completed:update_backup",
            source: e,
        })?;

    if result.rows_affected() == 0 {
        txn.rollback()
            .await
            .map_err(|e| BackupRunnerError::Database {
                operation: "mark_job_completed:rollback",
                source: e,
            })?;
        return Err(BackupRunnerError::ParentBackupNotFound {
            job_id,
            backup_id,
            final_state: "completed",
        });
    }

    txn.commit()
        .await
        .map_err(|e| BackupRunnerError::Database {
            operation: "mark_job_completed:commit",
            source: e,
        })?;

    // After committing the individual job result, attempt to close the parent
    // schedule_runs row if every sibling is now terminal. This is best-effort:
    // a failure here is logged by the caller but does not roll back the
    // completed backup row.
    mark_schedule_run_finished_if_done(db, backup_id).await?;

    Ok(())
}

// ── Job failure ───────────────────────────────────────────────────────────────

/// Mark a job as permanently failed and propagate the error to the parent
/// `backups` row (ADR-014 §"Runner loop", terminal failure branch).
///
/// Called when `attempts >= max_attempts` after `engine.rollback` completes.
/// The `finished_at` is stamped here — never at boot time — so duration metrics
/// are accurate (ADR-014 Problem 1 fix).
pub async fn mark_job_failed(
    db: &DatabaseConnection,
    job_id: i64,
    claim_token: Uuid,
    backup_id: i32,
    error_message: &str,
) -> Result<(), BackupRunnerError> {
    let txn: DatabaseTransaction = db.begin().await.map_err(|e| BackupRunnerError::Database {
        operation: "mark_job_failed:begin",
        source: e,
    })?;

    let job_sql = r#"
UPDATE backup_jobs
   SET state         = 'failed',
       error_message = $1,
       finished_at   = NOW(),
       updated_at    = NOW()
 WHERE id            = $2
   AND claim_token   = $3
    "#;

    txn.execute(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        job_sql,
        vec![
            SValue::from(error_message.to_owned()),
            SValue::from(job_id),
            SValue::from(claim_token),
        ],
    ))
    .await
    .map_err(|e| BackupRunnerError::Database {
        operation: "mark_job_failed:update_job",
        source: e,
    })?;

    let backup_sql = r#"
UPDATE backups
   SET state         = 'failed',
       error_message = $1,
       finished_at   = NOW()
 WHERE id = $2
    "#;

    let result = txn
        .execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            backup_sql,
            vec![
                SValue::from(error_message.to_owned()),
                SValue::from(backup_id),
            ],
        ))
        .await
        .map_err(|e| BackupRunnerError::Database {
            operation: "mark_job_failed:update_backup",
            source: e,
        })?;

    if result.rows_affected() == 0 {
        txn.rollback()
            .await
            .map_err(|e| BackupRunnerError::Database {
                operation: "mark_job_failed:rollback",
                source: e,
            })?;
        return Err(BackupRunnerError::ParentBackupNotFound {
            job_id,
            backup_id,
            final_state: "failed",
        });
    }

    txn.commit()
        .await
        .map_err(|e| BackupRunnerError::Database {
            operation: "mark_job_failed:commit",
            source: e,
        })?;

    // After committing the individual failure, close the parent schedule_runs
    // row if every sibling has also reached a terminal state.
    mark_schedule_run_finished_if_done(db, backup_id).await?;

    Ok(())
}

// ── Schedule-run completion ───────────────────────────────────────────────────

/// Mark the parent `schedule_runs` row as finished when all its child backups
/// have reached a terminal state.
///
/// This is called from both `mark_job_completed` and `mark_job_failed` after
/// the parent `backups` row has been updated. The UPDATE is a no-op if:
///
/// - The backup row has no `schedule_run_id` (legacy one-shot row).
/// - The `schedule_runs` row is already finished.
/// - At least one sibling backup is still `"pending"` or `"running"`.
///
/// The single-statement UPDATE is safe against races: the `NOT EXISTS` sub-
/// query is evaluated atomically within the same snapshot as the UPDATE. Two
/// concurrent workers cannot both see an empty pending set and both write
/// `finished_at`.
pub async fn mark_schedule_run_finished_if_done(
    db: &DatabaseConnection,
    backup_id: i32,
) -> Result<(), BackupRunnerError> {
    let sql = r#"
UPDATE schedule_runs sr
   SET finished_at = NOW()
 WHERE sr.id = (
     SELECT b.schedule_run_id
       FROM backups b
      WHERE b.id = $1
        AND b.schedule_run_id IS NOT NULL
   )
   AND sr.finished_at IS NULL
   AND NOT EXISTS (
       SELECT 1
         FROM backups b2
        WHERE b2.schedule_run_id = sr.id
          AND b2.state IN ('pending', 'running')
   )
    "#;

    db.execute(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        vec![SValue::from(backup_id)],
    ))
    .await
    .map_err(|e| BackupRunnerError::Database {
        operation: "mark_schedule_run_finished_if_done",
        source: e,
    })?;

    Ok(())
}

// ── Retry scheduling ──────────────────────────────────────────────────────────

/// Advance `next_attempt_at` using the backoff schedule and reset the job to
/// `state='pending'` (ADR-014 §"Backoff schedule").
///
/// Also surfaces the latest attempt's error on the parent `backups` row so the
/// UI can show "Retrying: <reason>" instead of a blank error field while the
/// job is pending a retry. The parent `backups.state` is deliberately left as
/// `'pending'` — the row is still live and will be retried.
///
/// Both updates run in one transaction so there is no window where the job is
/// reset to pending but the parent still shows a stale (or empty) error.
///
/// Called when the engine returns an error but `attempts < max_attempts`.
pub async fn schedule_retry(
    db: &DatabaseConnection,
    job_id: i64,
    claim_token: Uuid,
    next_attempt_at: chrono::DateTime<Utc>,
    backup_id: i32,
    error_message: &str,
) -> Result<(), BackupRunnerError> {
    let txn: DatabaseTransaction = db.begin().await.map_err(|e| BackupRunnerError::Database {
        operation: "schedule_retry:begin",
        source: e,
    })?;

    let job_sql = r#"
UPDATE backup_jobs
   SET state           = 'pending',
       next_attempt_at = $1,
       error_message   = $2,
       claim_token     = NULL,
       claimed_by      = NULL,
       leased_until    = NULL,
       updated_at      = NOW()
 WHERE id              = $3
   AND claim_token     = $4
    "#;

    txn.execute(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        job_sql,
        vec![
            SValue::from(next_attempt_at),
            SValue::from(error_message.to_owned()),
            SValue::from(job_id),
            SValue::from(claim_token),
        ],
    ))
    .await
    .map_err(|e| BackupRunnerError::Database {
        operation: "schedule_retry:update_job",
        source: e,
    })?;

    // Surface the latest attempt's error on the parent backup row.  The state
    // stays 'pending' — the backup is still alive and will be retried — but the
    // UI now shows "Retrying: <reason>" rather than a blank error_message.
    let backup_sql = r#"
UPDATE backups
   SET error_message = $1
 WHERE id = $2
    "#;

    txn.execute(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        backup_sql,
        vec![
            SValue::from(error_message.to_owned()),
            SValue::from(backup_id),
        ],
    ))
    .await
    .map_err(|e| BackupRunnerError::Database {
        operation: "schedule_retry:update_backup",
        source: e,
    })?;

    txn.commit()
        .await
        .map_err(|e| BackupRunnerError::Database {
            operation: "schedule_retry:commit",
            source: e,
        })?;

    Ok(())
}

// ── Backoff ───────────────────────────────────────────────────────────────────

/// Compute the backoff delay for a given attempt number (ADR-014 §"Backoff schedule").
///
/// Formula: `min(1 * 5^(attempt-1), 60) minutes`, capped at 60 minutes.
///
/// | attempt | delay   |
/// |---------|---------|
/// | 1       | 1 min   |
/// | 2       | 5 min   |
/// | 3       | 25 min  |
/// | 4       | 60 min  |
/// | 5+      | 60 min  |
pub fn backoff_delay(attempt: i32) -> Duration {
    if attempt <= 0 {
        return Duration::minutes(1);
    }
    // 5^(attempt-1), floored at 1 minute, capped at 60 minutes.
    let exp = (attempt - 1) as u32;
    let minutes = 5_i64.pow(exp).min(60);
    Duration::minutes(minutes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::{DatabaseBackend, MockDatabase, MockExecResult};

    // ── backoff_delay ─────────────────────────────────────────────────────────

    #[test]
    fn test_backoff_delay_attempt_1() {
        assert_eq!(backoff_delay(1), Duration::minutes(1));
    }

    #[test]
    fn test_backoff_delay_attempt_2() {
        assert_eq!(backoff_delay(2), Duration::minutes(5));
    }

    #[test]
    fn test_backoff_delay_attempt_3() {
        assert_eq!(backoff_delay(3), Duration::minutes(25));
    }

    #[test]
    fn test_backoff_delay_attempt_4_capped() {
        // 5^3 = 125 → capped at 60
        assert_eq!(backoff_delay(4), Duration::minutes(60));
    }

    #[test]
    fn test_backoff_delay_attempt_5_capped() {
        assert_eq!(backoff_delay(5), Duration::minutes(60));
    }

    #[test]
    fn test_backoff_delay_zero_or_negative() {
        assert_eq!(backoff_delay(0), Duration::minutes(1));
        assert_eq!(backoff_delay(-1), Duration::minutes(1));
    }

    // ── claim_one_job against MockDatabase ────────────────────────────────────

    #[tokio::test]
    async fn test_claim_one_job_empty_queue_returns_none() {
        use sea_orm::Value as SVal;
        use std::collections::BTreeMap;

        // MockDatabase expects BTreeMap<String, Value> rows for FromQueryResult types.
        // An empty vec simulates an empty queue result.
        let empty: Vec<BTreeMap<String, SVal>> = vec![];
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![empty])
            .into_connection();

        let result = claim_one_job(&db, "test-runner", 300).await;

        assert!(
            result.is_ok(),
            "claim_one_job should not fail on empty queue"
        );
        assert!(
            result.unwrap().is_none(),
            "claim_one_job should return None for empty queue"
        );
    }

    // ── extend_lease against MockDatabase ────────────────────────────────────

    #[tokio::test]
    async fn test_extend_lease_lease_lost_when_zero_rows_affected() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_exec_results(vec![MockExecResult {
                last_insert_id: 0,
                rows_affected: 0, // claim_token mismatch
            }])
            .into_connection();

        let token = Uuid::new_v4();
        let result = extend_lease(&db, 42, token, 300).await;

        assert!(result.is_err());
        assert!(
            matches!(
                result.unwrap_err(),
                BackupRunnerError::LeaseLost { job_id: 42 }
            ),
            "should return LeaseLost when zero rows affected"
        );
    }

    #[tokio::test]
    async fn test_extend_lease_success() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_exec_results(vec![MockExecResult {
                last_insert_id: 0,
                rows_affected: 1,
            }])
            .into_connection();

        let token = Uuid::new_v4();
        let result = extend_lease(&db, 42, token, 300).await;

        assert!(result.is_ok());
    }

    // ── schedule_retry updates both backup_jobs AND backups ───────────────────

    /// Bug 3 regression: verify that `schedule_retry` issues two UPDATE
    /// statements — one targeting `backup_jobs` and one targeting `backups`.
    /// Before the fix, only `backup_jobs.error_message` was updated; the
    /// parent `backups.error_message` stayed NULL so the UI showed a blank
    /// error while the row was in the "pending (retrying)" state.
    #[tokio::test]
    async fn test_schedule_retry_updates_both_job_and_backup_error_message() {
        // schedule_retry uses a transaction (begin → two EXECs → commit).
        // MockDatabase needs one exec result per statement executed inside
        // the transaction: the two UPDATEEs.
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_exec_results(vec![
                // UPDATE backup_jobs
                MockExecResult {
                    last_insert_id: 0,
                    rows_affected: 1,
                },
                // UPDATE backups
                MockExecResult {
                    last_insert_id: 0,
                    rows_affected: 1,
                },
            ])
            .into_connection();

        let token = Uuid::new_v4();
        let next_at = Utc::now() + Duration::minutes(1);

        let result = schedule_retry(&db, 42, token, next_at, 7, "bucket not reachable").await;

        assert!(
            result.is_ok(),
            "schedule_retry should succeed when both UPDATEEs return rows_affected=1: {:?}",
            result
        );

        // Verify the MockDatabase received exactly two UPDATE exec calls.
        // `into_transaction_log()` gives us the SQL log of what was executed.
        let log = db.into_transaction_log();
        // Each `sea_orm::Transaction` has a `statements()` slice. Count UPDATE
        // statements across all transactions.
        let update_count = log
            .iter()
            .flat_map(|txn| txn.statements())
            .filter(|stmt| stmt.sql.trim().to_uppercase().starts_with("UPDATE"))
            .count();

        assert_eq!(
            update_count, 2,
            "schedule_retry must issue exactly two UPDATE statements (backup_jobs + backups)"
        );
    }
}
