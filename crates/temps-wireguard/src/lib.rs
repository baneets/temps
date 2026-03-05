//! WireGuard mesh networking for Temps multi-node deployments.
//!
//! Wraps the `wg` and `ip` CLI commands to manage WireGuard interfaces
//! and peer connections. WireGuard is in-kernel on Linux 5.6+, so no
//! additional installation is required on modern Linux systems.

use serde::{Deserialize, Serialize};
use std::net::Ipv4Addr;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum WireGuardError {
    #[error("WireGuard command failed: {command} — {reason}")]
    CommandFailed { command: String, reason: String },

    #[error("WireGuard not available on this system: {0}")]
    NotAvailable(String),

    #[error("No available IP addresses in subnet {subnet}")]
    SubnetExhausted { subnet: String },

    #[error("Invalid configuration: {0}")]
    InvalidConfig(String),

    #[error("IO error running WireGuard command: {0}")]
    Io(#[from] std::io::Error),

    #[error("Interface {interface} already exists")]
    InterfaceExists { interface: String },

    #[error("Peer with public key {public_key} already configured")]
    PeerExists { public_key: String },
}

/// A WireGuard peer (another node in the mesh).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireGuardPeer {
    /// Base64-encoded public key
    pub public_key: String,
    /// External endpoint, e.g. "203.0.113.50:51820"
    pub endpoint: String,
    /// Allowed IPs for this peer, e.g. "10.100.0.2/32"
    pub allowed_ips: String,
}

/// Keypair generated for a node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireGuardKeypair {
    pub private_key: String,
    pub public_key: String,
}

/// Manages a WireGuard interface for the Temps mesh network.
#[derive(Debug)]
pub struct WireGuardManager {
    /// Interface name, e.g. "wg0"
    interface: String,
    /// Subnet prefix, e.g. "10.100.0"
    subnet_prefix: String,
    /// Subnet mask bits
    subnet_mask: u8,
    /// WireGuard listen port
    listen_port: u16,
}

impl WireGuardManager {
    /// Create a new manager for the given interface and subnet.
    ///
    /// Default subnet: 10.100.0.0/24, listen port: 51820.
    pub fn new(interface: &str, subnet: &str, listen_port: u16) -> Result<Self, WireGuardError> {
        let parts: Vec<&str> = subnet.split('/').collect();
        if parts.len() != 2 {
            return Err(WireGuardError::InvalidConfig(format!(
                "Invalid subnet format '{}', expected CIDR notation like '10.100.0.0/24'",
                subnet
            )));
        }

        let ip_parts: Vec<&str> = parts[0].split('.').collect();
        if ip_parts.len() != 4 {
            return Err(WireGuardError::InvalidConfig(format!(
                "Invalid IP in subnet: {}",
                parts[0]
            )));
        }

        let mask: u8 = parts[1].parse().map_err(|_| {
            WireGuardError::InvalidConfig(format!("Invalid subnet mask: {}", parts[1]))
        })?;

        let subnet_prefix = format!("{}.{}.{}", ip_parts[0], ip_parts[1], ip_parts[2]);

        Ok(Self {
            interface: interface.to_string(),
            subnet_prefix,
            subnet_mask: mask,
            listen_port,
        })
    }

    /// Create with default settings (wg0, 10.100.0.0/24, port 51820).
    pub fn default_config() -> Result<Self, WireGuardError> {
        let subnet = std::env::var("TEMPS_WG_SUBNET").unwrap_or_else(|_| "10.100.0.0/24".into());
        let port: u16 = std::env::var("TEMPS_WG_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(51820);
        Self::new("wg0", &subnet, port)
    }

    /// Check if WireGuard CLI tools are available on this system.
    pub async fn check_available(&self) -> Result<(), WireGuardError> {
        let output = tokio::process::Command::new("wg")
            .arg("--version")
            .output()
            .await
            .map_err(|e| WireGuardError::NotAvailable(format!("Failed to run 'wg': {}", e)))?;

        if !output.status.success() {
            return Err(WireGuardError::NotAvailable(
                "wg command returned non-zero exit code".into(),
            ));
        }

        Ok(())
    }

    /// Generate a new WireGuard keypair using `wg genkey` and `wg pubkey`.
    pub async fn generate_keypair(&self) -> Result<WireGuardKeypair, WireGuardError> {
        let genkey_output = tokio::process::Command::new("wg")
            .arg("genkey")
            .output()
            .await?;

        if !genkey_output.status.success() {
            return Err(WireGuardError::CommandFailed {
                command: "wg genkey".into(),
                reason: String::from_utf8_lossy(&genkey_output.stderr).to_string(),
            });
        }

        let private_key = String::from_utf8_lossy(&genkey_output.stdout)
            .trim()
            .to_string();

        // Pipe private key through wg pubkey
        let child = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(format!("echo '{}' | wg pubkey", private_key))
            .output()
            .await?;

        if !child.status.success() {
            return Err(WireGuardError::CommandFailed {
                command: "wg pubkey".into(),
                reason: String::from_utf8_lossy(&child.stderr).to_string(),
            });
        }

        let public_key = String::from_utf8_lossy(&child.stdout).trim().to_string();

        Ok(WireGuardKeypair {
            private_key,
            public_key,
        })
    }

    /// Initialize the WireGuard interface with the given IP address and private key.
    ///
    /// Creates the interface, assigns the IP, sets the private key, and brings it up.
    pub async fn init_interface(
        &self,
        ip: Ipv4Addr,
        private_key: &str,
    ) -> Result<(), WireGuardError> {
        // Create the WireGuard interface
        self.run_command(
            "ip",
            &["link", "add", "dev", &self.interface, "type", "wireguard"],
        )
        .await?;

        // Assign IP address
        let addr = format!("{}/{}", ip, self.subnet_mask);
        self.run_command("ip", &["address", "add", "dev", &self.interface, &addr])
            .await?;

        // Write private key to a temporary file and configure
        let key_path = format!("/tmp/temps-wg-{}.key", self.interface);
        tokio::fs::write(&key_path, private_key).await?;

        // Set permissions
        self.run_command("chmod", &["600", &key_path]).await?;

        // Configure WireGuard with private key and listen port
        let port_str = self.listen_port.to_string();
        self.run_command(
            "wg",
            &[
                "set",
                &self.interface,
                "listen-port",
                &port_str,
                "private-key",
                &key_path,
            ],
        )
        .await?;

        // Clean up key file
        let _ = tokio::fs::remove_file(&key_path).await;

        // Bring the interface up
        self.run_command("ip", &["link", "set", "up", "dev", &self.interface])
            .await?;

        tracing::info!(
            interface = %self.interface,
            ip = %ip,
            port = %self.listen_port,
            "WireGuard interface initialized"
        );

        Ok(())
    }

    /// Add a peer to the WireGuard interface.
    pub async fn add_peer(&self, peer: &WireGuardPeer) -> Result<(), WireGuardError> {
        let mut args = vec![
            "set",
            &self.interface,
            "peer",
            &peer.public_key,
            "allowed-ips",
            &peer.allowed_ips,
        ];

        // Only set endpoint if provided (peer may be behind NAT)
        if !peer.endpoint.is_empty() {
            args.push("endpoint");
            args.push(&peer.endpoint);
        }

        // Enable persistent keepalive for NAT traversal
        args.push("persistent-keepalive");
        args.push("25");

        self.run_command("wg", &args).await?;

        tracing::info!(
            interface = %self.interface,
            peer_key = %peer.public_key,
            endpoint = %peer.endpoint,
            allowed_ips = %peer.allowed_ips,
            "WireGuard peer added"
        );

        Ok(())
    }

    /// Remove a peer from the WireGuard interface.
    pub async fn remove_peer(&self, public_key: &str) -> Result<(), WireGuardError> {
        self.run_command(
            "wg",
            &["set", &self.interface, "peer", public_key, "remove"],
        )
        .await?;

        tracing::info!(
            interface = %self.interface,
            peer_key = %public_key,
            "WireGuard peer removed"
        );

        Ok(())
    }

    /// Tear down the WireGuard interface.
    pub async fn destroy_interface(&self) -> Result<(), WireGuardError> {
        self.run_command("ip", &["link", "del", "dev", &self.interface])
            .await?;

        tracing::info!(
            interface = %self.interface,
            "WireGuard interface destroyed"
        );

        Ok(())
    }

    /// Get the next available IP in the subnet, given a list of already-assigned IPs.
    ///
    /// Skips .0 (network) and .255 (broadcast). Starts from .1.
    pub fn next_available_ip(&self, existing: &[Ipv4Addr]) -> Result<Ipv4Addr, WireGuardError> {
        for last_octet in 1..255u8 {
            let candidate: Ipv4Addr = format!("{}.{}", self.subnet_prefix, last_octet)
                .parse()
                .map_err(|_| {
                    WireGuardError::InvalidConfig(format!(
                        "Failed to construct IP from prefix {} and octet {}",
                        self.subnet_prefix, last_octet
                    ))
                })?;

            if !existing.contains(&candidate) {
                return Ok(candidate);
            }
        }

        Err(WireGuardError::SubnetExhausted {
            subnet: format!("{}.0/{}", self.subnet_prefix, self.subnet_mask),
        })
    }

    /// Get the interface name.
    pub fn interface(&self) -> &str {
        &self.interface
    }

    /// Get the listen port.
    pub fn listen_port(&self) -> u16 {
        self.listen_port
    }

    /// Run a system command and return an error if it fails.
    async fn run_command(&self, program: &str, args: &[&str]) -> Result<(), WireGuardError> {
        let output = tokio::process::Command::new(program)
            .args(args)
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            return Err(WireGuardError::CommandFailed {
                command: format!("{} {}", program, args.join(" ")),
                reason: stderr,
            });
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wireguard_manager_creation() {
        let manager = WireGuardManager::new("wg0", "10.100.0.0/24", 51820).unwrap();
        assert_eq!(manager.interface(), "wg0");
        assert_eq!(manager.listen_port(), 51820);
        assert_eq!(manager.subnet_prefix, "10.100.0");
        assert_eq!(manager.subnet_mask, 24);
    }

    #[test]
    fn test_wireguard_manager_invalid_subnet() {
        let result = WireGuardManager::new("wg0", "invalid", 51820);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            WireGuardError::InvalidConfig(_)
        ));
    }

    #[test]
    fn test_next_available_ip_empty() {
        let manager = WireGuardManager::new("wg0", "10.100.0.0/24", 51820).unwrap();
        let ip = manager.next_available_ip(&[]).unwrap();
        assert_eq!(ip, Ipv4Addr::new(10, 100, 0, 1));
    }

    #[test]
    fn test_next_available_ip_skips_existing() {
        let manager = WireGuardManager::new("wg0", "10.100.0.0/24", 51820).unwrap();
        let existing = vec![Ipv4Addr::new(10, 100, 0, 1), Ipv4Addr::new(10, 100, 0, 2)];
        let ip = manager.next_available_ip(&existing).unwrap();
        assert_eq!(ip, Ipv4Addr::new(10, 100, 0, 3));
    }

    #[test]
    fn test_next_available_ip_exhausted() {
        let manager = WireGuardManager::new("wg0", "10.100.0.0/24", 51820).unwrap();
        let existing: Vec<Ipv4Addr> = (1..255u8).map(|i| Ipv4Addr::new(10, 100, 0, i)).collect();
        let result = manager.next_available_ip(&existing);
        assert!(matches!(
            result.unwrap_err(),
            WireGuardError::SubnetExhausted { .. }
        ));
    }

    #[test]
    fn test_wireguard_peer_serialization() {
        let peer = WireGuardPeer {
            public_key: "abc123def456".to_string(),
            endpoint: "203.0.113.50:51820".to_string(),
            allowed_ips: "10.100.0.2/32".to_string(),
        };

        let json = serde_json::to_string(&peer).unwrap();
        let deserialized: WireGuardPeer = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.public_key, peer.public_key);
        assert_eq!(deserialized.endpoint, peer.endpoint);
        assert_eq!(deserialized.allowed_ips, peer.allowed_ips);
    }
}
