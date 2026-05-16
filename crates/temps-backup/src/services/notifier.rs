//! `BackupNotificationAdapter`: concrete impl of [`BackupFailureNotifier`] for
//! `temps-backup` (deliverable 3).
//!
//! Lives in `temps-backup` so it can reach:
//! - [`temps_core::notifications::NotificationService`] (for dispatch)
//! - The `backups` entity (to look up schedule name)
//! - The `backup_schedules` entity (to look up schedule name for the notification)
//!
//! The adapter is wired into the `BackupRunner` via `runner.with_notifier(...)` in
//! `plugin.rs`.

use std::collections::HashMap;
use std::sync::Arc;

use sea_orm::{DatabaseConnection, EntityTrait};
use temps_backup_core::{BackupFailureContext, BackupFailureNotifier};
use temps_core::notifications::{
    NotificationData, NotificationPriority, NotificationService, NotificationType,
};
use tracing::error;

/// Dispatches a `NotificationData` event via the platform notification service
/// whenever a backup job reaches the terminal `failed` state.
///
/// The adapter performs a DB lookup to enrich the notification with the
/// schedule name (when available).  Any internal error is logged via
/// `tracing::error!` and swallowed — a notification failure must never
/// surface to the caller.
pub struct BackupNotificationAdapter {
    notification_service: Arc<dyn NotificationService>,
    db: Arc<DatabaseConnection>,
}

impl BackupNotificationAdapter {
    /// Create a new adapter.
    ///
    /// Both `notification_service` and `db` must be fully initialised before
    /// calling this constructor.
    pub fn new(
        notification_service: Arc<dyn NotificationService>,
        db: Arc<DatabaseConnection>,
    ) -> Self {
        Self {
            notification_service,
            db,
        }
    }
}

#[async_trait::async_trait]
impl BackupFailureNotifier for BackupNotificationAdapter {
    /// Dispatch a failure notification for `ctx`.
    ///
    /// Looks up the parent `backups` row to find a `schedule_id`; if present,
    /// looks up `backup_schedules` for a human-readable name.  Falls back to
    /// synthetic names gracefully — the notification is always sent even if
    /// lookups fail.
    async fn notify_failed(&self, ctx: BackupFailureContext) {
        // Look up the parent backups row to retrieve schedule_id.
        let schedule_name = match temps_entities::backups::Entity::find_by_id(ctx.backup_id)
            .one(self.db.as_ref())
            .await
        {
            Ok(Some(backup)) => {
                if let Some(sid) = backup.schedule_id {
                    match temps_entities::backup_schedules::Entity::find_by_id(sid)
                        .one(self.db.as_ref())
                        .await
                    {
                        Ok(Some(schedule)) => schedule.name,
                        Ok(None) => format!("schedule {}", sid),
                        Err(e) => {
                            error!(
                                backup_id = ctx.backup_id,
                                schedule_id = sid,
                                error = %e,
                                "BackupNotificationAdapter: failed to look up schedule name",
                            );
                            format!("schedule {}", sid)
                        }
                    }
                } else {
                    // Control-plane backup without a schedule (manual ad-hoc run).
                    format!("{} backup #{}", ctx.engine, ctx.backup_id)
                }
            }
            Ok(None) => {
                // Parent row disappeared — very unlikely; proceed with synthetic name.
                format!("{} backup #{}", ctx.engine, ctx.backup_id)
            }
            Err(e) => {
                error!(
                    backup_id = ctx.backup_id,
                    error = %e,
                    "BackupNotificationAdapter: failed to look up parent backup row",
                );
                format!("{} backup #{}", ctx.engine, ctx.backup_id)
            }
        };

        let mut metadata: HashMap<String, String> = HashMap::new();
        metadata.insert("backup_id".to_string(), ctx.backup_id.to_string());
        metadata.insert("job_id".to_string(), ctx.job_id.to_string());
        metadata.insert("engine".to_string(), ctx.engine.clone());
        metadata.insert("attempts".to_string(), ctx.attempts.to_string());
        metadata.insert("max_attempts".to_string(), ctx.max_attempts.to_string());
        metadata.insert("failed_at".to_string(), ctx.failed_at.to_rfc3339());

        let notification = NotificationData {
            id: uuid::Uuid::new_v4().to_string(),
            title: format!("Backup Failed: {}", schedule_name),
            message: format!(
                "Backup failed for {} (engine: {}, attempt {}/{}): {}",
                schedule_name, ctx.engine, ctx.attempts, ctx.max_attempts, ctx.error_message,
            ),
            notification_type: NotificationType::Error,
            priority: NotificationPriority::High,
            severity: Some("error".to_string()),
            timestamp: ctx.failed_at,
            metadata,
            bypass_throttling: false,
        };

        if let Err(e) = self
            .notification_service
            .send_notification(notification)
            .await
        {
            error!(
                backup_id = ctx.backup_id,
                job_id = ctx.job_id,
                engine = %ctx.engine,
                error = %e,
                "BackupNotificationAdapter: failed to dispatch failure notification (non-fatal)",
            );
        }
    }
}
