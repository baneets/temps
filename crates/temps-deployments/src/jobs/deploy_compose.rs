//! Deploy Compose Job
//!
//! Deploys a Docker Compose stack using the ComposeExecutor.
//! Outputs container IDs, names, ports, and service names for
//! MarkDeploymentCompleteJob to register in deployment_containers.

use async_trait::async_trait;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use temps_core::{JobResult, WorkflowContext, WorkflowError, WorkflowTask};
use temps_deployer::compose::{ComposeDeployRequest, ComposeExecutor};
use temps_logs::LogService;
use tracing::debug;

impl std::fmt::Debug for DeployComposeJob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeployComposeJob")
            .field("job_id", &self.job_id)
            .field("deployment_id", &self.deployment_id)
            .field("project_id", &self.project_id)
            .field("environment_id", &self.environment_id)
            .finish()
    }
}

/// Job that deploys a Docker Compose stack.
pub struct DeployComposeJob {
    job_id: String,
    deployment_id: i32,
    project_id: i32,
    environment_id: i32,
    compose_executor: Arc<ComposeExecutor>,
    compose_content: String,
    compose_path: Option<String>,
    env_content: Option<String>,
    environment_vars: HashMap<String, String>,
    log_id: Option<String>,
    log_service: Arc<LogService>,
}

pub struct DeployComposeJobBuilder {
    job_id: Option<String>,
    deployment_id: Option<i32>,
    project_id: Option<i32>,
    environment_id: Option<i32>,
    compose_executor: Option<Arc<ComposeExecutor>>,
    compose_content: Option<String>,
    compose_path: Option<String>,
    env_content: Option<String>,
    environment_vars: HashMap<String, String>,
    log_id: Option<String>,
    log_service: Option<Arc<LogService>>,
}

impl Default for DeployComposeJobBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl DeployComposeJobBuilder {
    pub fn new() -> Self {
        Self {
            job_id: None,
            deployment_id: None,
            project_id: None,
            environment_id: None,
            compose_executor: None,
            compose_content: None,
            compose_path: None,
            env_content: None,
            environment_vars: HashMap::new(),
            log_id: None,
            log_service: None,
        }
    }

    pub fn job_id(mut self, id: String) -> Self {
        self.job_id = Some(id);
        self
    }
    pub fn deployment_id(mut self, id: i32) -> Self {
        self.deployment_id = Some(id);
        self
    }
    pub fn project_id(mut self, id: i32) -> Self {
        self.project_id = Some(id);
        self
    }
    pub fn environment_id(mut self, id: i32) -> Self {
        self.environment_id = Some(id);
        self
    }
    pub fn compose_executor(mut self, executor: Arc<ComposeExecutor>) -> Self {
        self.compose_executor = Some(executor);
        self
    }
    pub fn compose_content(mut self, content: String) -> Self {
        self.compose_content = Some(content);
        self
    }
    pub fn compose_path(mut self, path: Option<String>) -> Self {
        self.compose_path = path;
        self
    }
    pub fn env_content(mut self, content: Option<String>) -> Self {
        self.env_content = content;
        self
    }
    pub fn environment_vars(mut self, vars: HashMap<String, String>) -> Self {
        self.environment_vars = vars;
        self
    }
    pub fn log_id(mut self, id: Option<String>) -> Self {
        self.log_id = id;
        self
    }
    pub fn log_service(mut self, service: Arc<LogService>) -> Self {
        self.log_service = Some(service);
        self
    }

    pub fn build(self) -> Result<DeployComposeJob, WorkflowError> {
        Ok(DeployComposeJob {
            job_id: self
                .job_id
                .ok_or_else(|| WorkflowError::JobValidationFailed("job_id required".into()))?,
            deployment_id: self.deployment_id.ok_or_else(|| {
                WorkflowError::JobValidationFailed("deployment_id required".into())
            })?,
            project_id: self
                .project_id
                .ok_or_else(|| WorkflowError::JobValidationFailed("project_id required".into()))?,
            environment_id: self.environment_id.ok_or_else(|| {
                WorkflowError::JobValidationFailed("environment_id required".into())
            })?,
            compose_executor: self.compose_executor.ok_or_else(|| {
                WorkflowError::JobValidationFailed("compose_executor required".into())
            })?,
            compose_content: self.compose_content.ok_or_else(|| {
                WorkflowError::JobValidationFailed("compose_content required".into())
            })?,
            compose_path: self.compose_path,
            env_content: self.env_content,
            environment_vars: self.environment_vars,
            log_id: self.log_id,
            log_service: self
                .log_service
                .ok_or_else(|| WorkflowError::JobValidationFailed("log_service required".into()))?,
        })
    }
}

#[async_trait]
impl WorkflowTask for DeployComposeJob {
    fn job_id(&self) -> &str {
        &self.job_id
    }

    fn name(&self) -> &str {
        "Deploy Compose Stack"
    }

    fn description(&self) -> &str {
        "Deploy a multi-container Docker Compose stack"
    }

    async fn execute(&self, mut context: WorkflowContext) -> Result<JobResult, WorkflowError> {
        let project_name = format!("temps-{}-{}", self.project_id, self.environment_id);

        // Log start
        if let Some(ref log_id) = self.log_id {
            let _ = self
                .log_service
                .log_info(
                    log_id,
                    &format!("Deploying Docker Compose stack (project: {})", project_name),
                )
                .await;
        }

        // Build Temps labels for container discovery
        let mut labels = HashMap::new();
        labels.insert(
            "sh.temps.project_id".to_string(),
            self.project_id.to_string(),
        );
        labels.insert(
            "sh.temps.environment_id".to_string(),
            self.environment_id.to_string(),
        );
        labels.insert(
            "sh.temps.deployment_id".to_string(),
            self.deployment_id.to_string(),
        );
        labels.insert("sh.temps.managed".to_string(), "true".to_string());

        let request = ComposeDeployRequest {
            project_name: project_name.clone(),
            compose_content: self.compose_content.clone(),
            env_content: self.env_content.clone(),
            work_dir: PathBuf::from("/tmp"), // ComposeExecutor uses its own data_dir
            compose_path: self.compose_path.clone(),
            environment_vars: self.environment_vars.clone(),
            labels,
        };

        // Deploy
        let services = self.compose_executor.deploy(request).await.map_err(|e| {
            WorkflowError::JobExecutionFailed(format!("Compose deploy failed: {}", e))
        })?;

        if services.is_empty() {
            return Err(WorkflowError::JobExecutionFailed(
                "No containers found after docker compose up".to_string(),
            ));
        }

        // Log discovered services
        for svc in &services {
            let ports_str = svc
                .ports
                .iter()
                .map(|p| format!("{}:{}", p.host_port, p.container_port))
                .collect::<Vec<_>>()
                .join(", ");

            if let Some(ref log_id) = self.log_id {
                let _ = self
                    .log_service
                    .log_info(
                        log_id,
                        &format!(
                            "Service '{}': container={}, image={}, ports=[{}], status={}",
                            svc.service_name,
                            &svc.container_id[..12.min(svc.container_id.len())],
                            svc.image_name,
                            ports_str,
                            svc.status
                        ),
                    )
                    .await;
            }
        }

        // Set outputs compatible with MarkDeploymentCompleteJob
        // Uses job_id "deploy_container" for backward compatibility
        let container_ids: Vec<String> = services.iter().map(|s| s.container_id.clone()).collect();
        let container_names: Vec<String> =
            services.iter().map(|s| s.container_name.clone()).collect();
        let service_names: Vec<String> = services.iter().map(|s| s.service_name.clone()).collect();
        let host_ports: Vec<u16> = services
            .iter()
            .map(|s| s.ports.first().map(|p| p.host_port).unwrap_or(0))
            .collect();
        let container_ports: Vec<i32> = services
            .iter()
            .map(|s| {
                s.ports
                    .first()
                    .map(|p| p.container_port as i32)
                    .unwrap_or(0)
            })
            .collect();

        // First service's port as the "main" container_port
        let main_port = container_ports.first().copied().unwrap_or(0);

        context.set_output("deploy_container", "container_ids", &container_ids)?;
        context.set_output("deploy_container", "container_id", &container_ids[0])?;
        context.set_output("deploy_container", "container_name", &container_names[0])?;
        context.set_output("deploy_container", "host_ports", &host_ports)?;
        context.set_output("deploy_container", "host_port", host_ports[0])?;
        context.set_output("deploy_container", "container_port", main_port)?;
        context.set_output(
            "deploy_container",
            "node_ids",
            vec![None::<i32>; services.len()],
        )?;

        // Compose-specific outputs for MarkDeploymentCompleteJob
        context.set_output("deploy_container", "service_names", &service_names)?;
        context.set_output("deploy_container", "container_names", &container_names)?;
        context.set_output("deploy_container", "container_ports", &container_ports)?;
        context.set_output(
            "deploy_container",
            "image_names",
            services
                .iter()
                .map(|s| s.image_name.clone())
                .collect::<Vec<_>>(),
        )?;

        debug!(
            project = %project_name,
            services = services.len(),
            "Compose deployment complete"
        );

        if let Some(ref log_id) = self.log_id {
            let _ = self
                .log_service
                .log_success(
                    log_id,
                    &format!(
                        "Docker Compose stack deployed: {} services running",
                        services.len()
                    ),
                )
                .await;
        }

        Ok(JobResult::success(context))
    }
}
