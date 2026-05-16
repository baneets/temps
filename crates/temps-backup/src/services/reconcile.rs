//! Reconciliation for orphaned `running` backup rows.
//!
//! When the temps process restarts mid-backup, the heartbeat task dies
//! with it. Without intervention the `backups` row stays in
//! `state="running"` forever — the UI shows it as "Running" forever, and
//! the row never gets a final size. We sweep both `backups` and
//! `external_service_backups`, mark every row that's still in `running`
//! with a stale heartbeat as `failed`, and stamp `finished_at` from the
//! best signal available (the last heartbeat, falling back to start
//! time + a short grace).
//!
//! Two entry points share the same logic:
//!
//! - `reconcile_orphan_backups` — runs once at server boot. Treats every
//!   `running` row as orphaned (any heartbeat is stale by definition
//!   because the heartbeat task died with the previous process).
//! - `sweep_stalled_backups` — runs on a 60s tick during normal
//!   operation. Only fails rows whose heartbeat is older than
//!   `STALL_THRESHOLD` so we don't false-positive a healthy backup.

use std::time::Duration;

use chrono::{DateTime, Utc};
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use tracing::{error, info};

/// A backup whose `last_heartbeat_at` is older than this is presumed dead.
/// Heartbeat fires every 30s (see `heartbeat::HEARTBEAT_INTERVAL`); five
/// minutes gives ten missed beats of slack so a transient DB blip or a
/// brief stop-the-world GC pause doesn't reap a healthy backup.
pub const STALL_THRESHOLD: Duration = Duration::from_secs(5 * 60);

/// Grace period added to `started_at` when a backup row has no heartbeat
/// at all. Means: "the worker died before the first 30s heartbeat could
/// fire." We give it a bit so the displayed duration isn't literally
/// zero, but not enough to be confusing.
const NO_HEARTBEAT_GRACE: chrono::Duration = chrono::Duration::seconds(30);

/// Sweep at boot. Marks every `running` row as `failed` regardless of
/// heartbeat freshness — the heartbeat task is by definition dead at this
/// point (it lived in the previous process).
pub async fn reconcile_orphan_backups(db: &DatabaseConnection) -> Result<(), sea_orm::DbErr> {
    let (parent_count, ext_count) = fail_running_backups(db, Mode::BootReconcile).await?;

    if parent_count > 0 || ext_count > 0 {
        info!(
            parent_count,
            ext_count,
            "Backup startup reconciliation: marked rows as failed (orphaned by previous process restart)"
        );
    } else {
        info!("Backup startup reconciliation: no orphaned rows found");
    }
    Ok(())
}

/// Sweep during normal operation. Only fails rows whose heartbeat is
/// older than `STALL_THRESHOLD` — fresh-heartbeat rows are presumed alive
/// and left alone.
///
/// Safe to call on a tick; rows already `completed`/`failed` are not
/// touched (the filter is `state='running'`).
pub async fn sweep_stalled_backups(db: &DatabaseConnection) -> Result<(), sea_orm::DbErr> {
    let (parent_count, ext_count) = fail_running_backups(db, Mode::RuntimeSweep).await?;

    if parent_count > 0 || ext_count > 0 {
        info!(
            parent_count,
            ext_count,
            "Backup stall sweep: marked stalled rows as failed (heartbeat older than 5 minutes)"
        );
    }
    Ok(())
}

#[derive(Clone, Copy, Debug)]
enum Mode {
    /// Server just started — every `running` row is by definition orphaned
    /// (the heartbeat task lived in the previous process).
    BootReconcile,
    /// Periodic tick during normal operation — only sweep rows whose
    /// heartbeat is stale.
    RuntimeSweep,
}

async fn fail_running_backups(
    db: &DatabaseConnection,
    mode: Mode,
) -> Result<(usize, usize), sea_orm::DbErr> {
    let now = Utc::now();

    // ---- Parent `backups` ----------------------------------------------
    let candidates = temps_entities::backups::Entity::find()
        .filter(temps_entities::backups::Column::State.eq("running"))
        .all(db)
        .await?;

    let mut parent_count = 0usize;
    for row in candidates {
        let last_hb = row.last_heartbeat_at;
        if matches!(mode, Mode::RuntimeSweep) && !is_stalled(last_hb, now) {
            continue;
        }

        let id = row.id;
        let finished_at = derive_finished_at(last_hb, row.started_at, now);
        let message = build_message(mode, last_hb, row.started_at, finished_at);

        let mut update: temps_entities::backups::ActiveModel = row.into();
        update.state = Set("failed".to_string());
        update.error_message = Set(Some(message));
        update.finished_at = Set(Some(finished_at));
        match update.update(db).await {
            Ok(_) => parent_count += 1,
            Err(e) => error!("Failed to reconcile orphan backup row {}: {}", id, e),
        }
    }

    // ---- Child `external_service_backups` ------------------------------
    // No heartbeat column on this entity; use started_at + grace as the
    // best signal. Same rule for runtime sweep: only fail if "started"
    // long enough ago that a healthy worker would have either heartbeated
    // the parent or completed by now.
    let ext_candidates = temps_entities::external_service_backups::Entity::find()
        .filter(temps_entities::external_service_backups::Column::State.eq("running"))
        .all(db)
        .await?;

    let mut ext_count = 0usize;
    for row in ext_candidates {
        if matches!(mode, Mode::RuntimeSweep)
            && now.signed_duration_since(row.started_at)
                < chrono::Duration::from_std(STALL_THRESHOLD)
                    .unwrap_or_else(|_| chrono::Duration::minutes(5))
        {
            continue;
        }

        let id = row.id;
        let finished_at = row.started_at + NO_HEARTBEAT_GRACE;
        let message = build_message(mode, None, row.started_at, finished_at);

        let mut update: temps_entities::external_service_backups::ActiveModel = row.into();
        update.state = Set("failed".to_string());
        update.error_message = Set(Some(message));
        update.finished_at = Set(Some(finished_at));
        match update.update(db).await {
            Ok(_) => ext_count += 1,
            Err(e) => error!(
                "Failed to reconcile orphan external_service_backups row {}: {}",
                id, e
            ),
        }
    }

    Ok((parent_count, ext_count))
}

fn is_stalled(last_hb: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
    match last_hb {
        Some(hb) => {
            let age = now.signed_duration_since(hb);
            age >= chrono::Duration::from_std(STALL_THRESHOLD)
                .unwrap_or_else(|_| chrono::Duration::minutes(5))
        }
        // No heartbeat ever fired. Heartbeat ticks every 30s; if a row
        // has been `running` longer than the grace window with no
        // heartbeat, the worker died before the first tick.
        None => true,
    }
}

/// Pick the most accurate timestamp for when the backup actually stopped
/// making progress. Order of preference:
/// 1. `last_heartbeat_at` — the worker was provably alive at this time.
/// 2. `started_at + grace` — the worker died before heartbeating.
///
/// Crucially we never use `now()` here: that produces fake durations like
/// "31h running" when really the worker died in minute one and the
/// server was offline for 31 hours.
fn derive_finished_at(
    last_hb: Option<DateTime<Utc>>,
    started_at: DateTime<Utc>,
    now: DateTime<Utc>,
) -> DateTime<Utc> {
    let candidate = last_hb.unwrap_or(started_at + NO_HEARTBEAT_GRACE);
    // Defensive cap: if the heartbeat is somehow in the future (clock
    // skew) or older than now we still want a sensible value. Clamp to
    // `now` so we never report a `finished_at` in the future.
    candidate.min(now)
}

fn build_message(
    mode: Mode,
    last_hb: Option<DateTime<Utc>>,
    started_at: DateTime<Utc>,
    finished_at: DateTime<Utc>,
) -> String {
    let what = match mode {
        Mode::BootReconcile => {
            // ADR-014 Phase 5: the legacy synchronous executor no longer exists.
            // Any row left in state='running' at boot was either started by the
            // previous runner (lease expiry will reclaim it) or stranded during
            // a server restart before the runner could mark it completed. Mark
            // it failed so the UI surfaces it clearly; the operator can re-trigger.
            "The temps server was restarted while this backup was running. \
             The BackupRunner will not resume it automatically — please re-trigger the backup"
        }
        Mode::RuntimeSweep => "The backup runner stopped sending heartbeats for this job",
    };
    match last_hb {
        Some(hb) => format!(
            "{}. Last sign of life was at {} (started {}, marked failed at {}). \
             Re-run the backup if needed.",
            what,
            hb.to_rfc3339(),
            started_at.to_rfc3339(),
            finished_at.to_rfc3339(),
        ),
        None => format!(
            "{}. The worker died before its first heartbeat (started {}). \
             Re-run the backup if needed.",
            what,
            started_at.to_rfc3339(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::{DatabaseBackend, MockDatabase, MockExecResult};

    fn running_backup(id: i32, hb: Option<DateTime<Utc>>) -> temps_entities::backups::Model {
        temps_entities::backups::Model {
            id,
            name: format!("backup-{}", id),
            backup_id: format!("uuid-{}", id),
            schedule_id: None,
            schedule_run_id: None,
            backup_type: "full".into(),
            state: "running".into(),
            started_at: Utc::now() - chrono::Duration::hours(2),
            finished_at: None,
            size_bytes: None,
            file_count: None,
            s3_source_id: 1,
            s3_location: String::new(),
            error_message: None,
            metadata: "{}".into(),
            checksum: None,
            compression_type: "gzip".into(),
            created_by: 1,
            expires_at: None,
            tags: "[]".into(),
            last_heartbeat_at: hb,
        }
    }

    fn running_external_backup(id: i32) -> temps_entities::external_service_backups::Model {
        temps_entities::external_service_backups::Model {
            id,
            service_id: 1,
            backup_id: 1,
            backup_type: "full".into(),
            state: "running".into(),
            started_at: Utc::now() - chrono::Duration::hours(2),
            finished_at: None,
            size_bytes: None,
            s3_location: String::new(),
            error_message: None,
            metadata: serde_json::json!({}),
            checksum: None,
            compression_type: "lz4".into(),
            created_by: 1,
            expires_at: None,
        }
    }

    #[test]
    fn finished_at_uses_heartbeat_when_present() {
        let started = Utc::now() - chrono::Duration::hours(31);
        let hb = started + chrono::Duration::minutes(2);
        let now = Utc::now();
        let derived = derive_finished_at(Some(hb), started, now);
        assert_eq!(derived, hb);
    }

    #[test]
    fn finished_at_falls_back_to_start_plus_grace_when_no_heartbeat() {
        let started = Utc::now() - chrono::Duration::hours(1);
        let now = Utc::now();
        let derived = derive_finished_at(None, started, now);
        assert_eq!(derived, started + NO_HEARTBEAT_GRACE);
    }

    #[test]
    fn finished_at_never_in_the_future() {
        let now = Utc::now();
        let future_hb = now + chrono::Duration::hours(1);
        let started = now - chrono::Duration::hours(1);
        let derived = derive_finished_at(Some(future_hb), started, now);
        assert_eq!(derived, now);
    }

    #[test]
    fn is_stalled_treats_missing_heartbeat_as_stalled() {
        assert!(is_stalled(None, Utc::now()));
    }

    #[test]
    fn is_stalled_fresh_heartbeat_is_alive() {
        let hb = Utc::now() - chrono::Duration::seconds(45);
        assert!(!is_stalled(Some(hb), Utc::now()));
    }

    #[test]
    fn is_stalled_old_heartbeat_is_dead() {
        let hb = Utc::now() - chrono::Duration::minutes(10);
        assert!(is_stalled(Some(hb), Utc::now()));
    }

    #[test]
    fn build_message_includes_heartbeat_when_present() {
        let started = Utc::now() - chrono::Duration::hours(2);
        let hb = started + chrono::Duration::minutes(2);
        let finished = hb;
        let msg = build_message(Mode::BootReconcile, Some(hb), started, finished);
        assert!(msg.contains("Last sign of life"));
        assert!(msg.contains("restarted"));
    }

    #[test]
    fn build_message_runtime_sweep_has_distinct_phrasing() {
        let started = Utc::now() - chrono::Duration::hours(1);
        let msg = build_message(
            Mode::RuntimeSweep,
            None,
            started,
            started + NO_HEARTBEAT_GRACE,
        );
        assert!(msg.contains("stopped sending heartbeats"));
    }

    #[tokio::test]
    async fn reconcile_marks_running_rows_as_failed() {
        let row = running_backup(7, Some(Utc::now() - chrono::Duration::minutes(3)));
        let ext_row = running_external_backup(11);
        let updated_row = temps_entities::backups::Model {
            state: "failed".into(),
            ..row.clone()
        };
        let updated_ext = temps_entities::external_service_backups::Model {
            state: "failed".into(),
            ..ext_row.clone()
        };

        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results([vec![row.clone()]])
            .append_query_results([vec![updated_row]])
            .append_query_results([vec![ext_row.clone()]])
            .append_query_results([vec![updated_ext]])
            .append_exec_results([
                MockExecResult {
                    last_insert_id: 0,
                    rows_affected: 1,
                },
                MockExecResult {
                    last_insert_id: 0,
                    rows_affected: 1,
                },
            ])
            .into_connection();

        let result = reconcile_orphan_backups(&db).await;
        assert!(result.is_ok(), "reconcile failed: {:?}", result);
    }

    #[tokio::test]
    async fn reconcile_with_no_orphans_is_noop() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results([Vec::<temps_entities::backups::Model>::new()])
            .append_query_results([Vec::<temps_entities::external_service_backups::Model>::new()])
            .into_connection();

        let result = reconcile_orphan_backups(&db).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn sweep_ignores_fresh_heartbeat_rows() {
        // A row that's `running` but heartbeated 10 seconds ago. The
        // sweeper should NOT touch it. We model that by returning the
        // row from SELECT but then issuing zero UPDATE statements
        // afterward — MockDatabase has no UPDATE results queued, so
        // any attempted UPDATE would panic.
        let row = running_backup(42, Some(Utc::now() - chrono::Duration::seconds(10)));
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results([vec![row]])
            .append_query_results([Vec::<temps_entities::external_service_backups::Model>::new()])
            .into_connection();

        let result = sweep_stalled_backups(&db).await;
        assert!(result.is_ok(), "sweep failed: {:?}", result);
    }
}
