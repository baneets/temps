use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use temps_backup_core::{BackupRunner, RunnerConfig};
use temps_core::plugin::{
    PluginContext, PluginError, PluginRoutes, ServiceRegistrationContext, TempsPlugin,
};
use tracing;
use tracing::{error, info};
use utoipa::openapi::OpenApi;
use utoipa::OpenApi as OpenApiTrait;

use crate::{
    engines::{
        control_plane::{ControlPlaneDeps, ControlPlaneEngine},
        mongodb::{MongodbDeps, MongodbEngine},
        postgres_cluster::{PostgresClusterDeps, PostgresClusterEngine},
        postgres_pgdump::{PostgresPgDumpDeps, PostgresPgDumpEngine},
        postgres_walg::{PostgresWalgDeps, PostgresWalgEngine},
        redis::{RedisDeps, RedisEngine},
        s3_mirror::{S3MirrorDeps, S3MirrorEngine},
    },
    handlers::{self, create_backup_app_state, BackupAppState},
    services::{
        reconcile_orphan_backups, sweep_backup_alerts, sweep_stalled_backups,
        BackupNotificationAdapter, BackupService, RestoreService,
    },
};
use temps_providers::externalsvc::postgres_upgrade::{
    PostgresContainerLifecycle, PreUpgradeBackupProvider,
};
use temps_providers::postgres_lifecycle::PostgresLifecycleAdapter;
use temps_providers::postgres_upgrade_service::PostgresUpgradeService;

/// Backup Plugin for managing backup operations and schedules
pub struct BackupPlugin;

impl BackupPlugin {
    pub fn new() -> Self {
        Self
    }
}

impl Default for BackupPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl TempsPlugin for BackupPlugin {
    fn name(&self) -> &'static str {
        "backup"
    }

    fn register_services<'a>(
        &'a self,
        context: &'a ServiceRegistrationContext,
    ) -> Pin<Box<dyn Future<Output = Result<(), PluginError>> + Send + 'a>> {
        Box::pin(async move {
            // Get required dependencies from the service registry
            let db = context.require_service::<sea_orm::DatabaseConnection>();
            let external_service_manager =
                context.require_service::<temps_providers::ExternalServiceManager>();
            let notification_service =
                context.require_service::<temps_notifications::NotificationService>();
            let config_service = context.require_service::<temps_config::ConfigService>();
            let encryption_service = context.require_service::<temps_core::EncryptionService>();

            // Create BackupService — clone notification_service so we can also
            // pass it to the BackupNotificationAdapter wired into the runner.
            let backup_service = Arc::new(BackupService::new(
                db.clone(),
                external_service_manager.clone(),
                notification_service.clone(),
                config_service.clone(),
                encryption_service.clone(),
            ));
            context.register_service(backup_service.clone());

            // Create RestoreService — orchestrates generic restore across
            // all engines via the ExternalService trait.
            let restore_service = Arc::new(RestoreService::new(
                db.clone(),
                external_service_manager.clone(),
                encryption_service.clone(),
            ));
            context.register_service(restore_service.clone());

            // Get AuditService dependency from other plugins
            let audit_service = context.require_service::<dyn temps_core::AuditLogger>();

            // Build the Postgres major-upgrade service. It needs Docker + the
            // log service (owned by temps-logs) and treats BackupService as
            // the pre-upgrade backup provider via a trait, avoiding a
            // temps-providers -> temps-backup circular dependency.
            let docker = context.require_service::<bollard::Docker>();
            let log_service = context.require_service::<temps_logs::LogService>();
            let backup_provider: Arc<dyn PreUpgradeBackupProvider> = backup_service.clone();
            let lifecycle: Arc<dyn PostgresContainerLifecycle> =
                Arc::new(PostgresLifecycleAdapter::new(
                    db.clone(),
                    docker.clone(),
                    external_service_manager.clone(),
                    encryption_service.clone(),
                ));
            let pg_upgrade_service = Arc::new(PostgresUpgradeService::new(
                db.clone(),
                docker.clone(),
                backup_provider,
                lifecycle,
                log_service,
            ));
            context.register_service(pg_upgrade_service.clone());

            // ── ADR-014 Phase 5: BackupRunner is always constructed ───────────
            // The legacy synchronous backup path has been removed. Every manual
            // backup trigger and every scheduled backup goes through the runner.
            // There is no feature flag — the runner is always on.
            let instance_id = std::env::var("TEMPS_BACKUP_RUNNER_INSTANCE_ID")
                .or_else(|_| std::env::var("HOSTNAME"))
                .unwrap_or_else(|_| "temps-server".to_string());

            let max_concurrent = std::env::var("TEMPS_BACKUP_RUNNER_MAX_CONCURRENT")
                .ok()
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(4);

            let runner_config = RunnerConfig {
                instance_id,
                max_concurrent,
                ..RunnerConfig::default()
            };

            // Register all engines (ADR-014 Phase 1–4).
            let mut runner = BackupRunner::new(db.clone(), runner_config);

            // Phase 1: control-plane backup.
            runner.register_engine(Arc::new(ControlPlaneEngine::new(ControlPlaneDeps {
                db: db.clone(),
                encryption_service: encryption_service.clone(),
                config_service: config_service.clone(),
            })));

            // Phase 2: Redis.
            runner.register_engine(Arc::new(RedisEngine::new(RedisDeps {
                db: db.clone(),
                encryption_service: encryption_service.clone(),
                docker: docker.as_ref().clone(),
            })));

            // Phase 3: Postgres (pg_dump fallback, WAL-G, cluster).
            runner.register_engine(Arc::new(PostgresPgDumpEngine::new(PostgresPgDumpDeps {
                db: db.clone(),
                encryption_service: encryption_service.clone(),
                docker: docker.as_ref().clone(),
            })));

            runner.register_engine(Arc::new(PostgresWalgEngine::new(PostgresWalgDeps {
                db: db.clone(),
                encryption_service: encryption_service.clone(),
                docker: docker.as_ref().clone(),
            })));

            runner.register_engine(Arc::new(PostgresClusterEngine::new(PostgresClusterDeps {
                db: db.clone(),
                encryption_service: encryption_service.clone(),
                docker: docker.as_ref().clone(),
            })));

            // Phase 4: MongoDB.
            runner.register_engine(Arc::new(MongodbEngine::new(MongodbDeps {
                db: db.clone(),
                encryption_service: encryption_service.clone(),
                docker: docker.as_ref().clone(),
            })));

            // Phase 4: S3 mirror.
            runner.register_engine(Arc::new(S3MirrorEngine::new(S3MirrorDeps {
                db: db.clone(),
                encryption_service: encryption_service.clone(),
                docker: docker.as_ref().clone(),
            })));

            info!(
                "BackupRunner: registered 7 engines: \
                 control_plane, redis, postgres_pgdump, postgres_walg, \
                 postgres_cluster, mongodb, s3_mirror (ADR-014 Phase 1–4)",
            );

            // Wire the failure notifier (deliverable 3).
            // The adapter lives in temps-backup (where NotificationService is
            // available) and is set on the runner via the builder.  Failure
            // notifications are fire-and-forget; the adapter logs errors internally.
            // Cast Arc<temps_notifications::NotificationService> to
            // Arc<dyn temps_core::notifications::NotificationService> so the
            // adapter's constructor receives the trait object it expects.
            let core_notif_svc: Arc<dyn temps_core::notifications::NotificationService> =
                notification_service.clone();
            let notifier: Arc<dyn temps_backup_core::BackupFailureNotifier> =
                Arc::new(BackupNotificationAdapter::new(core_notif_svc, db.clone()));
            let runner = runner.with_notifier(notifier);

            let runner = Arc::new(runner);

            // Create BackupAppState for handlers. The runner is required — there is
            // no optional or deferred wiring step.
            let backup_app_state_inner = create_backup_app_state(
                backup_service,
                restore_service,
                audit_service,
                pg_upgrade_service,
                db.clone(),
                Arc::clone(&runner),
            );

            context.register_service(backup_app_state_inner);

            tracing::debug!("Backup plugin services registered successfully");
            Ok(())
        })
    }

    fn initialize_plugin_services<'a>(
        &'a self,
        context: &'a PluginContext,
    ) -> Pin<Box<dyn Future<Output = Result<(), PluginError>> + Send + 'a>> {
        Box::pin(async move {
            // During the transition from the legacy synchronous backup path to the
            // runner-only architecture (ADR-014 Phase 5 onward), any in-flight backup
            // rows from a prior process are now stranded — the legacy executor no
            // longer exists to update them. Mark them failed once at boot with a
            // clear message so operators know to re-trigger.
            //
            // This is one-shot per process start; the runtime stall sweeper
            // (`sweep_stalled_backups`) continues to catch rows that wedge during
            // normal operation.
            let db = context.require_service::<sea_orm::DatabaseConnection>();
            if let Err(e) = reconcile_orphan_backups(db.as_ref()).await {
                error!(
                    "Backup orphan reconciliation failed at startup (will not retry until next boot): {}",
                    e
                );
            }

            // Runtime stall sweeper. The boot reconcile only catches rows
            // orphaned by the *previous* process; a backup that wedges
            // during normal operation (runner task stuck on a slow S3
            // upload, hung docker exec, etc.) needs continuous detection.
            // Fires every minute, fails any row whose heartbeat is older
            // than STALL_THRESHOLD (5 min).
            let sweep_db = db.clone();
            tokio::spawn(async move {
                let mut tick = tokio::time::interval(std::time::Duration::from_secs(60));
                tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                // First tick fires immediately — fine, since the boot
                // reconcile already ran above and `sweep_stalled_backups`
                // is idempotent.
                loop {
                    tick.tick().await;
                    if let Err(e) = sweep_stalled_backups(sweep_db.as_ref()).await {
                        error!("Backup stall sweep failed (will retry next tick): {}", e);
                    }
                }
            });

            // Alert watcher: detects overdue schedules and stalled jobs.
            // Fires every 5 minutes; uses Skip so a slow DB doesn't cause
            // accumulated ticks to run back-to-back.
            let alert_db = db.clone();
            tokio::spawn(async move {
                let mut tick = tokio::time::interval(std::time::Duration::from_secs(5 * 60));
                tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                loop {
                    tick.tick().await;
                    match sweep_backup_alerts(alert_db.as_ref()).await {
                        Ok(stats) if stats.has_changes() => info!(
                            opened_overdue = stats.opened_overdue,
                            opened_stalled = stats.opened_stalled,
                            resolved_overdue = stats.resolved_overdue,
                            resolved_stalled = stats.resolved_stalled,
                            "backup alert sweep: state changes detected"
                        ),
                        Ok(_) => tracing::debug!("backup alert sweep: no changes"),
                        Err(e) => error!("backup alert sweep failed: {}", e),
                    }
                }
            });

            // Crash recovery: if `temps serve` restarts while an upgrade is
            // mid-flight, the tokio task driving it is gone. Rows stay in
            // `pending`/`running` until we re-spawn an orchestrator for each.
            // Phases are idempotent, so resuming is safe; a short log line
            // records the resumption for the user.
            let pg_upgrade_service =
                context.require_service::<temps_providers::postgres_upgrade_service::PostgresUpgradeService>();

            match pg_upgrade_service.resume_active_upgrades().await {
                Ok(n) if n > 0 => {
                    tracing::info!(resumed = n, "resumed Postgres major upgrades after restart");
                }
                Ok(_) => {}
                Err(e) => {
                    // Don't fail server boot over a resume failure — surface
                    // it loudly and move on so the rest of the platform starts.
                    error!("Failed to resume Postgres major upgrades on boot: {}", e);
                }
            }

            // Rollback volume retention sweep. `phase_snapshot` renames the
            // pre-upgrade PGDATA volume and stamps a 7-day expiry; without a
            // sweeper, expired volumes leak forever. Hourly is plenty — the
            // retention window is measured in days.
            let sweeper = pg_upgrade_service.clone();
            tokio::spawn(async move {
                let mut tick = tokio::time::interval(std::time::Duration::from_secs(3600));
                // First tick fires immediately; use it for a one-shot startup
                // pass so a server restart inside the retention window doesn't
                // have to wait an hour to reclaim disk.
                loop {
                    tick.tick().await;
                    match sweeper.sweep_expired_rollback_volumes().await {
                        Ok(0) => {}
                        Ok(n) => tracing::info!(
                            removed = n,
                            "swept expired Postgres-upgrade rollback volumes"
                        ),
                        Err(e) => {
                            error!("Rollback-volume sweep failed (will retry next tick): {}", e)
                        }
                    }
                }
            });

            // ── ADR-014 Phase 5: BackupRunner poll loop ───────────────────────
            // The runner was pre-constructed with all 7 engines registered
            // during `register_services`. Retrieve it from BackupAppState and
            // spawn the poll loop now that all services are initialised.
            let backup_app_state = context.require_service::<BackupAppState>();
            let runner = Arc::clone(&backup_app_state.backup_runner);

            info!(
                "BackupRunner starting poll loop with 7 engines registered (ADR-014 Phase 5, runner-only mode)",
            );

            let runner_cancel = tokio_util::sync::CancellationToken::new();
            let runner_cancel_clone = runner_cancel.clone();

            tokio::spawn(async move {
                runner.run_forever(runner_cancel_clone).await;
            });
            // The cancel token runs for the lifetime of the process.
            // Phase 5 will thread it through the plugin context for clean shutdown.
            drop(runner_cancel);

            Ok(())
        })
    }

    fn configure_routes(&self, context: &PluginContext) -> Option<PluginRoutes> {
        // Get the BackupAppState
        let backup_app_state = context.require_service::<BackupAppState>();

        // Merge backup + pg-upgrade + restore routes under a single state
        // so all handler modules share the same auth/audit/service wiring.
        let routes = handlers::configure_routes()
            .merge(handlers::pg_upgrade_handler::configure_routes())
            .merge(handlers::restore_handler::configure_routes())
            .with_state(backup_app_state);

        Some(PluginRoutes { router: routes })
    }

    fn openapi_schema(&self) -> Option<OpenApi> {
        let mut doc = <handlers::backup_handler::BackupApiDoc as OpenApiTrait>::openapi();
        doc.merge(<handlers::pg_upgrade_handler::PgUpgradeApiDoc as OpenApiTrait>::openapi());
        doc.merge(<handlers::restore_handler::RestoreApiDoc as OpenApiTrait>::openapi());
        Some(doc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_backup_plugin_name() {
        let backup_plugin = BackupPlugin::new();
        assert_eq!(backup_plugin.name(), "backup");
    }

    #[tokio::test]
    async fn test_backup_plugin_default() {
        let backup_plugin = BackupPlugin;
        assert_eq!(backup_plugin.name(), "backup");
    }
}
