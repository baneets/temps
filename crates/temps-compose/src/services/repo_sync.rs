use git2::{build::RepoBuilder, Cred, FetchOptions, RemoteCallbacks};
use std::path::{Path, PathBuf};
use thiserror::Error;
use tracing::{debug, info};

#[derive(Error, Debug)]
pub enum RepoSyncError {
    #[error("Failed to clone repository from {url}: {reason}")]
    CloneFailed { url: String, reason: String },

    #[error("Compose file not found at path '{path}' in repository {url}")]
    ComposeFileNotFound { url: String, path: String },

    #[error("Failed to read compose file at '{path}': {reason}")]
    ReadFailed { path: String, reason: String },

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Sync a compose file from a git repository.
///
/// Clones the repo (shallow, single branch) into a temp dir, reads the compose
/// file at `compose_path`, and optionally reads `.env` if it exists next to it.
/// Returns (compose_content, env_content).
///
/// Uses `git2` (libgit2) directly, no git CLI required.
pub async fn sync_compose_from_repo(
    repo_url: &str,
    branch: Option<&str>,
    compose_path: &str,
    access_token: Option<&str>,
    work_dir: &Path,
) -> Result<(String, Option<String>), RepoSyncError> {
    let repo_url = repo_url.to_string();
    let branch = branch.map(|s| s.to_string());
    let compose_path = compose_path.to_string();
    let access_token = access_token.map(|s| s.to_string());
    let work_dir = work_dir.to_path_buf();

    // git2 operations are blocking, run in spawn_blocking
    let (compose_content, env_content) = tokio::task::spawn_blocking(move || {
        sync_blocking(
            &repo_url,
            branch.as_deref(),
            &compose_path,
            access_token.as_deref(),
            &work_dir,
        )
    })
    .await
    .map_err(|e| RepoSyncError::CloneFailed {
        url: "unknown".into(),
        reason: format!("Task join error: {}", e),
    })??;

    Ok((compose_content, env_content))
}

fn sync_blocking(
    repo_url: &str,
    branch: Option<&str>,
    compose_path: &str,
    access_token: Option<&str>,
    work_dir: &Path,
) -> Result<(String, Option<String>), RepoSyncError> {
    // Create a unique temp dir inside work_dir for the clone
    let clone_dir = work_dir.join(format!("repo-sync-{}", std::process::id()));
    if clone_dir.exists() {
        std::fs::remove_dir_all(&clone_dir).ok();
    }
    std::fs::create_dir_all(&clone_dir).map_err(|e| RepoSyncError::ReadFailed {
        path: clone_dir.display().to_string(),
        reason: e.to_string(),
    })?;

    debug!(url = %repo_url, branch = ?branch, path = %compose_path, "Cloning repository for compose sync");

    let _repo = clone_repo(repo_url, &clone_dir, branch, access_token)?;

    // Read the compose file
    let compose_file = clone_dir.join(compose_path);
    if !compose_file.exists() {
        // Clean up
        std::fs::remove_dir_all(&clone_dir).ok();
        return Err(RepoSyncError::ComposeFileNotFound {
            url: repo_url.to_string(),
            path: compose_path.to_string(),
        });
    }

    let compose_content =
        std::fs::read_to_string(&compose_file).map_err(|e| RepoSyncError::ReadFailed {
            path: compose_path.to_string(),
            reason: e.to_string(),
        })?;

    // Check for .env next to the compose file
    let env_content = compose_file
        .parent()
        .map(|dir| dir.join(".env"))
        .filter(|p| p.exists())
        .and_then(|p| std::fs::read_to_string(p).ok());

    info!(url = %repo_url, path = %compose_path, "Synced compose file from repository");

    // Clean up the clone dir
    std::fs::remove_dir_all(&clone_dir).ok();

    Ok((compose_content, env_content))
}

fn clone_repo(
    url: &str,
    target_dir: &Path,
    branch: Option<&str>,
    access_token: Option<&str>,
) -> Result<git2::Repository, RepoSyncError> {
    let mut builder = RepoBuilder::new();
    let mut fetch_opts = FetchOptions::new();

    if let Some(token) = access_token {
        let token = token.to_string();
        let mut callbacks = RemoteCallbacks::new();
        callbacks.credentials(move |_url, _username_from_url, _allowed_types| {
            Cred::userpass_plaintext("x-access-token", &token)
        });
        fetch_opts.remote_callbacks(callbacks);
    }

    if let Some(branch) = branch {
        builder.branch(branch);
        fetch_opts.depth(1);
    }

    builder.fetch_options(fetch_opts);

    builder
        .clone(url, target_dir)
        .map_err(|e| RepoSyncError::CloneFailed {
            url: url.to_string(),
            reason: e.message().to_string(),
        })
}

/// Discover all compose files in a git repository.
///
/// Clones the repo, walks the tree, and returns relative paths of all
/// files matching common compose file names.
pub async fn discover_compose_files(
    repo_url: &str,
    branch: Option<&str>,
    access_token: Option<&str>,
    work_dir: &Path,
) -> Result<Vec<String>, RepoSyncError> {
    let repo_url = repo_url.to_string();
    let branch = branch.map(|s| s.to_string());
    let access_token = access_token.map(|s| s.to_string());
    let work_dir = work_dir.to_path_buf();

    tokio::task::spawn_blocking(move || {
        discover_blocking(
            &repo_url,
            branch.as_deref(),
            access_token.as_deref(),
            &work_dir,
        )
    })
    .await
    .map_err(|e| RepoSyncError::CloneFailed {
        url: "unknown".into(),
        reason: format!("Task join error: {}", e),
    })?
}

const COMPOSE_FILE_NAMES: &[&str] = &[
    "docker-compose.yml",
    "docker-compose.yaml",
    "compose.yml",
    "compose.yaml",
];

fn discover_blocking(
    repo_url: &str,
    branch: Option<&str>,
    access_token: Option<&str>,
    work_dir: &Path,
) -> Result<Vec<String>, RepoSyncError> {
    let clone_dir = work_dir.join(format!("repo-discover-{}", std::process::id()));
    if clone_dir.exists() {
        std::fs::remove_dir_all(&clone_dir).ok();
    }
    std::fs::create_dir_all(&clone_dir).map_err(|e| RepoSyncError::ReadFailed {
        path: clone_dir.display().to_string(),
        reason: e.to_string(),
    })?;

    debug!(url = %repo_url, branch = ?branch, "Cloning repository to discover compose files");

    let _repo = clone_repo(repo_url, &clone_dir, branch, access_token)?;

    let mut compose_files = Vec::new();
    walk_for_compose_files(&clone_dir, &clone_dir, &mut compose_files);

    compose_files.sort();

    info!(url = %repo_url, count = compose_files.len(), "Discovered compose files in repository");

    std::fs::remove_dir_all(&clone_dir).ok();

    Ok(compose_files)
}

fn walk_for_compose_files(root: &Path, current: &Path, results: &mut Vec<String>) {
    let entries = match std::fs::read_dir(current) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();

        // Skip hidden dirs (.git, etc.)
        if path.is_dir() {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with('.') {
                    continue;
                }
            }
            walk_for_compose_files(root, &path, results);
        } else if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if COMPOSE_FILE_NAMES.contains(&name) {
                if let Ok(rel) = path.strip_prefix(root) {
                    results.push(rel.to_string_lossy().to_string());
                }
            }
        }
    }
}

/// Get the default temp directory for repo sync operations
pub fn repo_sync_work_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("repo-sync")
}
