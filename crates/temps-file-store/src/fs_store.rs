//! Content-addressable filesystem store.
//!
//! Layout:
//! ```text
//! {root}/
//!   blobs/{hash[0..2]}/{hash}                          <- deduplicated content
//!   manifests/{project}/{env}/{deployment}.json         <- path → hash maps
//!   .tmp/                                              <- atomic write staging
//! ```
//!
//! One blob per unique file content. One manifest per deployment.
//! 1000 deployments × 200 chunks = 1000 tiny JSON manifests + ~200 unique blobs
//! (most chunks are shared across deployments).

use async_trait::async_trait;
use bytes::Bytes;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;
use tracing::debug;

use crate::{FileStore, FileStoreError};

/// Content-addressable filesystem store.
pub struct FsFileStore {
    root: PathBuf,
}

impl FsFileStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &std::path::Path {
        &self.root
    }

    /// SHA-256 hex digest.
    pub fn content_hash(data: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(data);
        format!("{:x}", hasher.finalize())
    }

    /// Blob path: `blobs/{hash[0..2]}/{hash}`
    fn blob_path(&self, hash: &str) -> PathBuf {
        let prefix = &hash[..2.min(hash.len())];
        self.root.join("blobs").join(prefix).join(hash)
    }

    /// Manifest path: `manifests/{key}.json`
    fn manifest_path(&self, key: &str) -> PathBuf {
        let clean: PathBuf = key
            .trim_start_matches('/')
            .split('/')
            .filter(|seg| !seg.is_empty() && *seg != ".." && *seg != ".")
            .collect();
        self.root.join("manifests").join(format!("{}.json", clean.display()))
    }

    fn tmp_dir(&self) -> PathBuf {
        self.root.join(".tmp")
    }

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

    /// Recursively find all .json files under manifests/
    fn find_manifests_recursive(dir: &std::path::Path, base: &std::path::Path) -> Vec<String> {
        let mut results = Vec::new();
        let Ok(entries) = std::fs::read_dir(dir) else {
            return results;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                results.extend(Self::find_manifests_recursive(&path, base));
            } else if path.extension().map(|e| e == "json").unwrap_or(false) {
                if let Ok(rel) = path.strip_prefix(base) {
                    // Convert path back to key: strip .json extension
                    let key = rel.with_extension("").display().to_string();
                    results.push(key);
                }
            }
        }
        results
    }
}

#[async_trait]
impl FileStore for FsFileStore {
    async fn put_blob(&self, data: Bytes) -> Result<String, FileStoreError> {
        let hash = Self::content_hash(&data);
        let blob = self.blob_path(&hash);

        if !blob.exists() {
            self.atomic_write(&blob, &data).await?;
            debug!("CAS: stored blob {} ({} bytes)", &hash[..8], data.len());
        } else {
            debug!("CAS: dedup hit {} ({} bytes saved)", &hash[..8], data.len());
        }

        Ok(hash)
    }

    async fn get_blob(&self, hash: &str) -> Result<Bytes, FileStoreError> {
        let blob = self.blob_path(hash.trim());
        let data = tokio::fs::read(&blob).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                FileStoreError::NotFound { path: hash.to_string() }
            } else {
                FileStoreError::Io {
                    path: hash.to_string(),
                    reason: format!("read blob: {}", e),
                }
            }
        })?;
        Ok(Bytes::from(data))
    }

    async fn blob_exists(&self, hash: &str) -> Result<bool, FileStoreError> {
        Ok(self.blob_path(hash.trim()).exists())
    }

    async fn put_manifest(
        &self,
        manifest_key: &str,
        entries: &HashMap<String, String>,
    ) -> Result<(), FileStoreError> {
        let json = serde_json::to_vec(entries).map_err(|e| FileStoreError::Backend(e.to_string()))?;
        let path = self.manifest_path(manifest_key);
        self.atomic_write(&path, &json).await?;
        debug!("CAS: wrote manifest {} ({} entries)", manifest_key, entries.len());
        Ok(())
    }

    async fn get_manifest(
        &self,
        manifest_key: &str,
    ) -> Result<HashMap<String, String>, FileStoreError> {
        let path = self.manifest_path(manifest_key);
        let data = tokio::fs::read(&path).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                FileStoreError::NotFound { path: manifest_key.to_string() }
            } else {
                FileStoreError::Io {
                    path: manifest_key.to_string(),
                    reason: format!("read manifest: {}", e),
                }
            }
        })?;
        serde_json::from_slice(&data).map_err(|e| FileStoreError::Backend(e.to_string()))
    }

    async fn list_manifests(&self) -> Result<Vec<String>, FileStoreError> {
        let manifests_dir = self.root.join("manifests");
        if !manifests_dir.exists() {
            return Ok(Vec::new());
        }
        Ok(Self::find_manifests_recursive(&manifests_dir, &manifests_dir))
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
    async fn test_put_and_get_blob() {
        let (_dir, store) = temp_store();
        let data = Bytes::from("hello world");

        let hash = store.put_blob(data.clone()).await.unwrap();
        assert_eq!(hash.len(), 64);

        let retrieved = store.get_blob(&hash).await.unwrap();
        assert_eq!(retrieved, data);
    }

    #[tokio::test]
    async fn test_deduplication() {
        let (_dir, store) = temp_store();
        let data = Bytes::from("shared vendor chunk");

        let h1 = store.put_blob(data.clone()).await.unwrap();
        let h2 = store.put_blob(data.clone()).await.unwrap();
        assert_eq!(h1, h2);

        // Only one blob on disk
        let blob = store.blob_path(&h1);
        assert!(blob.exists());
    }

    #[tokio::test]
    async fn test_manifest_roundtrip() {
        let (_dir, store) = temp_store();

        let mut entries = HashMap::new();
        entries.insert("_next/static/chunks/main-abc.js".to_string(), "deadbeef".to_string());
        entries.insert("_next/static/css/app.css".to_string(), "cafebabe".to_string());

        store.put_manifest("1/1/100", &entries).await.unwrap();

        let loaded = store.get_manifest("1/1/100").await.unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded["_next/static/chunks/main-abc.js"], "deadbeef");
    }

    #[tokio::test]
    async fn test_list_manifests() {
        let (_dir, store) = temp_store();
        let entries = HashMap::new();

        store.put_manifest("1/1/100", &entries).await.unwrap();
        store.put_manifest("1/1/101", &entries).await.unwrap();
        store.put_manifest("2/3/200", &entries).await.unwrap();

        let mut manifests = store.list_manifests().await.unwrap();
        manifests.sort();
        assert_eq!(manifests.len(), 3);
        assert!(manifests.contains(&"1/1/100".to_string()));
        assert!(manifests.contains(&"1/1/101".to_string()));
        assert!(manifests.contains(&"2/3/200".to_string()));
    }

    #[tokio::test]
    async fn test_get_resolves_through_manifest() {
        let (_dir, store) = temp_store();
        let data = Bytes::from("chunk content");

        let hash = store.put_blob(data.clone()).await.unwrap();

        let mut entries = HashMap::new();
        entries.insert("_next/static/chunks/main.js".to_string(), hash.clone());
        store.put_manifest("1/1/100", &entries).await.unwrap();

        // get() should resolve path → manifest → blob
        let retrieved = store.get("_next/static/chunks/main.js").await.unwrap();
        assert_eq!(retrieved, data);
    }

    #[tokio::test]
    async fn test_get_not_found() {
        let (_dir, store) = temp_store();
        let result = store.get("nonexistent.js").await;
        assert!(matches!(result, Err(FileStoreError::NotFound { .. })));
    }

    #[tokio::test]
    async fn test_end_to_end_two_deployments() {
        let (_dir, store) = temp_store();

        // Deployment 1: 3 chunks
        let vendor = Bytes::from("vendor code shared");
        let main_v1 = Bytes::from("main v1");
        let css = Bytes::from("styles");

        let h_vendor = store.put_blob(vendor.clone()).await.unwrap();
        let h_main_v1 = store.put_blob(main_v1.clone()).await.unwrap();
        let h_css = store.put_blob(css.clone()).await.unwrap();

        let mut m1 = HashMap::new();
        m1.insert("_next/static/chunks/vendor.js".to_string(), h_vendor.clone());
        m1.insert("_next/static/chunks/main.js".to_string(), h_main_v1.clone());
        m1.insert("_next/static/css/app.css".to_string(), h_css.clone());
        store.put_manifest("1/1/100", &m1).await.unwrap();

        // Deployment 2: vendor unchanged, main updated
        let main_v2 = Bytes::from("main v2");
        let h_main_v2 = store.put_blob(main_v2.clone()).await.unwrap();
        // vendor and css dedup — same hash returned
        let h_vendor_2 = store.put_blob(vendor.clone()).await.unwrap();
        let h_css_2 = store.put_blob(css.clone()).await.unwrap();
        assert_eq!(h_vendor, h_vendor_2);
        assert_eq!(h_css, h_css_2);

        let mut m2 = HashMap::new();
        m2.insert("_next/static/chunks/vendor.js".to_string(), h_vendor.clone());
        m2.insert("_next/static/chunks/main.js".to_string(), h_main_v2.clone());
        m2.insert("_next/static/css/app.css".to_string(), h_css.clone());
        store.put_manifest("1/1/101", &m2).await.unwrap();

        // Blobs on disk: vendor, main_v1, main_v2, css = 4 blobs (not 6)
        assert!(store.blob_exists(&h_vendor).await.unwrap());
        assert!(store.blob_exists(&h_main_v1).await.unwrap());
        assert!(store.blob_exists(&h_main_v2).await.unwrap());
        assert!(store.blob_exists(&h_css).await.unwrap());

        // Both deployment manifests accessible
        assert_eq!(store.get_manifest("1/1/100").await.unwrap().len(), 3);
        assert_eq!(store.get_manifest("1/1/101").await.unwrap().len(), 3);
    }

    #[tokio::test]
    async fn test_content_hash_deterministic() {
        let h1 = FsFileStore::content_hash(b"hello");
        let h2 = FsFileStore::content_hash(b"hello");
        let h3 = FsFileStore::content_hash(b"world");
        assert_eq!(h1, h2);
        assert_ne!(h1, h3);
        assert_eq!(h1.len(), 64);
    }
}
