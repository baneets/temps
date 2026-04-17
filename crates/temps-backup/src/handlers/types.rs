use sea_orm::DatabaseConnection;
use std::sync::Arc;
use temps_core::AuditLogger;
use temps_providers::postgres_upgrade_service::PostgresUpgradeService;

use crate::services::BackupService;

pub struct BackupAppState {
    pub backup_service: Arc<BackupService>,
    pub audit_service: Arc<dyn AuditLogger>,
    pub pg_upgrade_service: Arc<PostgresUpgradeService>,
    pub db: Arc<DatabaseConnection>,
}

pub async fn create_backup_app_state(
    backup_service: Arc<BackupService>,
    audit_service: Arc<dyn AuditLogger>,
    pg_upgrade_service: Arc<PostgresUpgradeService>,
    db: Arc<DatabaseConnection>,
) -> Arc<BackupAppState> {
    Arc::new(BackupAppState {
        backup_service,
        audit_service,
        pg_upgrade_service,
        db,
    })
}
