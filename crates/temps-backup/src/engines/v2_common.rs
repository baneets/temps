//! Shared helpers for `engine_v2`-style backup engines.
//!
//! Every engine ported off the queue follows the same shape:
//!
//! 1. Validate params + look up S3 source.
//! 2. Build an S3 client (decrypting credentials at rest).
//! 3. Run a one-shot Docker container (`super::oneshot::run_one_shot`).
//! 4. Upload the resulting file to S3, single-part or multipart.
//! 5. Write a `metadata.json` companion object.
//!
//! Steps 2, 4, and 5 are identical across engines. Pull them in from here
//! so the per-engine code only owns step 3 (its specific Docker command)
//! and the param-validation in step 1.

use std::sync::{Arc, OnceLock};

use aws_sdk_s3::config::SharedHttpClient;
use aws_sdk_s3::Client as S3Client;
use aws_smithy_http_client::tls::{
    rustls_provider::CryptoMode, Provider as TlsProvider, TlsContext, TrustStore,
};
use chrono::Utc;
use serde_json::{json, Value};
use tracing::warn;

use temps_backup_core::engine_v2::BackupError;
use temps_core::EncryptionService;

/// Shared HTTPS client backed by the Mozilla CA bundle compiled in via
/// `webpki-root-certs`. Built once on first use, then reused for every
/// S3 client this crate constructs.
///
/// We bypass the SDK's default-https-client because it asks the OS for
/// trusted roots via `rustls-native-certs`. On some macOS dev machines
/// that returns zero parsed certs and `aws-smithy-http-client` then trips
/// a `debug_assert!`, panicking every test that touches the S3 builder.
/// Pinning a deterministic trust bundle makes the client constructable
/// in any environment (dev macOS, CI sandbox, minimal Linux container)
/// without depending on the OS trust store.
pub(crate) fn bundled_roots_http_client() -> SharedHttpClient {
    static CLIENT: OnceLock<SharedHttpClient> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            let mut trust_store = TrustStore::empty().with_native_roots(false);
            for der in webpki_root_certs::TLS_SERVER_ROOT_CERTS {
                let pem = pem::Pem::new("CERTIFICATE", der.to_vec());
                trust_store = trust_store.with_pem_certificate(pem::encode(&pem).into_bytes());
            }
            let tls_context = TlsContext::builder()
                .with_trust_store(trust_store)
                .build()
                .expect("static TLS context built from bundled roots");
            aws_smithy_http_client::Builder::new()
                .tls_provider(TlsProvider::Rustls(CryptoMode::AwsLc))
                .tls_context(tls_context)
                .build_https()
        })
        .clone()
}

/// Multipart upload threshold. Files larger than this use multipart
/// upload instead of a single PUT.
pub const MULTIPART_THRESHOLD: i64 = 30 * 1024 * 1024;

/// S3 object tags applied to every backup upload. These drive tag-based
/// `BucketLifecycleConfiguration` rules so S3 (or compatible storage)
/// expires backups even when temps is offline.
///
/// Tag keys are namespaced with `temps-` so user-managed rules on the
/// same bucket don't collide. Values are kept simple (digits/words) to
/// avoid URL-encoding surprises across providers.
#[derive(Debug, Clone, Default)]
pub struct BackupTags {
    /// Retention in days. `None` means the backup is kept indefinitely
    /// from S3's perspective — only app-side deletion removes it.
    pub retention_days: Option<i32>,
    /// The schedule that produced this backup, if any.
    pub schedule_id: Option<i32>,
    /// The backup row id, for traceability in the S3 console.
    pub backup_id: Option<i32>,
}

impl BackupTags {
    /// Load tag context for `backup_id` from the database. Looks up the
    /// backup row to find `schedule_id`, then resolves
    /// `schedule.retention_period`. Ad-hoc backups (no schedule) get
    /// `retention_days = None` which renders as `temps-retention-days=never`.
    /// Returns a best-effort tag set even on partial DB failure — tagging
    /// is observability/lifecycle plumbing, never a reason to fail the
    /// upload.
    pub async fn load_for_backup(db: &sea_orm::DatabaseConnection, backup_id: i32) -> Self {
        use sea_orm::EntityTrait;
        let mut tags = BackupTags {
            retention_days: None,
            schedule_id: None,
            backup_id: Some(backup_id),
        };
        let backup = match temps_entities::backups::Entity::find_by_id(backup_id)
            .one(db)
            .await
        {
            Ok(Some(b)) => b,
            _ => return tags,
        };
        let Some(sched_id) = backup.schedule_id else {
            return tags;
        };
        tags.schedule_id = Some(sched_id);
        if let Ok(Some(s)) = temps_entities::backup_schedules::Entity::find_by_id(sched_id)
            .one(db)
            .await
        {
            if s.retention_period > 0 {
                tags.retention_days = Some(s.retention_period);
            }
        }
        tags
    }

    /// Render the tag set as the `Tagging` HTTP header / SDK param
    /// (`k1=v1&k2=v2`, URL-encoded). Every backup carries
    /// `temps-managed=true` so lifecycle rules can target only objects
    /// temps wrote.
    pub fn to_tagging_string(&self) -> String {
        let mut parts: Vec<String> = Vec::with_capacity(4);
        parts.push("temps-managed=true".to_string());
        match self.retention_days {
            Some(days) if days > 0 => {
                parts.push(format!("temps-retention-days={}", days));
            }
            _ => {
                parts.push("temps-retention-days=never".to_string());
            }
        }
        if let Some(id) = self.schedule_id {
            parts.push(format!("temps-schedule-id={}", id));
        }
        if let Some(id) = self.backup_id {
            parts.push(format!("temps-backup-id={}", id));
        }
        parts.join("&")
    }
}

// ── S3 client construction ───────────────────────────────────────────────────

/// Load an S3 source row from the database. Maps not-found and DB errors
/// onto `BackupError` variants — not-found is permanent, DB-down is
/// transient.
pub async fn load_s3_source(
    db: &sea_orm::DatabaseConnection,
    s3_source_id: i32,
) -> Result<temps_entities::s3_sources::Model, BackupError> {
    use sea_orm::EntityTrait;
    temps_entities::s3_sources::Entity::find_by_id(s3_source_id)
        .one(db)
        .await
        .map_err(|e| BackupError::Failed {
            reason: format!(
                "database error looking up s3_source {}: {}",
                s3_source_id, e
            ),
        })?
        .ok_or_else(|| BackupError::PermanentFailure {
            reason: format!("s3_source {} not found", s3_source_id),
        })
}

/// Build an S3 client from an already-loaded S3 source row. Decrypts the
/// access/secret keys via the supplied `EncryptionService` at call time —
/// the engine never holds plaintext credentials beyond this point.
pub fn build_s3_client(
    s3_source: &temps_entities::s3_sources::Model,
    encryption_service: &Arc<EncryptionService>,
    user_agent: &'static str,
) -> Result<S3Client, BackupError> {
    use aws_sdk_s3::Config;

    let access_key = encryption_service
        .decrypt_string(&s3_source.access_key_id)
        .map_err(|e| BackupError::PermanentFailure {
            reason: format!("failed to decrypt S3 access key: {}", e),
        })?;
    let secret_key = encryption_service
        .decrypt_string(&s3_source.secret_key)
        .map_err(|e| BackupError::PermanentFailure {
            reason: format!("failed to decrypt S3 secret key: {}", e),
        })?;

    let creds =
        aws_sdk_s3::config::Credentials::new(access_key, secret_key, None, None, user_agent);

    let mut builder = Config::builder()
        .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
        .region(aws_sdk_s3::config::Region::new(s3_source.region.clone()))
        .force_path_style(s3_source.force_path_style.unwrap_or(true))
        .credentials_provider(creds)
        .http_client(bundled_roots_http_client());

    if let Some(endpoint) = &s3_source.endpoint {
        let url = if endpoint.starts_with("http") {
            endpoint.clone()
        } else {
            format!("http://{}", endpoint)
        };
        builder = builder.endpoint_url(url);
    }

    Ok(S3Client::from_conf(builder.build()))
}

/// Convenience wrapper: load the row + build the client in one call. Most
/// engines use this; only engines that need to inspect the row outside the
/// client (e.g. for the bucket name) call `load_s3_source` + `build_s3_client`
/// separately.
pub async fn load_and_build_s3_client(
    db: &sea_orm::DatabaseConnection,
    encryption_service: &Arc<EncryptionService>,
    s3_source_id: i32,
    user_agent: &'static str,
) -> Result<(temps_entities::s3_sources::Model, S3Client), BackupError> {
    let row = load_s3_source(db, s3_source_id).await?;
    let client = build_s3_client(&row, encryption_service, user_agent)?;
    Ok((row, client))
}

/// HEAD-bucket reachability check — cheap, fails fast on misconfigured S3
/// credentials or unreachable endpoint.
pub async fn assert_bucket_reachable(client: &S3Client, bucket: &str) -> Result<(), BackupError> {
    client
        .head_bucket()
        .bucket(bucket)
        .send()
        .await
        .map_err(|e| BackupError::Failed {
            reason: format!("S3 bucket '{}' is not reachable: {}", bucket, e),
        })?;
    Ok(())
}

// ── S3 key derivation ────────────────────────────────────────────────────────

/// Build a dump S3 key for a **control-plane** backup.
///
/// Pattern: `<bucket_path>/backups/YYYY/MM/DD/<uuid>/<filename>`
pub fn build_dump_s3_key(bucket_path: &str, backup_uuid: &str, filename: &str) -> String {
    let prefix = bucket_path.trim_matches('/');
    let date = Utc::now().format("%Y/%m/%d");
    if prefix.is_empty() {
        format!("backups/{}/{}/{}", date, backup_uuid, filename)
    } else {
        format!("{}/backups/{}/{}/{}", prefix, date, backup_uuid, filename)
    }
}

/// Build a dump S3 key for an **external service** backup.
///
/// Pattern: `<bucket_path>/external_services/<engine>/<service_name>/YYYY/MM/DD/<uuid>/<filename>`
///
/// The per-engine sub-prefix (`postgres`, `redis`, `mongodb`, ...) lives in
/// `engine`. Including the uuid in the path means concurrent or same-day
/// backups of the same service write to distinct keys, so the
/// idempotent-skip check in `upload_*` only fires for genuine resumes.
pub fn build_external_service_s3_key(
    bucket_path: &str,
    engine: &str,
    service_name: &str,
    backup_uuid: &str,
    filename: &str,
) -> String {
    let prefix = bucket_path.trim_matches('/');
    let date = Utc::now().format("%Y/%m/%d");
    if prefix.is_empty() {
        format!(
            "external_services/{}/{}/{}/{}/{}",
            engine, service_name, date, backup_uuid, filename
        )
    } else {
        format!(
            "{}/external_services/{}/{}/{}/{}/{}",
            prefix, engine, service_name, date, backup_uuid, filename
        )
    }
}

/// Derive the `metadata.json` companion key from a dump key by replacing
/// the last path segment with `metadata.json`.
pub fn derive_metadata_key(dump_key: &str) -> String {
    let parts: Vec<&str> = dump_key.rsplitn(2, '/').collect();
    if parts.len() == 2 {
        format!("{}/metadata.json", parts[1])
    } else {
        format!("{}.metadata.json", dump_key)
    }
}

// ── Shell escaping ───────────────────────────────────────────────────────────

/// POSIX-safe single-quote escape for embedding in `sh -c` strings.
pub fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

// ── S3 upload ────────────────────────────────────────────────────────────────

/// Single-part PUT upload. Use for files under [`MULTIPART_THRESHOLD`].
pub async fn upload_single_part(
    client: &S3Client,
    bucket: &str,
    key: &str,
    path: &str,
    content_type: &str,
    tags: Option<&BackupTags>,
) -> Result<(), BackupError> {
    let body = aws_sdk_s3::primitives::ByteStream::from_path(std::path::Path::new(path))
        .await
        .map_err(|e| BackupError::Failed {
            reason: format!("failed to create byte stream from {}: {}", path, e),
        })?;

    let mut req = client
        .put_object()
        .bucket(bucket)
        .key(key)
        .body(body)
        .content_type(content_type);
    if let Some(tags) = tags {
        req = req.tagging(tags.to_tagging_string());
    }
    req.send().await.map_err(|e| BackupError::Failed {
        reason: format!(
            "single-part upload to s3://{}/{} failed: {}",
            bucket, key, e
        ),
    })?;
    Ok(())
}

/// Multipart upload. Use for files over [`MULTIPART_THRESHOLD`]. Aborts the
/// upload on any per-part failure so the bucket does not accumulate
/// dangling multipart uploads after a transient error.
pub async fn upload_multipart(
    client: &S3Client,
    bucket: &str,
    key: &str,
    path: &str,
    content_type: &str,
    tags: Option<&BackupTags>,
) -> Result<(), BackupError> {
    use tokio_stream::StreamExt as TokioStreamExt;

    let mut create_req = client
        .create_multipart_upload()
        .bucket(bucket)
        .key(key)
        .content_type(content_type);
    if let Some(tags) = tags {
        create_req = create_req.tagging(tags.to_tagging_string());
    }
    let create_resp = create_req.send().await.map_err(|e| BackupError::Failed {
        reason: format!("create_multipart_upload failed: {}", e),
    })?;

    let upload_id = create_resp.upload_id().ok_or_else(|| BackupError::Failed {
        reason: "create_multipart_upload returned no upload_id".into(),
    })?;

    let file = tokio::fs::File::open(path)
        .await
        .map_err(|e| BackupError::Failed {
            reason: format!("failed to open {} for multipart upload: {}", path, e),
        })?;
    let reader = tokio::io::BufReader::new(file);
    let mut stream = tokio_util::io::ReaderStream::new(reader);

    const CHUNK_SIZE: usize = 5 * 1024 * 1024; // 5 MB
    let mut buffer = Vec::with_capacity(CHUNK_SIZE);
    let mut part_number = 1i32;
    let mut parts = aws_sdk_s3::types::CompletedMultipartUpload::builder();

    while let Some(chunk_result) = TokioStreamExt::next(&mut stream).await {
        let chunk = chunk_result.map_err(|e| BackupError::Failed {
            reason: format!("read error during multipart upload: {}", e),
        })?;
        buffer.extend_from_slice(&chunk);

        if buffer.len() >= CHUNK_SIZE {
            let data = std::mem::take(&mut buffer);
            buffer.reserve(CHUNK_SIZE);

            let part_resp = client
                .upload_part()
                .bucket(bucket)
                .key(key)
                .upload_id(upload_id)
                .part_number(part_number)
                .body(data.into())
                .send()
                .await
                .map_err(|e| {
                    abort_multipart_detached(client.clone(), bucket, key, upload_id);
                    BackupError::Failed {
                        reason: format!("upload_part {} failed: {}", part_number, e),
                    }
                })?;

            let completed_part = aws_sdk_s3::types::CompletedPart::builder()
                .e_tag(part_resp.e_tag().unwrap_or(""))
                .part_number(part_number)
                .build();
            parts = parts.parts(completed_part);
            part_number += 1;
        }
    }

    if !buffer.is_empty() {
        let part_resp = client
            .upload_part()
            .bucket(bucket)
            .key(key)
            .upload_id(upload_id)
            .part_number(part_number)
            .body(buffer.into())
            .send()
            .await
            .map_err(|e| {
                abort_multipart_detached(client.clone(), bucket, key, upload_id);
                BackupError::Failed {
                    reason: format!("upload_part {} (final) failed: {}", part_number, e),
                }
            })?;
        let completed_part = aws_sdk_s3::types::CompletedPart::builder()
            .e_tag(part_resp.e_tag().unwrap_or(""))
            .part_number(part_number)
            .build();
        parts = parts.parts(completed_part);
    }

    client
        .complete_multipart_upload()
        .bucket(bucket)
        .key(key)
        .upload_id(upload_id)
        .multipart_upload(parts.build())
        .send()
        .await
        .map_err(|e| BackupError::Failed {
            reason: format!("complete_multipart_upload failed: {}", e),
        })?;

    Ok(())
}

/// Auto-route between single-part and multipart based on file size.
pub async fn upload_file(
    client: &S3Client,
    bucket: &str,
    key: &str,
    path: &str,
    content_type: &str,
    file_size: i64,
    tags: Option<&BackupTags>,
) -> Result<(), BackupError> {
    if file_size > MULTIPART_THRESHOLD {
        upload_multipart(client, bucket, key, path, content_type, tags).await
    } else {
        upload_single_part(client, bucket, key, path, content_type, tags).await
    }
}

fn abort_multipart_detached(client: S3Client, bucket: &str, key: &str, upload_id: &str) {
    let bucket = bucket.to_string();
    let key = key.to_string();
    let upload_id = upload_id.to_string();
    tokio::spawn(async move {
        let _ = client
            .abort_multipart_upload()
            .bucket(&bucket)
            .key(&key)
            .upload_id(&upload_id)
            .send()
            .await;
    });
}

// ── Metadata.json companion ──────────────────────────────────────────────────

/// Upload a `metadata.json` companion object next to the dump.
///
/// The body has a uniform shape across engines so the restore path can
/// inspect any dump's metadata without engine-specific decoding.
#[allow(clippy::too_many_arguments)]
pub async fn write_metadata_companion(
    client: &S3Client,
    bucket: &str,
    metadata_key: &str,
    engine: &str,
    backup_uuid: &str,
    dump_key: &str,
    size_bytes: i64,
    s3_source_id: i32,
    compression: &str,
    extra: Option<Value>,
) -> Result<(), BackupError> {
    let mut metadata = json!({
        "backup_uuid": backup_uuid,
        "type": "full",
        "engine": engine,
        "created_at": Utc::now().to_rfc3339(),
        "size_bytes": size_bytes,
        "compression_type": compression,
        "source": { "id": s3_source_id },
        "s3_location": dump_key,
    });
    if let (Some(extra), Some(obj)) = (extra, metadata.as_object_mut()) {
        if let Some(extra_obj) = extra.as_object() {
            for (k, v) in extra_obj {
                obj.insert(k.clone(), v.clone());
            }
        }
    }
    let body = serde_json::to_vec(&metadata).map_err(|e| BackupError::Failed {
        reason: format!("failed to serialise metadata.json: {}", e),
    })?;

    client
        .put_object()
        .bucket(bucket)
        .key(metadata_key)
        .body(body.into())
        .content_type("application/json")
        .send()
        .await
        .map_err(|e| BackupError::Failed {
            reason: format!(
                "failed to upload metadata.json to s3://{}/{}: {}",
                bucket, metadata_key, e
            ),
        })?;
    Ok(())
}

// ── Param helpers ────────────────────────────────────────────────────────────

/// Extract an integer field from `ctx.params`, mapping a missing/bad field
/// to `BackupError::PermanentFailure` (no point retrying with the same
/// params).
pub fn require_i32_param(params: &Value, field: &str) -> Result<i32, BackupError> {
    params
        .get(field)
        .and_then(|v| v.as_i64())
        .map(|v| v as i32)
        .ok_or_else(|| BackupError::PermanentFailure {
            reason: format!("params.{} missing or not an integer", field),
        })
}

// ── Temp-file plumbing ───────────────────────────────────────────────────────

/// Create the engine-shared backup temp directory at
/// `<data_dir>/backups/tmp` and return the path. Idempotent.
pub async fn ensure_backup_tmpdir(
    config_service: &Arc<temps_config::ConfigService>,
) -> Result<std::path::PathBuf, BackupError> {
    let dir = config_service.data_dir().join("backups").join("tmp");
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| BackupError::Failed {
            reason: format!(
                "failed to create backup temp directory {}: {}",
                dir.display(),
                e
            ),
        })?;
    Ok(dir)
}

/// Best-effort `unlink` of a path. Logs (and ignores) any failure — used
/// on cleanup paths where we must not turn a cleanup failure into a backup
/// failure.
pub async fn best_effort_remove(path: &std::path::Path) {
    if let Err(e) = tokio::fs::remove_file(path).await {
        if e.kind() != std::io::ErrorKind::NotFound {
            warn!(path = %path.display(), error = %e, "best_effort_remove: unlink failed (non-fatal)");
        }
    }
}
