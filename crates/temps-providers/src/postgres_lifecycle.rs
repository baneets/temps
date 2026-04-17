//! DB-backed implementation of [`PostgresContainerLifecycle`].
//!
//! Reads service parameters via the shared `ExternalServiceManager`
//! (which owns encryption/decryption of the service config blob), drives
//! Docker via Bollard to create/remove containers, and persists image
//! swaps back through the manager. The upgrade orchestrator depends on
//! this trait (not on `PostgresService` or the manager directly) so it
//! stays stateless and service-agnostic — mirrors the
//! `PreUpgradeBackupProvider` pattern used for pre-upgrade backups.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use bollard::Docker;
use futures::TryStreamExt;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, DatabaseConnection, EntityTrait};
use temps_core::EncryptionService;
use temps_entities::external_services;

use crate::externalsvc::postgres::{PostgresConfig, PostgresInputConfig};
use crate::externalsvc::postgres_upgrade::{PostgresConnection, PostgresContainerLifecycle};
use crate::services::ExternalServiceManager;
use crate::utils::ensure_network_exists;

/// Adapter that fulfils `PostgresContainerLifecycle` by delegating
/// config-read and image-persist operations to `ExternalServiceManager`
/// (which handles encryption/decryption of the config blob).
pub struct PostgresLifecycleAdapter {
    db: Arc<DatabaseConnection>,
    docker: Arc<Docker>,
    manager: Arc<ExternalServiceManager>,
    encryption_service: Arc<EncryptionService>,
}

impl PostgresLifecycleAdapter {
    pub fn new(
        db: Arc<DatabaseConnection>,
        docker: Arc<Docker>,
        manager: Arc<ExternalServiceManager>,
        encryption_service: Arc<EncryptionService>,
    ) -> Self {
        Self {
            db,
            docker,
            manager,
            encryption_service,
        }
    }

    async fn load_service_row(&self, service_id: i32) -> Result<external_services::Model, String> {
        external_services::Entity::find_by_id(service_id)
            .one(self.db.as_ref())
            .await
            .map_err(|e| format!("db error loading service {}: {}", service_id, e))?
            .ok_or_else(|| format!("service {} not found", service_id))
    }

    /// Load + decrypt + parse the Postgres config blob.
    async fn load_postgres_config(&self, service_id: i32) -> Result<PostgresConfig, String> {
        let cfg = self
            .manager
            .get_service_config(service_id)
            .await
            .map_err(|e| format!("get_service_config({}) failed: {}", service_id, e))?;

        let input: PostgresInputConfig = serde_json::from_value(cfg.parameters)
            .map_err(|e| format!("service {} has invalid postgres params: {}", service_id, e))?;
        Ok(PostgresConfig::from(input))
    }

    /// Mirrors `PostgresService::get_pgdata_path`.
    fn pgdata_path_for(image: &str) -> Result<String, String> {
        let tag = image
            .split(':')
            .nth(1)
            .ok_or_else(|| format!("image '{}' has no tag", image))?;
        let version = tag
            .trim_start_matches("pg")
            .split('-')
            .next()
            .and_then(|v| v.split('.').next())
            .ok_or_else(|| format!("could not extract version from '{}'", image))?
            .parse::<u32>()
            .map_err(|e| format!("bad version in '{}': {}", image, e))?;
        Ok(format!("/var/lib/postgresql/{}/docker", version))
    }
}

#[async_trait]
impl PostgresContainerLifecycle for PostgresLifecycleAdapter {
    async fn container_name(&self, service_id: i32) -> Result<String, String> {
        let svc = self.load_service_row(service_id).await?;
        Ok(format!("postgres-{}", svc.name))
    }

    async fn connection_params(&self, service_id: i32) -> Result<PostgresConnection, String> {
        let cfg = self.load_postgres_config(service_id).await?;
        Ok(PostgresConnection {
            username: cfg.username,
            password: cfg.password,
            database: cfg.database,
            port: cfg.port,
        })
    }

    async fn stop_and_remove(&self, service_id: i32) -> Result<(), String> {
        let svc = self.load_service_row(service_id).await?;
        let container_name = format!("postgres-{}", svc.name);

        let _ = self
            .docker
            .stop_container(
                &container_name,
                None::<bollard::query_parameters::StopContainerOptions>,
            )
            .await;
        let remove = self
            .docker
            .remove_container(
                &container_name,
                Some(bollard::query_parameters::RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                }),
            )
            .await;
        match remove {
            Ok(_) => Ok(()),
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("No such container") || msg.contains("no such container") {
                    Ok(())
                } else {
                    Err(format!(
                        "remove_container({}) failed: {}",
                        container_name, msg
                    ))
                }
            }
        }
    }

    async fn create_and_start(&self, service_id: i32, image: &str) -> Result<(), String> {
        let svc = self.load_service_row(service_id).await?;
        let cfg = self.load_postgres_config(service_id).await?;
        let container_name = format!("postgres-{}", svc.name);
        let volume_name = format!("{}_data", container_name);
        let pgdata_path = Self::pgdata_path_for(image)?;

        // Pull image first for clear fail-fast errors.
        let (image_name, tag) = match image.split_once(':') {
            Some((n, t)) => (n.to_string(), t.to_string()),
            None => (image.to_string(), "latest".to_string()),
        };
        self.docker
            .create_image(
                Some(bollard::query_parameters::CreateImageOptions {
                    from_image: Some(image_name),
                    tag: Some(tag),
                    ..Default::default()
                }),
                None,
                None,
            )
            .try_collect::<Vec<_>>()
            .await
            .map_err(|e| format!("pull image '{}' failed: {}", image, e))?;

        // Create volume if missing — idempotent.
        self.docker
            .create_volume(bollard::models::VolumeCreateRequest {
                name: Some(volume_name.clone()),
                ..Default::default()
            })
            .await
            .map_err(|e| format!("create_volume({}) failed: {:?}", volume_name, e))?;

        ensure_network_exists(&self.docker)
            .await
            .map_err(|e| format!("ensure_network_exists: {:?}", e))?;

        let service_label_key = format!("{}service_type", temps_core::DOCKER_LABEL_PREFIX);
        let name_label_key = format!("{}service_name", temps_core::DOCKER_LABEL_PREFIX);
        let labels = HashMap::from([
            (service_label_key, "postgres".to_string()),
            (name_label_key, svc.name.clone()),
        ]);

        let env_vars = vec![
            format!("POSTGRES_USER={}", cfg.username),
            format!("POSTGRES_PASSWORD={}", cfg.password),
            format!("POSTGRES_DB={}", cfg.database),
            format!("PGDATA={}", pgdata_path),
            "POSTGRES_HOST_AUTH_METHOD=md5".to_string(),
        ];

        let host_config = bollard::models::HostConfig {
            port_bindings: Some(HashMap::from([(
                "5432/tcp".to_string(),
                Some(vec![bollard::models::PortBinding {
                    host_ip: Some("0.0.0.0".to_string()),
                    host_port: Some(cfg.port.clone()),
                }]),
            )])),
            mounts: Some(vec![bollard::models::Mount {
                target: Some("/var/lib/postgresql".to_string()),
                source: Some(volume_name.clone()),
                typ: Some(bollard::models::MountTypeEnum::VOLUME),
                ..Default::default()
            }]),
            log_config: Some(crate::utils::default_service_log_config()),
            security_opt: Some(vec!["no-new-privileges:true".to_string()]),
            pids_limit: Some(512),
            restart_policy: Some(bollard::models::RestartPolicy {
                name: Some(bollard::models::RestartPolicyNameEnum::ALWAYS),
                maximum_retry_count: None,
            }),
            ..Default::default()
        };

        let networking_config = Some(bollard::models::NetworkingConfig {
            endpoints_config: Some(HashMap::from([(
                temps_core::NETWORK_NAME.to_string(),
                bollard::models::EndpointSettings::default(),
            )])),
        });

        let container_cfg = bollard::models::ContainerCreateBody {
            image: Some(image.to_string()),
            exposed_ports: Some(vec!["5432/tcp".to_string()]),
            env: Some(env_vars),
            labels: Some(labels),
            cmd: Some(vec![
                "postgres".to_string(),
                "-c".to_string(),
                format!("max_connections={}", cfg.max_connections),
            ]),
            host_config: Some(host_config),
            networking_config,
            healthcheck: Some(bollard::models::HealthConfig {
                test: Some(vec![
                    "CMD-SHELL".to_string(),
                    format!("pg_isready -U {}", cfg.username),
                ]),
                interval: Some(1_000_000_000),
                timeout: Some(3_000_000_000),
                retries: Some(3),
                start_period: Some(30_000_000_000),
                start_interval: Some(1_000_000_000),
            }),
            ..Default::default()
        };

        // Remove any pre-existing container with the same name so create
        // doesn't 409.
        let _ = self
            .docker
            .stop_container(
                &container_name,
                None::<bollard::query_parameters::StopContainerOptions>,
            )
            .await;
        let _ = self
            .docker
            .remove_container(
                &container_name,
                Some(bollard::query_parameters::RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                }),
            )
            .await;

        self.docker
            .create_container(
                Some(
                    bollard::query_parameters::CreateContainerOptionsBuilder::new()
                        .name(&container_name)
                        .build(),
                ),
                container_cfg,
            )
            .await
            .map_err(|e| format!("create_container({}) failed: {}", container_name, e))?;

        self.docker
            .start_container(
                &container_name,
                None::<bollard::query_parameters::StartContainerOptions>,
            )
            .await
            .map_err(|e| format!("start_container({}) failed: {}", container_name, e))?;

        // Block until Postgres accepts connections.
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            if Instant::now() > deadline {
                return Err(format!(
                    "container '{}' failed to become ready within 120s",
                    container_name
                ));
            }
            let exec = self
                .docker
                .create_exec(
                    &container_name,
                    bollard::models::ExecConfig {
                        cmd: Some(vec![
                            "pg_isready".to_string(),
                            "-U".to_string(),
                            cfg.username.clone(),
                            "-d".to_string(),
                            cfg.database.clone(),
                        ]),
                        attach_stdout: Some(true),
                        attach_stderr: Some(true),
                        ..Default::default()
                    },
                )
                .await;
            if let Ok(id) = exec {
                // Drain the attached stream before inspect_exec, otherwise stdout
                // backpressure stalls the exec and exit_code never surfaces.
                use futures::StreamExt;
                if let Ok(bollard::exec::StartExecResults::Attached { mut output, .. }) =
                    self.docker.start_exec(&id.id, None).await
                {
                    while output.next().await.is_some() {}
                }
                let inspect = self.docker.inspect_exec(&id.id).await;
                if let Ok(info) = inspect {
                    if info.exit_code == Some(0) {
                        break;
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }

        Ok(())
    }

    async fn set_docker_image(&self, service_id: i32, image: &str) -> Result<(), String> {
        // Read current decrypted config, swap docker_image, re-encrypt,
        // and write directly via Sea-ORM. We intentionally bypass
        // `ExternalServiceManager::update_service` because it triggers a
        // service reinitialize that would stop/remove the container we
        // just upgraded.
        let cfg = self
            .manager
            .get_service_config(service_id)
            .await
            .map_err(|e| format!("get_service_config({}) failed: {}", service_id, e))?;

        let mut params = match cfg.parameters {
            serde_json::Value::Object(m) => m,
            other => {
                return Err(format!(
                    "service {} parameters not a JSON object (got {})",
                    service_id, other
                ))
            }
        };
        params.insert(
            "docker_image".to_string(),
            serde_json::Value::String(image.to_string()),
        );

        let config_json = serde_json::to_string(&serde_json::Value::Object(params))
            .map_err(|e| format!("serialize config for service {}: {}", service_id, e))?;
        let encrypted = self
            .encryption_service
            .encrypt_string(&config_json)
            .map_err(|e| format!("encrypt config for service {}: {}", service_id, e))?;

        let svc = self.load_service_row(service_id).await?;
        let mut active: external_services::ActiveModel = svc.into();
        active.config = Set(Some(encrypted));
        active.updated_at = Set(chrono::Utc::now());
        active
            .update(self.db.as_ref())
            .await
            .map_err(|e| format!("update service {} config: {}", service_id, e))?;
        Ok(())
    }
}
