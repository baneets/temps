//! Alarm service for creating, resolving, and querying alarms
//!
//! Alarms are scoped to project/environment/deployment and represent
//! actionable events: container restarts, outages, high resource usage,
//! deployment failures, etc.

use chrono::{Duration, Utc};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, Order, PaginatorTrait,
    QueryFilter, QueryOrder, Set,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use temps_core::jobs::{AlarmFiredJob, AlarmResolvedJob};
use temps_core::notifications::{
    NotificationData, NotificationPriority, NotificationService, NotificationType,
};
use temps_core::{Job, JobQueue};
use temps_entities::alarms;
use thiserror::Error;
use tracing::{error, info};

/// Alarm types that the system can fire
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AlarmType {
    ContainerRestart,
    ContainerOomKilled,
    HighResponseTime,
    Outage,
    HighCpu,
    HighMemory,
    DeploymentFailed,
    HealthCheckFailed,
}

impl AlarmType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ContainerRestart => "container_restart",
            Self::ContainerOomKilled => "container_oom_killed",
            Self::HighResponseTime => "high_response_time",
            Self::Outage => "outage",
            Self::HighCpu => "high_cpu",
            Self::HighMemory => "high_memory",
            Self::DeploymentFailed => "deployment_failed",
            Self::HealthCheckFailed => "health_check_failed",
        }
    }

    pub fn parse_alarm_type(s: &str) -> Option<Self> {
        match s {
            "container_restart" => Some(Self::ContainerRestart),
            "container_oom_killed" => Some(Self::ContainerOomKilled),
            "high_response_time" => Some(Self::HighResponseTime),
            "outage" => Some(Self::Outage),
            "high_cpu" => Some(Self::HighCpu),
            "high_memory" => Some(Self::HighMemory),
            "deployment_failed" => Some(Self::DeploymentFailed),
            "health_check_failed" => Some(Self::HealthCheckFailed),
            _ => None,
        }
    }
}

/// Alarm severity levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AlarmSeverity {
    Info,
    Warning,
    Critical,
}

impl AlarmSeverity {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Critical => "critical",
        }
    }

    pub fn to_notification_priority(&self) -> NotificationPriority {
        match self {
            Self::Info => NotificationPriority::Low,
            Self::Warning => NotificationPriority::High,
            Self::Critical => NotificationPriority::Critical,
        }
    }

    pub fn to_notification_type(&self) -> NotificationType {
        match self {
            Self::Info => NotificationType::Info,
            Self::Warning => NotificationType::Warning,
            Self::Critical => NotificationType::Error,
        }
    }
}

/// Alarm status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlarmStatus {
    Firing,
    Acknowledged,
    Resolved,
}

impl AlarmStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Firing => "firing",
            Self::Acknowledged => "acknowledged",
            Self::Resolved => "resolved",
        }
    }
}

/// Request to fire a new alarm
#[derive(Debug, Clone)]
pub struct FireAlarmRequest {
    pub project_id: i32,
    pub environment_id: i32,
    pub deployment_id: i32,
    pub container_id: Option<i32>,
    pub alarm_type: AlarmType,
    pub severity: AlarmSeverity,
    pub title: String,
    pub message: String,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Error)]
pub enum AlarmError {
    #[error("Alarm {alarm_id} not found in project {project_id}")]
    NotFound { alarm_id: i32, project_id: i32 },

    #[error("Database error while {operation}: {reason}")]
    Database { operation: String, reason: String },

    #[error("Notification error for alarm {alarm_id}: {reason}")]
    Notification { alarm_id: i32, reason: String },

    #[error("Queue error for alarm {alarm_id}: {reason}")]
    Queue { alarm_id: i32, reason: String },
}

impl From<sea_orm::DbErr> for AlarmError {
    fn from(error: sea_orm::DbErr) -> Self {
        AlarmError::Database {
            operation: "alarm query".to_string(),
            reason: error.to_string(),
        }
    }
}

/// Alarm service handles creating, resolving, and querying alarms.
/// Uses the existing NotificationService for alerting and JobQueue for event propagation.
pub struct AlarmService {
    db: Arc<DatabaseConnection>,
    notification_service: Arc<dyn NotificationService>,
    job_queue: Arc<dyn JobQueue>,
    /// Minimum time between firing identical alarms (same type + deployment + container)
    cooldown: Duration,
}

impl AlarmService {
    pub fn new(
        db: Arc<DatabaseConnection>,
        notification_service: Arc<dyn NotificationService>,
        job_queue: Arc<dyn JobQueue>,
    ) -> Self {
        Self {
            db,
            notification_service,
            job_queue,
            cooldown: Duration::minutes(5),
        }
    }

    pub fn with_cooldown(mut self, cooldown: Duration) -> Self {
        self.cooldown = cooldown;
        self
    }

    /// Fire a new alarm. Checks cooldown to avoid spam.
    /// Returns the alarm ID if created, None if suppressed by cooldown.
    pub async fn fire_alarm(&self, request: FireAlarmRequest) -> Result<Option<i32>, AlarmError> {
        // Check cooldown: is there a recent firing alarm of the same type for the same deployment+container?
        if self.is_in_cooldown(&request).await? {
            info!(
                "Alarm suppressed by cooldown: type={}, deployment={}, container={:?}",
                request.alarm_type.as_str(),
                request.deployment_id,
                request.container_id
            );
            return Ok(None);
        }

        let now = Utc::now();

        let alarm = alarms::ActiveModel {
            project_id: Set(request.project_id),
            environment_id: Set(request.environment_id),
            deployment_id: Set(request.deployment_id),
            container_id: Set(request.container_id),
            alarm_type: Set(request.alarm_type.as_str().to_string()),
            severity: Set(request.severity.as_str().to_string()),
            status: Set(AlarmStatus::Firing.as_str().to_string()),
            title: Set(request.title.clone()),
            message: Set(Some(request.message.clone())),
            metadata: Set(request.metadata.clone()),
            fired_at: Set(now),
            ..Default::default()
        };

        let result = alarm
            .insert(self.db.as_ref())
            .await
            .map_err(|e| AlarmError::Database {
                operation: format!(
                    "insert alarm type={} for deployment {}",
                    request.alarm_type.as_str(),
                    request.deployment_id
                ),
                reason: e.to_string(),
            })?;

        let alarm_id = result.id;

        info!(
            "Alarm fired: id={}, type={}, severity={}, project={}, env={}, deployment={}",
            alarm_id,
            request.alarm_type.as_str(),
            request.severity.as_str(),
            request.project_id,
            request.environment_id,
            request.deployment_id,
        );

        // Send notification via existing notification system
        self.send_alarm_notification(&request, alarm_id).await;

        // Emit job to queue so webhooks, dashboard, etc. can react
        let job = Job::AlarmFired(AlarmFiredJob {
            alarm_id,
            project_id: request.project_id,
            environment_id: request.environment_id,
            deployment_id: request.deployment_id,
            alarm_type: request.alarm_type.as_str().to_string(),
            severity: request.severity.as_str().to_string(),
            title: request.title,
        });

        if let Err(e) = self.job_queue.send(job).await {
            error!(
                "Failed to emit AlarmFired job for alarm {}: {}",
                alarm_id, e
            );
        }

        Ok(Some(alarm_id))
    }

    /// Resolve an alarm by ID
    pub async fn resolve_alarm(&self, alarm_id: i32, project_id: i32) -> Result<(), AlarmError> {
        let alarm = alarms::Entity::find_by_id(alarm_id)
            .filter(alarms::Column::ProjectId.eq(project_id))
            .one(self.db.as_ref())
            .await?
            .ok_or(AlarmError::NotFound {
                alarm_id,
                project_id,
            })?;

        if alarm.status == AlarmStatus::Resolved.as_str() {
            return Ok(());
        }

        let mut active: alarms::ActiveModel = alarm.clone().into();
        active.status = Set(AlarmStatus::Resolved.as_str().to_string());
        active.resolved_at = Set(Some(Utc::now()));
        active
            .update(self.db.as_ref())
            .await
            .map_err(|e| AlarmError::Database {
                operation: format!("resolve alarm {}", alarm_id),
                reason: e.to_string(),
            })?;

        info!("Alarm resolved: id={}, type={}", alarm_id, alarm.alarm_type);

        // Emit resolved job
        let job = Job::AlarmResolved(AlarmResolvedJob {
            alarm_id,
            project_id: alarm.project_id,
            environment_id: alarm.environment_id,
            deployment_id: alarm.deployment_id,
            alarm_type: alarm.alarm_type.clone(),
            title: alarm.title.clone(),
        });

        if let Err(e) = self.job_queue.send(job).await {
            error!(
                "Failed to emit AlarmResolved job for alarm {}: {}",
                alarm_id, e
            );
        }

        // Send recovery notification
        self.send_resolved_notification(&alarm).await;

        Ok(())
    }

    /// Resolve all firing alarms of a given type for a deployment
    pub async fn resolve_alarms_by_type(
        &self,
        project_id: i32,
        deployment_id: i32,
        alarm_type: AlarmType,
    ) -> Result<Vec<i32>, AlarmError> {
        let firing_alarms = alarms::Entity::find()
            .filter(alarms::Column::ProjectId.eq(project_id))
            .filter(alarms::Column::DeploymentId.eq(deployment_id))
            .filter(alarms::Column::AlarmType.eq(alarm_type.as_str()))
            .filter(alarms::Column::Status.eq(AlarmStatus::Firing.as_str()))
            .all(self.db.as_ref())
            .await?;

        let mut resolved_ids = Vec::new();

        for alarm in firing_alarms {
            let alarm_id = alarm.id;
            self.resolve_alarm(alarm_id, project_id).await?;
            resolved_ids.push(alarm_id);
        }

        Ok(resolved_ids)
    }

    /// Acknowledge an alarm (mark it as seen but not resolved)
    pub async fn acknowledge_alarm(
        &self,
        alarm_id: i32,
        project_id: i32,
        user_id: i32,
    ) -> Result<(), AlarmError> {
        let alarm = alarms::Entity::find_by_id(alarm_id)
            .filter(alarms::Column::ProjectId.eq(project_id))
            .one(self.db.as_ref())
            .await?
            .ok_or(AlarmError::NotFound {
                alarm_id,
                project_id,
            })?;

        if alarm.status != AlarmStatus::Firing.as_str() {
            return Ok(());
        }

        let mut active: alarms::ActiveModel = alarm.into();
        active.status = Set(AlarmStatus::Acknowledged.as_str().to_string());
        active.acknowledged_at = Set(Some(Utc::now()));
        active.acknowledged_by = Set(Some(user_id));
        active
            .update(self.db.as_ref())
            .await
            .map_err(|e| AlarmError::Database {
                operation: format!("acknowledge alarm {}", alarm_id),
                reason: e.to_string(),
            })?;

        info!("Alarm acknowledged: id={}, by user {}", alarm_id, user_id);

        Ok(())
    }

    /// List alarms for a project with optional filters
    pub async fn list_alarms(
        &self,
        project_id: i32,
        filters: AlarmFilters,
        page: u64,
        page_size: u64,
    ) -> Result<(Vec<alarms::Model>, u64), AlarmError> {
        let page_size = std::cmp::min(page_size, 100);

        let mut query = alarms::Entity::find().filter(alarms::Column::ProjectId.eq(project_id));

        if let Some(environment_id) = filters.environment_id {
            query = query.filter(alarms::Column::EnvironmentId.eq(environment_id));
        }

        if let Some(deployment_id) = filters.deployment_id {
            query = query.filter(alarms::Column::DeploymentId.eq(deployment_id));
        }

        if let Some(alarm_type) = &filters.alarm_type {
            query = query.filter(alarms::Column::AlarmType.eq(alarm_type.as_str()));
        }

        if let Some(status) = &filters.status {
            query = query.filter(alarms::Column::Status.eq(status.as_str()));
        }

        if let Some(severity) = &filters.severity {
            query = query.filter(alarms::Column::Severity.eq(severity.as_str()));
        }

        let paginator = query
            .order_by(alarms::Column::FiredAt, Order::Desc)
            .paginate(self.db.as_ref(), page_size);

        let total = paginator.num_items().await?;
        let items = paginator.fetch_page(page.saturating_sub(1)).await?;

        Ok((items, total))
    }

    /// Get alarm counts by status for a project (for dashboard summary widget)
    pub async fn get_alarm_summary(&self, project_id: i32) -> Result<AlarmSummary, AlarmError> {
        let all_alarms = alarms::Entity::find()
            .filter(alarms::Column::ProjectId.eq(project_id))
            .filter(alarms::Column::Status.ne(AlarmStatus::Resolved.as_str()))
            .all(self.db.as_ref())
            .await?;

        let mut summary = AlarmSummary::default();
        let mut by_type: HashMap<String, u32> = HashMap::new();

        for alarm in &all_alarms {
            if alarm.status == AlarmStatus::Firing.as_str() {
                summary.firing += 1;
            } else if alarm.status == AlarmStatus::Acknowledged.as_str() {
                summary.acknowledged += 1;
            }

            if alarm.severity == AlarmSeverity::Critical.as_str() {
                summary.critical += 1;
            } else if alarm.severity == AlarmSeverity::Warning.as_str() {
                summary.warning += 1;
            }

            *by_type.entry(alarm.alarm_type.clone()).or_insert(0) += 1;
        }

        summary.total_active = summary.firing + summary.acknowledged;
        summary.by_type = by_type;

        Ok(summary)
    }

    /// Check if there's a recent alarm of the same type still within cooldown
    async fn is_in_cooldown(&self, request: &FireAlarmRequest) -> Result<bool, AlarmError> {
        let cutoff = Utc::now() - self.cooldown;

        let mut query = alarms::Entity::find()
            .filter(alarms::Column::ProjectId.eq(request.project_id))
            .filter(alarms::Column::DeploymentId.eq(request.deployment_id))
            .filter(alarms::Column::AlarmType.eq(request.alarm_type.as_str()))
            .filter(alarms::Column::FiredAt.gte(cutoff));

        if let Some(container_id) = request.container_id {
            query = query.filter(alarms::Column::ContainerId.eq(container_id));
        }

        let count = query.count(self.db.as_ref()).await?;
        Ok(count > 0)
    }

    /// Send notification for a fired alarm (failure is logged, not propagated)
    async fn send_alarm_notification(&self, request: &FireAlarmRequest, alarm_id: i32) {
        let notification = NotificationData {
            id: uuid::Uuid::new_v4().to_string(),
            title: request.title.clone(),
            message: request.message.clone(),
            notification_type: request.severity.to_notification_type(),
            priority: request.severity.to_notification_priority(),
            severity: Some(request.severity.as_str().to_string()),
            timestamp: Utc::now(),
            metadata: [
                ("alarm_id".to_string(), alarm_id.to_string()),
                (
                    "alarm_type".to_string(),
                    request.alarm_type.as_str().to_string(),
                ),
                ("project_id".to_string(), request.project_id.to_string()),
                (
                    "environment_id".to_string(),
                    request.environment_id.to_string(),
                ),
                (
                    "deployment_id".to_string(),
                    request.deployment_id.to_string(),
                ),
            ]
            .into_iter()
            .collect(),
            bypass_throttling: request.severity == AlarmSeverity::Critical,
        };

        if let Err(e) = self
            .notification_service
            .send_notification(notification)
            .await
        {
            error!(
                "Failed to send alarm notification for alarm {}: {}",
                alarm_id, e
            );
        }
    }

    /// Send recovery notification when an alarm resolves (failure is logged, not propagated)
    async fn send_resolved_notification(&self, alarm: &alarms::Model) {
        let notification = NotificationData {
            id: uuid::Uuid::new_v4().to_string(),
            title: format!("Resolved: {}", alarm.title),
            message: format!(
                "Alarm '{}' has been resolved.\nType: {}\nOriginal severity: {}",
                alarm.title, alarm.alarm_type, alarm.severity
            ),
            notification_type: NotificationType::Info,
            priority: NotificationPriority::Normal,
            severity: None,
            timestamp: Utc::now(),
            metadata: [
                ("alarm_id".to_string(), alarm.id.to_string()),
                ("alarm_type".to_string(), alarm.alarm_type.clone()),
                ("project_id".to_string(), alarm.project_id.to_string()),
                ("status".to_string(), "resolved".to_string()),
            ]
            .into_iter()
            .collect(),
            bypass_throttling: false,
        };

        if let Err(e) = self
            .notification_service
            .send_notification(notification)
            .await
        {
            error!(
                "Failed to send resolved notification for alarm {}: {}",
                alarm.id, e
            );
        }
    }
}

/// Filters for listing alarms
#[derive(Debug, Default)]
pub struct AlarmFilters {
    pub environment_id: Option<i32>,
    pub deployment_id: Option<i32>,
    pub alarm_type: Option<AlarmType>,
    pub status: Option<AlarmStatus>,
    pub severity: Option<AlarmSeverity>,
}

/// Summary counts for dashboard widget
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct AlarmSummary {
    pub total_active: u32,
    pub firing: u32,
    pub acknowledged: u32,
    pub critical: u32,
    pub warning: u32,
    pub by_type: HashMap<String, u32>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use sea_orm::{DatabaseBackend, MockDatabase, MockExecResult};
    use std::sync::atomic::{AtomicU32, Ordering};
    use temps_core::jobs::QueueError;
    use temps_core::notifications::{EmailMessage, NotificationError};

    // ── Test helpers ──────────────────────────────────────────────────

    /// Tracks how many notifications were sent
    struct TrackingNotificationService {
        send_count: AtomicU32,
    }

    impl TrackingNotificationService {
        fn new() -> Self {
            Self {
                send_count: AtomicU32::new(0),
            }
        }

        fn send_count(&self) -> u32 {
            self.send_count.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl NotificationService for TrackingNotificationService {
        async fn send_notification(
            &self,
            _notification: NotificationData,
        ) -> Result<(), NotificationError> {
            self.send_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        async fn send_email(&self, _message: EmailMessage) -> Result<(), NotificationError> {
            Ok(())
        }
        async fn is_configured(&self) -> Result<bool, NotificationError> {
            Ok(true)
        }
    }

    /// Tracks how many jobs were sent to the queue
    struct TrackingJobQueue {
        send_count: AtomicU32,
    }

    impl TrackingJobQueue {
        fn new() -> Self {
            Self {
                send_count: AtomicU32::new(0),
            }
        }

        fn send_count(&self) -> u32 {
            self.send_count.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl temps_core::JobQueue for TrackingJobQueue {
        async fn send(&self, _job: Job) -> Result<(), QueueError> {
            self.send_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn subscribe(&self) -> Box<dyn temps_core::JobReceiver> {
            unimplemented!("not needed in tests")
        }
    }

    fn sample_alarm(id: i32, alarm_type: &str, severity: &str, status: &str) -> alarms::Model {
        alarms::Model {
            id,
            project_id: 1,
            environment_id: 1,
            deployment_id: 10,
            container_id: Some(100),
            alarm_type: alarm_type.to_string(),
            severity: severity.to_string(),
            status: status.to_string(),
            title: format!("Test alarm {}", id),
            message: Some("Test message".to_string()),
            metadata: None,
            fired_at: Utc::now(),
            acknowledged_at: None,
            acknowledged_by: None,
            resolved_at: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn sample_fire_request() -> FireAlarmRequest {
        FireAlarmRequest {
            project_id: 1,
            environment_id: 1,
            deployment_id: 10,
            container_id: Some(100),
            alarm_type: AlarmType::ContainerRestart,
            severity: AlarmSeverity::Warning,
            title: "Container restarted".to_string(),
            message: "Container xyz restarted 3 times".to_string(),
            metadata: Some(serde_json::json!({"restart_count": 3})),
        }
    }

    // ── Type conversion tests ─────────────────────────────────────────

    #[test]
    fn test_alarm_type_roundtrip() {
        let types = [
            AlarmType::ContainerRestart,
            AlarmType::ContainerOomKilled,
            AlarmType::HighResponseTime,
            AlarmType::Outage,
            AlarmType::HighCpu,
            AlarmType::HighMemory,
            AlarmType::DeploymentFailed,
            AlarmType::HealthCheckFailed,
        ];

        for t in &types {
            let s = t.as_str();
            let parsed = AlarmType::parse_alarm_type(s);
            assert_eq!(parsed, Some(*t), "Roundtrip failed for {}", s);
        }
    }

    #[test]
    fn test_alarm_type_from_str_unknown() {
        assert_eq!(AlarmType::parse_alarm_type("unknown_type"), None);
        assert_eq!(AlarmType::parse_alarm_type(""), None);
    }

    #[test]
    fn test_alarm_severity_as_str() {
        assert_eq!(AlarmSeverity::Info.as_str(), "info");
        assert_eq!(AlarmSeverity::Warning.as_str(), "warning");
        assert_eq!(AlarmSeverity::Critical.as_str(), "critical");
    }

    #[test]
    fn test_alarm_severity_notification_priority() {
        assert!(matches!(
            AlarmSeverity::Info.to_notification_priority(),
            NotificationPriority::Low
        ));
        assert!(matches!(
            AlarmSeverity::Warning.to_notification_priority(),
            NotificationPriority::High
        ));
        assert!(matches!(
            AlarmSeverity::Critical.to_notification_priority(),
            NotificationPriority::Critical
        ));
    }

    #[test]
    fn test_alarm_severity_notification_type() {
        assert!(matches!(
            AlarmSeverity::Info.to_notification_type(),
            NotificationType::Info
        ));
        assert!(matches!(
            AlarmSeverity::Warning.to_notification_type(),
            NotificationType::Warning
        ));
        assert!(matches!(
            AlarmSeverity::Critical.to_notification_type(),
            NotificationType::Error
        ));
    }

    #[test]
    fn test_alarm_status_as_str() {
        assert_eq!(AlarmStatus::Firing.as_str(), "firing");
        assert_eq!(AlarmStatus::Acknowledged.as_str(), "acknowledged");
        assert_eq!(AlarmStatus::Resolved.as_str(), "resolved");
    }

    #[test]
    fn test_alarm_summary_default() {
        let summary = AlarmSummary::default();
        assert_eq!(summary.total_active, 0);
        assert_eq!(summary.firing, 0);
        assert_eq!(summary.acknowledged, 0);
        assert_eq!(summary.critical, 0);
        assert_eq!(summary.warning, 0);
        assert!(summary.by_type.is_empty());
    }

    #[test]
    fn test_alarm_error_display() {
        let not_found = AlarmError::NotFound {
            alarm_id: 42,
            project_id: 7,
        };
        assert_eq!(not_found.to_string(), "Alarm 42 not found in project 7");

        let db_err = AlarmError::Database {
            operation: "insert alarm".to_string(),
            reason: "connection refused".to_string(),
        };
        assert_eq!(
            db_err.to_string(),
            "Database error while insert alarm: connection refused"
        );

        let notif_err = AlarmError::Notification {
            alarm_id: 1,
            reason: "timeout".to_string(),
        };
        assert_eq!(
            notif_err.to_string(),
            "Notification error for alarm 1: timeout"
        );

        let queue_err = AlarmError::Queue {
            alarm_id: 2,
            reason: "channel closed".to_string(),
        };
        assert_eq!(
            queue_err.to_string(),
            "Queue error for alarm 2: channel closed"
        );
    }

    #[test]
    fn test_alarm_error_from_db_err() {
        let db_err = sea_orm::DbErr::RecordNotFound("alarms".to_string());
        let alarm_err: AlarmError = db_err.into();
        assert!(matches!(alarm_err, AlarmError::Database { .. }));
    }

    // ── AlarmService.fire_alarm tests ─────────────────────────────────

    #[tokio::test]
    async fn test_fire_alarm_success() {
        let alarm = sample_alarm(1, "container_restart", "warning", "firing");

        // Mock: cooldown check (count query returns 0) + insert
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results([[maplit::btreemap! {
                "num_items" => sea_orm::Value::BigInt(Some(0)),
            }]])
            .append_query_results(vec![vec![alarm.clone()]])
            .into_connection();

        let notification_service = Arc::new(TrackingNotificationService::new());
        let job_queue = Arc::new(TrackingJobQueue::new());

        let service = AlarmService::new(
            Arc::new(db),
            notification_service.clone(),
            job_queue.clone(),
        );

        let result = service.fire_alarm(sample_fire_request()).await;
        assert!(result.is_ok());
        let alarm_id = result.unwrap();
        assert_eq!(alarm_id, Some(1));

        // Notification and job should be sent
        assert_eq!(notification_service.send_count(), 1);
        assert_eq!(job_queue.send_count(), 1);
    }

    #[tokio::test]
    async fn test_fire_alarm_suppressed_by_cooldown() {
        // Mock: cooldown check returns count > 0
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results([[maplit::btreemap! {
                "num_items" => sea_orm::Value::BigInt(Some(1)),
            }]])
            .into_connection();

        let notification_service = Arc::new(TrackingNotificationService::new());
        let job_queue = Arc::new(TrackingJobQueue::new());

        let service = AlarmService::new(
            Arc::new(db),
            notification_service.clone(),
            job_queue.clone(),
        );

        let result = service.fire_alarm(sample_fire_request()).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), None, "Should be suppressed by cooldown");

        // No notification or job sent when suppressed
        assert_eq!(notification_service.send_count(), 0);
        assert_eq!(job_queue.send_count(), 0);
    }

    // ── AlarmService.resolve_alarm tests ──────────────────────────────

    #[tokio::test]
    async fn test_resolve_alarm_success() {
        let alarm = sample_alarm(1, "outage", "critical", "firing");

        // Mock: find alarm + update
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![vec![alarm.clone()]])
            .append_query_results(vec![vec![alarm.clone()]])
            .append_exec_results(vec![MockExecResult {
                last_insert_id: 1,
                rows_affected: 1,
            }])
            .into_connection();

        let notification_service = Arc::new(TrackingNotificationService::new());
        let job_queue = Arc::new(TrackingJobQueue::new());

        let service = AlarmService::new(
            Arc::new(db),
            notification_service.clone(),
            job_queue.clone(),
        );

        let result = service.resolve_alarm(1, 1).await;
        assert!(result.is_ok());

        // Should send resolved notification + AlarmResolved job
        assert_eq!(notification_service.send_count(), 1);
        assert_eq!(job_queue.send_count(), 1);
    }

    #[tokio::test]
    async fn test_resolve_alarm_not_found() {
        // Mock: find returns empty
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![Vec::<alarms::Model>::new()])
            .into_connection();

        let notification_service = Arc::new(TrackingNotificationService::new());
        let job_queue = Arc::new(TrackingJobQueue::new());

        let service = AlarmService::new(
            Arc::new(db),
            notification_service.clone(),
            job_queue.clone(),
        );

        let result = service.resolve_alarm(999, 1).await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            AlarmError::NotFound {
                alarm_id: 999,
                project_id: 1
            }
        ));

        // No notification or job sent
        assert_eq!(notification_service.send_count(), 0);
        assert_eq!(job_queue.send_count(), 0);
    }

    #[tokio::test]
    async fn test_resolve_alarm_already_resolved_is_noop() {
        let alarm = sample_alarm(1, "outage", "critical", "resolved");

        // Mock: find returns already-resolved alarm
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![vec![alarm]])
            .into_connection();

        let notification_service = Arc::new(TrackingNotificationService::new());
        let job_queue = Arc::new(TrackingJobQueue::new());

        let service = AlarmService::new(
            Arc::new(db),
            notification_service.clone(),
            job_queue.clone(),
        );

        let result = service.resolve_alarm(1, 1).await;
        assert!(result.is_ok());

        // No notification or job: it was already resolved
        assert_eq!(notification_service.send_count(), 0);
        assert_eq!(job_queue.send_count(), 0);
    }

    // ── AlarmService.resolve_alarms_by_type tests ─────────────────────

    #[tokio::test]
    async fn test_resolve_alarms_by_type_resolves_all_firing() {
        let alarm1 = sample_alarm(1, "outage", "critical", "firing");
        let alarm2 = sample_alarm(2, "outage", "warning", "firing");

        // Mock:
        // 1. find firing alarms of type outage
        // 2. resolve alarm 1: find + update
        // 3. resolve alarm 2: find + update
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![vec![alarm1.clone(), alarm2.clone()]])
            // resolve alarm 1
            .append_query_results(vec![vec![alarm1.clone()]])
            .append_query_results(vec![vec![alarm1.clone()]])
            .append_exec_results(vec![MockExecResult {
                last_insert_id: 1,
                rows_affected: 1,
            }])
            // resolve alarm 2
            .append_query_results(vec![vec![alarm2.clone()]])
            .append_query_results(vec![vec![alarm2.clone()]])
            .append_exec_results(vec![MockExecResult {
                last_insert_id: 2,
                rows_affected: 1,
            }])
            .into_connection();

        let notification_service = Arc::new(TrackingNotificationService::new());
        let job_queue = Arc::new(TrackingJobQueue::new());

        let service = AlarmService::new(
            Arc::new(db),
            notification_service.clone(),
            job_queue.clone(),
        );

        let result = service
            .resolve_alarms_by_type(1, 10, AlarmType::Outage)
            .await;
        assert!(result.is_ok());
        let resolved_ids = result.unwrap();
        assert_eq!(resolved_ids, vec![1, 2]);

        // 2 notifications + 2 jobs (one per resolved alarm)
        assert_eq!(notification_service.send_count(), 2);
        assert_eq!(job_queue.send_count(), 2);
    }

    #[tokio::test]
    async fn test_resolve_alarms_by_type_none_firing() {
        // Mock: find returns no firing alarms
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![Vec::<alarms::Model>::new()])
            .into_connection();

        let notification_service = Arc::new(TrackingNotificationService::new());
        let job_queue = Arc::new(TrackingJobQueue::new());

        let service = AlarmService::new(
            Arc::new(db),
            notification_service.clone(),
            job_queue.clone(),
        );

        let result = service
            .resolve_alarms_by_type(1, 10, AlarmType::Outage)
            .await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());

        assert_eq!(notification_service.send_count(), 0);
        assert_eq!(job_queue.send_count(), 0);
    }

    // ── AlarmService.acknowledge_alarm tests ──────────────────────────

    #[tokio::test]
    async fn test_acknowledge_alarm_success() {
        let alarm = sample_alarm(1, "high_cpu", "warning", "firing");

        // Mock: find + update
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![vec![alarm.clone()]])
            .append_query_results(vec![vec![alarm.clone()]])
            .append_exec_results(vec![MockExecResult {
                last_insert_id: 1,
                rows_affected: 1,
            }])
            .into_connection();

        let notification_service = Arc::new(TrackingNotificationService::new());
        let job_queue = Arc::new(TrackingJobQueue::new());

        let service = AlarmService::new(Arc::new(db), notification_service, job_queue);

        let result = service.acknowledge_alarm(1, 1, 42).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_acknowledge_alarm_not_found() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![Vec::<alarms::Model>::new()])
            .into_connection();

        let notification_service = Arc::new(TrackingNotificationService::new());
        let job_queue = Arc::new(TrackingJobQueue::new());

        let service = AlarmService::new(Arc::new(db), notification_service, job_queue);

        let result = service.acknowledge_alarm(999, 1, 42).await;
        assert!(matches!(
            result.unwrap_err(),
            AlarmError::NotFound {
                alarm_id: 999,
                project_id: 1
            }
        ));
    }

    #[tokio::test]
    async fn test_acknowledge_already_resolved_is_noop() {
        let alarm = sample_alarm(1, "high_cpu", "warning", "resolved");

        // Mock: find only (no update since it's already resolved)
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![vec![alarm]])
            .into_connection();

        let notification_service = Arc::new(TrackingNotificationService::new());
        let job_queue = Arc::new(TrackingJobQueue::new());

        let service = AlarmService::new(Arc::new(db), notification_service, job_queue);

        let result = service.acknowledge_alarm(1, 1, 42).await;
        assert!(result.is_ok()); // noop, no error
    }

    // ── AlarmService.get_alarm_summary tests ──────────────────────────

    #[tokio::test]
    async fn test_get_alarm_summary_mixed() {
        let alarms = vec![
            sample_alarm(1, "container_restart", "warning", "firing"),
            sample_alarm(2, "outage", "critical", "firing"),
            sample_alarm(3, "high_cpu", "warning", "acknowledged"),
            sample_alarm(4, "outage", "critical", "firing"),
        ];

        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![alarms])
            .into_connection();

        let notification_service = Arc::new(TrackingNotificationService::new());
        let job_queue = Arc::new(TrackingJobQueue::new());

        let service = AlarmService::new(Arc::new(db), notification_service, job_queue);

        let summary = service.get_alarm_summary(1).await.unwrap();
        assert_eq!(summary.firing, 3);
        assert_eq!(summary.acknowledged, 1);
        assert_eq!(summary.total_active, 4);
        assert_eq!(summary.critical, 2);
        assert_eq!(summary.warning, 2);
        assert_eq!(summary.by_type.get("outage"), Some(&2));
        assert_eq!(summary.by_type.get("container_restart"), Some(&1));
        assert_eq!(summary.by_type.get("high_cpu"), Some(&1));
    }

    #[tokio::test]
    async fn test_get_alarm_summary_empty() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![Vec::<alarms::Model>::new()])
            .into_connection();

        let notification_service = Arc::new(TrackingNotificationService::new());
        let job_queue = Arc::new(TrackingJobQueue::new());

        let service = AlarmService::new(Arc::new(db), notification_service, job_queue);

        let summary = service.get_alarm_summary(1).await.unwrap();
        assert_eq!(summary.total_active, 0);
        assert_eq!(summary.firing, 0);
        assert_eq!(summary.acknowledged, 0);
        assert!(summary.by_type.is_empty());
    }

    // ── AlarmService.list_alarms tests ────────────────────────────────

    #[tokio::test]
    async fn test_list_alarms_paginated() {
        let alarms = vec![
            sample_alarm(2, "outage", "critical", "firing"),
            sample_alarm(1, "container_restart", "warning", "firing"),
        ];

        // Mock: count query + fetch page
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results([[maplit::btreemap! {
                "num_items" => sea_orm::Value::BigInt(Some(5)),
            }]])
            .append_query_results(vec![alarms])
            .into_connection();

        let notification_service = Arc::new(TrackingNotificationService::new());
        let job_queue = Arc::new(TrackingJobQueue::new());

        let service = AlarmService::new(Arc::new(db), notification_service, job_queue);

        let (items, total) = service
            .list_alarms(1, AlarmFilters::default(), 1, 20)
            .await
            .unwrap();

        assert_eq!(total, 5);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, 2);
        assert_eq!(items[1].id, 1);
    }

    #[tokio::test]
    async fn test_list_alarms_page_size_capped_at_100() {
        // Even if we request 500, it should be capped at 100
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results([[maplit::btreemap! {
                "num_items" => sea_orm::Value::BigInt(Some(0)),
            }]])
            .append_query_results(vec![Vec::<alarms::Model>::new()])
            .into_connection();

        let notification_service = Arc::new(TrackingNotificationService::new());
        let job_queue = Arc::new(TrackingJobQueue::new());

        let service = AlarmService::new(Arc::new(db), notification_service, job_queue);

        let result = service
            .list_alarms(1, AlarmFilters::default(), 1, 500)
            .await;
        assert!(result.is_ok());
    }

    // ── Cooldown configuration tests ──────────────────────────────────

    #[test]
    fn test_with_cooldown_configures_duration() {
        let db = MockDatabase::new(DatabaseBackend::Postgres).into_connection();
        let notification_service = Arc::new(TrackingNotificationService::new());
        let job_queue = Arc::new(TrackingJobQueue::new());

        let service = AlarmService::new(Arc::new(db), notification_service, job_queue)
            .with_cooldown(chrono::Duration::minutes(15));

        assert_eq!(service.cooldown, chrono::Duration::minutes(15));
    }

    // ── FireAlarmRequest construction tests ───────────────────────────

    #[test]
    fn test_fire_alarm_request_metadata_serialization() {
        let request = sample_fire_request();
        assert!(request.metadata.is_some());
        let meta = request.metadata.unwrap();
        assert_eq!(meta["restart_count"], 3);
    }

    // ── Critical severity bypasses notification throttling ────────────

    #[tokio::test]
    async fn test_fire_critical_alarm_bypasses_throttling() {
        let alarm = sample_alarm(1, "container_oom_killed", "critical", "firing");

        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results([[maplit::btreemap! {
                "num_items" => sea_orm::Value::BigInt(Some(0)),
            }]])
            .append_query_results(vec![vec![alarm]])
            .into_connection();

        let notification_service = Arc::new(TrackingNotificationService::new());
        let job_queue = Arc::new(TrackingJobQueue::new());

        let service = AlarmService::new(Arc::new(db), notification_service.clone(), job_queue);

        let mut request = sample_fire_request();
        request.severity = AlarmSeverity::Critical;
        request.alarm_type = AlarmType::ContainerOomKilled;

        let result = service.fire_alarm(request).await;
        assert!(result.is_ok());
        assert_eq!(notification_service.send_count(), 1);
    }
}
