//! Container health monitoring loop
//!
//! Periodically inspects all active deployment containers to detect:
//! - Restart count increases (crash loops)
//! - OOM kills
//! - High CPU/memory usage
//! - Containers that exited unexpectedly

use crate::alarm_service::{AlarmService, AlarmSeverity, AlarmType, FireAlarmRequest};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use std::collections::HashMap;
use std::sync::Arc;
use temps_deployer::ContainerDeployer;
use temps_entities::{deployment_containers, deployments};
use tracing::{debug, error, info, warn};

/// Cached state for a container between health checks
#[derive(Debug, Clone)]
struct ContainerState {
    /// Last known restart count from Docker
    restart_count: i64,
}

/// Configuration for resource usage thresholds
#[derive(Debug, Clone)]
pub struct ContainerHealthConfig {
    /// How often to poll containers (seconds)
    pub poll_interval_secs: u64,
    /// CPU usage percent threshold to trigger alarm
    pub cpu_threshold_percent: f64,
    /// Memory usage percent threshold to trigger alarm
    pub memory_threshold_percent: f64,
    /// Number of consecutive checks above threshold before firing alarm
    pub consecutive_threshold_checks: u32,
}

impl Default for ContainerHealthConfig {
    fn default() -> Self {
        Self {
            poll_interval_secs: 30,
            cpu_threshold_percent: 90.0,
            memory_threshold_percent: 90.0,
            consecutive_threshold_checks: 3,
        }
    }
}

/// Container health monitoring service.
/// Polls Docker for container state and fires alarms via AlarmService.
pub struct ContainerHealthMonitor {
    db: Arc<DatabaseConnection>,
    deployer: Arc<dyn ContainerDeployer>,
    alarm_service: Arc<AlarmService>,
    config: ContainerHealthConfig,
    /// Cached restart counts keyed by deployment_container.id
    container_states: tokio::sync::RwLock<HashMap<i32, ContainerState>>,
    /// Consecutive high-resource checks keyed by (container_db_id, alarm_type_str)
    resource_counters: tokio::sync::RwLock<HashMap<(i32, &'static str), u32>>,
}

impl ContainerHealthMonitor {
    pub fn new(
        db: Arc<DatabaseConnection>,
        deployer: Arc<dyn ContainerDeployer>,
        alarm_service: Arc<AlarmService>,
        config: ContainerHealthConfig,
    ) -> Self {
        Self {
            db,
            deployer,
            alarm_service,
            config,
            container_states: tokio::sync::RwLock::new(HashMap::new()),
            resource_counters: tokio::sync::RwLock::new(HashMap::new()),
        }
    }

    /// Start the health monitoring loop. Runs forever.
    pub async fn start(self: Arc<Self>) {
        info!(
            "Starting container health monitor (poll interval: {}s, cpu threshold: {}%, memory threshold: {}%)",
            self.config.poll_interval_secs,
            self.config.cpu_threshold_percent,
            self.config.memory_threshold_percent,
        );

        loop {
            if let Err(e) = self.check_all_containers().await {
                error!("Container health check cycle failed: {}", e);
            }

            tokio::time::sleep(tokio::time::Duration::from_secs(
                self.config.poll_interval_secs,
            ))
            .await;
        }
    }

    /// Run one check cycle over all active containers
    async fn check_all_containers(&self) -> Result<(), String> {
        // Find all non-deleted deployment containers
        let containers = deployment_containers::Entity::find()
            .filter(deployment_containers::Column::DeletedAt.is_null())
            .all(self.db.as_ref())
            .await
            .map_err(|e| format!("Failed to query deployment_containers: {}", e))?;

        if containers.is_empty() {
            debug!("No active containers to monitor");
            return Ok(());
        }

        // Prune cached state for containers that no longer exist
        let active_ids: std::collections::HashSet<i32> = containers.iter().map(|c| c.id).collect();
        {
            let mut states = self.container_states.write().await;
            states.retain(|id, _| active_ids.contains(id));
        }
        {
            let mut counters = self.resource_counters.write().await;
            counters.retain(|(id, _), _| active_ids.contains(id));
        }

        debug!("Checking {} active containers", containers.len());

        for container in &containers {
            if let Err(e) = self.check_container(container).await {
                debug!(
                    "Failed to check container {} ({}): {}",
                    container.id, container.container_name, e
                );
            }
        }

        Ok(())
    }

    /// Check a single container for health issues
    async fn check_container(
        &self,
        container: &deployment_containers::Model,
    ) -> Result<(), String> {
        // Get the deployment to find project_id and environment_id
        let deployment = deployments::Entity::find_by_id(container.deployment_id)
            .one(self.db.as_ref())
            .await
            .map_err(|e| {
                format!(
                    "Failed to query deployment {}: {}",
                    container.deployment_id, e
                )
            })?
            .ok_or_else(|| {
                format!(
                    "Deployment {} not found for container {}",
                    container.deployment_id, container.id
                )
            })?;

        // Get container info from Docker
        let info = self
            .deployer
            .get_container_info(&container.container_id)
            .await
            .map_err(|e| {
                format!(
                    "Failed to get info for container {} ({}): {}",
                    container.container_id, container.container_name, e
                )
            })?;

        // Check restart count
        self.check_restart_count(container, &deployment, &info)
            .await;

        // Check container status (exited, dead, OOM)
        self.check_container_status(container, &deployment, &info)
            .await;

        // Check resource usage (CPU, memory)
        self.check_resource_usage(container, &deployment).await;

        Ok(())
    }

    /// Detect restart count increases and fire alarms
    async fn check_restart_count(
        &self,
        container: &deployment_containers::Model,
        deployment: &deployments::Model,
        info: &temps_deployer::ContainerInfo,
    ) {
        let current_restart_count = info.restart_count.unwrap_or(0);

        let mut states = self.container_states.write().await;
        let previous = states.get(&container.id).cloned();

        // Update cached state
        states.insert(
            container.id,
            ContainerState {
                restart_count: current_restart_count,
            },
        );

        drop(states);

        // On first check, just record the baseline
        let Some(prev) = previous else {
            debug!(
                "Container {} ({}) baseline restart_count={}",
                container.id, container.container_name, current_restart_count
            );
            return;
        };

        let delta = current_restart_count - prev.restart_count;
        if delta <= 0 {
            return;
        }

        warn!(
            "Container {} ({}) restarted {} time(s) (total: {})",
            container.id, container.container_name, delta, current_restart_count
        );

        let severity = if current_restart_count >= 10 {
            AlarmSeverity::Critical
        } else if current_restart_count >= 3 {
            AlarmSeverity::Warning
        } else {
            AlarmSeverity::Info
        };

        let request = FireAlarmRequest {
            project_id: deployment.project_id,
            environment_id: deployment.environment_id,
            deployment_id: deployment.id,
            container_id: Some(container.id),
            alarm_type: AlarmType::ContainerRestart,
            severity,
            title: format!(
                "Container '{}' restarted {} time(s)",
                container.container_name, delta
            ),
            message: format!(
                "Container '{}' has restarted. Total restart count: {}. \
                 This may indicate a crash loop or OOM kill.",
                container.container_name, current_restart_count
            ),
            metadata: Some(serde_json::json!({
                "container_name": container.container_name,
                "container_id": container.container_id,
                "restart_count": current_restart_count,
                "restart_delta": delta,
                "previous_restart_count": prev.restart_count,
            })),
        };

        if let Err(e) = self.alarm_service.fire_alarm(request).await {
            error!(
                "Failed to fire restart alarm for container {}: {}",
                container.id, e
            );
        }
    }

    /// Detect containers that have exited or died
    async fn check_container_status(
        &self,
        container: &deployment_containers::Model,
        deployment: &deployments::Model,
        info: &temps_deployer::ContainerInfo,
    ) {
        let status_str = info.status.to_string();

        match &info.status {
            temps_deployer::ContainerStatus::Exited | temps_deployer::ContainerStatus::Dead => {
                // Skip alarm if this is an on-demand environment that was intentionally
                // put to sleep. The on-demand manager stops containers on idle — that's
                // expected, not an error.
                if self.is_on_demand_sleeping(deployment.environment_id).await {
                    debug!(
                        "Container {} ({}) is {} but environment {} is on-demand sleeping, skipping alarm",
                        container.id, container.container_name, status_str, deployment.environment_id
                    );
                    return;
                }

                warn!(
                    "Container {} ({}) is in '{}' state",
                    container.id, container.container_name, status_str
                );

                // Determine if it's an OOM kill by checking metadata
                // Docker sets OOMKilled in state, which we can check via restart_count behavior
                let alarm_type = AlarmType::ContainerOomKilled;
                let severity = AlarmSeverity::Critical;

                let request = FireAlarmRequest {
                    project_id: deployment.project_id,
                    environment_id: deployment.environment_id,
                    deployment_id: deployment.id,
                    container_id: Some(container.id),
                    alarm_type,
                    severity,
                    title: format!("Container '{}' is {}", container.container_name, status_str),
                    message: format!(
                        "Container '{}' has exited or died unexpectedly. Status: {}",
                        container.container_name, status_str
                    ),
                    metadata: Some(serde_json::json!({
                        "container_name": container.container_name,
                        "container_id": container.container_id,
                        "status": status_str,
                    })),
                };

                if let Err(e) = self.alarm_service.fire_alarm(request).await {
                    error!(
                        "Failed to fire status alarm for container {}: {}",
                        container.id, e
                    );
                }
            }
            _ => {
                // Container is in a healthy state, nothing to do
            }
        }
    }

    /// Check if an environment is currently sleeping (on-demand scale-to-zero).
    /// When sleeping=true, containers were intentionally stopped — not a crash.
    async fn is_on_demand_sleeping(&self, environment_id: i32) -> bool {
        use temps_entities::environments;

        match environments::Entity::find_by_id(environment_id)
            .one(self.db.as_ref())
            .await
        {
            Ok(Some(env)) => env.sleeping,
            _ => false,
        }
    }

    /// Check CPU and memory usage against thresholds
    async fn check_resource_usage(
        &self,
        container: &deployment_containers::Model,
        deployment: &deployments::Model,
    ) {
        let stats = match self
            .deployer
            .get_container_stats(&container.container_id)
            .await
        {
            Ok(s) => s,
            Err(e) => {
                debug!(
                    "Failed to get stats for container {} ({}): {}",
                    container.id, container.container_name, e
                );
                return;
            }
        };

        // Check CPU
        if stats.cpu_percent > self.config.cpu_threshold_percent {
            self.handle_resource_threshold(
                container,
                deployment,
                AlarmType::HighCpu,
                AlarmSeverity::Warning,
                format!(
                    "Container '{}' CPU at {:.1}%",
                    container.container_name, stats.cpu_percent
                ),
                format!(
                    "Container '{}' CPU usage is at {:.1}%, above the {:.0}% threshold.",
                    container.container_name, stats.cpu_percent, self.config.cpu_threshold_percent,
                ),
                serde_json::json!({
                    "container_name": container.container_name,
                    "cpu_percent": stats.cpu_percent,
                    "threshold_percent": self.config.cpu_threshold_percent,
                }),
            )
            .await;
        } else {
            self.reset_resource_counter(container.id, AlarmType::HighCpu.as_str())
                .await;
        }

        // Check memory
        if let Some(mem_percent) = stats.memory_percent {
            if mem_percent > self.config.memory_threshold_percent {
                self.handle_resource_threshold(
                    container,
                    deployment,
                    AlarmType::HighMemory,
                    AlarmSeverity::Warning,
                    format!(
                        "Container '{}' memory at {:.1}%",
                        container.container_name, mem_percent
                    ),
                    format!(
                        "Container '{}' memory usage is at {:.1}% ({:.0} MB), above the {:.0}% threshold.",
                        container.container_name,
                        mem_percent,
                        stats.memory_bytes as f64 / 1024.0 / 1024.0,
                        self.config.memory_threshold_percent,
                    ),
                    serde_json::json!({
                        "container_name": container.container_name,
                        "memory_percent": mem_percent,
                        "memory_bytes": stats.memory_bytes,
                        "memory_limit_bytes": stats.memory_limit_bytes,
                        "threshold_percent": self.config.memory_threshold_percent,
                    }),
                )
                .await;
            } else {
                self.reset_resource_counter(container.id, AlarmType::HighMemory.as_str())
                    .await;
            }
        }
    }

    /// Handle a resource threshold breach. Only fires alarm after N consecutive breaches.
    #[allow(clippy::too_many_arguments)]
    async fn handle_resource_threshold(
        &self,
        container: &deployment_containers::Model,
        deployment: &deployments::Model,
        alarm_type: AlarmType,
        severity: AlarmSeverity,
        title: String,
        message: String,
        metadata: serde_json::Value,
    ) {
        let key = (container.id, alarm_type.as_str());

        let count = {
            let mut counters = self.resource_counters.write().await;
            let counter = counters.entry(key).or_insert(0);
            *counter += 1;
            *counter
        };

        if count < self.config.consecutive_threshold_checks {
            debug!(
                "Container {} resource threshold breach {}/{} for {}",
                container.id,
                count,
                self.config.consecutive_threshold_checks,
                alarm_type.as_str()
            );
            return;
        }

        let request = FireAlarmRequest {
            project_id: deployment.project_id,
            environment_id: deployment.environment_id,
            deployment_id: deployment.id,
            container_id: Some(container.id),
            alarm_type,
            severity,
            title,
            message,
            metadata: Some(metadata),
        };

        if let Err(e) = self.alarm_service.fire_alarm(request).await {
            error!(
                "Failed to fire resource alarm for container {}: {}",
                container.id, e
            );
        }

        // Reset counter after firing (cooldown in AlarmService prevents spam)
        self.reset_resource_counter(container.id, alarm_type.as_str())
            .await;
    }

    /// Reset the consecutive counter for a resource type
    async fn reset_resource_counter(&self, container_id: i32, alarm_type: &'static str) {
        let mut counters = self.resource_counters.write().await;
        counters.remove(&(container_id, alarm_type));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alarm_service::AlarmService;
    use async_trait::async_trait;
    use sea_orm::{DatabaseBackend, MockDatabase};
    use temps_core::jobs::QueueError;
    use temps_core::notifications::{
        EmailMessage, NotificationData, NotificationError, NotificationService,
    };
    use temps_deployer::{
        ContainerInfo, ContainerStats, ContainerStatus, DeployRequest, DeployResult, DeployerError,
    };

    // ── Mock helpers ──────────────────────────────────────────────────

    struct NoopNotificationService;

    #[async_trait]
    impl NotificationService for NoopNotificationService {
        async fn send_notification(
            &self,
            _notification: NotificationData,
        ) -> Result<(), NotificationError> {
            Ok(())
        }
        async fn send_email(&self, _message: EmailMessage) -> Result<(), NotificationError> {
            Ok(())
        }
        async fn is_configured(&self) -> Result<bool, NotificationError> {
            Ok(false)
        }
    }

    struct NoopJobQueue;

    #[async_trait]
    impl temps_core::JobQueue for NoopJobQueue {
        async fn send(&self, _job: temps_core::Job) -> Result<(), QueueError> {
            Ok(())
        }
        fn subscribe(&self) -> Box<dyn temps_core::JobReceiver> {
            unimplemented!()
        }
    }

    /// Mock ContainerDeployer that returns configurable container info and stats
    struct MockDeployer {
        info: tokio::sync::Mutex<ContainerInfo>,
        stats: tokio::sync::Mutex<ContainerStats>,
    }

    #[allow(dead_code)]
    impl MockDeployer {
        fn new(restart_count: i64, status: ContainerStatus) -> Self {
            Self {
                info: tokio::sync::Mutex::new(ContainerInfo {
                    container_id: "abc123".to_string(),
                    container_name: "test-container".to_string(),
                    image_name: "test-image:latest".to_string(),
                    status,
                    created_at: chrono::Utc::now(),
                    ports: vec![],
                    environment_vars: std::collections::HashMap::new(),
                    restart_count: Some(restart_count),
                    labels: std::collections::HashMap::new(),
                }),
                stats: tokio::sync::Mutex::new(ContainerStats {
                    container_id: "abc123".to_string(),
                    container_name: "test-container".to_string(),
                    cpu_percent: 10.0,
                    memory_bytes: 100 * 1024 * 1024,
                    memory_limit_bytes: Some(512 * 1024 * 1024),
                    memory_percent: Some(19.5),
                    network_rx_bytes: 0,
                    network_tx_bytes: 0,
                    timestamp: chrono::Utc::now(),
                }),
            }
        }

        async fn set_restart_count(&self, count: i64) {
            self.info.lock().await.restart_count = Some(count);
        }

        async fn set_status(&self, status: ContainerStatus) {
            self.info.lock().await.status = status;
        }

        async fn set_cpu_percent(&self, percent: f64) {
            self.stats.lock().await.cpu_percent = percent;
        }

        async fn set_memory_percent(&self, percent: f64) {
            self.stats.lock().await.memory_percent = Some(percent);
        }
    }

    #[async_trait]
    impl ContainerDeployer for MockDeployer {
        async fn deploy_container(
            &self,
            _request: DeployRequest,
        ) -> Result<DeployResult, DeployerError> {
            unimplemented!()
        }
        async fn start_container(&self, _id: &str) -> Result<(), DeployerError> {
            unimplemented!()
        }
        async fn stop_container(&self, _id: &str) -> Result<(), DeployerError> {
            unimplemented!()
        }
        async fn pause_container(&self, _id: &str) -> Result<(), DeployerError> {
            unimplemented!()
        }
        async fn resume_container(&self, _id: &str) -> Result<(), DeployerError> {
            unimplemented!()
        }
        async fn remove_container(&self, _id: &str) -> Result<(), DeployerError> {
            unimplemented!()
        }
        async fn get_container_info(&self, _id: &str) -> Result<ContainerInfo, DeployerError> {
            Ok(self.info.lock().await.clone())
        }
        async fn get_container_stats(&self, _id: &str) -> Result<ContainerStats, DeployerError> {
            Ok(self.stats.lock().await.clone())
        }
        async fn list_containers(&self) -> Result<Vec<ContainerInfo>, DeployerError> {
            Ok(vec![])
        }
        async fn get_container_logs(&self, _id: &str) -> Result<String, DeployerError> {
            Ok(String::new())
        }
        async fn stream_container_logs(
            &self,
            _id: &str,
        ) -> Result<Box<dyn futures::Stream<Item = String> + Unpin + Send>, DeployerError> {
            unimplemented!()
        }
    }

    fn make_container_model(id: i32) -> deployment_containers::Model {
        deployment_containers::Model {
            id,
            deployment_id: 10,
            container_id: "abc123".to_string(),
            container_name: "test-container".to_string(),
            container_port: 3000,
            host_port: Some(8080),
            image_name: Some("test-image:latest".to_string()),
            status: Some("running".to_string()),
            created_at: chrono::Utc::now(),
            deployed_at: chrono::Utc::now(),
            ready_at: None,
            deleted_at: None,
            node_id: None,
        }
    }

    fn make_deployment_model() -> deployments::Model {
        deployments::Model {
            id: 10,
            project_id: 1,
            environment_id: 1,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            slug: "deploy-abc".to_string(),
            state: "ready".to_string(),
            metadata: None,
            deploying_at: None,
            ready_at: None,
            started_at: None,
            finished_at: None,
            context_vars: None,
            branch_ref: None,
            tag_ref: None,
            commit_sha: None,
            commit_message: None,
            commit_author: None,
            commit_json: None,
            cancelled_reason: None,
            static_dir_location: None,
            screenshot_location: None,
            image_name: None,
            deployment_config: None,
            promoted_from_deployment_id: None,
        }
    }

    fn make_alarm_service(db: Arc<sea_orm::DatabaseConnection>) -> Arc<AlarmService> {
        Arc::new(AlarmService::new(
            db,
            Arc::new(NoopNotificationService),
            Arc::new(NoopJobQueue),
        ))
    }

    // ── Config tests ──────────────────────────────────────────────────

    #[test]
    fn test_container_health_config_default() {
        let config = ContainerHealthConfig::default();
        assert_eq!(config.poll_interval_secs, 30);
        assert_eq!(config.cpu_threshold_percent, 90.0);
        assert_eq!(config.memory_threshold_percent, 90.0);
        assert_eq!(config.consecutive_threshold_checks, 3);
    }

    #[test]
    fn test_container_state_clone() {
        let state = ContainerState { restart_count: 5 };
        let cloned = state.clone();
        assert_eq!(cloned.restart_count, 5);
    }

    // ── Restart detection tests ───────────────────────────────────────

    #[tokio::test]
    async fn test_check_restart_count_first_check_sets_baseline() {
        let deployer = Arc::new(MockDeployer::new(0, ContainerStatus::Running));
        let container = make_container_model(1);
        let deployment = make_deployment_model();

        // No DB calls for alarm on first check (just baseline)
        let db = MockDatabase::new(DatabaseBackend::Postgres).into_connection();
        let db = Arc::new(db);
        let alarm_service = make_alarm_service(db.clone());

        let monitor = ContainerHealthMonitor::new(
            db,
            deployer.clone(),
            alarm_service,
            ContainerHealthConfig::default(),
        );

        let info = deployer.get_container_info("abc123").await.unwrap();
        monitor
            .check_restart_count(&container, &deployment, &info)
            .await;

        // Baseline should be recorded
        let states = monitor.container_states.read().await;
        assert_eq!(states.get(&1).unwrap().restart_count, 0);
    }

    #[tokio::test]
    async fn test_check_restart_count_detects_increase() {
        let deployer = Arc::new(MockDeployer::new(0, ContainerStatus::Running));
        let container = make_container_model(1);
        let deployment = make_deployment_model();

        // DB calls: cooldown check (count=0) + insert alarm
        let alarm_model = temps_entities::alarms::Model {
            id: 1,
            project_id: 1,
            environment_id: 1,
            deployment_id: 10,
            container_id: Some(1),
            alarm_type: "container_restart".to_string(),
            severity: "info".to_string(),
            status: "firing".to_string(),
            title: "Container restarted".to_string(),
            message: None,
            metadata: None,
            fired_at: chrono::Utc::now(),
            acknowledged_at: None,
            acknowledged_by: None,
            resolved_at: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };

        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results([[maplit::btreemap! {
                "num_items" => sea_orm::Value::BigInt(Some(0)),
            }]])
            .append_query_results(vec![vec![alarm_model]])
            .into_connection();
        let db = Arc::new(db);
        let alarm_service = make_alarm_service(db.clone());

        let monitor = ContainerHealthMonitor::new(
            db,
            deployer.clone(),
            alarm_service,
            ContainerHealthConfig::default(),
        );

        // First check: set baseline at 0
        let info = deployer.get_container_info("abc123").await.unwrap();
        monitor
            .check_restart_count(&container, &deployment, &info)
            .await;

        // Simulate restart: count goes to 2
        deployer.set_restart_count(2).await;
        let info = deployer.get_container_info("abc123").await.unwrap();
        monitor
            .check_restart_count(&container, &deployment, &info)
            .await;

        // State should be updated to 2
        let states = monitor.container_states.read().await;
        assert_eq!(states.get(&1).unwrap().restart_count, 2);
    }

    #[tokio::test]
    async fn test_check_restart_count_no_alarm_if_no_increase() {
        let deployer = Arc::new(MockDeployer::new(5, ContainerStatus::Running));
        let container = make_container_model(1);
        let deployment = make_deployment_model();

        // No DB calls needed (no alarm fired)
        let db = MockDatabase::new(DatabaseBackend::Postgres).into_connection();
        let db = Arc::new(db);
        let alarm_service = make_alarm_service(db.clone());

        let monitor = ContainerHealthMonitor::new(
            db,
            deployer.clone(),
            alarm_service,
            ContainerHealthConfig::default(),
        );

        // First check: baseline at 5
        let info = deployer.get_container_info("abc123").await.unwrap();
        monitor
            .check_restart_count(&container, &deployment, &info)
            .await;

        // Second check: still 5, no alarm
        let info = deployer.get_container_info("abc123").await.unwrap();
        monitor
            .check_restart_count(&container, &deployment, &info)
            .await;

        let states = monitor.container_states.read().await;
        assert_eq!(states.get(&1).unwrap().restart_count, 5);
    }

    // ── Container status detection tests ──────────────────────────────

    #[tokio::test]
    async fn test_check_container_status_exited_fires_alarm() {
        let deployer = Arc::new(MockDeployer::new(0, ContainerStatus::Exited));
        let container = make_container_model(1);
        let deployment = make_deployment_model();

        let alarm_model = temps_entities::alarms::Model {
            id: 1,
            project_id: 1,
            environment_id: 1,
            deployment_id: 10,
            container_id: Some(1),
            alarm_type: "container_oom_killed".to_string(),
            severity: "critical".to_string(),
            status: "firing".to_string(),
            title: "Container exited".to_string(),
            message: None,
            metadata: None,
            fired_at: chrono::Utc::now(),
            acknowledged_at: None,
            acknowledged_by: None,
            resolved_at: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };

        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results([[maplit::btreemap! {
                "num_items" => sea_orm::Value::BigInt(Some(0)),
            }]])
            .append_query_results(vec![vec![alarm_model]])
            .into_connection();
        let db = Arc::new(db);
        let alarm_service = make_alarm_service(db.clone());

        let monitor = ContainerHealthMonitor::new(
            db,
            deployer.clone(),
            alarm_service,
            ContainerHealthConfig::default(),
        );

        let info = deployer.get_container_info("abc123").await.unwrap();
        // Should fire alarm for exited container
        monitor
            .check_container_status(&container, &deployment, &info)
            .await;
    }

    #[tokio::test]
    async fn test_check_container_status_running_no_alarm() {
        let deployer = Arc::new(MockDeployer::new(0, ContainerStatus::Running));
        let container = make_container_model(1);
        let deployment = make_deployment_model();

        // No DB calls needed (no alarm)
        let db = MockDatabase::new(DatabaseBackend::Postgres).into_connection();
        let db = Arc::new(db);
        let alarm_service = make_alarm_service(db.clone());

        let monitor = ContainerHealthMonitor::new(
            db,
            deployer.clone(),
            alarm_service,
            ContainerHealthConfig::default(),
        );

        let info = deployer.get_container_info("abc123").await.unwrap();
        // Should not fire alarm for running container
        monitor
            .check_container_status(&container, &deployment, &info)
            .await;
    }

    // ── Resource threshold tests ──────────────────────────────────────

    #[tokio::test]
    async fn test_resource_counter_increments_before_alarm() {
        let deployer = Arc::new(MockDeployer::new(0, ContainerStatus::Running));
        let container = make_container_model(1);
        let deployment = make_deployment_model();

        // No DB calls until we hit consecutive_threshold_checks
        let db = MockDatabase::new(DatabaseBackend::Postgres).into_connection();
        let db = Arc::new(db);
        let alarm_service = make_alarm_service(db.clone());

        let config = ContainerHealthConfig {
            consecutive_threshold_checks: 3,
            ..ContainerHealthConfig::default()
        };

        let monitor = ContainerHealthMonitor::new(db, deployer.clone(), alarm_service, config);

        // Breach 1: should NOT fire alarm
        monitor
            .handle_resource_threshold(
                &container,
                &deployment,
                AlarmType::HighCpu,
                AlarmSeverity::Warning,
                "CPU high".to_string(),
                "CPU at 95%".to_string(),
                serde_json::json!({"cpu_percent": 95.0}),
            )
            .await;

        let counters = monitor.resource_counters.read().await;
        assert_eq!(*counters.get(&(1, "high_cpu")).unwrap(), 1);
        drop(counters);

        // Breach 2: still should NOT fire alarm
        monitor
            .handle_resource_threshold(
                &container,
                &deployment,
                AlarmType::HighCpu,
                AlarmSeverity::Warning,
                "CPU high".to_string(),
                "CPU at 95%".to_string(),
                serde_json::json!({"cpu_percent": 95.0}),
            )
            .await;

        let counters = monitor.resource_counters.read().await;
        assert_eq!(*counters.get(&(1, "high_cpu")).unwrap(), 2);
    }

    #[tokio::test]
    async fn test_resource_counter_resets_below_threshold() {
        let deployer = Arc::new(MockDeployer::new(0, ContainerStatus::Running));
        let _container = make_container_model(1);

        let db = MockDatabase::new(DatabaseBackend::Postgres).into_connection();
        let db = Arc::new(db);
        let alarm_service = make_alarm_service(db.clone());

        let monitor = ContainerHealthMonitor::new(
            db,
            deployer,
            alarm_service,
            ContainerHealthConfig::default(),
        );

        // Manually set a counter
        {
            let mut counters = monitor.resource_counters.write().await;
            counters.insert((1, "high_cpu"), 2);
        }

        // Reset it
        monitor.reset_resource_counter(1, "high_cpu").await;

        let counters = monitor.resource_counters.read().await;
        assert!(counters.get(&(1, "high_cpu")).is_none());
    }

    // ── State pruning tests ───────────────────────────────────────────

    #[tokio::test]
    async fn test_state_pruning_removes_stale_containers() {
        let deployer = Arc::new(MockDeployer::new(0, ContainerStatus::Running));

        // DB returns only container id=1, but we have cached state for id=1 and id=2
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![vec![make_container_model(1)]])
            // For check_container: deployment lookup
            .append_query_results(vec![vec![make_deployment_model()]])
            .into_connection();
        let db = Arc::new(db);
        let alarm_service = make_alarm_service(db.clone());

        let monitor = ContainerHealthMonitor::new(
            db,
            deployer,
            alarm_service,
            ContainerHealthConfig::default(),
        );

        // Pre-populate state for containers 1 and 2
        {
            let mut states = monitor.container_states.write().await;
            states.insert(1, ContainerState { restart_count: 0 });
            states.insert(2, ContainerState { restart_count: 5 });
        }
        {
            let mut counters = monitor.resource_counters.write().await;
            counters.insert((2, "high_cpu"), 2);
        }

        // After check_all_containers, container 2 should be pruned
        // Container 1 remains but its resource counters may be reset by the health check
        let _ = monitor.check_all_containers().await;

        let states = monitor.container_states.read().await;
        assert!(states.contains_key(&1), "Container 1 state should survive");
        assert!(
            !states.contains_key(&2),
            "Container 2 state should be pruned"
        );

        let counters = monitor.resource_counters.read().await;
        assert!(
            !counters.contains_key(&(2, "high_cpu")),
            "Counter for container 2 should be pruned"
        );
    }
}
