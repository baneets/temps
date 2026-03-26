//! Content-addressable file store for temps.sh
//!
//! Blobs stored by SHA-256 content hash with git-style sharding:
//!   blobs/{hash[0..2]}/{hash[2..4]}/{hash}
//!
//! URL path → content hash mapping stored in a database table
//! (`static_asset_cache`), queried by the proxy with in-memory caching.

pub mod fs_store;

use async_trait::async_trait;
use bytes::Bytes;
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

/// Content-addressable blob store.
///
/// Stores and retrieves blobs by content hash.
/// URL path → hash mapping is handled by the database (`static_asset_cache` table).
#[async_trait]
pub trait FileStore: Send + Sync {
    /// Store a blob and return its content hash. Skips write if already exists.
    async fn put_blob(&self, data: Bytes) -> Result<String, FileStoreError>;

    /// Retrieve a blob by its content hash.
    async fn get_blob(&self, hash: &str) -> Result<Bytes, FileStoreError>;

    /// Check if a blob exists.
    async fn blob_exists(&self, hash: &str) -> Result<bool, FileStoreError>;

    /// Delete a blob by hash. Returns true if it existed.
    async fn delete_blob(&self, hash: &str) -> Result<bool, FileStoreError>;

    /// Store data by path key (for non-CAS use cases like edge caching).
    async fn put(&self, path: &str, data: Bytes) -> Result<u64, FileStoreError>;

    /// Retrieve data by path key (for non-CAS use cases like edge caching).
    async fn get(&self, path: &str) -> Result<Bytes, FileStoreError>;

    /// Check if a path key exists (for non-CAS use cases).
    async fn exists(&self, path: &str) -> Result<bool, FileStoreError>;
}
