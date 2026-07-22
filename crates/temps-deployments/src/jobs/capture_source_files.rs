//! Capture Source Files Job
//!
//! Uploads raw application source from the git checkout so native (Go, Rust,
//! Python, …) stack frames can be shown with source context — the counterpart
//! to [`super::capture_source_maps`], which extracts `.map` files from the built
//! image for JavaScript. Native source is NOT in the compiled image (the binary
//! strips it), so this reads the `DownloadRepoJob` checkout (`repo_dir`) instead.
//!
//! Runs as a post-deployment job (never blocks the pipeline) and is only
//! scheduled when the project has opted into source context, so it is a no-op
//! for everyone else.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use temps_core::{JobResult, WorkflowContext, WorkflowError, WorkflowTask};
use temps_error_tracking::services::SourceMapService;
use temps_logs::{LogLevel, LogService};
use tracing::{debug, info, warn};

/// Directory names never walked for source (build output, deps, VCS metadata).
const SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "vendor",
    "target",
    "dist",
    "build",
    ".next",
    ".nuxt",
    ".venv",
    "venv",
    "__pycache__",
    ".temps",
];

/// Safety bounds so a huge repo can't exhaust memory/storage on a small box.
const MAX_FILES: usize = 5000;
const MAX_FILE_SIZE: u64 = 2 * 1024 * 1024; // 2 MB per source file

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureSourceFilesOutput {
    pub source_files_captured: u32,
    pub total_size_bytes: u64,
    pub release: String,
}

/// Job that uploads raw source files from the git checkout for native
/// (non-source-map) stack-trace symbolication.
pub struct CaptureSourceFilesJob {
    job_id: String,
    project_id: i32,
    release: String,
    /// Job whose `repo_dir` output holds the checkout path (the DownloadRepoJob).
    download_job_id: String,
    /// Project subdirectory (module root) within the checkout; frame paths are
    /// reported relative to this, so files are keyed relative to it too.
    project_directory: String,
    /// File extensions (without a dot) to upload.
    extensions: Vec<String>,
    source_map_service: Arc<SourceMapService>,
    log_id: Option<String>,
    log_service: Option<Arc<LogService>>,
}

impl std::fmt::Debug for CaptureSourceFilesJob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CaptureSourceFilesJob")
            .field("job_id", &self.job_id)
            .field("project_id", &self.project_id)
            .field("release", &self.release)
            .field("download_job_id", &self.download_job_id)
            .field("project_directory", &self.project_directory)
            .field("extensions", &self.extensions)
            .finish()
    }
}

impl CaptureSourceFilesJob {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        job_id: String,
        project_id: i32,
        release: String,
        download_job_id: String,
        project_directory: String,
        extensions: Vec<String>,
        source_map_service: Arc<SourceMapService>,
    ) -> Self {
        Self {
            job_id,
            project_id,
            release,
            download_job_id,
            project_directory,
            extensions,
            source_map_service,
            log_id: None,
            log_service: None,
        }
    }

    pub fn with_log_id(mut self, log_id: String) -> Self {
        self.log_id = Some(log_id);
        self
    }

    pub fn with_log_service(mut self, log_service: Arc<LogService>) -> Self {
        self.log_service = Some(log_service);
        self
    }

    async fn log(&self, message: String) -> Result<(), WorkflowError> {
        let level = Self::detect_log_level(&message);
        if let (Some(log_id), Some(log_service)) = (&self.log_id, &self.log_service) {
            log_service
                .append_structured_log(log_id, level, message)
                .await
                .map_err(|e| {
                    WorkflowError::JobExecutionFailed(format!("Failed to write log: {}", e))
                })?;
        }
        Ok(())
    }

    fn detect_log_level(message: &str) -> LogLevel {
        if message.contains("✅") || message.contains("success") {
            LogLevel::Success
        } else if message.contains("❌") || message.contains("Failed") {
            LogLevel::Error
        } else if message.contains("⚠️") {
            LogLevel::Warning
        } else {
            LogLevel::Info
        }
    }
}

/// Recursively collect files under `root` whose extension is in `exts`,
/// skipping [`SKIP_DIRS`]. Returns `(relative_path, absolute_path)` pairs, with
/// the relative path using forward slashes (to match stack-frame filenames).
/// Stops at [`MAX_FILES`].
fn collect_source_files(root: &Path, exts: &[String]) -> Vec<(String, PathBuf)> {
    fn walk(dir: &Path, root: &Path, exts: &[String], out: &mut Vec<(String, PathBuf)>) {
        if out.len() >= MAX_FILES {
            return;
        }
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            if out.len() >= MAX_FILES {
                return;
            }
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if path.is_dir() {
                if SKIP_DIRS.contains(&name.as_ref()) {
                    continue;
                }
                walk(&path, root, exts, out);
            } else {
                let matches = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| exts.iter().any(|want| want.eq_ignore_ascii_case(e)))
                    .unwrap_or(false);
                if matches {
                    if let Ok(rel) = path.strip_prefix(root) {
                        let rel = rel.to_string_lossy().replace('\\', "/");
                        out.push((rel, path));
                    }
                }
            }
        }
    }

    let mut out = Vec::new();
    walk(root, root, exts, &mut out);
    out
}

#[async_trait]
impl WorkflowTask for CaptureSourceFilesJob {
    fn job_id(&self) -> &str {
        &self.job_id
    }

    fn name(&self) -> &str {
        "Capture Source Files"
    }

    fn description(&self) -> &str {
        "Upload application source from the checkout for native error symbolication"
    }

    async fn execute(&self, mut context: WorkflowContext) -> Result<JobResult, WorkflowError> {
        info!(
            "Starting source file capture for project {} (release: {})",
            self.project_id, self.release
        );
        self.log("📄 Capturing source files for native symbolication...".to_string())
            .await?;

        // The checkout dir is the DownloadRepoJob's `repo_dir` output. If it is
        // gone (e.g. an image deploy with no checkout), there is nothing to do.
        let repo_dir = match context.get_output::<String>(&self.download_job_id, "repo_dir")? {
            Some(dir) => PathBuf::from(dir),
            None => {
                self.log("⚠️ No checkout available; skipping source file capture".to_string())
                    .await?;
                return Ok(JobResult::success(context));
            }
        };

        // Resolve the module root (project subdirectory within the checkout).
        let source_root = {
            let dir = self.project_directory.trim_matches('/');
            if dir.is_empty() || dir == "." {
                repo_dir.clone()
            } else {
                repo_dir.join(dir)
            }
        };
        if !source_root.is_dir() {
            self.log(format!(
                "⚠️ Source root {} not found; skipping",
                source_root.display()
            ))
            .await?;
            return Ok(JobResult::success(context));
        }

        let files = collect_source_files(&source_root, &self.extensions);
        if files.len() >= MAX_FILES {
            self.log(format!(
                "⚠️ Source file cap reached ({} files); uploading the first {}",
                MAX_FILES, MAX_FILES
            ))
            .await?;
        }

        let mut captured: u32 = 0;
        let mut total_size: u64 = 0;
        let mut skipped_large: u32 = 0;

        for (rel_path, abs_path) in files {
            let content = match tokio::fs::read(&abs_path).await {
                Ok(c) => c,
                Err(e) => {
                    warn!("Failed to read {}: {}", abs_path.display(), e);
                    continue;
                }
            };
            if content.len() as u64 > MAX_FILE_SIZE {
                skipped_large += 1;
                continue;
            }
            total_size += content.len() as u64;

            match self
                .source_map_service
                .upload_source_file(self.project_id, &self.release, &rel_path, content)
                .await
            {
                Ok(_) => captured += 1,
                Err(e) => {
                    warn!("Failed to upload source file '{}': {}", rel_path, e);
                }
            }
        }

        if skipped_large > 0 {
            self.log(format!(
                "⚠️ Skipped {} file(s) larger than {} bytes",
                skipped_large, MAX_FILE_SIZE
            ))
            .await?;
        }

        self.log(format!(
            "✅ Captured {} source file(s) for release '{}' ({} bytes)",
            captured, self.release, total_size
        ))
        .await?;

        // Drop source for releases no longer tied to an active deployment.
        match self
            .source_map_service
            .delete_stale_source_files(self.project_id)
            .await
        {
            Ok(n) if n > 0 => {
                debug!(
                    "Removed {} stale source file(s) for project {}",
                    n, self.project_id
                );
            }
            Ok(_) => {}
            Err(e) => warn!("Stale source file cleanup failed: {}", e),
        }

        context.set_output(&self.job_id, "source_files_captured", captured)?;
        context.set_output(&self.job_id, "total_size_bytes", total_size)?;
        Ok(JobResult::success(context))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collect_skips_build_and_vcs_dirs_and_filters_extensions() {
        let root = std::env::temp_dir().join(format!("tsf-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("internal/gateway")).unwrap();
        std::fs::create_dir_all(root.join("node_modules/foo")).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join("main.go"), "package main").unwrap();
        std::fs::write(root.join("internal/gateway/mw.go"), "package gateway").unwrap();
        std::fs::write(root.join("README.md"), "# docs").unwrap();
        std::fs::write(root.join("node_modules/foo/index.go"), "skip me").unwrap();
        std::fs::write(root.join(".git/config"), "skip").unwrap();

        let exts = vec!["go".to_string()];
        let mut found: Vec<String> = collect_source_files(&root, &exts)
            .into_iter()
            .map(|(rel, _)| rel)
            .collect();
        found.sort();

        assert_eq!(found, vec!["internal/gateway/mw.go", "main.go"]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
