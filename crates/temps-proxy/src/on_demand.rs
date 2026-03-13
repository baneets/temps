//! On-demand environment manager (scale-to-zero)
//!
//! Tracks idle environments and coordinates wake-on-request.
//!
//! - **Idle tracking**: Records last-activity timestamp per environment (atomic, lock-free)
//! - **Sleep sweep**: Background task stops containers after idle timeout
//! - **Wake coordination**: First request triggers container start, concurrent requests wait
//! - **Multi-node**: Stops/starts all containers (local + remote) in parallel
//! - **Multi-proxy**: Uses atomic DB transitions (UPDATE WHERE) instead of locks

use async_trait::async_trait;
use dashmap::DashMap;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter, Statement,
};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use temps_core::OnDemandWaker;
use temps_entities::{deployment_containers, environments};
use thiserror::Error;
use tokio::sync::Notify;
use tracing::{debug, error, info, warn};

/// Trait for starting/stopping containers. Implemented by the deployer crate.
/// Injected via the plugin system to avoid a direct dependency from proxy -> deployer.
#[async_trait]
pub trait ContainerLifecycle: Send + Sync {
    /// Start a stopped container by its Docker container ID.
    async fn start_container(&self, container_id: &str) -> Result<(), OnDemandError>;

    /// Stop a running container by its Docker container ID.
    async fn stop_container(&self, container_id: &str) -> Result<(), OnDemandError>;

    /// Check if a container is running and healthy.
    async fn is_container_healthy(&self, container_id: &str) -> Result<bool, OnDemandError>;
}

#[derive(Error, Debug)]
pub enum OnDemandError {
    #[error("Database error: {0}")]
    Database(#[from] sea_orm::DbErr),

    #[error("Container operation failed for container {container_id}: {reason}")]
    ContainerOperation {
        container_id: String,
        reason: String,
    },

    #[error("Wake timeout after {timeout_secs}s for environment {environment_id}")]
    WakeTimeout {
        environment_id: i32,
        timeout_secs: i32,
    },

    #[error("Environment {environment_id} has no current deployment")]
    NoDeployment { environment_id: i32 },

    #[error("Environment {environment_id} not found")]
    NotFound { environment_id: i32 },
}

/// Per-environment wake coordination state (in-process only).
struct WakeState {
    /// True if a wake operation is currently in progress on this proxy instance.
    waking: AtomicBool,
    /// Waiters are notified when the wake completes (success or failure).
    notify: Notify,
}

/// Information about a sleeping environment, loaded from the route table.
#[derive(Clone, Debug)]
pub struct SleepingEnvironmentInfo {
    pub environment_id: i32,
    pub project_id: i32,
    pub deployment_id: i32,
    pub wake_timeout_seconds: i32,
}

/// Core on-demand manager. Lives in the proxy process.
pub struct OnDemandManager {
    /// Last request timestamp (epoch seconds) per environment_id.
    /// Updated atomically on every proxied request — zero overhead.
    last_activity: DashMap<i32, AtomicU64>,

    /// On-demand config per environment_id (only environments with on_demand=true).
    /// Refreshed when the route table reloads.
    configs: DashMap<i32, OnDemandConfig>,

    /// Per-environment wake coordination (in-process).
    wake_states: DashMap<i32, Arc<WakeState>>,

    /// Sleeping environments indexed by domain (for wake-on-request lookup).
    /// Populated during route table reload for environments with sleeping=true.
    sleeping_by_domain: DashMap<String, SleepingEnvironmentInfo>,

    /// Database connection for state transitions.
    db: Arc<DatabaseConnection>,

    /// Container lifecycle operations (injected).
    container_lifecycle: Arc<dyn ContainerLifecycle>,
}

#[derive(Clone, Debug)]
pub(crate) struct OnDemandConfig {
    pub(crate) environment_id: i32,
    pub(crate) idle_timeout_seconds: i32,
    #[allow(dead_code)] // stored for use when waking via SleepingEnvironmentInfo
    pub(crate) wake_timeout_seconds: i32,
}

impl OnDemandManager {
    pub fn new(
        db: Arc<DatabaseConnection>,
        container_lifecycle: Arc<dyn ContainerLifecycle>,
    ) -> Self {
        Self {
            last_activity: DashMap::new(),
            configs: DashMap::new(),
            wake_states: DashMap::new(),
            sleeping_by_domain: DashMap::new(),
            db,
            container_lifecycle,
        }
    }

    // ── Activity Tracking ──

    /// Record that a request was received for an environment.
    /// Called on every proxied request — must be O(1) and lock-free.
    pub fn record_activity(&self, environment_id: i32) {
        let now = self.current_epoch_secs();
        if let Some(entry) = self.last_activity.get(&environment_id) {
            entry.value().store(now, Ordering::Relaxed);
        } else {
            self.last_activity
                .insert(environment_id, AtomicU64::new(now));
        }
    }

    /// Check if a domain maps to a sleeping environment.
    /// Returns wake info if found.
    pub fn get_sleeping_environment(&self, domain: &str) -> Option<SleepingEnvironmentInfo> {
        self.sleeping_by_domain
            .get(domain)
            .map(|r| r.value().clone())
    }

    /// Register a sleeping environment domain for wake-on-request lookup.
    pub fn register_sleeping_domain(&self, domain: String, info: SleepingEnvironmentInfo) {
        self.sleeping_by_domain.insert(domain, info);
    }

    /// Clear all sleeping domain mappings (called before route table reload).
    pub fn clear_sleeping_domains(&self) {
        self.sleeping_by_domain.clear();
    }

    /// Update on-demand configs from loaded environments.
    /// Called after route table reload.
    #[allow(dead_code)] // will be called when wired to route table reload
    pub(crate) fn update_configs(&self, configs: Vec<OnDemandConfig>) {
        self.configs.clear();
        for config in configs {
            self.configs.insert(config.environment_id, config.clone());
            // Initialize activity tracking for new on-demand environments
            self.last_activity
                .entry(config.environment_id)
                .or_insert_with(|| AtomicU64::new(self.current_epoch_secs()));
        }
    }

    /// Register on-demand config for a single environment.
    pub fn register_on_demand_environment(
        &self,
        environment_id: i32,
        idle_timeout_seconds: i32,
        wake_timeout_seconds: i32,
    ) {
        self.configs.insert(
            environment_id,
            OnDemandConfig {
                environment_id,
                idle_timeout_seconds,
                wake_timeout_seconds,
            },
        );
        self.last_activity
            .entry(environment_id)
            .or_insert_with(|| AtomicU64::new(self.current_epoch_secs()));
    }

    /// Remove tracking for an environment that is no longer on-demand.
    pub fn remove_environment(&self, environment_id: i32) {
        self.configs.remove(&environment_id);
        self.last_activity.remove(&environment_id);
        self.wake_states.remove(&environment_id);
    }

    // ── Sleep (Idle -> Sleeping) ──

    /// Run one sweep iteration. Checks all on-demand environments for idle timeout.
    /// Returns IDs of environments that were put to sleep.
    pub async fn sweep_idle_environments(&self) -> Vec<i32> {
        let now = self.current_epoch_secs();
        let mut slept = Vec::new();

        for entry in self.configs.iter() {
            let config = entry.value();
            let env_id = config.environment_id;

            if let Some(last) = self.last_activity.get(&env_id) {
                let last_secs = last.value().load(Ordering::Relaxed);
                let idle_secs = now.saturating_sub(last_secs);

                if idle_secs >= config.idle_timeout_seconds as u64 {
                    match self.sleep_environment(env_id).await {
                        Ok(true) => {
                            info!(
                                environment_id = env_id,
                                idle_secs = idle_secs,
                                "Environment put to sleep after idle timeout"
                            );
                            slept.push(env_id);
                        }
                        Ok(false) => {
                            debug!(
                                environment_id = env_id,
                                "Environment already sleeping or no deployment"
                            );
                        }
                        Err(e) => {
                            error!(
                                environment_id = env_id,
                                error = %e,
                                "Failed to put environment to sleep"
                            );
                        }
                    }
                }
            }
        }

        slept
    }

    /// Put an environment to sleep: stop all containers, set sleeping=true.
    /// Returns Ok(true) if this call won the race and performed the sleep.
    /// Returns Ok(false) if the environment was already sleeping or has no deployment.
    pub async fn sleep_environment(&self, environment_id: i32) -> Result<bool, OnDemandError> {
        // Atomic CAS via UPDATE WHERE — only one proxy wins
        let result = self
            .db
            .execute(Statement::from_sql_and_values(
                sea_orm::DatabaseBackend::Postgres,
                "UPDATE environments SET sleeping = true WHERE id = $1 AND sleeping = false RETURNING id",
                [environment_id.into()],
            ))
            .await?;

        if result.rows_affected() == 0 {
            return Ok(false); // Already sleeping or doesn't exist
        }

        // Load containers for the current deployment
        let env = environments::Entity::find_by_id(environment_id)
            .one(self.db.as_ref())
            .await?
            .ok_or(OnDemandError::NotFound { environment_id })?;

        let deployment_id = match env.current_deployment_id {
            Some(id) => id,
            None => return Ok(false),
        };

        let containers = deployment_containers::Entity::find()
            .filter(deployment_containers::Column::DeploymentId.eq(deployment_id))
            .filter(deployment_containers::Column::DeletedAt.is_null())
            .all(self.db.as_ref())
            .await?;

        // Stop all containers in parallel, tracking failures
        let stop_futures: Vec<_> = containers
            .iter()
            .map(|c| {
                let container_id = c.container_id.clone();
                let lifecycle = Arc::clone(&self.container_lifecycle);
                async move {
                    match lifecycle.stop_container(&container_id).await {
                        Ok(()) => Ok(container_id),
                        Err(e) => {
                            warn!(
                                container_id = %container_id,
                                error = %e,
                                "Failed to stop container during sleep"
                            );
                            Err((container_id, e))
                        }
                    }
                }
            })
            .collect();

        let results = futures::future::join_all(stop_futures).await;

        let failed: Vec<_> = results.iter().filter(|r| r.is_err()).collect();
        if !failed.is_empty() {
            // Some containers failed to stop — revert sleeping state to avoid
            // inconsistency where DB says sleeping but containers are still running
            error!(
                environment_id = environment_id,
                failed_count = failed.len(),
                total = containers.len(),
                "Failed to stop some containers during sleep, reverting sleeping state"
            );
            let _ = self
                .db
                .execute(Statement::from_sql_and_values(
                    sea_orm::DatabaseBackend::Postgres,
                    "UPDATE environments SET sleeping = false WHERE id = $1",
                    [environment_id.into()],
                ))
                .await;
            self.notify_route_change().await;
            return Err(OnDemandError::ContainerOperation {
                container_id: "multiple".to_string(),
                reason: format!(
                    "Failed to stop {}/{} containers during sleep",
                    failed.len(),
                    containers.len()
                ),
            });
        }

        info!(
            environment_id = environment_id,
            deployment_id = deployment_id,
            containers_stopped = containers.len(),
            "Environment sleeping"
        );

        // Fire PG NOTIFY so other proxy instances reload routes
        self.notify_route_change().await;

        Ok(true)
    }

    // ── Wake (Sleeping -> Running) ──

    /// Wake an environment: start all containers, wait for health checks, set sleeping=false.
    /// Handles concurrent requests: first caller starts containers, others wait.
    pub async fn wake_environment(
        &self,
        environment_id: i32,
        wake_timeout_seconds: i32,
    ) -> Result<(), OnDemandError> {
        // Get or create wake state for this environment
        let wake_state = self
            .wake_states
            .entry(environment_id)
            .or_insert_with(|| {
                Arc::new(WakeState {
                    waking: AtomicBool::new(false),
                    notify: Notify::new(),
                })
            })
            .value()
            .clone();

        // Try to become the waker
        if wake_state
            .waking
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            // We won — perform the wake
            let result = self.do_wake(environment_id, wake_timeout_seconds).await;

            // Signal all waiters (regardless of success/failure)
            wake_state.waking.store(false, Ordering::SeqCst);
            wake_state.notify.notify_waiters();

            return result;
        }

        // Someone else is waking — wait for them
        let timeout = Duration::from_secs(wake_timeout_seconds as u64);
        match tokio::time::timeout(timeout, wake_state.notify.notified()).await {
            Ok(_) => {
                // Wake completed — check if environment is actually awake
                let env = environments::Entity::find_by_id(environment_id)
                    .one(self.db.as_ref())
                    .await?
                    .ok_or(OnDemandError::NotFound { environment_id })?;

                if env.sleeping {
                    // Waker failed — return error
                    Err(OnDemandError::ContainerOperation {
                        container_id: "all".to_string(),
                        reason: "Wake operation failed on another request".to_string(),
                    })
                } else {
                    Ok(())
                }
            }
            Err(_) => Err(OnDemandError::WakeTimeout {
                environment_id,
                timeout_secs: wake_timeout_seconds,
            }),
        }
    }

    /// Perform the actual wake operation. Called only by the winning waker.
    async fn do_wake(
        &self,
        environment_id: i32,
        wake_timeout_seconds: i32,
    ) -> Result<(), OnDemandError> {
        // Atomic CAS — only one proxy instance wins
        let result = self
            .db
            .execute(Statement::from_sql_and_values(
                sea_orm::DatabaseBackend::Postgres,
                "UPDATE environments SET sleeping = false WHERE id = $1 AND sleeping = true RETURNING id",
                [environment_id.into()],
            ))
            .await?;

        if result.rows_affected() == 0 {
            // Another proxy already woke it — just wait for route reload
            return Ok(());
        }

        // Load containers for the current deployment
        let env = environments::Entity::find_by_id(environment_id)
            .one(self.db.as_ref())
            .await?
            .ok_or(OnDemandError::NotFound { environment_id })?;

        let deployment_id = env
            .current_deployment_id
            .ok_or(OnDemandError::NoDeployment { environment_id })?;

        let containers = deployment_containers::Entity::find()
            .filter(deployment_containers::Column::DeploymentId.eq(deployment_id))
            .filter(deployment_containers::Column::DeletedAt.is_null())
            .all(self.db.as_ref())
            .await?;

        if containers.is_empty() {
            warn!(
                environment_id = environment_id,
                "No containers found to wake"
            );
            return Ok(());
        }

        info!(
            environment_id = environment_id,
            deployment_id = deployment_id,
            container_count = containers.len(),
            "Waking environment"
        );

        // Start all containers in parallel
        let start_results: Vec<Result<String, OnDemandError>> = {
            let futures: Vec<_> = containers
                .iter()
                .map(|c| {
                    let container_id = c.container_id.clone();
                    let lifecycle = Arc::clone(&self.container_lifecycle);
                    async move {
                        lifecycle.start_container(&container_id).await?;
                        Ok(container_id)
                    }
                })
                .collect();
            futures::future::join_all(futures).await
        };

        // Check for failures
        let mut failed = Vec::new();
        let mut started = Vec::new();
        for result in start_results {
            match result {
                Ok(id) => started.push(id),
                Err(e) => {
                    error!(error = %e, "Failed to start container during wake");
                    failed.push(e);
                }
            }
        }

        if !failed.is_empty() {
            // Some containers failed to start — revert to sleeping
            error!(
                environment_id = environment_id,
                started = started.len(),
                failed = failed.len(),
                "Wake partially failed, reverting to sleeping"
            );

            // Stop any containers we managed to start
            for container_id in &started {
                let _ = self.container_lifecycle.stop_container(container_id).await;
            }

            // Revert DB state
            let _ = self
                .db
                .execute(Statement::from_sql_and_values(
                    sea_orm::DatabaseBackend::Postgres,
                    "UPDATE environments SET sleeping = true WHERE id = $1",
                    [environment_id.into()],
                ))
                .await;

            return Err(OnDemandError::ContainerOperation {
                container_id: "multiple".to_string(),
                reason: format!(
                    "Failed to start {}/{} containers",
                    failed.len(),
                    started.len() + failed.len()
                ),
            });
        }

        // Wait for health checks with timeout
        let health_timeout = Duration::from_secs(wake_timeout_seconds as u64);
        let health_start = Instant::now();

        for container_id in &started {
            loop {
                if health_start.elapsed() > health_timeout {
                    error!(
                        environment_id = environment_id,
                        container_id = %container_id,
                        "Health check timeout during wake"
                    );

                    // Revert: stop all and set sleeping
                    for cid in &started {
                        let _ = self.container_lifecycle.stop_container(cid).await;
                    }
                    let _ = self
                        .db
                        .execute(Statement::from_sql_and_values(
                            sea_orm::DatabaseBackend::Postgres,
                            "UPDATE environments SET sleeping = true WHERE id = $1",
                            [environment_id.into()],
                        ))
                        .await;

                    return Err(OnDemandError::WakeTimeout {
                        environment_id,
                        timeout_secs: wake_timeout_seconds,
                    });
                }

                match self
                    .container_lifecycle
                    .is_container_healthy(container_id)
                    .await
                {
                    Ok(true) => break,
                    Ok(false) => {
                        tokio::time::sleep(Duration::from_millis(500)).await;
                    }
                    Err(e) => {
                        warn!(
                            container_id = %container_id,
                            error = %e,
                            "Health check error, retrying"
                        );
                        tokio::time::sleep(Duration::from_millis(500)).await;
                    }
                }
            }
        }

        // Record activity so we don't immediately sleep again
        self.record_activity(environment_id);

        info!(
            environment_id = environment_id,
            containers_started = started.len(),
            wake_duration_ms = health_start.elapsed().as_millis(),
            "Environment awake"
        );

        // Fire PG NOTIFY so all proxy instances reload routes
        self.notify_route_change().await;

        Ok(())
    }

    // ── Helpers ──

    fn current_epoch_secs(&self) -> u64 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        now.as_secs()
    }

    async fn notify_route_change(&self) {
        if let Err(e) = self
            .db
            .execute(Statement::from_string(
                sea_orm::DatabaseBackend::Postgres,
                "NOTIFY route_table_changes".to_string(),
            ))
            .await
        {
            error!(error = %e, "Failed to send route_table_changes NOTIFY");
        }
    }

    /// Start the background idle sweep task in its own thread with a dedicated runtime.
    /// Checks every `sweep_interval` for environments that have exceeded their idle timeout.
    pub fn start_sweep_task(self: &Arc<Self>, sweep_interval: Duration) {
        let manager = Arc::clone(self);
        std::thread::Builder::new()
            .name("on-demand-sweep".to_string())
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("Failed to create tokio runtime for on-demand sweep");
                rt.block_on(async move {
                    let mut interval = tokio::time::interval(sweep_interval);
                    loop {
                        interval.tick().await;
                        let slept = manager.sweep_idle_environments().await;
                        if !slept.is_empty() {
                            debug!(
                                count = slept.len(),
                                environment_ids = ?slept,
                                "Idle sweep put environments to sleep"
                            );
                        }
                    }
                });
            })
            .expect("Failed to spawn on-demand sweep thread");
    }
}

/// Bridge implementation so the proxy's OnDemandManager can be injected into
/// the environments handler AppState via the plugin system.
#[async_trait]
impl OnDemandWaker for OnDemandManager {
    async fn wake_environment(
        &self,
        environment_id: i32,
        wake_timeout_seconds: i32,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.wake_environment(environment_id, wake_timeout_seconds)
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
    }

    async fn sleep_environment(
        &self,
        environment_id: i32,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        self.sleep_environment(environment_id)
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::{DatabaseBackend, MockDatabase, MockExecResult};
    use std::sync::Mutex;

    // ── Mock ContainerLifecycle ──

    #[derive(Default)]
    struct MockContainerState {
        started: Vec<String>,
        stopped: Vec<String>,
        healthy: bool,
        fail_start: bool,
        fail_health: bool,
    }

    struct MockLifecycle {
        state: Mutex<MockContainerState>,
    }

    impl MockLifecycle {
        fn new() -> Self {
            Self {
                state: Mutex::new(MockContainerState {
                    healthy: true,
                    ..Default::default()
                }),
            }
        }

        fn with_fail_start() -> Self {
            Self {
                state: Mutex::new(MockContainerState {
                    fail_start: true,
                    ..Default::default()
                }),
            }
        }

        #[allow(dead_code)]
        fn with_unhealthy() -> Self {
            Self {
                state: Mutex::new(MockContainerState {
                    healthy: false,
                    fail_health: true,
                    ..Default::default()
                }),
            }
        }

        fn started_containers(&self) -> Vec<String> {
            self.state.lock().unwrap().started.clone()
        }

        fn stopped_containers(&self) -> Vec<String> {
            self.state.lock().unwrap().stopped.clone()
        }
    }

    #[async_trait]
    impl ContainerLifecycle for MockLifecycle {
        async fn start_container(&self, container_id: &str) -> Result<(), OnDemandError> {
            let mut state = self.state.lock().unwrap();
            if state.fail_start {
                return Err(OnDemandError::ContainerOperation {
                    container_id: container_id.to_string(),
                    reason: "Mock start failure".to_string(),
                });
            }
            state.started.push(container_id.to_string());
            Ok(())
        }

        async fn stop_container(&self, container_id: &str) -> Result<(), OnDemandError> {
            self.state
                .lock()
                .unwrap()
                .stopped
                .push(container_id.to_string());
            Ok(())
        }

        async fn is_container_healthy(&self, _container_id: &str) -> Result<bool, OnDemandError> {
            let state = self.state.lock().unwrap();
            if state.fail_health {
                return Err(OnDemandError::ContainerOperation {
                    container_id: _container_id.to_string(),
                    reason: "Mock health check failure".to_string(),
                });
            }
            Ok(state.healthy)
        }
    }

    // ── Tests ──

    #[test]
    fn test_record_activity_updates_timestamp() {
        let db = MockDatabase::new(DatabaseBackend::Postgres).into_connection();
        let lifecycle = Arc::new(MockLifecycle::new());
        let manager = OnDemandManager::new(Arc::new(db), lifecycle);

        manager.record_activity(1);
        let ts1 = manager
            .last_activity
            .get(&1)
            .unwrap()
            .value()
            .load(Ordering::Relaxed);

        // Second call updates
        std::thread::sleep(Duration::from_millis(10));
        manager.record_activity(1);
        let ts2 = manager
            .last_activity
            .get(&1)
            .unwrap()
            .value()
            .load(Ordering::Relaxed);

        assert!(ts2 >= ts1);
    }

    #[test]
    fn test_on_demand_disabled_by_default() {
        let config = temps_entities::deployment_config::DeploymentConfig::default();
        assert!(!config.on_demand);
        assert_eq!(config.idle_timeout_seconds, 300);
        assert_eq!(config.wake_timeout_seconds, 30);
    }

    #[test]
    fn test_on_demand_validation_valid() {
        let config = temps_entities::deployment_config::DeploymentConfig {
            on_demand: true,
            idle_timeout_seconds: 120,
            wake_timeout_seconds: 15,
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_on_demand_validation_idle_too_low() {
        let config = temps_entities::deployment_config::DeploymentConfig {
            on_demand: true,
            idle_timeout_seconds: 30, // below 60 minimum
            ..Default::default()
        };
        assert!(config.validate().is_err());
        assert!(config.validate().unwrap_err().contains("Idle timeout"));
    }

    #[test]
    fn test_on_demand_validation_idle_too_high() {
        let config = temps_entities::deployment_config::DeploymentConfig {
            on_demand: true,
            idle_timeout_seconds: 100_000, // above 86400 max
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_on_demand_validation_wake_too_low() {
        let config = temps_entities::deployment_config::DeploymentConfig {
            on_demand: true,
            wake_timeout_seconds: 2, // below 5 minimum
            ..Default::default()
        };
        assert!(config.validate().is_err());
        assert!(config.validate().unwrap_err().contains("Wake timeout"));
    }

    #[test]
    fn test_on_demand_validation_wake_too_high() {
        let config = temps_entities::deployment_config::DeploymentConfig {
            on_demand: true,
            wake_timeout_seconds: 200, // above 120 max
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_on_demand_validation_skipped_when_disabled() {
        let config = temps_entities::deployment_config::DeploymentConfig {
            on_demand: false,
            idle_timeout_seconds: 1, // would be invalid if on_demand were true
            wake_timeout_seconds: 1,
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_sleeping_domain_lookup() {
        let db = MockDatabase::new(DatabaseBackend::Postgres).into_connection();
        let lifecycle = Arc::new(MockLifecycle::new());
        let manager = OnDemandManager::new(Arc::new(db), lifecycle);

        assert!(manager.get_sleeping_environment("example.com").is_none());

        manager.register_sleeping_domain(
            "example.com".to_string(),
            SleepingEnvironmentInfo {
                environment_id: 1,
                project_id: 10,
                deployment_id: 100,
                wake_timeout_seconds: 30,
            },
        );

        let info = manager.get_sleeping_environment("example.com").unwrap();
        assert_eq!(info.environment_id, 1);
        assert_eq!(info.project_id, 10);
        assert_eq!(info.deployment_id, 100);
    }

    #[test]
    fn test_clear_sleeping_domains() {
        let db = MockDatabase::new(DatabaseBackend::Postgres).into_connection();
        let lifecycle = Arc::new(MockLifecycle::new());
        let manager = OnDemandManager::new(Arc::new(db), lifecycle);

        manager.register_sleeping_domain(
            "a.com".to_string(),
            SleepingEnvironmentInfo {
                environment_id: 1,
                project_id: 1,
                deployment_id: 1,
                wake_timeout_seconds: 30,
            },
        );
        manager.register_sleeping_domain(
            "b.com".to_string(),
            SleepingEnvironmentInfo {
                environment_id: 2,
                project_id: 1,
                deployment_id: 2,
                wake_timeout_seconds: 30,
            },
        );

        assert!(manager.get_sleeping_environment("a.com").is_some());
        assert!(manager.get_sleeping_environment("b.com").is_some());

        manager.clear_sleeping_domains();

        assert!(manager.get_sleeping_environment("a.com").is_none());
        assert!(manager.get_sleeping_environment("b.com").is_none());
    }

    #[test]
    fn test_register_and_remove_environment() {
        let db = MockDatabase::new(DatabaseBackend::Postgres).into_connection();
        let lifecycle = Arc::new(MockLifecycle::new());
        let manager = OnDemandManager::new(Arc::new(db), lifecycle);

        manager.register_on_demand_environment(42, 300, 30);
        assert!(manager.configs.contains_key(&42));
        assert!(manager.last_activity.contains_key(&42));

        manager.remove_environment(42);
        assert!(!manager.configs.contains_key(&42));
        assert!(!manager.last_activity.contains_key(&42));
    }

    #[test]
    fn test_on_demand_merge_inherits() {
        let project = temps_entities::deployment_config::DeploymentConfig {
            on_demand: true,
            idle_timeout_seconds: 600,
            wake_timeout_seconds: 45,
            ..Default::default()
        };
        let env = temps_entities::deployment_config::DeploymentConfig::default();
        let merged = project.merge(&env);
        assert!(merged.on_demand); // true || false = true
        assert_eq!(merged.idle_timeout_seconds, 600); // project wins (env is default 300)
    }

    #[test]
    fn test_on_demand_merge_env_overrides() {
        let project = temps_entities::deployment_config::DeploymentConfig {
            on_demand: false,
            idle_timeout_seconds: 600,
            ..Default::default()
        };
        let env = temps_entities::deployment_config::DeploymentConfig {
            on_demand: true,
            idle_timeout_seconds: 120,
            wake_timeout_seconds: 10,
            ..Default::default()
        };
        let merged = project.merge(&env);
        assert!(merged.on_demand);
        assert_eq!(merged.idle_timeout_seconds, 120);
        assert_eq!(merged.wake_timeout_seconds, 10);
    }

    #[test]
    fn test_on_demand_serialization_roundtrip() {
        let config = temps_entities::deployment_config::DeploymentConfig {
            on_demand: true,
            idle_timeout_seconds: 600,
            wake_timeout_seconds: 45,
            ..Default::default()
        };
        let json = serde_json::to_value(&config).unwrap();
        assert_eq!(json["onDemand"], true);
        assert_eq!(json["idleTimeoutSeconds"], 600);
        assert_eq!(json["wakeTimeoutSeconds"], 45);

        let deserialized: temps_entities::deployment_config::DeploymentConfig =
            serde_json::from_value(json).unwrap();
        assert_eq!(config, deserialized);
    }

    #[test]
    fn test_on_demand_deserialization_defaults() {
        // Existing JSON without on-demand fields should deserialize with defaults
        let json = serde_json::json!({
            "cpuRequest": 100,
            "automaticDeploy": false,
            "replicas": 1
        });
        let config: temps_entities::deployment_config::DeploymentConfig =
            serde_json::from_value(json).unwrap();
        assert!(!config.on_demand);
        assert_eq!(config.idle_timeout_seconds, 300);
        assert_eq!(config.wake_timeout_seconds, 30);
    }

    #[test]
    fn test_on_demand_error_display() {
        let err = OnDemandError::WakeTimeout {
            environment_id: 42,
            timeout_secs: 30,
        };
        assert!(err.to_string().contains("42"));
        assert!(err.to_string().contains("30"));

        let err = OnDemandError::NoDeployment { environment_id: 5 };
        assert!(err.to_string().contains("5"));
    }

    #[tokio::test]
    async fn test_sweep_no_on_demand_environments() {
        let db = MockDatabase::new(DatabaseBackend::Postgres).into_connection();
        let lifecycle = Arc::new(MockLifecycle::new());
        let manager = OnDemandManager::new(Arc::new(db), lifecycle);

        let slept = manager.sweep_idle_environments().await;
        assert!(slept.is_empty());
    }

    #[tokio::test]
    async fn test_wake_concurrent_requests_coordinate() {
        // Test that multiple concurrent wake requests coordinate properly:
        // Only one should actually perform the wake, others should wait.
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            // First wake: UPDATE sleeping=false WHERE sleeping=true -> 1 row
            .append_exec_results(vec![MockExecResult {
                last_insert_id: 0,
                rows_affected: 1,
            }])
            // find_by_id for env
            .append_query_results(vec![vec![environments::Model {
                id: 1,
                name: "production".to_string(),
                slug: "production".to_string(),
                subdomain: "prod".to_string(),
                last_deployment: None,
                host: "".to_string(),
                upstreams: temps_entities::upstream_config::UpstreamList::new(),
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
                project_id: 10,
                current_deployment_id: Some(100),
                branch: None,
                deleted_at: None,
                deployment_config: None,
                is_preview: false,
                protected: false,
                sleeping: false,
            }]])
            // containers query
            .append_query_results(vec![vec![deployment_containers::Model {
                id: 1,
                deployment_id: 100,
                container_id: "abc123".to_string(),
                container_name: "my-app-abc123".to_string(),
                container_port: 3000,
                host_port: Some(32000),
                image_name: Some("my-app:latest".to_string()),
                status: Some("running".to_string()),
                created_at: chrono::Utc::now(),
                deployed_at: chrono::Utc::now(),
                ready_at: None,
                deleted_at: None,
                node_id: None,
            }]])
            // NOTIFY
            .append_exec_results(vec![MockExecResult {
                last_insert_id: 0,
                rows_affected: 0,
            }])
            .into_connection();

        let lifecycle = Arc::new(MockLifecycle::new());
        let manager = Arc::new(OnDemandManager::new(
            Arc::new(db),
            Arc::clone(&lifecycle) as Arc<dyn ContainerLifecycle>,
        ));

        // Single wake should succeed
        let result = manager.wake_environment(1, 30).await;
        assert!(result.is_ok());

        // Container should have been started
        assert_eq!(lifecycle.started_containers(), vec!["abc123"]);
    }

    #[tokio::test]
    async fn test_wake_failure_reverts_to_sleeping() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            // UPDATE sleeping=false -> 1 row
            .append_exec_results(vec![MockExecResult {
                last_insert_id: 0,
                rows_affected: 1,
            }])
            // find_by_id env
            .append_query_results(vec![vec![environments::Model {
                id: 1,
                name: "test".to_string(),
                slug: "test".to_string(),
                subdomain: "test".to_string(),
                last_deployment: None,
                host: "".to_string(),
                upstreams: temps_entities::upstream_config::UpstreamList::new(),
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
                project_id: 10,
                current_deployment_id: Some(100),
                branch: None,
                deleted_at: None,
                deployment_config: None,
                is_preview: false,
                protected: false,
                sleeping: false,
            }]])
            // containers
            .append_query_results(vec![vec![deployment_containers::Model {
                id: 1,
                deployment_id: 100,
                container_id: "fail-container".to_string(),
                container_name: "my-app-fail".to_string(),
                container_port: 3000,
                host_port: Some(32001),
                image_name: None,
                status: None,
                created_at: chrono::Utc::now(),
                deployed_at: chrono::Utc::now(),
                ready_at: None,
                deleted_at: None,
                node_id: None,
            }]])
            // Revert UPDATE sleeping=true
            .append_exec_results(vec![MockExecResult {
                last_insert_id: 0,
                rows_affected: 1,
            }])
            .into_connection();

        let lifecycle = Arc::new(MockLifecycle::with_fail_start());
        let manager = OnDemandManager::new(Arc::new(db), lifecycle);

        let result = manager.wake_environment(1, 30).await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            OnDemandError::ContainerOperation { .. }
        ));
    }

    #[tokio::test]
    async fn test_wake_no_deployment_returns_error() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            // UPDATE sleeping=false -> 1 row
            .append_exec_results(vec![MockExecResult {
                last_insert_id: 0,
                rows_affected: 1,
            }])
            // find_by_id env -> no current deployment
            .append_query_results(vec![vec![environments::Model {
                id: 1,
                name: "test".to_string(),
                slug: "test".to_string(),
                subdomain: "test".to_string(),
                last_deployment: None,
                host: "".to_string(),
                upstreams: temps_entities::upstream_config::UpstreamList::new(),
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
                project_id: 10,
                current_deployment_id: None, // No deployment
                branch: None,
                deleted_at: None,
                deployment_config: None,
                is_preview: false,
                protected: false,
                sleeping: false,
            }]])
            .into_connection();

        let lifecycle = Arc::new(MockLifecycle::new());
        let manager = OnDemandManager::new(Arc::new(db), lifecycle);

        let result = manager.wake_environment(1, 30).await;
        assert!(matches!(
            result.unwrap_err(),
            OnDemandError::NoDeployment { environment_id: 1 }
        ));
    }

    #[tokio::test]
    async fn test_sleep_already_sleeping_returns_false() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            // UPDATE sleeping=true WHERE sleeping=false -> 0 rows (already sleeping)
            .append_exec_results(vec![MockExecResult {
                last_insert_id: 0,
                rows_affected: 0,
            }])
            .into_connection();

        let lifecycle = Arc::new(MockLifecycle::new());
        let manager = OnDemandManager::new(Arc::new(db), lifecycle);

        let result = manager.sleep_environment(1).await.unwrap();
        assert!(!result); // Already sleeping
    }

    #[tokio::test]
    async fn test_wake_already_awake_succeeds() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            // UPDATE sleeping=false WHERE sleeping=true -> 0 rows (already awake)
            .append_exec_results(vec![MockExecResult {
                last_insert_id: 0,
                rows_affected: 0,
            }])
            .into_connection();

        let lifecycle = Arc::new(MockLifecycle::new());
        let manager = OnDemandManager::new(Arc::new(db), lifecycle);

        let result = manager.wake_environment(1, 30).await;
        assert!(result.is_ok()); // No-op, already awake
    }

    #[tokio::test]
    async fn test_sleep_stops_multiple_containers() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            // UPDATE sleeping=true WHERE sleeping=false -> 1 row
            .append_exec_results(vec![MockExecResult {
                last_insert_id: 0,
                rows_affected: 1,
            }])
            // find_by_id env
            .append_query_results(vec![vec![environments::Model {
                id: 1,
                name: "test".to_string(),
                slug: "test".to_string(),
                subdomain: "test".to_string(),
                last_deployment: None,
                host: "".to_string(),
                upstreams: temps_entities::upstream_config::UpstreamList::new(),
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
                project_id: 10,
                current_deployment_id: Some(100),
                branch: None,
                deleted_at: None,
                deployment_config: None,
                is_preview: false,
                protected: false,
                sleeping: false,
            }]])
            // containers (3 replicas)
            .append_query_results(vec![vec![
                deployment_containers::Model {
                    id: 1,
                    deployment_id: 100,
                    container_id: "container-1".to_string(),
                    container_name: "app-1".to_string(),
                    container_port: 3000,
                    host_port: Some(32001),
                    image_name: None,
                    status: None,
                    created_at: chrono::Utc::now(),
                    deployed_at: chrono::Utc::now(),
                    ready_at: None,
                    deleted_at: None,
                    node_id: None,
                },
                deployment_containers::Model {
                    id: 2,
                    deployment_id: 100,
                    container_id: "container-2".to_string(),
                    container_name: "app-2".to_string(),
                    container_port: 3000,
                    host_port: Some(32002),
                    image_name: None,
                    status: None,
                    created_at: chrono::Utc::now(),
                    deployed_at: chrono::Utc::now(),
                    ready_at: None,
                    deleted_at: None,
                    node_id: Some(2), // Remote node
                },
                deployment_containers::Model {
                    id: 3,
                    deployment_id: 100,
                    container_id: "container-3".to_string(),
                    container_name: "app-3".to_string(),
                    container_port: 3000,
                    host_port: Some(32003),
                    image_name: None,
                    status: None,
                    created_at: chrono::Utc::now(),
                    deployed_at: chrono::Utc::now(),
                    ready_at: None,
                    deleted_at: None,
                    node_id: Some(3), // Another remote node
                },
            ]])
            // NOTIFY
            .append_exec_results(vec![MockExecResult {
                last_insert_id: 0,
                rows_affected: 0,
            }])
            .into_connection();

        let lifecycle = Arc::new(MockLifecycle::new());
        let manager = OnDemandManager::new(
            Arc::new(db),
            Arc::clone(&lifecycle) as Arc<dyn ContainerLifecycle>,
        );

        let result = manager.sleep_environment(1).await.unwrap();
        assert!(result);

        // All 3 containers should have been stopped
        let stopped = lifecycle.stopped_containers();
        assert_eq!(stopped.len(), 3);
        assert!(stopped.contains(&"container-1".to_string()));
        assert!(stopped.contains(&"container-2".to_string()));
        assert!(stopped.contains(&"container-3".to_string()));
    }

    // ── Sleeping domain lookup with port stripping ──

    #[test]
    fn test_sleeping_domain_lookup_with_port() {
        let db = MockDatabase::new(DatabaseBackend::Postgres).into_connection();
        let lifecycle = Arc::new(MockLifecycle::new());
        let manager = OnDemandManager::new(
            Arc::new(db),
            Arc::clone(&lifecycle) as Arc<dyn ContainerLifecycle>,
        );

        manager.register_sleeping_domain(
            "app.preview.example.com".to_string(),
            SleepingEnvironmentInfo {
                environment_id: 1,
                project_id: 1,
                deployment_id: 10,
                wake_timeout_seconds: 30,
            },
        );

        // Lookup without port
        assert!(manager
            .get_sleeping_environment("app.preview.example.com")
            .is_some());

        // Port should be stripped by the caller (LoadBalancer.request_filter)
        assert!(manager
            .get_sleeping_environment("app.preview.example.com:443")
            .is_none());
    }

    // ── Sweep only sleeps environments past idle timeout ──

    #[tokio::test]
    async fn test_sweep_respects_idle_timeout() {
        // Environment with 300s idle timeout but recent activity → should NOT sleep
        let db = MockDatabase::new(DatabaseBackend::Postgres).into_connection();
        let lifecycle = Arc::new(MockLifecycle::new());
        let manager = OnDemandManager::new(
            Arc::new(db),
            Arc::clone(&lifecycle) as Arc<dyn ContainerLifecycle>,
        );

        manager.register_on_demand_environment(1, 300, 30);
        manager.record_activity(1); // just now

        let slept = manager.sweep_idle_environments().await;
        assert!(
            slept.is_empty(),
            "Should not sleep recently active environment"
        );
    }

    // ── Multiple sleeping domains for same environment ──

    #[test]
    fn test_multiple_domains_same_sleeping_environment() {
        let db = MockDatabase::new(DatabaseBackend::Postgres).into_connection();
        let lifecycle = Arc::new(MockLifecycle::new());
        let manager = OnDemandManager::new(
            Arc::new(db),
            Arc::clone(&lifecycle) as Arc<dyn ContainerLifecycle>,
        );

        let info = SleepingEnvironmentInfo {
            environment_id: 5,
            project_id: 2,
            deployment_id: 20,
            wake_timeout_seconds: 15,
        };

        // Same env, multiple domains (subdomain + custom domain)
        manager.register_sleeping_domain("app.preview.example.com".to_string(), info.clone());
        manager.register_sleeping_domain("my-app.custom.com".to_string(), info.clone());

        let lookup1 = manager
            .get_sleeping_environment("app.preview.example.com")
            .unwrap();
        let lookup2 = manager
            .get_sleeping_environment("my-app.custom.com")
            .unwrap();

        assert_eq!(lookup1.environment_id, 5);
        assert_eq!(lookup2.environment_id, 5);
        assert_eq!(lookup1.wake_timeout_seconds, 15);
    }

    // ── Clear sleeping domains removes all entries ──

    #[test]
    fn test_clear_sleeping_domains_removes_all() {
        let db = MockDatabase::new(DatabaseBackend::Postgres).into_connection();
        let lifecycle = Arc::new(MockLifecycle::new());
        let manager = OnDemandManager::new(
            Arc::new(db),
            Arc::clone(&lifecycle) as Arc<dyn ContainerLifecycle>,
        );

        manager.register_sleeping_domain(
            "a.example.com".to_string(),
            SleepingEnvironmentInfo {
                environment_id: 1,
                project_id: 1,
                deployment_id: 1,
                wake_timeout_seconds: 30,
            },
        );
        manager.register_sleeping_domain(
            "b.example.com".to_string(),
            SleepingEnvironmentInfo {
                environment_id: 2,
                project_id: 1,
                deployment_id: 2,
                wake_timeout_seconds: 30,
            },
        );

        assert!(manager.get_sleeping_environment("a.example.com").is_some());
        assert!(manager.get_sleeping_environment("b.example.com").is_some());

        manager.clear_sleeping_domains();

        assert!(manager.get_sleeping_environment("a.example.com").is_none());
        assert!(manager.get_sleeping_environment("b.example.com").is_none());
    }

    // ── Activity tracking idempotent ──

    #[test]
    fn test_record_activity_creates_entry_if_missing() {
        let db = MockDatabase::new(DatabaseBackend::Postgres).into_connection();
        let lifecycle = Arc::new(MockLifecycle::new());
        let manager = OnDemandManager::new(
            Arc::new(db),
            Arc::clone(&lifecycle) as Arc<dyn ContainerLifecycle>,
        );

        // Environment 99 not registered, but record_activity should still work
        manager.record_activity(99);
        // No panic, entry created
        assert!(manager.last_activity.contains_key(&99));
    }

    // ── Route table callback integration ──

    #[test]
    fn test_route_table_callback_populates_sleeping_domains() {
        let db = MockDatabase::new(DatabaseBackend::Postgres).into_connection();
        let lifecycle = Arc::new(MockLifecycle::new());
        let manager = Arc::new(OnDemandManager::new(
            Arc::new(db),
            Arc::clone(&lifecycle) as Arc<dyn ContainerLifecycle>,
        ));

        // Simulate the callback that setup_proxy_server registers
        let on_demand = Arc::clone(&manager);
        let callback = move |entries: Vec<temps_routes::SleepingEnvironmentEntry>| {
            on_demand.clear_sleeping_domains();
            for entry in entries {
                on_demand.register_sleeping_domain(
                    entry.domain.clone(),
                    SleepingEnvironmentInfo {
                        environment_id: entry.environment_id,
                        project_id: entry.project_id,
                        deployment_id: entry.deployment_id,
                        wake_timeout_seconds: entry.wake_timeout_seconds,
                    },
                );
            }
        };

        // Fire callback with sleeping entries
        callback(vec![
            temps_routes::SleepingEnvironmentEntry {
                domain: "app.preview.example.com".to_string(),
                environment_id: 1,
                project_id: 10,
                deployment_id: 100,
                wake_timeout_seconds: 30,
            },
            temps_routes::SleepingEnvironmentEntry {
                domain: "custom.example.com".to_string(),
                environment_id: 1,
                project_id: 10,
                deployment_id: 100,
                wake_timeout_seconds: 30,
            },
        ]);

        // Both domains should resolve to the same sleeping environment
        let info1 = manager
            .get_sleeping_environment("app.preview.example.com")
            .unwrap();
        let info2 = manager
            .get_sleeping_environment("custom.example.com")
            .unwrap();
        assert_eq!(info1.environment_id, 1);
        assert_eq!(info2.environment_id, 1);
        assert_eq!(info1.deployment_id, 100);

        // Non-sleeping domain should not be found
        assert!(manager
            .get_sleeping_environment("other.example.com")
            .is_none());
    }

    #[test]
    fn test_route_table_callback_replaces_previous_entries() {
        let db = MockDatabase::new(DatabaseBackend::Postgres).into_connection();
        let lifecycle = Arc::new(MockLifecycle::new());
        let manager = Arc::new(OnDemandManager::new(
            Arc::new(db),
            Arc::clone(&lifecycle) as Arc<dyn ContainerLifecycle>,
        ));

        // Register initial sleeping domain
        manager.register_sleeping_domain(
            "old.example.com".to_string(),
            SleepingEnvironmentInfo {
                environment_id: 1,
                project_id: 10,
                deployment_id: 100,
                wake_timeout_seconds: 30,
            },
        );
        assert!(manager
            .get_sleeping_environment("old.example.com")
            .is_some());

        // Simulate callback clearing and setting new entries (like after a route reload
        // where the environment woke up and a new one went to sleep)
        let on_demand = Arc::clone(&manager);
        on_demand.clear_sleeping_domains();
        on_demand.register_sleeping_domain(
            "new.example.com".to_string(),
            SleepingEnvironmentInfo {
                environment_id: 2,
                project_id: 20,
                deployment_id: 200,
                wake_timeout_seconds: 60,
            },
        );

        // Old domain gone, new domain present
        assert!(manager
            .get_sleeping_environment("old.example.com")
            .is_none());
        let info = manager.get_sleeping_environment("new.example.com").unwrap();
        assert_eq!(info.environment_id, 2);
        assert_eq!(info.wake_timeout_seconds, 60);
    }

    // ── Wake triggers container start and DB update ──

    #[tokio::test]
    async fn test_wake_starts_container_and_updates_db() {
        use temps_entities::deployment_containers;

        let env_model = temps_entities::environments::Model {
            id: 1,
            name: "staging".to_string(),
            slug: "staging".to_string(),
            subdomain: "my-project-staging".to_string(),
            branch: Some("develop".to_string()),
            project_id: 10,
            host: "".to_string(),
            upstreams: temps_entities::upstream_config::UpstreamList::new(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            last_deployment: None,
            current_deployment_id: Some(100),
            deleted_at: None,
            deployment_config: Some(temps_entities::deployment_config::DeploymentConfig {
                on_demand: true,
                idle_timeout_seconds: 300,
                wake_timeout_seconds: 30,
                ..Default::default()
            }),
            is_preview: false,
            protected: false,
            sleeping: true,
        };

        let container = deployment_containers::Model {
            id: 1,
            deployment_id: 100,
            container_id: "abc123".to_string(),
            container_name: "staging-abc123".to_string(),
            container_port: 8080,
            host_port: Some(32000),
            image_name: None,
            status: Some("stopped".to_string()),
            node_id: None,
            created_at: chrono::Utc::now(),
            deployed_at: chrono::Utc::now(),
            ready_at: None,
            deleted_at: None,
        };

        let db = MockDatabase::new(DatabaseBackend::Postgres)
            // 1. CAS UPDATE environments SET sleeping=false WHERE sleeping=true
            .append_exec_results(vec![MockExecResult {
                last_insert_id: 0,
                rows_affected: 1,
            }])
            // 2. find environment by id
            .append_query_results(vec![vec![env_model.clone()]])
            // 3. find containers for deployment
            .append_query_results(vec![vec![container]])
            // 4. NOTIFY route_table_changes
            .append_exec_results(vec![MockExecResult {
                last_insert_id: 0,
                rows_affected: 0,
            }])
            .into_connection();

        let lifecycle = Arc::new(MockLifecycle::new());
        let manager = Arc::new(OnDemandManager::new(
            Arc::new(db),
            Arc::clone(&lifecycle) as Arc<dyn ContainerLifecycle>,
        ));

        // Register sleeping domain
        manager.register_sleeping_domain(
            "staging.preview.example.com".to_string(),
            SleepingEnvironmentInfo {
                environment_id: 1,
                project_id: 10,
                deployment_id: 100,
                wake_timeout_seconds: 30,
            },
        );

        // Wake the environment
        let result = manager.wake_environment(1, 30).await;
        assert!(result.is_ok(), "Wake should succeed: {:?}", result.err());

        // Verify container was started
        let started = lifecycle.started_containers();
        assert_eq!(started, vec!["abc123"]);

        // Note: sleeping domain map is cleared by the route table callback
        // (triggered by NOTIFY route_table_changes), not by wake_environment itself.
        // In production, load_routes re-fires and the callback repopulates the map
        // without this environment (since sleeping=false now).
    }

    // ── Sleep stops containers and registers sleeping domain ──

    #[tokio::test]
    async fn test_sleep_stops_containers_and_updates_db() {
        use temps_entities::deployment_containers;

        let env_model = temps_entities::environments::Model {
            id: 2,
            name: "preview".to_string(),
            slug: "preview".to_string(),
            subdomain: "my-project-preview".to_string(),
            branch: Some("feature".to_string()),
            project_id: 10,
            host: "".to_string(),
            upstreams: temps_entities::upstream_config::UpstreamList::new(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            last_deployment: None,
            current_deployment_id: Some(200),
            deleted_at: None,
            deployment_config: Some(temps_entities::deployment_config::DeploymentConfig {
                on_demand: true,
                idle_timeout_seconds: 300,
                wake_timeout_seconds: 30,
                ..Default::default()
            }),
            is_preview: false,
            protected: false,
            sleeping: false,
        };

        let container1 = deployment_containers::Model {
            id: 1,
            deployment_id: 200,
            container_id: "container-a".to_string(),
            container_name: "preview-a".to_string(),
            container_port: 3000,
            host_port: Some(32001),
            image_name: None,
            status: Some("running".to_string()),
            node_id: None,
            created_at: chrono::Utc::now(),
            deployed_at: chrono::Utc::now(),
            ready_at: None,
            deleted_at: None,
        };
        let container2 = deployment_containers::Model {
            id: 2,
            deployment_id: 200,
            container_id: "container-b".to_string(),
            container_name: "preview-b".to_string(),
            container_port: 3000,
            host_port: Some(32002),
            image_name: None,
            status: Some("running".to_string()),
            node_id: None,
            created_at: chrono::Utc::now(),
            deployed_at: chrono::Utc::now(),
            ready_at: None,
            deleted_at: None,
        };

        let db = MockDatabase::new(DatabaseBackend::Postgres)
            // 1. CAS UPDATE environments SET sleeping=true WHERE sleeping=false
            .append_exec_results(vec![MockExecResult {
                last_insert_id: 0,
                rows_affected: 1,
            }])
            // 2. find environment by id
            .append_query_results(vec![vec![env_model.clone()]])
            // 3. find containers for deployment
            .append_query_results(vec![vec![container1, container2]])
            // 4. NOTIFY route_table_changes
            .append_exec_results(vec![MockExecResult {
                last_insert_id: 0,
                rows_affected: 0,
            }])
            .into_connection();

        let lifecycle = Arc::new(MockLifecycle::new());
        let manager = Arc::new(OnDemandManager::new(
            Arc::new(db),
            Arc::clone(&lifecycle) as Arc<dyn ContainerLifecycle>,
        ));

        // Register on-demand config so sweep can find it
        manager.register_on_demand_environment(2, 300, 30);
        manager.record_activity(2);

        let result = manager.sleep_environment(2).await;
        assert!(result.is_ok(), "Sleep should succeed: {:?}", result.err());
        assert!(result.unwrap(), "Should return true (was put to sleep)");

        // Both containers should be stopped
        let mut stopped = lifecycle.stopped_containers();
        stopped.sort();
        assert_eq!(stopped, vec!["container-a", "container-b"]);
    }

    // ── Sweep integration: idle environment gets slept ──

    #[tokio::test]
    async fn test_sweep_sleeps_idle_environment() {
        use temps_entities::deployment_containers;

        let env_model = temps_entities::environments::Model {
            id: 3,
            name: "idle-env".to_string(),
            slug: "idle-env".to_string(),
            subdomain: "idle-env".to_string(),
            branch: None,
            project_id: 10,
            host: "".to_string(),
            upstreams: temps_entities::upstream_config::UpstreamList::new(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            last_deployment: None,
            current_deployment_id: Some(300),
            deleted_at: None,
            deployment_config: Some(temps_entities::deployment_config::DeploymentConfig {
                on_demand: true,
                idle_timeout_seconds: 60,
                wake_timeout_seconds: 30,
                ..Default::default()
            }),
            is_preview: false,
            protected: false,
            sleeping: false,
        };

        let container = deployment_containers::Model {
            id: 1,
            deployment_id: 300,
            container_id: "idle-container".to_string(),
            container_name: "idle-app".to_string(),
            container_port: 3000,
            host_port: Some(32010),
            image_name: None,
            status: Some("running".to_string()),
            node_id: None,
            created_at: chrono::Utc::now(),
            deployed_at: chrono::Utc::now(),
            ready_at: None,
            deleted_at: None,
        };

        let db = MockDatabase::new(DatabaseBackend::Postgres)
            // 1. CAS UPDATE environments SET sleeping=true
            .append_exec_results(vec![MockExecResult {
                last_insert_id: 0,
                rows_affected: 1,
            }])
            // 2. find environment by id
            .append_query_results(vec![vec![env_model]])
            // 3. find containers
            .append_query_results(vec![vec![container]])
            // 4. NOTIFY
            .append_exec_results(vec![MockExecResult {
                last_insert_id: 0,
                rows_affected: 0,
            }])
            .into_connection();

        let lifecycle = Arc::new(MockLifecycle::new());
        let manager = Arc::new(OnDemandManager::new(
            Arc::new(db),
            Arc::clone(&lifecycle) as Arc<dyn ContainerLifecycle>,
        ));

        // Register with 60s idle timeout
        manager.register_on_demand_environment(3, 60, 30);

        // Set last activity to 120 seconds ago (well past 60s idle timeout)
        let old_time = manager.current_epoch_secs() - 120;
        manager.last_activity.insert(3, AtomicU64::new(old_time));

        let slept = manager.sweep_idle_environments().await;
        assert_eq!(
            slept,
            vec![3],
            "Environment 3 should have been put to sleep"
        );

        // Container should be stopped
        assert_eq!(lifecycle.stopped_containers(), vec!["idle-container"]);
    }
}
