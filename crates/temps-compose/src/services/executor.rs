use bollard::query_parameters::{ListContainersOptions, StatsOptions};
use futures_util::TryStreamExt;
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use thiserror::Error;
use tokio::fs;
use tokio::process::Command;
use tracing::{debug, error, info};
use utoipa::ToSchema;

#[derive(Error, Debug)]
pub enum ExecutorError {
    #[error("Failed to write compose files for stack {stack_id} at {path}: {reason}")]
    FileWrite {
        stack_id: i32,
        path: String,
        reason: String,
    },

    #[error("docker compose {command} failed for stack {stack_id}: {stderr}")]
    CommandFailed {
        stack_id: i32,
        command: String,
        stderr: String,
    },

    #[error("docker compose not available: {reason}")]
    DockerComposeNotAvailable { reason: String },
}

#[derive(Debug, Serialize, Clone, ToSchema)]
pub struct ContainerMetrics {
    pub container_id: String,
    pub container_name: String,
    pub service: String,
    pub cpu_percent: f64,
    pub memory_bytes: u64,
    pub memory_limit: u64,
    pub memory_percent: f64,
    pub network_rx_bytes: u64,
    pub network_tx_bytes: u64,
}

fn calculate_cpu_percent(stats: &bollard::models::ContainerStatsResponse) -> f64 {
    let current_cpu = stats
        .cpu_stats
        .as_ref()
        .and_then(|cs| cs.cpu_usage.as_ref())
        .and_then(|cu| cu.total_usage);
    let current_system = stats.cpu_stats.as_ref().and_then(|cs| cs.system_cpu_usage);
    let prev_cpu = stats
        .precpu_stats
        .as_ref()
        .and_then(|cs| cs.cpu_usage.as_ref())
        .and_then(|cu| cu.total_usage);
    let prev_system = stats
        .precpu_stats
        .as_ref()
        .and_then(|cs| cs.system_cpu_usage);

    match (current_cpu, current_system, prev_cpu, prev_system) {
        (Some(cur_cpu), Some(cur_sys), Some(pre_cpu), Some(pre_sys)) => {
            let cpu_delta = cur_cpu as f64 - pre_cpu as f64;
            let system_delta = cur_sys as f64 - pre_sys as f64;
            if system_delta > 0.0 && cpu_delta >= 0.0 {
                let num_cpus = stats
                    .cpu_stats
                    .as_ref()
                    .and_then(|cs| cs.online_cpus)
                    .unwrap_or(1) as f64;
                ((cpu_delta / system_delta) * num_cpus * 100.0).clamp(0.0, 100.0)
            } else {
                0.0
            }
        }
        _ => 0.0,
    }
}

pub struct ComposeExecutor {
    stacks_dir: PathBuf,
}

impl ComposeExecutor {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            stacks_dir: data_dir.join("stacks"),
        }
    }

    fn stack_dir(&self, stack_id: i32) -> PathBuf {
        self.stacks_dir.join(stack_id.to_string())
    }

    pub async fn write_stack_files(
        &self,
        stack_id: i32,
        compose_content: &str,
        env_content: Option<&str>,
    ) -> Result<PathBuf, ExecutorError> {
        let dir = self.stack_dir(stack_id);

        fs::create_dir_all(&dir)
            .await
            .map_err(|e| ExecutorError::FileWrite {
                stack_id,
                path: dir.display().to_string(),
                reason: e.to_string(),
            })?;

        let compose_path = dir.join("docker-compose.yml");
        fs::write(&compose_path, compose_content)
            .await
            .map_err(|e| ExecutorError::FileWrite {
                stack_id,
                path: compose_path.display().to_string(),
                reason: e.to_string(),
            })?;

        let env_path = dir.join(".env");
        if let Some(env) = env_content {
            fs::write(&env_path, env)
                .await
                .map_err(|e| ExecutorError::FileWrite {
                    stack_id,
                    path: env_path.display().to_string(),
                    reason: e.to_string(),
                })?;
        } else {
            // Remove stale .env if it exists
            let _ = fs::remove_file(&env_path).await;
        }

        debug!(stack_id, path = %dir.display(), "Wrote compose files to disk");
        Ok(dir)
    }

    pub async fn cleanup_stack_files(&self, stack_id: i32) -> Result<(), ExecutorError> {
        let dir = self.stack_dir(stack_id);
        if dir.exists() {
            fs::remove_dir_all(&dir)
                .await
                .map_err(|e| ExecutorError::FileWrite {
                    stack_id,
                    path: dir.display().to_string(),
                    reason: format!("Failed to remove stack directory: {}", e),
                })?;
            debug!(stack_id, "Cleaned up stack files");
        }
        Ok(())
    }

    async fn run_compose(
        &self,
        stack_id: i32,
        project_dir: &Path,
        args: &[&str],
        command_label: &str,
    ) -> Result<String, ExecutorError> {
        let project_name = format!("temps-stack-{}", stack_id);

        let mut cmd = Command::new("docker");
        cmd.arg("compose")
            .arg("-p")
            .arg(&project_name)
            .arg("-f")
            .arg(project_dir.join("docker-compose.yml"));

        // --env-file is a global compose flag, must go before the subcommand
        let env_path = project_dir.join(".env");
        if env_path.exists() {
            cmd.arg("--env-file").arg(&env_path);
        }

        cmd.args(args).current_dir(project_dir);

        debug!(
            stack_id,
            command = command_label,
            project_name,
            "Running docker compose"
        );

        let output = cmd.output().await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                ExecutorError::DockerComposeNotAvailable {
                    reason: "docker binary not found in PATH".to_string(),
                }
            } else {
                ExecutorError::CommandFailed {
                    stack_id,
                    command: command_label.to_string(),
                    stderr: e.to_string(),
                }
            }
        })?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if !output.status.success() {
            error!(
                stack_id,
                command = command_label,
                exit_code = output.status.code(),
                stderr = %stderr,
                "docker compose command failed"
            );
            return Err(ExecutorError::CommandFailed {
                stack_id,
                command: command_label.to_string(),
                stderr,
            });
        }

        if !stderr.is_empty() {
            debug!(stack_id, command = command_label, stderr = %stderr, "docker compose stderr (non-fatal)");
        }

        info!(
            stack_id,
            command = command_label,
            "docker compose command succeeded"
        );
        Ok(stdout)
    }

    pub async fn up(
        &self,
        stack_id: i32,
        compose_content: &str,
        env_content: Option<&str>,
    ) -> Result<String, ExecutorError> {
        let dir = self
            .write_stack_files(stack_id, compose_content, env_content)
            .await?;
        self.run_compose(stack_id, &dir, &["up", "-d", "--remove-orphans"], "up")
            .await
    }

    pub async fn down(&self, stack_id: i32) -> Result<String, ExecutorError> {
        let dir = self.stack_dir(stack_id);
        if !dir.join("docker-compose.yml").exists() {
            debug!(stack_id, "No compose file on disk, nothing to stop");
            return Ok(String::new());
        }
        self.run_compose(stack_id, &dir, &["down"], "down").await
    }

    pub async fn restart(
        &self,
        stack_id: i32,
        compose_content: &str,
        env_content: Option<&str>,
    ) -> Result<String, ExecutorError> {
        let dir = self
            .write_stack_files(stack_id, compose_content, env_content)
            .await?;
        // Use "up -d --force-recreate" instead of "restart" so containers
        // pick up any changes to compose config or env variables.
        // Plain "restart" only sends SIGHUP without recreating containers.
        self.run_compose(
            stack_id,
            &dir,
            &["up", "-d", "--force-recreate", "--remove-orphans"],
            "restart",
        )
        .await
    }

    pub async fn pull(
        &self,
        stack_id: i32,
        compose_content: &str,
        env_content: Option<&str>,
    ) -> Result<String, ExecutorError> {
        let dir = self
            .write_stack_files(stack_id, compose_content, env_content)
            .await?;
        self.run_compose(stack_id, &dir, &["pull"], "pull").await
    }

    pub async fn ps(&self, stack_id: i32) -> Result<String, ExecutorError> {
        let dir = self.stack_dir(stack_id);
        if !dir.join("docker-compose.yml").exists() {
            return Ok("[]".to_string());
        }
        self.run_compose(stack_id, &dir, &["ps", "--format", "json", "-a"], "ps")
            .await
    }

    pub async fn logs(
        &self,
        stack_id: i32,
        service: Option<&str>,
        tail: u32,
    ) -> Result<String, ExecutorError> {
        let dir = self.stack_dir(stack_id);
        if !dir.join("docker-compose.yml").exists() {
            return Ok(String::new());
        }
        let tail_str = tail.to_string();
        let mut args = vec!["logs", "--no-color", "--tail", &tail_str];
        if let Some(svc) = service {
            args.push(svc);
        }
        self.run_compose(stack_id, &dir, &args, "logs").await
    }

    pub async fn stats(&self, stack_id: i32) -> Result<Vec<ContainerMetrics>, ExecutorError> {
        let project_name = format!("temps-stack-{}", stack_id);
        let label_filter = format!("com.docker.compose.project={}", project_name);

        let docker = bollard::Docker::connect_with_defaults().map_err(|e| {
            ExecutorError::DockerComposeNotAvailable {
                reason: format!("Failed to connect to Docker: {}", e),
            }
        })?;

        let containers = docker
            .list_containers(Some(ListContainersOptions {
                all: false,
                filters: Some(HashMap::from([(
                    "label".to_string(),
                    vec![label_filter.clone()],
                )])),
                ..Default::default()
            }))
            .await
            .map_err(|e| ExecutorError::CommandFailed {
                stack_id,
                command: "stats".to_string(),
                stderr: format!("Failed to list containers: {}", e),
            })?;

        let mut metrics = Vec::new();

        for container in containers {
            let container_id = match &container.id {
                Some(id) => id.clone(),
                None => continue,
            };

            let service_name = container
                .labels
                .as_ref()
                .and_then(|l| l.get("com.docker.compose.service"))
                .cloned()
                .unwrap_or_default();

            let container_name = container
                .names
                .as_ref()
                .and_then(|n| n.first())
                .map(|n| n.trim_start_matches('/').to_string())
                .unwrap_or_default();

            // stream: true gives us precpu_stats populated on the first result,
            // which is required for accurate CPU delta calculation.
            // one_shot: true with stream: false returns empty precpu_stats.
            let mut stats_stream = docker.stats(
                &container_id,
                Some(StatsOptions {
                    stream: true,
                    one_shot: false,
                }),
            );

            if let Ok(Some(stats_data)) = stats_stream.try_next().await {
                let cpu_percent = calculate_cpu_percent(&stats_data);
                let memory_stats = stats_data.memory_stats.as_ref();
                let memory_bytes = memory_stats.and_then(|ms| ms.usage).unwrap_or(0);
                let memory_limit = memory_stats.and_then(|ms| ms.limit).unwrap_or(0);
                let memory_percent = if memory_limit > 0 {
                    (memory_bytes as f64 / memory_limit as f64) * 100.0
                } else {
                    0.0
                };

                let default_networks = Default::default();
                let networks = stats_data.networks.as_ref().unwrap_or(&default_networks);
                let (rx, tx) = networks.values().fold((0u64, 0u64), |(rx, tx), net| {
                    (
                        rx + net.rx_bytes.unwrap_or(0),
                        tx + net.tx_bytes.unwrap_or(0),
                    )
                });

                metrics.push(ContainerMetrics {
                    container_id: container_id[..12].to_string(),
                    container_name,
                    service: service_name,
                    cpu_percent,
                    memory_bytes,
                    memory_limit,
                    memory_percent,
                    network_rx_bytes: rx,
                    network_tx_bytes: tx,
                });
            }
        }

        Ok(metrics)
    }

    pub async fn destroy(&self, stack_id: i32) -> Result<(), ExecutorError> {
        let dir = self.stack_dir(stack_id);
        if dir.join("docker-compose.yml").exists() {
            // Stop and remove containers, networks, volumes
            let _ = self
                .run_compose(
                    stack_id,
                    &dir,
                    &["down", "-v", "--remove-orphans"],
                    "destroy",
                )
                .await;
        }
        self.cleanup_stack_files(stack_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_write_stack_files_creates_compose_file() {
        let tmp = TempDir::new().unwrap();
        let executor = ComposeExecutor::new(tmp.path());

        let dir = executor
            .write_stack_files(1, "version: '3'\nservices:\n  web:\n    image: nginx", None)
            .await
            .unwrap();

        assert!(dir.join("docker-compose.yml").exists());
        let content = fs::read_to_string(dir.join("docker-compose.yml"))
            .await
            .unwrap();
        assert!(content.contains("nginx"));
        // No .env should exist
        assert!(!dir.join(".env").exists());
    }

    #[tokio::test]
    async fn test_write_stack_files_creates_env_file() {
        let tmp = TempDir::new().unwrap();
        let executor = ComposeExecutor::new(tmp.path());

        let dir = executor
            .write_stack_files(1, "version: '3'", Some("DB_HOST=localhost"))
            .await
            .unwrap();

        assert!(dir.join(".env").exists());
        let content = fs::read_to_string(dir.join(".env")).await.unwrap();
        assert_eq!(content, "DB_HOST=localhost");
    }

    #[tokio::test]
    async fn test_write_stack_files_removes_stale_env() {
        let tmp = TempDir::new().unwrap();
        let executor = ComposeExecutor::new(tmp.path());

        // First write with env
        executor
            .write_stack_files(1, "v1", Some("KEY=val"))
            .await
            .unwrap();
        assert!(executor.stack_dir(1).join(".env").exists());

        // Second write without env should remove .env
        executor.write_stack_files(1, "v2", None).await.unwrap();
        assert!(!executor.stack_dir(1).join(".env").exists());
    }

    #[tokio::test]
    async fn test_cleanup_stack_files() {
        let tmp = TempDir::new().unwrap();
        let executor = ComposeExecutor::new(tmp.path());

        executor
            .write_stack_files(42, "content", None)
            .await
            .unwrap();
        assert!(executor.stack_dir(42).exists());

        executor.cleanup_stack_files(42).await.unwrap();
        assert!(!executor.stack_dir(42).exists());
    }

    #[tokio::test]
    async fn test_cleanup_nonexistent_stack_is_ok() {
        let tmp = TempDir::new().unwrap();
        let executor = ComposeExecutor::new(tmp.path());

        // Should not error
        executor.cleanup_stack_files(999).await.unwrap();
    }

    #[tokio::test]
    async fn test_stack_dir_structure() {
        let tmp = TempDir::new().unwrap();
        let executor = ComposeExecutor::new(tmp.path());

        let dir = executor.stack_dir(7);
        assert_eq!(dir, tmp.path().join("stacks").join("7"));
    }
}
