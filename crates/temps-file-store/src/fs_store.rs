//! Content-addressable filesystem store.
//!
//! Layout:
//! ```text
//! {root}/
//!   blobs/{hash[0..2]}/{hash}       <- deduplicated content blobs
//!   paths/{url_path}.ref            <- tiny files containing the content hash
//!   .tmp/                           <- atomic write staging
//! ```
//!
//! `put` hashes the content (SHA-256), stores the blob once under `blobs/`,
//! and writes a reference file under `paths/` mapping the URL path to the hash.
//! Identical content across deployments shares a single blob on disk.

use async_trait::async_trait;
use bytes::Bytes;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use tracing::debug;

use crate::{FileStore, FileStoreError};

/// Content-addressable filesystem store.
pub struct FsFileStore {
    root: PathBuf,
}

impl FsFileStore {
    /// Create a new store rooted at the given directory.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Get the root directory of this store.
    pub fn root(&self) -> &std::path::Path {
        &self.root
    }

    /// Compute the SHA-256 hex digest of the given data.
    fn content_hash(data: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(data);
        format!("{:x}", hasher.finalize())
    }

    /// On-disk path for a content blob: `blobs/{hash[0..2]}/{hash}`
    fn blob_path(&self, hash: &str) -> PathBuf {
        let prefix = &hash[..2.min(hash.len())];
        self.root.join("blobs").join(prefix).join(hash)
    }

    /// On-disk path for the reference file: `paths/{sanitized_url_path}.ref`
    fn ref_path(&self, url_path: &str) -> PathBuf {
        let clean: PathBuf = url_path
            .trim_start_matches('/')
            .split('/')
            .filter(|seg| !seg.is_empty() && *seg != ".." && *seg != ".")
            .collect();
        self.root.join("paths").join(format!("{}.ref", clean.display()))
    }

    /// Path to the tmp staging directory for atomic writes.
    fn tmp_dir(&self) -> PathBuf {
        self.root.join(".tmp")
    }

    /// Atomically write data to a target path via tmp rename.
    async fn atomic_write(&self, target: &std::path::Path, data: &[u8]) -> Result<(), FileStoreError> {
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| FileStoreError::Io {
                path: target.display().to_string(),
                reason: format!("mkdir: {}", e),
            })?;
        }

        let tmp = self.tmp_dir();
        tokio::fs::create_dir_all(&tmp).await.map_err(|e| FileStoreError::Io {
            path: tmp.display().to_string(),
            reason: format!("mkdir tmp: {}", e),
        })?;

        let tmp_file = tmp.join(uuid::Uuid::new_v4().to_string());
        tokio::fs::write(&tmp_file, data).await.map_err(|e| FileStoreError::Io {
            path: tmp_file.display().to_string(),
            reason: format!("write tmp: {}", e),
        })?;

        if let Err(e) = tokio::fs::rename(&tmp_file, target).await {
            let _ = tokio::fs::remove_file(&tmp_file).await;
            return Err(FileStoreError::Io {
                path: target.display().to_string(),
                reason: format!("rename: {}", e),
            });
        }

        Ok(())
    }
}

#[async_trait]
impl FileStore for FsFileStore {
    async fn put(&self, path: &str, data: Bytes) -> Result<u64, FileStoreError> {
        let size_bytes = data.len() as u64;
        let hash = Self::content_hash(&data);

        // Write blob (skip if already exists — deduplication)
        let blob = self.blob_path(&hash);
        if !blob.exists() {
            self.atomic_write(&blob, &data).await?;
            debug!("CAS: stored new blob {} ({} bytes)", &hash[..8], size_bytes);
        } else {
            debug!("CAS: dedup hit for {} ({} bytes saved)", &hash[..8], size_bytes);
        }

        // Write path → hash reference
        let ref_file = self.ref_path(path);
        self.atomic_write(&ref_file, hash.as_bytes()).await?;

        debug!("CAS: {} -> {}", path, &hash[..8]);

        Ok(size_bytes)
    }

    async fn get(&self, path: &str) -> Result<Bytes, FileStoreError> {
        // Read hash from reference file
        let ref_file = self.ref_path(path);
        let hash = tokio::fs::read_to_string(&ref_file).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                FileStoreError::NotFound {
                    path: path.to_string(),
                }
            } else {
                FileStoreError::Io {
                    path: path.to_string(),
                    reason: format!("read ref: {}", e),
                }
            }
        })?;

        // Read blob by hash
        let blob = self.blob_path(hash.trim());
        let data = tokio::fs::read(&blob).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                FileStoreError::NotFound {
                    path: path.to_string(),
                }
            } else {
                FileStoreError::Io {
                    path: path.to_string(),
                    reason: format!("read blob: {}", e),
                }
            }
        })?;

        Ok(Bytes::from(data))
    }

    async fn exists(&self, path: &str) -> Result<bool, FileStoreError> {
        let ref_file = self.ref_path(path);
        Ok(ref_file.exists())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store() -> (tempfile::TempDir, FsFileStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = FsFileStore::new(dir.path().join("cas"));
        (dir, store)
    }

    #[tokio::test]
    async fn test_put_and_get() {
        let (_dir, store) = temp_store();
        let data = Bytes::from("hello world");

        let size = store.put("assets/main.js", data.clone()).await.unwrap();
        assert_eq!(size, 11);

        let retrieved = store.get("assets/main.js").await.unwrap();
        assert_eq!(retrieved, data);
    }

    #[tokio::test]
    async fn test_deduplication() {
        let (_dir, store) = temp_store();
        let data = Bytes::from("shared vendor chunk");

        // Same content stored under two different paths
        store.put("v1/_next/static/vendor.js", data.clone()).await.unwrap();
        store.put("v2/_next/static/vendor.js", data.clone()).await.unwrap();

        // Both resolve to the same content
        assert_eq!(store.get("v1/_next/static/vendor.js").await.unwrap(), data);
        assert_eq!(store.get("v2/_next/static/vendor.js").await.unwrap(), data);

        // Only one blob on disk
        let hash = FsFileStore::content_hash(&data);
        let blob = store.blob_path(&hash);
        assert!(blob.exists());
    }

    #[tokio::test]
    async fn test_put_overwrites_ref() {
        let (_dir, store) = temp_store();

        store.put("style.css", Bytes::from("v1")).await.unwrap();
        store.put("style.css", Bytes::from("v2")).await.unwrap();

        let retrieved = store.get("style.css").await.unwrap();
        assert_eq!(retrieved, Bytes::from("v2"));
    }

    #[tokio::test]
    async fn test_nested_paths() {
        let (_dir, store) = temp_store();
        let data = Bytes::from("chunk");

        store
            .put("_next/static/chunks/main-abc123.js", data.clone())
            .await
            .unwrap();

        let retrieved = store
            .get("_next/static/chunks/main-abc123.js")
            .await
            .unwrap();
        assert_eq!(retrieved, data);
    }

    #[tokio::test]
    async fn test_exists() {
        let (_dir, store) = temp_store();
        store.put("test.js", Bytes::from("test")).await.unwrap();

        assert!(store.exists("test.js").await.unwrap());
        assert!(!store.exists("nope.js").await.unwrap());
    }

    #[tokio::test]
    async fn test_get_not_found() {
        let (_dir, store) = temp_store();
        let result = store.get("nonexistent.js").await;
        assert!(matches!(result, Err(FileStoreError::NotFound { .. })));
    }

    #[tokio::test]
    async fn test_leading_slash_stripped() {
        let (_dir, store) = temp_store();
        store
            .put("/assets/main.js", Bytes::from("js"))
            .await
            .unwrap();

        let retrieved = store.get("assets/main.js").await.unwrap();
        assert_eq!(retrieved, Bytes::from("js"));
    }

    #[tokio::test]
    async fn test_path_traversal_sanitized() {
        let (_dir, store) = temp_store();
        store
            .put("../etc/passwd", Bytes::from("nope"))
            .await
            .unwrap();

        assert!(store.exists("etc/passwd").await.unwrap());
    }

    #[tokio::test]
    async fn test_content_hash_deterministic() {
        let h1 = FsFileStore::content_hash(b"hello");
        let h2 = FsFileStore::content_hash(b"hello");
        let h3 = FsFileStore::content_hash(b"world");
        assert_eq!(h1, h2);
        assert_ne!(h1, h3);
        assert_eq!(h1.len(), 64); // SHA-256 hex = 64 chars
    }
}
