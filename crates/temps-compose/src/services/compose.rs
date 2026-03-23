use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter,
    QueryOrder, Set,
};
use std::path::PathBuf;
use std::sync::Arc;
use temps_entities::{compose_stack_routes, compose_stacks};
use thiserror::Error;
use tracing::{debug, error};

use super::executor::{ComposeExecutor, ContainerMetrics, ExecutorError};
use super::port_validator;
use super::repo_sync::{self, RepoSyncError};

#[derive(Error, Debug)]
pub enum ComposeError {
    #[error("Database error: {0}")]
    Database(sea_orm::DbErr),

    #[error("Stack {stack_id} not found")]
    NotFound { stack_id: i32 },

    #[error("Validation error: {message}")]
    Validation { message: String },

    #[error("Docker Compose error for stack {stack_id}: {reason}")]
    Docker { stack_id: i32, reason: String },

    #[error("Stack '{name}' is currently {state}, cannot perform {operation}")]
    InvalidState {
        name: String,
        state: String,
        operation: String,
    },

    #[error("Repository sync failed for stack {stack_id}: {reason}")]
    RepoSync { stack_id: i32, reason: String },

    #[error("Port conflict: {message}")]
    PortConflict { message: String },
}

impl From<ExecutorError> for ComposeError {
    fn from(error: ExecutorError) -> Self {
        match &error {
            ExecutorError::CommandFailed { stack_id, .. } => ComposeError::Docker {
                stack_id: *stack_id,
                reason: error.to_string(),
            },
            ExecutorError::FileWrite { stack_id, .. } => ComposeError::Docker {
                stack_id: *stack_id,
                reason: error.to_string(),
            },
            ExecutorError::DockerComposeNotAvailable { .. } => ComposeError::Docker {
                stack_id: 0,
                reason: error.to_string(),
            },
        }
    }
}

impl From<RepoSyncError> for ComposeError {
    fn from(error: RepoSyncError) -> Self {
        ComposeError::RepoSync {
            stack_id: 0,
            reason: error.to_string(),
        }
    }
}

impl From<sea_orm::DbErr> for ComposeError {
    fn from(error: sea_orm::DbErr) -> Self {
        match error {
            sea_orm::DbErr::RecordNotFound(msg) => ComposeError::NotFound {
                stack_id: msg.parse().unwrap_or(0),
            },
            sea_orm::DbErr::RecordNotInserted => ComposeError::Validation {
                message: format!("Duplicate record: {}", error),
            },
            _ => ComposeError::Database(error),
        }
    }
}

pub struct ComposeService {
    db: Arc<DatabaseConnection>,
    executor: ComposeExecutor,
    data_dir: PathBuf,
}

impl ComposeService {
    pub fn new(db: Arc<DatabaseConnection>, executor: ComposeExecutor, data_dir: PathBuf) -> Self {
        Self {
            db,
            executor,
            data_dir,
        }
    }

    pub async fn list(
        &self,
        page: Option<u64>,
        page_size: Option<u64>,
    ) -> Result<(Vec<compose_stacks::Model>, u64), ComposeError> {
        let page = page.unwrap_or(1);
        let page_size = std::cmp::min(page_size.unwrap_or(20), 100);
        let paginator = compose_stacks::Entity::find()
            .order_by_desc(compose_stacks::Column::CreatedAt)
            .paginate(self.db.as_ref(), page_size);
        let total = paginator.num_items().await?;
        let items = paginator.fetch_page(page - 1).await?;
        Ok((items, total))
    }

    pub async fn get(&self, id: i32) -> Result<compose_stacks::Model, ComposeError> {
        compose_stacks::Entity::find_by_id(id)
            .one(self.db.as_ref())
            .await?
            .ok_or(ComposeError::NotFound { stack_id: id })
    }

    pub async fn create(
        &self,
        name: String,
        description: Option<String>,
        compose_content: String,
        env_content: Option<String>,
        node_id: Option<i32>,
    ) -> Result<compose_stacks::Model, ComposeError> {
        if name.is_empty() {
            return Err(ComposeError::Validation {
                message: "Stack name cannot be empty".into(),
            });
        }

        if compose_content.is_empty() {
            return Err(ComposeError::Validation {
                message: "Compose content cannot be empty".into(),
            });
        }

        let now = Utc::now();
        let model = compose_stacks::ActiveModel {
            name: Set(name.clone()),
            description: Set(description),
            compose_content: Set(compose_content),
            env_content: Set(env_content),
            node_id: Set(node_id),
            state: Set("stopped".to_string()),
            created_at: Set(now),
            updated_at: Set(now),
            ..Default::default()
        }
        .insert(self.db.as_ref())
        .await?;

        debug!(
            "Created compose stack '{}' with id {}",
            model.name, model.id
        );
        Ok(model)
    }

    pub async fn update(
        &self,
        id: i32,
        name: Option<String>,
        description: Option<Option<String>>,
        compose_content: Option<String>,
        env_content: Option<Option<String>>,
    ) -> Result<compose_stacks::Model, ComposeError> {
        let stack = self.get(id).await?;
        let mut active: compose_stacks::ActiveModel = stack.into();

        if let Some(name) = name {
            if name.is_empty() {
                return Err(ComposeError::Validation {
                    message: "Stack name cannot be empty".into(),
                });
            }
            active.name = Set(name);
        }

        if let Some(desc) = description {
            active.description = Set(desc);
        }

        if let Some(content) = compose_content {
            if content.is_empty() {
                return Err(ComposeError::Validation {
                    message: "Compose content cannot be empty".into(),
                });
            }
            active.compose_content = Set(content);
        }

        if let Some(env) = env_content {
            active.env_content = Set(env);
        }

        active.updated_at = Set(Utc::now());

        let model = active.update(self.db.as_ref()).await?;
        debug!("Updated compose stack '{}' (id: {})", model.name, model.id);
        Ok(model)
    }

    pub async fn delete(&self, id: i32) -> Result<(), ComposeError> {
        self.destroy(id).await
    }

    pub async fn set_state(
        &self,
        id: i32,
        state: &str,
    ) -> Result<compose_stacks::Model, ComposeError> {
        let stack = self.get(id).await?;
        let mut active: compose_stacks::ActiveModel = stack.into();
        active.state = Set(state.to_string());
        active.updated_at = Set(Utc::now());
        let model = active.update(self.db.as_ref()).await?;
        Ok(model)
    }

    pub async fn deploy(&self, id: i32) -> Result<compose_stacks::Model, ComposeError> {
        let stack = self.get(id).await?;

        // Validate ports before deploying
        self.validate_compose_ports(id, &stack.compose_content)
            .await?;

        // Write files and run docker compose up
        match self
            .executor
            .up(id, &stack.compose_content, stack.env_content.as_deref())
            .await
        {
            Ok(_) => {
                let model = self.set_state(id, "running").await?;
                debug!(stack_id = id, name = %model.name, "Stack deployed");
                Ok(model)
            }
            Err(e) => {
                error!(stack_id = id, error = %e, "Stack deploy failed, setting state to error");
                let _ = self.set_state(id, "error").await;
                Err(e.into())
            }
        }
    }

    pub async fn stop(&self, id: i32) -> Result<compose_stacks::Model, ComposeError> {
        let stack = self.get(id).await?;

        if stack.state != "running" {
            return Err(ComposeError::InvalidState {
                name: stack.name,
                state: stack.state,
                operation: "stop".to_string(),
            });
        }

        self.executor.down(id).await?;

        let model = self.set_state(id, "stopped").await?;
        debug!(stack_id = id, name = %model.name, "Stack stopped");
        Ok(model)
    }

    pub async fn restart(&self, id: i32) -> Result<compose_stacks::Model, ComposeError> {
        let stack = self.get(id).await?;

        if stack.state != "running" {
            return Err(ComposeError::InvalidState {
                name: stack.name,
                state: stack.state,
                operation: "restart".to_string(),
            });
        }

        // Validate ports (config may have changed since last deploy)
        self.validate_compose_ports(id, &stack.compose_content)
            .await?;

        self.executor
            .restart(id, &stack.compose_content, stack.env_content.as_deref())
            .await?;

        let model = self.set_state(id, "running").await?;
        debug!(stack_id = id, name = %model.name, "Stack restarted");
        Ok(model)
    }

    pub async fn pull(&self, id: i32) -> Result<compose_stacks::Model, ComposeError> {
        let stack = self.get(id).await?;

        self.executor
            .pull(id, &stack.compose_content, stack.env_content.as_deref())
            .await?;

        debug!(stack_id = id, name = %stack.name, "Pulled latest images");
        Ok(stack)
    }

    pub async fn containers(&self, id: i32) -> Result<String, ComposeError> {
        // Verify stack exists
        self.get(id).await?;
        let output = self.executor.ps(id).await?;
        Ok(output)
    }

    pub async fn logs(
        &self,
        id: i32,
        service: Option<&str>,
        tail: u32,
    ) -> Result<String, ComposeError> {
        // Verify stack exists
        self.get(id).await?;
        let output = self.executor.logs(id, service, tail).await?;
        Ok(output)
    }

    pub async fn stats(&self, id: i32) -> Result<Vec<ContainerMetrics>, ComposeError> {
        self.get(id).await?;
        let metrics = self.executor.stats(id).await?;
        Ok(metrics)
    }

    // --- Port validation ---

    async fn validate_compose_ports(
        &self,
        stack_id: i32,
        compose_content: &str,
    ) -> Result<(), ComposeError> {
        let bindings = port_validator::extract_ports(compose_content).map_err(|e| {
            ComposeError::Validation {
                message: format!("Failed to parse compose file for port validation: {}", e),
            }
        })?;

        if bindings.is_empty() {
            return Ok(());
        }

        // Check against Docker containers and system ports
        let mut conflicts = port_validator::validate_ports(&bindings, stack_id).await;

        // Check against routes from other stacks targeting the same ports
        let route_conflicts = self.check_route_port_conflicts(stack_id, &bindings).await?;
        conflicts.extend(route_conflicts);

        if !conflicts.is_empty() {
            return Err(ComposeError::PortConflict {
                message: port_validator::format_conflicts(&conflicts),
            });
        }

        debug!(
            stack_id,
            port_count = bindings.len(),
            "Port validation passed"
        );
        Ok(())
    }

    /// Check if any requested host ports conflict with routes from other stacks.
    async fn check_route_port_conflicts(
        &self,
        stack_id: i32,
        bindings: &[port_validator::PortBinding],
    ) -> Result<Vec<port_validator::PortConflict>, ComposeError> {
        let wanted_ports: Vec<i32> = bindings.iter().map(|b| b.host_port as i32).collect();

        // Find routes from OTHER stacks that target any of these ports
        let conflicting_routes = compose_stack_routes::Entity::find()
            .filter(compose_stack_routes::Column::TargetPort.is_in(wanted_ports))
            .filter(compose_stack_routes::Column::StackId.ne(stack_id))
            .filter(compose_stack_routes::Column::Enabled.eq(true))
            .all(self.db.as_ref())
            .await?;

        if conflicting_routes.is_empty() {
            return Ok(Vec::new());
        }

        // Fetch stack names for the conflicting routes
        let stack_ids: Vec<i32> = conflicting_routes.iter().map(|r| r.stack_id).collect();
        let stacks: Vec<compose_stacks::Model> = compose_stacks::Entity::find()
            .filter(compose_stacks::Column::Id.is_in(stack_ids))
            .all(self.db.as_ref())
            .await?;
        let stack_names: std::collections::HashMap<i32, String> =
            stacks.into_iter().map(|s| (s.id, s.name)).collect();

        let mut conflicts = Vec::new();
        for route in &conflicting_routes {
            let port = route.target_port as u16;
            // Find which binding requested this port
            for binding in bindings {
                if binding.host_port == port {
                    conflicts.push(port_validator::PortConflict {
                        host_port: port,
                        protocol: binding.protocol.clone(),
                        requesting_service: binding.service.clone(),
                        owner: port_validator::PortOwner::Route {
                            stack_id: route.stack_id,
                            stack_name: stack_names
                                .get(&route.stack_id)
                                .cloned()
                                .unwrap_or_else(|| format!("stack-{}", route.stack_id)),
                            domain: route.domain.clone(),
                        },
                    });
                }
            }
        }

        Ok(conflicts)
    }

    // --- Repository sync ---

    #[allow(clippy::too_many_arguments)]
    pub async fn create_from_repo(
        &self,
        name: String,
        description: Option<String>,
        repo_url: String,
        repo_branch: Option<String>,
        repo_compose_path: Option<String>,
        repo_access_token: Option<String>,
        node_id: Option<i32>,
    ) -> Result<compose_stacks::Model, ComposeError> {
        if name.is_empty() {
            return Err(ComposeError::Validation {
                message: "Stack name cannot be empty".into(),
            });
        }

        if repo_url.is_empty() {
            return Err(ComposeError::Validation {
                message: "Repository URL cannot be empty".into(),
            });
        }

        let compose_path = repo_compose_path
            .clone()
            .unwrap_or_else(|| "docker-compose.yml".to_string());
        let work_dir = repo_sync::repo_sync_work_dir(&self.data_dir);

        let (compose_content, env_content) = repo_sync::sync_compose_from_repo(
            &repo_url,
            repo_branch.as_deref(),
            &compose_path,
            repo_access_token.as_deref(),
            &work_dir,
        )
        .await?;

        let now = Utc::now();
        let model = compose_stacks::ActiveModel {
            name: Set(name),
            description: Set(description),
            compose_content: Set(compose_content),
            env_content: Set(env_content),
            node_id: Set(node_id),
            state: Set("stopped".to_string()),
            repo_url: Set(Some(repo_url)),
            repo_branch: Set(repo_branch),
            repo_compose_path: Set(Some(compose_path)),
            repo_access_token: Set(repo_access_token),
            last_synced_at: Set(Some(now)),
            created_at: Set(now),
            updated_at: Set(now),
            ..Default::default()
        }
        .insert(self.db.as_ref())
        .await?;

        debug!(stack_id = model.id, name = %model.name, "Created compose stack from repository");
        Ok(model)
    }

    pub async fn sync_from_repo(&self, id: i32) -> Result<compose_stacks::Model, ComposeError> {
        let stack = self.get(id).await?;

        let repo_url = stack.repo_url.as_deref().ok_or(ComposeError::Validation {
            message: format!("Stack {} is not linked to a repository", id),
        })?;

        let compose_path = stack
            .repo_compose_path
            .as_deref()
            .unwrap_or("docker-compose.yml");

        let work_dir = repo_sync::repo_sync_work_dir(&self.data_dir);

        let (compose_content, env_content) = repo_sync::sync_compose_from_repo(
            repo_url,
            stack.repo_branch.as_deref(),
            compose_path,
            stack.repo_access_token.as_deref(),
            &work_dir,
        )
        .await
        .map_err(|e| ComposeError::RepoSync {
            stack_id: id,
            reason: e.to_string(),
        })?;

        let mut active: compose_stacks::ActiveModel = stack.into();
        active.compose_content = Set(compose_content);
        active.env_content = Set(env_content);
        active.last_synced_at = Set(Some(Utc::now()));
        active.updated_at = Set(Utc::now());

        let model = active.update(self.db.as_ref()).await?;
        debug!(stack_id = id, name = %model.name, "Synced compose stack from repository");
        Ok(model)
    }

    pub async fn discover_compose_files(
        &self,
        repo_url: &str,
        repo_branch: Option<&str>,
        repo_access_token: Option<&str>,
    ) -> Result<Vec<String>, ComposeError> {
        let work_dir = repo_sync::repo_sync_work_dir(&self.data_dir);
        let files =
            repo_sync::discover_compose_files(repo_url, repo_branch, repo_access_token, &work_dir)
                .await?;
        Ok(files)
    }

    // --- Stack route management ---

    pub async fn list_routes(
        &self,
        stack_id: i32,
    ) -> Result<Vec<compose_stack_routes::Model>, ComposeError> {
        self.get(stack_id).await?;
        let routes = compose_stack_routes::Entity::find()
            .filter(compose_stack_routes::Column::StackId.eq(stack_id))
            .order_by_asc(compose_stack_routes::Column::Domain)
            .all(self.db.as_ref())
            .await?;
        Ok(routes)
    }

    pub async fn create_route(
        &self,
        stack_id: i32,
        domain: String,
        target_port: i32,
        service_name: Option<String>,
    ) -> Result<compose_stack_routes::Model, ComposeError> {
        self.get(stack_id).await?;

        if domain.is_empty() {
            return Err(ComposeError::Validation {
                message: "Domain cannot be empty".into(),
            });
        }

        if target_port <= 0 || target_port > 65535 {
            return Err(ComposeError::Validation {
                message: format!("Invalid port {}: must be between 1 and 65535", target_port),
            });
        }

        // Check if another stack already has a route to this port
        let existing = compose_stack_routes::Entity::find()
            .filter(compose_stack_routes::Column::TargetPort.eq(target_port))
            .filter(compose_stack_routes::Column::StackId.ne(stack_id))
            .filter(compose_stack_routes::Column::Enabled.eq(true))
            .one(self.db.as_ref())
            .await?;

        if let Some(route) = existing {
            let owner_stack = self.get(route.stack_id).await.ok();
            let owner_name = owner_stack
                .map(|s| s.name)
                .unwrap_or_else(|| format!("stack-{}", route.stack_id));
            return Err(ComposeError::PortConflict {
                message: format!(
                    "Port {} is already routed to stack '{}' (id: {}) via domain '{}'. \
                     Each port can only be routed to one stack.",
                    target_port, owner_name, route.stack_id, route.domain
                ),
            });
        }

        let now = Utc::now();
        let model = compose_stack_routes::ActiveModel {
            stack_id: Set(stack_id),
            domain: Set(domain.clone()),
            target_port: Set(target_port),
            service_name: Set(service_name),
            enabled: Set(true),
            created_at: Set(now),
            updated_at: Set(now),
            ..Default::default()
        }
        .insert(self.db.as_ref())
        .await?;

        debug!(stack_id, domain = %domain, port = target_port, "Created stack route");
        Ok(model)
    }

    pub async fn delete_route(&self, stack_id: i32, route_id: i32) -> Result<(), ComposeError> {
        self.get(stack_id).await?;

        let route = compose_stack_routes::Entity::find_by_id(route_id)
            .filter(compose_stack_routes::Column::StackId.eq(stack_id))
            .one(self.db.as_ref())
            .await?
            .ok_or(ComposeError::NotFound { stack_id: route_id })?;

        compose_stack_routes::Entity::delete_by_id(route_id)
            .exec(self.db.as_ref())
            .await?;

        debug!(stack_id, route_id, domain = %route.domain, "Deleted stack route");
        Ok(())
    }

    pub async fn toggle_route(
        &self,
        stack_id: i32,
        route_id: i32,
        enabled: bool,
    ) -> Result<compose_stack_routes::Model, ComposeError> {
        self.get(stack_id).await?;

        let route = compose_stack_routes::Entity::find_by_id(route_id)
            .filter(compose_stack_routes::Column::StackId.eq(stack_id))
            .one(self.db.as_ref())
            .await?
            .ok_or(ComposeError::NotFound { stack_id: route_id })?;

        let mut active: compose_stack_routes::ActiveModel = route.into();
        active.enabled = Set(enabled);
        active.updated_at = Set(Utc::now());

        let model = active.update(self.db.as_ref()).await?;
        debug!(stack_id, route_id, enabled, "Toggled stack route");
        Ok(model)
    }

    pub async fn destroy(&self, id: i32) -> Result<(), ComposeError> {
        let stack = self.get(id).await?;

        // Stop containers and remove files
        if let Err(e) = self.executor.destroy(id).await {
            error!(stack_id = id, error = %e, "Failed to destroy stack containers, proceeding with DB deletion");
        }

        compose_stacks::Entity::delete_by_id(id)
            .exec(self.db.as_ref())
            .await?;

        debug!(stack_id = id, name = %stack.name, "Stack destroyed (containers + DB record)");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::{DatabaseBackend, MockDatabase, MockExecResult};

    fn mock_stack() -> compose_stacks::Model {
        compose_stacks::Model {
            id: 1,
            name: "my-stack".to_string(),
            description: Some("Test stack".to_string()),
            compose_content: "version: '3'\nservices:\n  web:\n    image: nginx".to_string(),
            env_content: None,
            node_id: None,
            state: "stopped".to_string(),
            repo_url: None,
            repo_branch: None,
            repo_compose_path: None,
            repo_access_token: None,
            last_synced_at: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn test_service(db: sea_orm::DatabaseConnection) -> ComposeService {
        let tmp = std::env::temp_dir().join("temps-compose-test");
        let executor = ComposeExecutor::new(&tmp);
        ComposeService::new(Arc::new(db), executor, tmp)
    }

    #[tokio::test]
    async fn test_get_stack_success() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![vec![mock_stack()]])
            .into_connection();

        let service = test_service(db);
        let result = service.get(1).await;
        assert!(result.is_ok());
        let stack = result.unwrap();
        assert_eq!(stack.name, "my-stack");
        assert_eq!(stack.state, "stopped");
    }

    #[tokio::test]
    async fn test_get_stack_not_found() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![Vec::<compose_stacks::Model>::new()])
            .into_connection();

        let service = test_service(db);
        let result = service.get(999).await;
        assert!(matches!(
            result.unwrap_err(),
            ComposeError::NotFound { stack_id: 999 }
        ));
    }

    #[tokio::test]
    async fn test_create_stack_empty_name() {
        let db = MockDatabase::new(DatabaseBackend::Postgres).into_connection();
        let service = test_service(db);

        let result = service
            .create("".to_string(), None, "content".to_string(), None, None)
            .await;
        assert!(matches!(
            result.unwrap_err(),
            ComposeError::Validation { .. }
        ));
    }

    #[tokio::test]
    async fn test_create_stack_empty_content() {
        let db = MockDatabase::new(DatabaseBackend::Postgres).into_connection();
        let service = test_service(db);

        let result = service
            .create("test".to_string(), None, "".to_string(), None, None)
            .await;
        assert!(matches!(
            result.unwrap_err(),
            ComposeError::Validation { .. }
        ));
    }

    #[tokio::test]
    async fn test_create_stack_success() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![vec![mock_stack()]])
            .append_exec_results(vec![MockExecResult {
                last_insert_id: 1,
                rows_affected: 1,
            }])
            .into_connection();

        let service = test_service(db);
        let result = service
            .create(
                "my-stack".to_string(),
                Some("Test".to_string()),
                "version: '3'".to_string(),
                None,
                None,
            )
            .await;
        assert!(result.is_ok());
    }
}
