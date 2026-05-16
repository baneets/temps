//! `temps-backup-core`: engine-agnostic backup queue primitives (ADR-014).
//!
//! This crate defines the `BackupEngine` trait, `BackupRunner` struct, and all
//! SQL queue primitives. It deliberately has **no dependency on
//! `temps-providers` or `temps-backup`** — engines (in `temps-providers`) depend
//! on this crate, not the reverse.
//!
//! ## Crate structure
//!
//! - [`engine`] — `BackupEngine` trait and associated types (`StepEvent`,
//!   `StepCursor`, `BackupContext`, `BackupEngineError`).
//! - [`runner`] — `BackupRunner` struct with `run_forever`, `enqueue_job`,
//!   and the poll-claim-dispatch loop.
//! - [`queue`] — Low-level SQL primitives: claim, lease extension, step
//!   persistence, job completion/failure, retry scheduling, and backoff.
//! - [`config`] — `RunnerConfig` with defaults matching the ADR recommendations.
//! - [`error`] — `BackupRunnerError` enum (thiserror, typed, contextual).
//! - [`timeouts`] — Per-engine default wall-clock timeouts and the
//!   three-tier resolution helper (`resolve_max_runtime`).

pub mod config;
pub mod engine;
pub mod engine_v2;
pub mod error;
pub mod executor;
pub mod notifier;
pub mod queue;
pub mod runner;
pub mod timeouts;

// Flatten the most-used public types for convenience.
pub use config::RunnerConfig;
pub use engine::{BackupContext, BackupEngine, BackupEngineError, StepCursor, StepEvent};
pub use error::BackupRunnerError;
pub use notifier::{BackupFailureContext, BackupFailureNotifier};
pub use queue::{
    backoff_delay, cancel_backup, cancel_schedule_run, mark_schedule_run_finished_if_done,
    reclaim_orphan_jobs_on_startup, BackupJobRow,
};
pub use runner::{BackupRunner, EnqueueJobParams};

// v2 single-process executor — replaces the poll-claim-dispatch runner.
// Old `BackupRunner` is still wired up; engines and callers will be
// migrated to the executor one at a time.
pub use engine_v2::{
    BackupContext as ExecutorContext, BackupEngine as ExecutorEngine,
    BackupError as ExecutorBackupError, BackupOutcome as ExecutorOutcome,
};
pub use executor::{BackupExecutor, BackupExecutorBuilder, SpawnError, SpawnParams};
pub use timeouts::{default_max_runtime_secs, resolve_max_runtime};
