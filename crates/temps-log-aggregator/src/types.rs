//! Core types for the log aggregator

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

/// Fixed namespace UUID for generating deterministic project UUIDs from integer IDs.
/// This allows the log aggregator (which uses UUIDs) to map to the platform's integer project IDs.
const PROJECT_UUID_NAMESPACE: Uuid = Uuid::from_bytes([
    0x6b, 0xa7, 0xb8, 0x10, 0x9d, 0xad, 0x11, 0xd1, 0x80, 0xb4, 0x00, 0xc0, 0x4f, 0xd4, 0x30, 0xc8,
]);

/// Convert an integer project ID to a deterministic UUID.
///
/// Uses UUID v5 (SHA-1 name-based) with a fixed namespace so the same project_id
/// always maps to the same UUID. This bridges the platform's `i32` project IDs
/// with the log aggregator's UUID-based storage.
pub fn project_id_to_uuid(project_id: i32) -> Uuid {
    Uuid::new_v5(&PROJECT_UUID_NAMESPACE, project_id.to_string().as_bytes())
}

/// A single parsed log line ready for storage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogLine {
    /// ISO 8601 timestamp with millisecond precision
    pub ts: DateTime<Utc>,
    /// stdout or stderr
    pub stream: LogStream,
    /// Normalized log level
    pub level: LogLevel,
    /// Raw log message string
    pub msg: String,
    /// Extracted key-value pairs from structured log lines
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<serde_json::Value>,
    /// Docker container ID
    pub container_id: String,
    /// Service name from Docker label
    pub service: String,
    /// Environment from Docker label
    pub env: String,
    /// Project UUID from Docker label
    pub project_id: Uuid,
    /// Active deploy UUID at time of log
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deploy_id: Option<Uuid>,
}

/// Log output stream
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum LogStream {
    Stdout,
    Stderr,
}

impl std::fmt::Display for LogStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LogStream::Stdout => write!(f, "stdout"),
            LogStream::Stderr => write!(f, "stderr"),
        }
    }
}

/// Normalized log level
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ToSchema,
)]
#[serde(rename_all = "UPPERCASE")]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    /// Returns true if this level should be indexed in the log_events hypertable
    pub fn is_indexable(&self) -> bool {
        matches!(self, LogLevel::Error | LogLevel::Warn)
    }

    /// Parse a log level from a string, case-insensitive
    pub fn parse(s: &str) -> Option<LogLevel> {
        match s.to_uppercase().as_str() {
            "ERROR" | "ERR" | "FATAL" | "CRITICAL" | "CRIT" => Some(LogLevel::Error),
            "WARN" | "WARNING" => Some(LogLevel::Warn),
            "INFO" | "INFORMATION" => Some(LogLevel::Info),
            "DEBUG" | "DBG" => Some(LogLevel::Debug),
            "TRACE" | "TRC" => Some(LogLevel::Trace),
            _ => None,
        }
    }
}

impl std::fmt::Display for LogLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LogLevel::Trace => write!(f, "TRACE"),
            LogLevel::Debug => write!(f, "DEBUG"),
            LogLevel::Info => write!(f, "INFO"),
            LogLevel::Warn => write!(f, "WARN"),
            LogLevel::Error => write!(f, "ERROR"),
        }
    }
}

/// Metadata about a stored chunk (mirrors the log_chunks DB row)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkMeta {
    pub id: Uuid,
    pub project_id: Uuid,
    pub env: String,
    pub service: String,
    pub container_id: String,
    pub deploy_id: Option<Uuid>,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub storage_key: String,
    pub line_count: i32,
    pub compressed_size_bytes: i32,
    pub has_errors: bool,
    /// Byte offset of every 100th line (uncompressed) for partial retrieval
    pub line_offsets: Vec<i32>,
}

/// Enrichment context applied to every log line from a container
#[derive(Debug, Clone)]
pub struct ContainerContext {
    pub project_id: Uuid,
    pub env: String,
    pub service: String,
    pub container_id: String,
    pub deploy_id: Option<Uuid>,
}

/// Search filter parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogSearchFilter {
    pub project_id: Uuid,
    pub start_time: DateTime<Utc>,
    pub end_time: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub levels: Vec<LogLevel>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub services: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub envs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deploy_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Field-level filters: key=value exact match, key>value range
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub field_filters: Vec<FieldFilter>,
    /// Cursor for pagination
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// Page size (default: 100, max: 500)
    #[serde(default = "default_page_size")]
    pub page_size: u32,
}

fn default_page_size() -> u32 {
    100
}

/// A single field-level filter
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldFilter {
    pub key: String,
    pub op: FieldFilterOp,
    pub value: String,
}

/// Field filter operator
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldFilterOp {
    Eq,
    Gt,
    Lt,
    Gte,
    Lte,
}

/// Search result page
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogSearchResult {
    pub lines: Vec<LogSearchLine>,
    /// Cursor for the next page, None if no more results
    pub next_cursor: Option<String>,
    /// Whether results came from index or archive scan
    pub search_mode: SearchMode,
    /// How many lines were examined to produce the results
    pub total_scanned: u64,
}

/// A single line in search results
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct LogSearchLine {
    #[schema(value_type = String)]
    pub timestamp: DateTime<Utc>,
    pub level: LogLevel,
    pub service: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<serde_json::Value>,
    #[schema(value_type = String)]
    pub chunk_id: Uuid,
    pub line_offset: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<String>)]
    pub deploy_id: Option<Uuid>,
}

/// Search execution mode
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SearchMode {
    /// Queried from TimescaleDB index (fast)
    Index,
    /// Scanned from S3/filesystem archive (slower)
    Archive,
}

/// Context lines request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextRequest {
    pub chunk_id: Uuid,
    pub line_offset: i32,
    /// Number of lines before and after to return (default: 25)
    #[serde(default = "default_context_lines")]
    pub lines: u32,
}

fn default_context_lines() -> u32 {
    25
}

/// Context lines response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextResponse {
    pub lines: Vec<ContextLine>,
    /// The index of the target line within the returned lines
    pub target_index: usize,
}

/// A line in context response
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ContextLine {
    #[schema(value_type = String)]
    pub timestamp: DateTime<Utc>,
    pub level: LogLevel,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<serde_json::Value>,
    pub line_offset: i32,
    /// Whether this line matched the original search
    pub is_match: bool,
}

/// Live tail filter
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TailFilter {
    pub project_id: Uuid,
    pub service: String,
    pub env: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub levels: Vec<LogLevel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// Service tag
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceTag {
    pub project_id: Uuid,
    pub service: String,
    pub key: String,
    pub value: String,
}

/// Storage configuration
#[derive(Debug, Clone)]
pub enum StorageConfig {
    Filesystem {
        base_path: std::path::PathBuf,
    },
    S3 {
        bucket: String,
        prefix: Option<String>,
        region: String,
        endpoint: Option<String>,
        access_key_id: String,
        secret_access_key: String,
        force_path_style: bool,
    },
}

/// Retention configuration per project
#[derive(Debug, Clone)]
pub struct RetentionConfig {
    /// How long to keep raw chunks in S3/filesystem (default: 30 days)
    pub chunk_retention_days: u32,
    /// How long to keep log_events in TimescaleDB (managed by TimescaleDB retention policy: 7 days)
    pub event_retention_days: u32,
}

impl Default for RetentionConfig {
    fn default() -> Self {
        Self {
            chunk_retention_days: 30,
            event_retention_days: 7,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_level_parse() {
        assert_eq!(LogLevel::parse("ERROR"), Some(LogLevel::Error));
        assert_eq!(LogLevel::parse("error"), Some(LogLevel::Error));
        assert_eq!(LogLevel::parse("ERR"), Some(LogLevel::Error));
        assert_eq!(LogLevel::parse("FATAL"), Some(LogLevel::Error));
        assert_eq!(LogLevel::parse("WARN"), Some(LogLevel::Warn));
        assert_eq!(LogLevel::parse("warning"), Some(LogLevel::Warn));
        assert_eq!(LogLevel::parse("INFO"), Some(LogLevel::Info));
        assert_eq!(LogLevel::parse("DEBUG"), Some(LogLevel::Debug));
        assert_eq!(LogLevel::parse("TRACE"), Some(LogLevel::Trace));
        assert_eq!(LogLevel::parse("unknown"), None);
    }

    #[test]
    fn test_log_level_is_indexable() {
        assert!(LogLevel::Error.is_indexable());
        assert!(LogLevel::Warn.is_indexable());
        assert!(!LogLevel::Info.is_indexable());
        assert!(!LogLevel::Debug.is_indexable());
        assert!(!LogLevel::Trace.is_indexable());
    }

    #[test]
    fn test_log_level_display() {
        assert_eq!(LogLevel::Error.to_string(), "ERROR");
        assert_eq!(LogLevel::Warn.to_string(), "WARN");
        assert_eq!(LogLevel::Info.to_string(), "INFO");
        assert_eq!(LogLevel::Debug.to_string(), "DEBUG");
        assert_eq!(LogLevel::Trace.to_string(), "TRACE");
    }

    #[test]
    fn test_log_stream_display() {
        assert_eq!(LogStream::Stdout.to_string(), "stdout");
        assert_eq!(LogStream::Stderr.to_string(), "stderr");
    }

    #[test]
    fn test_default_retention_config() {
        let config = RetentionConfig::default();
        assert_eq!(config.chunk_retention_days, 30);
        assert_eq!(config.event_retention_days, 7);
    }

    #[test]
    fn test_project_id_to_uuid_determinism() {
        // Same integer always maps to the same UUID
        let uuid1 = project_id_to_uuid(42);
        let uuid2 = project_id_to_uuid(42);
        assert_eq!(uuid1, uuid2, "Same project_id must produce the same UUID");

        // Different integers produce different UUIDs
        let uuid3 = project_id_to_uuid(43);
        assert_ne!(
            uuid1, uuid3,
            "Different project_ids must produce different UUIDs"
        );

        // Edge cases: 0, negative, i32::MAX, i32::MIN
        let uuid_zero = project_id_to_uuid(0);
        let uuid_neg = project_id_to_uuid(-1);
        let uuid_max = project_id_to_uuid(i32::MAX);
        let uuid_min = project_id_to_uuid(i32::MIN);
        let all_uuids = vec![uuid_zero, uuid_neg, uuid_max, uuid_min, uuid1];
        for i in 0..all_uuids.len() {
            for j in (i + 1)..all_uuids.len() {
                assert_ne!(
                    all_uuids[i], all_uuids[j],
                    "All project_id UUIDs must be unique"
                );
            }
        }

        // UUID version must be 5 (name-based SHA-1)
        assert_eq!(uuid1.get_version_num(), 5, "Must be UUID v5");
    }

    #[test]
    fn test_log_line_serialization() {
        let line = LogLine {
            ts: Utc::now(),
            stream: LogStream::Stdout,
            level: LogLevel::Info,
            msg: "test message".to_string(),
            fields: Some(serde_json::json!({"key": "value"})),
            container_id: "abc123".to_string(),
            service: "web".to_string(),
            env: "production".to_string(),
            project_id: Uuid::new_v4(),
            deploy_id: None,
        };

        let json = serde_json::to_string(&line).expect("should serialize");
        let parsed: LogLine = serde_json::from_str(&json).expect("should deserialize");
        assert_eq!(parsed.msg, "test message");
        assert_eq!(parsed.level, LogLevel::Info);
        assert_eq!(parsed.stream, LogStream::Stdout);
    }
}
