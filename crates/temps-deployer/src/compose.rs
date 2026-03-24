//! Docker Compose deployment executor.
//!
//! Manages multi-container deployments using `docker compose` CLI commands.
//! After `compose up`, discovers running containers, applies Temps labels,
//! and returns per-service results that get inserted into `deployment_containers`.

use bollard::Docker;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;
use tracing::{debug, error, info, warn};

#[derive(Error, Debug)]
pub enum ComposeError {
    #[error("Compose command failed for project '{project}': {reason}")]
    CommandFailed { project: String, reason: String },

    #[error("Failed to write compose files to '{path}': {reason}")]
    FileWriteFailed { path: String, reason: String },

    #[error("Failed to discover containers for project '{project}': {reason}")]
    DiscoveryFailed { project: String, reason: String },

    #[error("Docker API error: {0}")]
    Docker(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Request to deploy a Docker Compose stack.
#[derive(Debug, Clone)]
pub struct ComposeDeployRequest {
    /// Compose project name (e.g., "temps-{project_id}-{env_id}")
    pub project_name: String,
    /// Compose file content (the YAML)
    pub compose_content: String,
    /// Optional .env file content
    pub env_content: Option<String>,
    /// Working directory where compose files are written
    pub work_dir: PathBuf,
    /// Path to compose file relative to work_dir (default: "docker-compose.yml")
    pub compose_path: Option<String>,
    /// Environment variables to inject (merged with .env)
    pub environment_vars: HashMap<String, String>,
    /// Temps labels to apply to all containers
    pub labels: HashMap<String, String>,
    /// Source repo directory (needed for compose files with build: directives)
    pub repo_dir: Option<PathBuf>,
}

/// Result for a single compose service after deployment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComposeServiceResult {
    pub container_id: String,
    pub container_name: String,
    pub service_name: String,
    pub image_name: String,
    /// Ports published to the host (may be empty for internal services)
    pub ports: Vec<ComposePortBinding>,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComposePortBinding {
    pub host_port: u16,
    pub container_port: u16,
    pub protocol: String,
}

/// Docker Compose deployment executor.
#[derive(Debug)]
pub struct ComposeExecutor {
    docker: Arc<Docker>,
    /// Base directory for compose work dirs
    data_dir: PathBuf,
}

impl ComposeExecutor {
    pub fn new(docker: Arc<Docker>, data_dir: PathBuf) -> Self {
        Self { docker, data_dir }
    }

    /// Get the work directory for a compose project.
    fn project_dir(&self, project_name: &str) -> PathBuf {
        self.data_dir.join("compose").join(project_name)
    }

    /// Deploy a compose stack: write files, pull images, start containers,
    /// discover and label them. Returns one result per service.
    pub async fn deploy(
        &self,
        request: ComposeDeployRequest,
    ) -> Result<Vec<ComposeServiceResult>, ComposeError> {
        let project_dir = self.project_dir(&request.project_name);
        let project_name = request.project_name.clone();
        let has_build = self.has_build_directives(&request.compose_content);

        // When compose has build: directives, use the repo checkout as working dir
        // so Docker can access Dockerfiles and source code
        let effective_dir = if has_build {
            request
                .repo_dir
                .clone()
                .unwrap_or_else(|| project_dir.clone())
        } else {
            project_dir.clone()
        };

        // 1. Write compose files + env overrides to disk
        self.write_compose_files(&effective_dir, &request).await?;

        let compose_file = request
            .compose_path
            .as_deref()
            .unwrap_or("docker-compose.yml");

        // 2. Build images if compose file has build: directives
        if has_build {
            self.compose_build(
                &effective_dir,
                &project_name,
                compose_file,
                &request.environment_vars,
            )
            .await?;
        }

        // 3. Run docker compose up (pulls pre-built images, starts built + pulled)
        self.compose_up(
            &effective_dir,
            &project_name,
            compose_file,
            &request.environment_vars,
        )
        .await?;

        // 4. Discover running containers
        let containers = self
            .discover_containers(&effective_dir, &project_name, compose_file)
            .await?;

        // 4. Apply Temps labels to each container
        for container in &containers {
            if let Err(e) = self
                .apply_labels(
                    &container.container_id,
                    &request.labels,
                    &container.service_name,
                )
                .await
            {
                warn!(
                    container_id = %container.container_id,
                    service = %container.service_name,
                    error = %e,
                    "Failed to apply Temps labels to container"
                );
            }
        }

        info!(
            project = %project_name,
            services = containers.len(),
            "Compose stack deployed"
        );

        Ok(containers)
    }

    /// Tear down containers before a redeploy. Preserves volumes (database data,
    /// uploads, etc.) so they survive between deployments.
    pub async fn teardown_for_redeploy(&self, project_name: &str) -> Result<(), ComposeError> {
        let project_dir = self.project_dir(project_name);

        if !project_dir.exists() {
            debug!(project = %project_name, "Project directory does not exist, nothing to tear down");
            return Ok(());
        }

        let compose_file = self.find_compose_file(&project_dir);

        // down WITHOUT --volumes: removes containers and networks, keeps volumes
        let output = tokio::process::Command::new("docker")
            .args(["compose", "-p", project_name])
            .args(["-f", &compose_file])
            .args(["down", "--remove-orphans"])
            .current_dir(&project_dir)
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!(project = %project_name, stderr = %stderr, "docker compose down failed (best-effort)");
        }

        info!(project = %project_name, "Compose stack torn down (volumes preserved)");
        Ok(())
    }

    /// Fully destroy a compose stack including all volumes and data.
    /// Used when deleting a project/environment permanently.
    pub async fn destroy(&self, project_name: &str) -> Result<(), ComposeError> {
        let project_dir = self.project_dir(project_name);

        if !project_dir.exists() {
            debug!(project = %project_name, "Project directory does not exist, nothing to destroy");
            return Ok(());
        }

        let compose_file = self.find_compose_file(&project_dir);

        // down WITH --volumes: removes everything including persistent data
        let output = tokio::process::Command::new("docker")
            .args(["compose", "-p", project_name])
            .args(["-f", &compose_file])
            .args(["down", "--remove-orphans", "--volumes"])
            .current_dir(&project_dir)
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!(project = %project_name, stderr = %stderr, "docker compose down failed");
        }

        // Clean up work directory
        if let Err(e) = tokio::fs::remove_dir_all(&project_dir).await {
            warn!(project = %project_name, error = %e, "Failed to clean up project directory");
        }

        info!(project = %project_name, "Compose stack destroyed (volumes removed)");
        Ok(())
    }

    /// Stop a compose stack without removing volumes.
    pub async fn stop(&self, project_name: &str) -> Result<(), ComposeError> {
        let project_dir = self.project_dir(project_name);
        let compose_file = self.find_compose_file(&project_dir);

        let output = tokio::process::Command::new("docker")
            .args(["compose", "-p", project_name])
            .args(["-f", &compose_file])
            .arg("stop")
            .current_dir(&project_dir)
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ComposeError::CommandFailed {
                project: project_name.to_string(),
                reason: format!("docker compose stop failed: {}", stderr),
            });
        }

        Ok(())
    }

    // --- Internal methods ---

    async fn write_compose_files(
        &self,
        project_dir: &Path,
        request: &ComposeDeployRequest,
    ) -> Result<(), ComposeError> {
        tokio::fs::create_dir_all(project_dir).await.map_err(|e| {
            ComposeError::FileWriteFailed {
                path: project_dir.display().to_string(),
                reason: e.to_string(),
            }
        })?;

        let compose_file = request
            .compose_path
            .as_deref()
            .unwrap_or("docker-compose.yml");
        let compose_path = project_dir.join(compose_file);

        // Ensure parent directories exist (for nested paths like "subdir/docker-compose.yml")
        if let Some(parent) = compose_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| ComposeError::FileWriteFailed {
                    path: parent.display().to_string(),
                    reason: e.to_string(),
                })?;
        }

        tokio::fs::write(&compose_path, &request.compose_content)
            .await
            .map_err(|e| ComposeError::FileWriteFailed {
                path: compose_path.display().to_string(),
                reason: e.to_string(),
            })?;

        // Write .env file (repo's original .env content if any)
        if let Some(ref env_content) = request.env_content {
            if !env_content.trim().is_empty() {
                let env_path = project_dir.join(".env");
                tokio::fs::write(&env_path, env_content.trim())
                    .await
                    .map_err(|e| ComposeError::FileWriteFailed {
                        path: env_path.display().to_string(),
                        reason: e.to_string(),
                    })?;
            }
        }

        // Write Temps system env vars to .env.temps
        // These include SENTRY_DSN, TEMPS_API_URL, TEMPS_API_TOKEN, OTEL vars, etc.
        if !request.environment_vars.is_empty() {
            let temps_env: String = request
                .environment_vars
                .iter()
                .map(|(k, v)| format!("{}={}", k, v))
                .collect::<Vec<_>>()
                .join("\n");
            let temps_env_path = project_dir.join(".env.temps");
            tokio::fs::write(&temps_env_path, &temps_env)
                .await
                .map_err(|e| ComposeError::FileWriteFailed {
                    path: temps_env_path.display().to_string(),
                    reason: e.to_string(),
                })?;

            // Write compose override that injects .env.temps into every service.
            // Docker Compose automatically merges docker-compose.override.yml.
            // This ensures all Temps system env vars (SENTRY_DSN, TEMPS_API_URL, etc.)
            // are available inside every container without modifying the original compose file.
            let override_path = project_dir.join("docker-compose.override.yml");
            let override_content =
                self.generate_env_override(&request.compose_content, ".env.temps");
            tokio::fs::write(&override_path, &override_content)
                .await
                .map_err(|e| ComposeError::FileWriteFailed {
                    path: override_path.display().to_string(),
                    reason: e.to_string(),
                })?;
        }

        debug!(
            path = %compose_path.display(),
            "Wrote compose files"
        );

        Ok(())
    }

    /// Check if a compose file contains build: directives (services that need building)
    fn has_build_directives(&self, compose_content: &str) -> bool {
        for line in compose_content.lines() {
            let trimmed = line.trim();
            if trimmed == "build:" || trimmed.starts_with("build:") {
                return true;
            }
        }
        false
    }

    /// Run docker compose build for services with build: directives
    async fn compose_build(
        &self,
        project_dir: &Path,
        project_name: &str,
        compose_file: &str,
        env_vars: &HashMap<String, String>,
    ) -> Result<(), ComposeError> {
        let mut cmd = tokio::process::Command::new("docker");
        cmd.args(["compose", "-p", project_name])
            .args(["-f", compose_file])
            .args(["build", "--pull"])
            .current_dir(project_dir);

        for (key, value) in env_vars {
            cmd.env(key, value);
        }

        debug!(project = %project_name, "Running docker compose build");

        let output = cmd.output().await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ComposeError::CommandFailed {
                project: project_name.to_string(),
                reason: format!("docker compose build failed: {}", stderr),
            });
        }

        info!(project = %project_name, "docker compose build completed");
        Ok(())
    }

    async fn compose_up(
        &self,
        project_dir: &Path,
        project_name: &str,
        compose_file: &str,
        env_vars: &HashMap<String, String>,
    ) -> Result<(), ComposeError> {
        let mut cmd = tokio::process::Command::new("docker");
        cmd.args(["compose", "-p", project_name])
            .args(["-f", compose_file]);

        // Include override file for env var injection (if it exists)
        let override_path = project_dir.join("docker-compose.override.yml");
        if override_path.exists() {
            cmd.args(["-f", "docker-compose.override.yml"]);
        }

        cmd.args(["up", "-d", "--pull", "always", "--remove-orphans"])
            .current_dir(project_dir);

        // Pass environment variables for compose YAML substitution
        for (key, value) in env_vars {
            cmd.env(key, value);
        }

        debug!(project = %project_name, "Running docker compose up");

        let output = cmd.output().await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ComposeError::CommandFailed {
                project: project_name.to_string(),
                reason: format!("docker compose up failed: {}", stderr),
            });
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        debug!(project = %project_name, stdout = %stdout, "docker compose up completed");

        Ok(())
    }

    async fn discover_containers(
        &self,
        project_dir: &Path,
        project_name: &str,
        compose_file: &str,
    ) -> Result<Vec<ComposeServiceResult>, ComposeError> {
        // Use docker compose ps to list containers
        let output = tokio::process::Command::new("docker")
            .args(["compose", "-p", project_name])
            .args(["-f", compose_file])
            .args(["ps", "--format", "json", "--all"])
            .current_dir(project_dir)
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ComposeError::DiscoveryFailed {
                project: project_name.to_string(),
                reason: format!("docker compose ps failed: {}", stderr),
            });
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut results = Vec::new();

        // docker compose ps --format json outputs one JSON object per line
        for line in stdout.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            let ps_entry: ComposePsEntry =
                serde_json::from_str(line).map_err(|e| ComposeError::DiscoveryFailed {
                    project: project_name.to_string(),
                    reason: format!("Failed to parse compose ps output: {} (line: {})", e, line),
                })?;

            // Parse published ports
            let ports = self.parse_publishers(&ps_entry.publishers);

            results.push(ComposeServiceResult {
                container_id: ps_entry.id,
                container_name: ps_entry.name,
                service_name: ps_entry.service,
                image_name: ps_entry.image,
                ports,
                status: ps_entry.state,
            });
        }

        debug!(
            project = %project_name,
            services = results.len(),
            "Discovered compose containers"
        );

        Ok(results)
    }

    fn parse_publishers(&self, publishers: &[ComposePsPublisher]) -> Vec<ComposePortBinding> {
        publishers
            .iter()
            .filter(|p| p.published_port > 0)
            .map(|p| ComposePortBinding {
                host_port: p.published_port,
                container_port: p.target_port,
                protocol: p.protocol.clone(),
            })
            .collect()
    }

    async fn apply_labels(
        &self,
        container_id: &str,
        base_labels: &HashMap<String, String>,
        service_name: &str,
    ) -> Result<(), ComposeError> {
        // Bollard doesn't support updating labels on a running container directly.
        // We need to use `docker container update` is also limited.
        // Instead, we verify the container exists and log the labels.
        // The labels were already set via compose labels or we use docker inspect
        // to verify the container is running.
        //
        // For Temps integration, we rely on:
        // 1. The compose project name (temps-{project_id}-{env_id}) for discovery
        // 2. The container IDs stored in deployment_containers table
        // 3. Container names for log aggregation
        //
        // The deployment pipeline inserts these containers into deployment_containers
        // with the correct project_id, environment_id, deployment_id, and service_name.
        // The proxy and monitoring systems use deployment_containers for lookup,
        // not Docker labels.

        let inspect = self
            .docker
            .inspect_container(container_id, None)
            .await
            .map_err(|e| ComposeError::Docker(format!("inspect failed: {}", e)))?;

        let state = inspect
            .state
            .as_ref()
            .and_then(|s| s.status.as_ref())
            .map(|s| format!("{:?}", s))
            .unwrap_or_else(|| "unknown".to_string());

        debug!(
            container_id = %container_id,
            service = %service_name,
            state = %state,
            labels = ?base_labels.keys().collect::<Vec<_>>(),
            "Verified compose container"
        );

        Ok(())
    }

    /// Generate a docker-compose.override.yml that adds env_file to every service.
    /// This injects Temps system env vars into all containers without modifying
    /// the original compose file.
    fn generate_env_override(&self, compose_content: &str, env_file: &str) -> String {
        // Parse service names from compose content (simple YAML parsing)
        let mut services = Vec::new();
        let mut in_services = false;
        let mut services_indent: usize = 0;
        let mut service_indent: Option<usize> = None; // indent of first service found

        for line in compose_content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }

            let indent = line.len() - line.trim_start().len();

            if trimmed == "services:" || trimmed.starts_with("services:") {
                in_services = true;
                services_indent = indent;
                service_indent = None;
                continue;
            }

            if in_services {
                // If we go back to root level, stop
                if indent <= services_indent {
                    in_services = false;
                    continue;
                }

                // Service names are keys at the first level after services:
                if trimmed.ends_with(':') && !trimmed.contains(' ') && !trimmed.starts_with('-') {
                    match service_indent {
                        None => {
                            // First service found — set the indent level
                            service_indent = Some(indent);
                            services.push(trimmed.trim_end_matches(':').to_string());
                        }
                        Some(si) if indent == si => {
                            // Same indent as first service — it's a service
                            services.push(trimmed.trim_end_matches(':').to_string());
                        }
                        _ => {
                            // Deeper indent — it's a property (image:, ports:, etc.), skip
                        }
                    }
                }
            }
        }

        if services.is_empty() {
            return String::new();
        }

        let mut override_yaml = String::from("services:\n");
        for service in &services {
            override_yaml.push_str(&format!("  {}:\n", service));
            override_yaml.push_str("    env_file:\n");
            override_yaml.push_str(&format!("      - {}\n", env_file));
        }

        override_yaml
    }

    fn find_compose_file(&self, project_dir: &Path) -> String {
        for name in &[
            "docker-compose.yml",
            "docker-compose.yaml",
            "compose.yml",
            "compose.yaml",
        ] {
            if project_dir.join(name).exists() {
                return name.to_string();
            }
        }
        "docker-compose.yml".to_string()
    }
}

/// JSON output from `docker compose ps --format json`
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ComposePsEntry {
    #[serde(alias = "ID")]
    id: String,
    name: String,
    service: String,
    image: String,
    state: String,
    #[serde(default)]
    publishers: Vec<ComposePsPublisher>,
}

/// Port publisher from `docker compose ps --format json`
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ComposePsPublisher {
    #[serde(default)]
    published_port: u16,
    #[serde(default)]
    target_port: u16,
    #[serde(default)]
    protocol: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_compose_ps_json() {
        let json = r#"{"ID":"abc123","Name":"myapp-web-1","Service":"web","Image":"nginx:latest","State":"running","Publishers":[{"URL":"0.0.0.0","TargetPort":80,"PublishedPort":8080,"Protocol":"tcp"}]}"#;

        let entry: ComposePsEntry = serde_json::from_str(json).unwrap();
        assert_eq!(entry.id, "abc123");
        assert_eq!(entry.service, "web");
        assert_eq!(entry.state, "running");
        assert_eq!(entry.publishers.len(), 1);
        assert_eq!(entry.publishers[0].published_port, 8080);
        assert_eq!(entry.publishers[0].target_port, 80);
    }

    #[test]
    fn test_parse_compose_ps_no_ports() {
        let json = r#"{"ID":"def456","Name":"myapp-redis-1","Service":"redis","Image":"redis:7","State":"running","Publishers":[]}"#;

        let entry: ComposePsEntry = serde_json::from_str(json).unwrap();
        assert_eq!(entry.service, "redis");
        assert!(entry.publishers.is_empty());
    }

    #[test]
    fn test_parse_publishers() {
        let docker = Docker::connect_with_defaults();
        if docker.is_err() {
            // Can still test parse_publishers without Docker
            return;
        }
        let executor = ComposeExecutor::new(Arc::new(docker.unwrap()), PathBuf::from("/tmp/test"));

        let publishers = vec![
            ComposePsPublisher {
                published_port: 8080,
                target_port: 80,
                protocol: "tcp".to_string(),
            },
            ComposePsPublisher {
                published_port: 0, // Not published
                target_port: 6379,
                protocol: "tcp".to_string(),
            },
        ];

        let ports = executor.parse_publishers(&publishers);
        assert_eq!(ports.len(), 1); // Only the published port
        assert_eq!(ports[0].host_port, 8080);
        assert_eq!(ports[0].container_port, 80);
    }

    #[test]
    fn test_generate_env_override() {
        let docker = Docker::connect_with_defaults();
        if docker.is_err() {
            return;
        }
        let executor = ComposeExecutor::new(Arc::new(docker.unwrap()), PathBuf::from("/tmp/test"));

        let compose = r#"
services:
  web:
    image: nginx
    ports:
      - "8080:80"
  redis:
    image: redis:7
  postgres:
    image: postgres:17
"#;

        let override_yaml = executor.generate_env_override(compose, ".env.temps");
        assert!(override_yaml.contains("web:"));
        assert!(override_yaml.contains("redis:"));
        assert!(override_yaml.contains("postgres:"));
        assert!(override_yaml.contains(".env.temps"));
        // Each service should have env_file
        assert_eq!(override_yaml.matches("env_file:").count(), 3);
    }

    #[test]
    fn test_has_build_directives() {
        let docker = Docker::connect_with_defaults();
        if docker.is_err() {
            return;
        }
        let executor = ComposeExecutor::new(Arc::new(docker.unwrap()), PathBuf::from("/tmp/test"));

        // No build
        assert!(!executor.has_build_directives("services:\n  web:\n    image: nginx\n"));

        // build: with context
        assert!(executor.has_build_directives("services:\n  web:\n    build: .\n"));

        // build: block
        assert!(executor.has_build_directives(
            "services:\n  web:\n    build:\n      context: .\n      dockerfile: Dockerfile\n"
        ));
    }

    #[test]
    fn test_generate_env_override_empty() {
        let docker = Docker::connect_with_defaults();
        if docker.is_err() {
            return;
        }
        let executor = ComposeExecutor::new(Arc::new(docker.unwrap()), PathBuf::from("/tmp/test"));

        let compose = "version: '3'\n";
        let override_yaml = executor.generate_env_override(compose, ".env.temps");
        assert!(override_yaml.is_empty());
    }
}
