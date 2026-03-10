use chrono::{DateTime, Utc};
use sea_orm::{
    ActiveModelTrait, DatabaseBackend, DatabaseConnection, FromQueryResult, Set, Statement,
};
use serde::Serialize;
use std::sync::Arc;
use temps_entities::ai_usage_logs;
use utoipa::ToSchema;

use crate::error::AiGatewayError;

// ============================================================================
// Response structs
// ============================================================================

#[derive(Debug, Serialize, ToSchema)]
pub struct UsageSummary {
    pub total_requests: i64,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_tokens: i64,
    pub avg_latency_ms: f64,
    pub total_cost_microcents: i64,
    pub error_count: i64,
    pub streaming_count: i64,
    pub byok_count: i64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ProviderUsage {
    pub provider: String,
    pub request_count: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub avg_latency_ms: f64,
    pub error_count: i64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TimeseriesBucket {
    /// ISO 8601 timestamp
    pub bucket: String,
    pub request_count: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub avg_latency_ms: f64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ModelUsage {
    pub model: String,
    pub provider: String,
    pub request_count: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
    pub avg_latency_ms: f64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct UsageLogEntry {
    pub id: i64,
    pub timestamp: String,
    pub provider: String,
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub latency_ms: i32,
    pub estimated_cost_microcents: i64,
    pub status: i16,
    pub is_streaming: bool,
    pub is_byok: bool,
}

// ============================================================================
// Internal query result structs (FromQueryResult)
// ============================================================================

#[derive(Debug, FromQueryResult)]
struct SummaryRow {
    total_requests: Option<i64>,
    total_input_tokens: Option<i64>,
    total_output_tokens: Option<i64>,
    total_tokens: Option<i64>,
    avg_latency_ms: Option<f64>,
    total_cost_microcents: Option<i64>,
    error_count: Option<i64>,
    streaming_count: Option<i64>,
    byok_count: Option<i64>,
}

#[derive(Debug, FromQueryResult)]
struct ProviderUsageRow {
    provider: Option<String>,
    request_count: Option<i64>,
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    avg_latency_ms: Option<f64>,
    error_count: Option<i64>,
}

#[derive(Debug, FromQueryResult)]
struct TimeseriesBucketRow {
    bucket: Option<String>,
    request_count: Option<i64>,
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    avg_latency_ms: Option<f64>,
}

#[derive(Debug, FromQueryResult)]
struct ModelUsageRow {
    model: Option<String>,
    provider: Option<String>,
    request_count: Option<i64>,
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    total_tokens: Option<i64>,
    avg_latency_ms: Option<f64>,
}

#[derive(Debug, FromQueryResult)]
struct UsageLogRow {
    id: Option<i64>,
    timestamp: Option<String>,
    provider: Option<String>,
    model: Option<String>,
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    latency_ms: Option<i32>,
    estimated_cost_microcents: Option<i64>,
    status: Option<i16>,
    is_streaming: Option<bool>,
    is_byok: Option<bool>,
}

// ============================================================================
// Service
// ============================================================================

pub struct UsageService {
    db: Arc<DatabaseConnection>,
}

impl UsageService {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn log_usage(
        &self,
        user_id: Option<i32>,
        provider: &str,
        model: &str,
        input_tokens: i64,
        output_tokens: i64,
        latency_ms: i32,
        estimated_cost_microcents: i64,
        status: i16,
        is_streaming: bool,
        is_byok: bool,
    ) -> Result<(), AiGatewayError> {
        let record = ai_usage_logs::ActiveModel {
            timestamp: Set(chrono::Utc::now()),
            user_id: Set(user_id),
            provider: Set(provider.to_string()),
            model: Set(model.to_string()),
            input_tokens: Set(input_tokens),
            output_tokens: Set(output_tokens),
            latency_ms: Set(latency_ms),
            estimated_cost_microcents: Set(estimated_cost_microcents),
            status: Set(status),
            is_streaming: Set(is_streaming),
            is_byok: Set(is_byok),
            ..Default::default()
        };

        record.insert(self.db.as_ref()).await?;
        Ok(())
    }

    /// Get usage summary (totals) for a time range.
    pub async fn get_summary(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<UsageSummary, AiGatewayError> {
        let row = SummaryRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"SELECT
                COUNT(*) as total_requests,
                COALESCE(SUM(input_tokens), 0)::INT8 as total_input_tokens,
                COALESCE(SUM(output_tokens), 0)::INT8 as total_output_tokens,
                COALESCE(SUM(input_tokens + output_tokens), 0)::INT8 as total_tokens,
                COALESCE(AVG(latency_ms)::float8, 0) as avg_latency_ms,
                COALESCE(SUM(estimated_cost_microcents), 0)::INT8 as total_cost_microcents,
                COUNT(*) FILTER (WHERE status >= 400) as error_count,
                COUNT(*) FILTER (WHERE is_streaming = true) as streaming_count,
                COUNT(*) FILTER (WHERE is_byok = true) as byok_count
            FROM ai_usage_logs
            WHERE timestamp >= $1 AND timestamp < $2"#,
            [from.into(), to.into()],
        ))
        .one(self.db.as_ref())
        .await?;

        let row = row.unwrap_or(SummaryRow {
            total_requests: Some(0),
            total_input_tokens: Some(0),
            total_output_tokens: Some(0),
            total_tokens: Some(0),
            avg_latency_ms: Some(0.0),
            total_cost_microcents: Some(0),
            error_count: Some(0),
            streaming_count: Some(0),
            byok_count: Some(0),
        });

        Ok(UsageSummary {
            total_requests: row.total_requests.unwrap_or(0),
            total_input_tokens: row.total_input_tokens.unwrap_or(0),
            total_output_tokens: row.total_output_tokens.unwrap_or(0),
            total_tokens: row.total_tokens.unwrap_or(0),
            avg_latency_ms: row.avg_latency_ms.unwrap_or(0.0),
            total_cost_microcents: row.total_cost_microcents.unwrap_or(0),
            error_count: row.error_count.unwrap_or(0),
            streaming_count: row.streaming_count.unwrap_or(0),
            byok_count: row.byok_count.unwrap_or(0),
        })
    }

    /// Get usage broken down by provider for a time range.
    pub async fn get_by_provider(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<ProviderUsage>, AiGatewayError> {
        let rows = ProviderUsageRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"SELECT
                provider,
                COUNT(*) as request_count,
                COALESCE(SUM(input_tokens), 0)::INT8 as input_tokens,
                COALESCE(SUM(output_tokens), 0)::INT8 as output_tokens,
                COALESCE(AVG(latency_ms)::float8, 0) as avg_latency_ms,
                COUNT(*) FILTER (WHERE status >= 400) as error_count
            FROM ai_usage_logs
            WHERE timestamp >= $1 AND timestamp < $2
            GROUP BY provider
            ORDER BY request_count DESC"#,
            [from.into(), to.into()],
        ))
        .all(self.db.as_ref())
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| ProviderUsage {
                provider: r.provider.unwrap_or_default(),
                request_count: r.request_count.unwrap_or(0),
                input_tokens: r.input_tokens.unwrap_or(0),
                output_tokens: r.output_tokens.unwrap_or(0),
                avg_latency_ms: r.avg_latency_ms.unwrap_or(0.0),
                error_count: r.error_count.unwrap_or(0),
            })
            .collect())
    }

    /// Get time-series usage data bucketed by interval.
    pub async fn get_timeseries(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        bucket: &str,
    ) -> Result<Vec<TimeseriesBucket>, AiGatewayError> {
        let interval = match bucket {
            "hour" => "1 hour",
            "day" => "1 day",
            "week" => "1 week",
            _ => {
                return Err(AiGatewayError::Validation {
                    message: format!(
                        "Invalid bucket '{}': must be 'hour', 'day', or 'week'",
                        bucket
                    ),
                })
            }
        };

        let rows = TimeseriesBucketRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"SELECT bucket::text, request_count, input_tokens, output_tokens, avg_latency_ms
            FROM (
                SELECT time_bucket($1::interval, timestamp) as bucket,
                       COUNT(*) as request_count,
                       COALESCE(SUM(input_tokens), 0)::INT8 as input_tokens,
                       COALESCE(SUM(output_tokens), 0)::INT8 as output_tokens,
                       COALESCE(AVG(latency_ms)::float8, 0) as avg_latency_ms
                FROM ai_usage_logs
                WHERE timestamp >= $2 AND timestamp < $3
                GROUP BY time_bucket($1::interval, timestamp)
            ) sub
            ORDER BY bucket ASC"#,
            [interval.into(), from.into(), to.into()],
        ))
        .all(self.db.as_ref())
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| TimeseriesBucket {
                bucket: r.bucket.unwrap_or_default(),
                request_count: r.request_count.unwrap_or(0),
                input_tokens: r.input_tokens.unwrap_or(0),
                output_tokens: r.output_tokens.unwrap_or(0),
                avg_latency_ms: r.avg_latency_ms.unwrap_or(0.0),
            })
            .collect())
    }

    /// Get top models by request count.
    pub async fn get_top_models(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        limit: u64,
    ) -> Result<Vec<ModelUsage>, AiGatewayError> {
        let rows = ModelUsageRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"SELECT
                model,
                provider,
                COUNT(*) as request_count,
                COALESCE(SUM(input_tokens), 0)::INT8 as input_tokens,
                COALESCE(SUM(output_tokens), 0)::INT8 as output_tokens,
                COALESCE(SUM(input_tokens + output_tokens), 0)::INT8 as total_tokens,
                COALESCE(AVG(latency_ms)::float8, 0) as avg_latency_ms
            FROM ai_usage_logs
            WHERE timestamp >= $1 AND timestamp < $2
            GROUP BY model, provider
            ORDER BY request_count DESC
            LIMIT $3"#,
            [from.into(), to.into(), (limit as i64).into()],
        ))
        .all(self.db.as_ref())
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| ModelUsage {
                model: r.model.unwrap_or_default(),
                provider: r.provider.unwrap_or_default(),
                request_count: r.request_count.unwrap_or(0),
                input_tokens: r.input_tokens.unwrap_or(0),
                output_tokens: r.output_tokens.unwrap_or(0),
                total_tokens: r.total_tokens.unwrap_or(0),
                avg_latency_ms: r.avg_latency_ms.unwrap_or(0.0),
            })
            .collect())
    }

    /// Get recent usage log entries.
    pub async fn get_recent(&self, limit: u64) -> Result<Vec<UsageLogEntry>, AiGatewayError> {
        let rows = UsageLogRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"SELECT
                id,
                timestamp::text,
                provider,
                model,
                input_tokens,
                output_tokens,
                latency_ms,
                estimated_cost_microcents,
                status,
                is_streaming,
                is_byok
            FROM ai_usage_logs
            ORDER BY timestamp DESC
            LIMIT $1"#,
            [(limit as i64).into()],
        ))
        .all(self.db.as_ref())
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| UsageLogEntry {
                id: r.id.unwrap_or(0),
                timestamp: r.timestamp.unwrap_or_default(),
                provider: r.provider.unwrap_or_default(),
                model: r.model.unwrap_or_default(),
                input_tokens: r.input_tokens.unwrap_or(0),
                output_tokens: r.output_tokens.unwrap_or(0),
                latency_ms: r.latency_ms.unwrap_or(0),
                estimated_cost_microcents: r.estimated_cost_microcents.unwrap_or(0),
                status: r.status.unwrap_or(0),
                is_streaming: r.is_streaming.unwrap_or(false),
                is_byok: r.is_byok.unwrap_or(false),
            })
            .collect())
    }
}
