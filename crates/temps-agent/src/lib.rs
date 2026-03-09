//! Temps Agent — lightweight HTTP server wrapping the local Docker runtime.
//!
//! Runs on worker nodes. Exposes a small bearer-token–authenticated API that
//! the control plane (or `RemoteNodeDeployer`) calls to manage containers.

pub mod auth;
pub mod handlers;
pub mod server;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum AgentError {
    #[error("Container operation failed for '{container_id}': {reason}")]
    ContainerOperation {
        container_id: String,
        reason: String,
    },

    #[error("Image operation failed for '{image_name}': {reason}")]
    ImageOperation { image_name: String, reason: String },

    #[error("Authentication failed: {0}")]
    AuthenticationFailed(String),

    #[error("Agent server error: {0}")]
    ServerError(String),

    #[error("Deployer error: {0}")]
    Deployer(#[from] temps_deployer::DeployerError),

    #[error("Builder error: {0}")]
    Builder(#[from] temps_deployer::BuilderError),
}

/// Health report sent in heartbeats and returned from GET /agent/health.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct NodeHealthReport {
    /// CPU usage percentage (0–100)
    pub cpu_percent: f64,
    /// Memory used in bytes
    pub memory_used_bytes: u64,
    /// Total memory in bytes
    pub memory_total_bytes: u64,
    /// Disk used in bytes
    pub disk_used_bytes: u64,
    /// Disk total in bytes
    pub disk_total_bytes: u64,
    /// Number of running containers
    pub running_containers: u64,
}

/// Configuration for the agent server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Listen address, e.g. "0.0.0.0:3100"
    pub listen_address: String,
    /// Pre-shared bearer token for authenticating requests from control plane
    pub token: String,
    /// Node name
    pub node_name: String,
    /// Control plane URL for registration and heartbeats
    pub control_plane_url: String,
    /// Node ID assigned by the control plane (used for heartbeat endpoint)
    pub node_id: i32,
    /// Node labels for scheduling (e.g., {"region": "us-east", "gpu": "true"}).
    /// Sent in every heartbeat so the control plane has up-to-date label info.
    #[serde(default)]
    pub labels: serde_json::Value,
}
