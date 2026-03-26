//! Content-addressable file store for temps.sh
//!
//! Blobs are stored once by SHA-256 hash (deduplication). Manifests map
//! URL paths to content hashes — one manifest per deployment, not one file
//! per path. The proxy loads manifests into memory for O(1) lookups.
//!
//! Used for static asset chunks (stale-chunk fallback across deployments).

pub mod fs_store;

use async_trait::async_trait;
use bytes::Bytes;
use std::collections::HashMap;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum FileStoreError {
    #[error("File not found: {path}")]
    NotFound { path: String },

    #[error("IO error for file {path}: {reason}")]
    Io { path: String, reason: String },

    #[error("Backend error: {0}")]
    Backend(String),
}

/// Content-addressable file store.
///
/// Blobs are stored by content hash. Manifests map URL paths to hashes.
/// Implementations must be safe for concurrent use from multiple tasks.
#[async_trait]
pub trait FileStore: Send + Sync {
    /// Store a blob and return its content hash. Skips write if hash already exists.
    async fn put_blob(&self, data: Bytes) -> Result<String, FileStoreError>;

    /// Retrieve a blob by its content hash.
    async fn get_blob(&self, hash: &str) -> Result<Bytes, FileStoreError>;

    /// Check if a blob exists by hash.
    async fn blob_exists(&self, hash: &str) -> Result<bool, FileStoreError>;

    /// Write a deployment manifest: maps URL paths to content hashes.
    /// `manifest_key` is typically `"{project_id}/{environment_id}/{deployment_id}"`.
    async fn put_manifest(
        &self,
        manifest_key: &str,
        entries: &HashMap<String, String>,
    ) -> Result<(), FileStoreError>;

    /// Load a deployment manifest. Returns path → hash map.
    async fn get_manifest(
        &self,
        manifest_key: &str,
    ) -> Result<HashMap<String, String>, FileStoreError>;

    /// List all manifest keys (for loading recent deployments into memory).
    async fn list_manifests(&self) -> Result<Vec<String>, FileStoreError>;

    /// Resolve a URL path to blob content by searching manifests (convenience method).
    /// Default implementation loads manifests and looks up the path.
    async fn get(&self, path: &str) -> Result<Bytes, FileStoreError> {
        let manifests = self.list_manifests().await?;
        // Search most recent manifests first (sorted by key descending)
        let mut sorted = manifests;
        sorted.sort();
        sorted.reverse();

        for key in sorted {
            if let Ok(manifest) = self.get_manifest(&key).await {
                if let Some(hash) = manifest.get(path) {
                    return self.get_blob(hash).await;
                }
            }
        }
        Err(FileStoreError::NotFound {
            path: path.to_string(),
        })
    }
}
