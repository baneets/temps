use super::{DockerfileWithArgs, PackageManager, Preset, ProjectType};
use async_trait::async_trait;
use std::fmt;
use std::path::Path;

pub struct DockerComposePreset;

#[async_trait]
impl Preset for DockerComposePreset {
    fn slug(&self) -> String {
        "docker-compose".to_string()
    }

    fn project_type(&self) -> ProjectType {
        ProjectType::Server
    }

    fn label(&self) -> String {
        "Docker Compose".to_string()
    }

    fn icon_url(&self) -> String {
        "/presets/docker.svg".to_string()
    }

    fn description(&self) -> String {
        "Deploy a multi-container application using Docker Compose".to_string()
    }

    async fn dockerfile(&self, _config: super::DockerfileConfig<'_>) -> DockerfileWithArgs {
        // Docker Compose doesn't use a Dockerfile — it manages its own images.
        // Return a placeholder. The deployment pipeline skips the build step
        // for this preset and uses `docker compose up` instead.
        DockerfileWithArgs::new(
            "# Docker Compose preset — no Dockerfile needed".to_string(),
        )
    }

    async fn dockerfile_with_build_dir(&self, _local_path: &Path) -> DockerfileWithArgs {
        DockerfileWithArgs::new(
            "# Docker Compose preset — no Dockerfile needed".to_string(),
        )
    }

    fn install_command(&self, local_path: &Path) -> String {
        PackageManager::detect(local_path)
            .install_command()
            .to_string()
    }

    fn build_command(&self, _local_path: &Path) -> String {
        // No build step — compose pulls its own images
        String::new()
    }

    fn dirs_to_upload(&self) -> Vec<String> {
        vec![".".to_string()]
    }

    fn default_port(&self) -> u16 {
        // No single default port — compose has multiple services
        0
    }
}

impl fmt::Display for DockerComposePreset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.label())
    }
}

/// Common Docker Compose file names to detect
pub const COMPOSE_FILE_NAMES: &[&str] = &[
    "docker-compose.yml",
    "docker-compose.yaml",
    "compose.yml",
    "compose.yaml",
];
