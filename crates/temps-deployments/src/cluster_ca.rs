//! Lazy per-cluster CA provisioning for multi-node mTLS (ADR-020 WS-2.1).
//!
//! The control plane mints ONE per-cluster CA the first time it needs to sign a
//! worker CSR, stores the CA cert (public) and the AES-256-GCM-encrypted CA key
//! in `settings.multi_node`, and reuses it thereafter. The CA cert is handed to
//! nodes at enrollment as their trust root; the CA private key never leaves the
//! control plane.

use temps_config::ConfigService;
use temps_core::EncryptionService;
use temps_deployer::remote::RemoteNodeDeployer;
use temps_deployer::DeployerError;

/// A resolved cluster CA: the public cert PEM and the DECRYPTED key PEM.
pub struct ClusterCa {
    pub cert_pem: String,
    pub key_pem: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ClusterCaError {
    #[error("Failed to read/write settings: {0}")]
    Settings(String),
    #[error("Cluster CA generation failed: {0}")]
    Pki(String),
    #[error("Encryption error: {0}")]
    Encryption(String),
}

fn decrypt_key(
    encryption_service: &EncryptionService,
    encrypted: &str,
) -> Result<String, ClusterCaError> {
    let bytes = encryption_service
        .decrypt(encrypted)
        .map_err(|e| ClusterCaError::Encryption(e.to_string()))?;
    String::from_utf8(bytes).map_err(|e| ClusterCaError::Encryption(e.to_string()))
}

/// Get the cluster CA (cert + decrypted key), minting and persisting it on first
/// use. Idempotent under concurrency: if a racing caller already stored a CA, we
/// re-read and return the winner's CA so every node pins the same root.
pub async fn ensure_cluster_ca(
    config_service: &ConfigService,
    encryption_service: &EncryptionService,
) -> Result<ClusterCa, ClusterCaError> {
    let settings = config_service
        .get_settings()
        .await
        .map_err(|e| ClusterCaError::Settings(e.to_string()))?;

    if let (Some(cert), Some(enc_key)) = (
        settings.multi_node.cluster_ca_cert_pem.clone(),
        settings.multi_node.cluster_ca_key_encrypted.clone(),
    ) {
        return Ok(ClusterCa {
            cert_pem: cert,
            key_pem: decrypt_key(encryption_service, &enc_key)?,
        });
    }

    // Mint a new CA and encrypt its private key for storage.
    let ca = temps_core::node_pki::generate_cluster_ca()
        .map_err(|e| ClusterCaError::Pki(e.to_string()))?;
    let enc_key = encryption_service
        .encrypt(ca.key_pem.as_bytes())
        .map_err(|e| ClusterCaError::Encryption(e.to_string()))?;
    let cert_pem = ca.cert_pem.clone();

    // Persist only if still absent — a concurrent minter may have won.
    config_service
        .update_setting_field(move |s| {
            if s.multi_node.cluster_ca_cert_pem.is_none() {
                s.multi_node.cluster_ca_cert_pem = Some(cert_pem.clone());
                s.multi_node.cluster_ca_key_encrypted = Some(enc_key.clone());
            }
        })
        .await
        .map_err(|e| ClusterCaError::Settings(e.to_string()))?;

    // Re-read to return whatever CA actually persisted (ours or the winner's).
    let settings = config_service
        .get_settings()
        .await
        .map_err(|e| ClusterCaError::Settings(e.to_string()))?;
    let cert = settings
        .multi_node
        .cluster_ca_cert_pem
        .ok_or_else(|| ClusterCaError::Settings("cluster CA cert missing after mint".into()))?;
    let enc_key = settings
        .multi_node
        .cluster_ca_key_encrypted
        .ok_or_else(|| ClusterCaError::Settings("cluster CA key missing after mint".into()))?;

    Ok(ClusterCa {
        cert_pem: cert,
        key_pem: decrypt_key(encryption_service, &enc_key)?,
    })
}

/// Mint a control-plane CLIENT identity (cert + key) signed by the cluster CA,
/// returned as a single combined PEM suitable for `reqwest::Identity::from_pem`
/// (ADR-020 WS-2.1). The CP presents this when calling agents over mTLS; the
/// agent accepts it because it chains to the cluster CA. Generated on demand —
/// cheap (one keygen + one signature) and avoids persisting another secret.
pub fn cp_client_identity(ca: &ClusterCa) -> Result<String, ClusterCaError> {
    let csr = temps_core::node_pki::generate_node_keypair_csr("temps-control-plane")
        .map_err(|e| ClusterCaError::Pki(e.to_string()))?;
    let signed = temps_core::node_pki::sign_node_csr(&ca.cert_pem, &ca.key_pem, &csr.csr_pem)
        .map_err(|e| ClusterCaError::Pki(e.to_string()))?;
    // reqwest's PEM Identity wants the cert chain followed by the private key.
    Ok(format!("{}\n{}", signed.cert_pem, csr.key_pem))
}

/// Build a `ContainerDeployer` for a remote node, transparently using mutual TLS
/// when the node's `address` is `https://` (ADR-020 WS-2.1) and plain HTTP
/// otherwise. This is the single place every CP→agent deployer is constructed so
/// no call site can accidentally fall back to plaintext against an mTLS node.
pub async fn build_node_deployer(
    address: &str,
    token: String,
    node_name: String,
    config_service: &ConfigService,
    encryption_service: &EncryptionService,
) -> Result<RemoteNodeDeployer, DeployerError> {
    if address.starts_with("https://") {
        let ca = ensure_cluster_ca(config_service, encryption_service)
            .await
            .map_err(|e| {
                DeployerError::NetworkError(format!(
                    "cluster CA unavailable for node {}: {}",
                    node_name, e
                ))
            })?;
        let identity = cp_client_identity(&ca).map_err(|e| {
            DeployerError::NetworkError(format!(
                "control-plane client identity unavailable for node {}: {}",
                node_name, e
            ))
        })?;
        RemoteNodeDeployer::new_mtls(
            address.to_string(),
            token,
            node_name,
            &identity,
            &ca.cert_pem,
        )
    } else {
        RemoteNodeDeployer::new(address.to_string(), token, node_name)
    }
}
