use axum::{
    extract::{Path, Query, State},
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::sync::Arc;
use temps_auth::permission_guard;
use temps_auth::RequireAuth;
use temps_core::problemdetails::{Problem, ProblemDetails};
use utoipa::{OpenApi, ToSchema};

use crate::error::AiGatewayError;
use crate::handlers::types::AiGatewayAppState;
use crate::services::usage_service::{
    ConversationSummary, ModelUsage, ProviderUsage, TimeseriesBucket, UsageFilter, UsageLogEntry,
    UsageSummary,
};

// ============================================================================
// OpenAPI schema
// ============================================================================

#[derive(OpenApi)]
#[openapi(
    paths(
        get_usage_summary,
        get_usage_by_provider,
        get_usage_timeseries,
        get_usage_top_models,
        get_usage_recent,
        get_conversations,
        get_conversation_detail,
    ),
    components(schemas(
        UsageQueryParams,
        TimeseriesQueryParams,
        TopModelsQueryParams,
        RecentQueryParams,
        ConversationsQueryParams,
        UsageSummary,
        ProviderUsage,
        TimeseriesBucket,
        ModelUsage,
        UsageLogEntry,
        ConversationSummary,
        UsageFilter,
    )),
    info(
        title = "AI Gateway Usage API",
        description = "Usage analytics endpoints for the AI gateway",
        version = "1.0.0"
    ),
    tags(
        (name = "AI Gateway Usage", description = "Usage analytics and reporting endpoints")
    )
)]
pub struct AiGatewayUsageApiDoc;

pub fn configure_usage_routes() -> Router<Arc<AiGatewayAppState>> {
    Router::new()
        .route("/ai/usage/summary", get(get_usage_summary))
        .route("/ai/usage/by-provider", get(get_usage_by_provider))
        .route("/ai/usage/timeseries", get(get_usage_timeseries))
        .route("/ai/usage/top-models", get(get_usage_top_models))
        .route("/ai/usage/recent", get(get_usage_recent))
        .route("/ai/usage/conversations", get(get_conversations))
        .route(
            "/ai/usage/conversations/{conversation_id}",
            get(get_conversation_detail),
        )
}

// ============================================================================
// Query param structs
// ============================================================================

#[derive(Debug, Deserialize, ToSchema)]
pub struct UsageQueryParams {
    /// ISO 8601 start time (defaults to 24h ago)
    pub from: Option<String>,
    /// ISO 8601 end time (defaults to now)
    pub to: Option<String>,
    /// Filter by user ID
    pub user_id: Option<i32>,
    /// Filter by conversation ID
    pub conversation_id: Option<String>,
    /// Filter by tags (comma-separated, AND logic)
    pub tags: Option<String>,
    /// Filter by model name
    pub model: Option<String>,
    /// Filter by provider name
    pub provider: Option<String>,
}

impl UsageQueryParams {
    fn to_filter(&self) -> UsageFilter {
        UsageFilter {
            user_id: self.user_id,
            conversation_id: self.conversation_id.clone(),
            tags: self.tags.clone(),
            model: self.model.clone(),
            provider: self.provider.clone(),
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct TimeseriesQueryParams {
    /// ISO 8601 start time (defaults to 24h ago)
    pub from: Option<String>,
    /// ISO 8601 end time (defaults to now)
    pub to: Option<String>,
    /// Bucket size: "hour", "day", "week" (defaults to "day")
    pub bucket: Option<String>,
    /// Filter by user ID
    pub user_id: Option<i32>,
    /// Filter by conversation ID
    pub conversation_id: Option<String>,
    /// Filter by tags (comma-separated, AND logic)
    pub tags: Option<String>,
    /// Filter by model name
    pub model: Option<String>,
    /// Filter by provider name
    pub provider: Option<String>,
}

impl TimeseriesQueryParams {
    fn to_filter(&self) -> UsageFilter {
        UsageFilter {
            user_id: self.user_id,
            conversation_id: self.conversation_id.clone(),
            tags: self.tags.clone(),
            model: self.model.clone(),
            provider: self.provider.clone(),
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct TopModelsQueryParams {
    /// ISO 8601 start time (defaults to 24h ago)
    pub from: Option<String>,
    /// ISO 8601 end time (defaults to now)
    pub to: Option<String>,
    /// Max results (defaults to 10)
    pub limit: Option<u64>,
    /// Filter by user ID
    pub user_id: Option<i32>,
    /// Filter by tags (comma-separated, AND logic)
    pub tags: Option<String>,
}

impl TopModelsQueryParams {
    fn to_filter(&self) -> UsageFilter {
        UsageFilter {
            user_id: self.user_id,
            tags: self.tags.clone(),
            ..Default::default()
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct RecentQueryParams {
    /// Max results (defaults to 50, max 100)
    pub limit: Option<u64>,
    /// Filter by user ID
    pub user_id: Option<i32>,
    /// Filter by conversation ID
    pub conversation_id: Option<String>,
    /// Filter by tags (comma-separated, AND logic)
    pub tags: Option<String>,
    /// Filter by model name
    pub model: Option<String>,
    /// Filter by provider name
    pub provider: Option<String>,
}

impl RecentQueryParams {
    fn to_filter(&self) -> UsageFilter {
        UsageFilter {
            user_id: self.user_id,
            conversation_id: self.conversation_id.clone(),
            tags: self.tags.clone(),
            model: self.model.clone(),
            provider: self.provider.clone(),
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ConversationsQueryParams {
    /// ISO 8601 start time (defaults to 24h ago)
    pub from: Option<String>,
    /// ISO 8601 end time (defaults to now)
    pub to: Option<String>,
    /// Max results (defaults to 50, max 100)
    pub limit: Option<u64>,
    /// Filter by user ID
    pub user_id: Option<i32>,
    /// Filter by tags (comma-separated, AND logic)
    pub tags: Option<String>,
    /// Filter by model name
    pub model: Option<String>,
}

impl ConversationsQueryParams {
    fn to_filter(&self) -> UsageFilter {
        UsageFilter {
            user_id: self.user_id,
            tags: self.tags.clone(),
            model: self.model.clone(),
            ..Default::default()
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ConversationDetailQueryParams {
    /// Max results (defaults to 100)
    pub limit: Option<u64>,
}

// ============================================================================
// Helpers
// ============================================================================

fn parse_time_range(
    from: Option<&str>,
    to: Option<&str>,
) -> Result<(DateTime<Utc>, DateTime<Utc>), AiGatewayError> {
    let now = Utc::now();
    let from = match from {
        Some(s) => DateTime::parse_from_rfc3339(s)
            .map(|dt| dt.with_timezone(&Utc))
            .map_err(|_| AiGatewayError::Validation {
                message: format!("Invalid 'from' timestamp: '{}'. Use ISO 8601 format.", s),
            })?,
        None => now - chrono::Duration::hours(24),
    };
    let to = match to {
        Some(s) => DateTime::parse_from_rfc3339(s)
            .map(|dt| dt.with_timezone(&Utc))
            .map_err(|_| AiGatewayError::Validation {
                message: format!("Invalid 'to' timestamp: '{}'. Use ISO 8601 format.", s),
            })?,
        None => now,
    };
    Ok((from, to))
}

// ============================================================================
// Handlers
// ============================================================================

#[utoipa::path(
    tag = "AI Gateway Usage",
    get,
    path = "/ai/usage/summary",
    params(
        ("from" = Option<String>, Query, description = "ISO 8601 start time (defaults to 24h ago)"),
        ("to" = Option<String>, Query, description = "ISO 8601 end time (defaults to now)"),
    ),
    responses(
        (status = 200, description = "Usage summary for the time range", body = UsageSummary),
        (status = 400, description = "Invalid query parameters", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "Insufficient permissions", body = ProblemDetails),
    ),
    security(("bearer_auth" = []))
)]
async fn get_usage_summary(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<AiGatewayAppState>>,
    Query(params): Query<UsageQueryParams>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, AiGatewayRead);

    let (from, to) = parse_time_range(params.from.as_deref(), params.to.as_deref())?;
    let filter = params.to_filter();
    let summary = app_state
        .usage_service
        .get_summary_filtered(from, to, &filter)
        .await?;

    Ok(Json(summary))
}

#[utoipa::path(
    tag = "AI Gateway Usage",
    get,
    path = "/ai/usage/by-provider",
    params(
        ("from" = Option<String>, Query, description = "ISO 8601 start time (defaults to 24h ago)"),
        ("to" = Option<String>, Query, description = "ISO 8601 end time (defaults to now)"),
    ),
    responses(
        (status = 200, description = "Usage broken down by provider", body = Vec<ProviderUsage>),
        (status = 400, description = "Invalid query parameters", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "Insufficient permissions", body = ProblemDetails),
    ),
    security(("bearer_auth" = []))
)]
async fn get_usage_by_provider(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<AiGatewayAppState>>,
    Query(params): Query<UsageQueryParams>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, AiGatewayRead);

    let (from, to) = parse_time_range(params.from.as_deref(), params.to.as_deref())?;
    let filter = params.to_filter();
    let usage = app_state
        .usage_service
        .get_by_provider_filtered(from, to, &filter)
        .await?;

    Ok(Json(usage))
}

#[utoipa::path(
    tag = "AI Gateway Usage",
    get,
    path = "/ai/usage/timeseries",
    params(
        ("from" = Option<String>, Query, description = "ISO 8601 start time (defaults to 24h ago)"),
        ("to" = Option<String>, Query, description = "ISO 8601 end time (defaults to now)"),
        ("bucket" = Option<String>, Query, description = "Bucket size: hour, day, week (defaults to day)"),
    ),
    responses(
        (status = 200, description = "Time-series usage data", body = Vec<TimeseriesBucket>),
        (status = 400, description = "Invalid query parameters", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "Insufficient permissions", body = ProblemDetails),
    ),
    security(("bearer_auth" = []))
)]
async fn get_usage_timeseries(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<AiGatewayAppState>>,
    Query(params): Query<TimeseriesQueryParams>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, AiGatewayRead);

    let (from, to) = parse_time_range(params.from.as_deref(), params.to.as_deref())?;
    let bucket = params.bucket.as_deref().unwrap_or("day");
    let filter = params.to_filter();
    let timeseries = app_state
        .usage_service
        .get_timeseries_filtered(from, to, bucket, &filter)
        .await?;

    Ok(Json(timeseries))
}

#[utoipa::path(
    tag = "AI Gateway Usage",
    get,
    path = "/ai/usage/top-models",
    params(
        ("from" = Option<String>, Query, description = "ISO 8601 start time (defaults to 24h ago)"),
        ("to" = Option<String>, Query, description = "ISO 8601 end time (defaults to now)"),
        ("limit" = Option<u64>, Query, description = "Max results (defaults to 10)"),
    ),
    responses(
        (status = 200, description = "Top models by request count", body = Vec<ModelUsage>),
        (status = 400, description = "Invalid query parameters", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "Insufficient permissions", body = ProblemDetails),
    ),
    security(("bearer_auth" = []))
)]
async fn get_usage_top_models(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<AiGatewayAppState>>,
    Query(params): Query<TopModelsQueryParams>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, AiGatewayRead);

    let (from, to) = parse_time_range(params.from.as_deref(), params.to.as_deref())?;
    let limit = std::cmp::min(params.limit.unwrap_or(10), 100);
    let filter = params.to_filter();
    let models = app_state
        .usage_service
        .get_top_models_filtered(from, to, limit, &filter)
        .await?;

    Ok(Json(models))
}

#[utoipa::path(
    tag = "AI Gateway Usage",
    get,
    path = "/ai/usage/recent",
    params(
        ("limit" = Option<u64>, Query, description = "Max results (defaults to 50, max 100)"),
    ),
    responses(
        (status = 200, description = "Recent usage log entries", body = Vec<UsageLogEntry>),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "Insufficient permissions", body = ProblemDetails),
    ),
    security(("bearer_auth" = []))
)]
async fn get_usage_recent(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<AiGatewayAppState>>,
    Query(params): Query<RecentQueryParams>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, AiGatewayRead);

    let limit = std::cmp::min(params.limit.unwrap_or(50), 100);
    let filter = params.to_filter();
    let entries = app_state
        .usage_service
        .get_recent_filtered(limit, &filter)
        .await?;

    Ok(Json(entries))
}

#[utoipa::path(
    tag = "AI Gateway Usage",
    get,
    path = "/ai/usage/conversations",
    params(
        ("from" = Option<String>, Query, description = "ISO 8601 start time (defaults to 24h ago)"),
        ("to" = Option<String>, Query, description = "ISO 8601 end time (defaults to now)"),
        ("limit" = Option<u64>, Query, description = "Max results (defaults to 50, max 100)"),
        ("user_id" = Option<i32>, Query, description = "Filter by user ID"),
        ("tags" = Option<String>, Query, description = "Filter by tags (comma-separated)"),
        ("model" = Option<String>, Query, description = "Filter by model name"),
    ),
    responses(
        (status = 200, description = "Conversation summaries", body = Vec<ConversationSummary>),
        (status = 400, description = "Invalid query parameters", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "Insufficient permissions", body = ProblemDetails),
    ),
    security(("bearer_auth" = []))
)]
async fn get_conversations(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<AiGatewayAppState>>,
    Query(params): Query<ConversationsQueryParams>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, AiGatewayRead);

    let (from, to) = parse_time_range(params.from.as_deref(), params.to.as_deref())?;
    let limit = std::cmp::min(params.limit.unwrap_or(50), 100);
    let filter = params.to_filter();
    let conversations = app_state
        .usage_service
        .get_conversations(from, to, &filter, limit)
        .await?;

    Ok(Json(conversations))
}

#[utoipa::path(
    tag = "AI Gateway Usage",
    get,
    path = "/ai/usage/conversations/{conversation_id}",
    params(
        ("conversation_id" = String, Path, description = "Conversation ID"),
        ("limit" = Option<u64>, Query, description = "Max results (defaults to 100)"),
    ),
    responses(
        (status = 200, description = "Invocations within a conversation", body = Vec<UsageLogEntry>),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "Insufficient permissions", body = ProblemDetails),
    ),
    security(("bearer_auth" = []))
)]
async fn get_conversation_detail(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<AiGatewayAppState>>,
    Path(conversation_id): Path<String>,
    Query(params): Query<ConversationDetailQueryParams>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, AiGatewayRead);

    let limit = std::cmp::min(params.limit.unwrap_or(100), 500);
    let entries = app_state
        .usage_service
        .get_conversation_detail(&conversation_id, limit)
        .await?;

    Ok(Json(entries))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Datelike, Timelike};

    #[test]
    fn test_parse_time_range_defaults() {
        let (from, to) = parse_time_range(None, None).unwrap();
        let now = Utc::now();
        // `from` should be approximately 24h ago
        let diff = now - from;
        assert!(diff.num_hours() >= 23 && diff.num_hours() <= 25);
        // `to` should be approximately now
        let diff_to = now - to;
        assert!(diff_to.num_seconds().abs() < 5);
    }

    #[test]
    fn test_parse_time_range_with_valid_from() {
        let (from, _to) = parse_time_range(Some("2025-01-15T00:00:00Z"), None).unwrap();
        assert_eq!(from.year(), 2025);
        assert_eq!(from.month(), 1);
        assert_eq!(from.day(), 15);
    }

    #[test]
    fn test_parse_time_range_with_valid_to() {
        let (_from, to) = parse_time_range(None, Some("2025-06-01T12:00:00Z")).unwrap();
        assert_eq!(to.year(), 2025);
        assert_eq!(to.month(), 6);
        assert_eq!(to.day(), 1);
        assert_eq!(to.hour(), 12);
    }

    #[test]
    fn test_parse_time_range_both_specified() {
        let (from, to) =
            parse_time_range(Some("2025-01-01T00:00:00Z"), Some("2025-01-31T23:59:59Z")).unwrap();
        assert_eq!(from.year(), 2025);
        assert_eq!(from.month(), 1);
        assert_eq!(from.day(), 1);
        assert_eq!(to.day(), 31);
    }

    #[test]
    fn test_parse_time_range_with_timezone_offset() {
        let (from, _to) = parse_time_range(Some("2025-01-15T10:00:00+05:00"), None).unwrap();
        // Should be converted to UTC: 10:00 +05:00 = 05:00 UTC
        assert_eq!(from.hour(), 5);
    }

    #[test]
    fn test_parse_time_range_invalid_from() {
        let result = parse_time_range(Some("not-a-date"), None);
        assert!(result.is_err());
        match result.unwrap_err() {
            AiGatewayError::Validation { message } => {
                assert!(message.contains("Invalid 'from' timestamp"));
                assert!(message.contains("not-a-date"));
            }
            other => panic!("Expected Validation error, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_time_range_invalid_to() {
        let result = parse_time_range(None, Some("bad-date"));
        assert!(result.is_err());
        match result.unwrap_err() {
            AiGatewayError::Validation { message } => {
                assert!(message.contains("Invalid 'to' timestamp"));
                assert!(message.contains("bad-date"));
            }
            other => panic!("Expected Validation error, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_time_range_empty_string_from() {
        let result = parse_time_range(Some(""), None);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            AiGatewayError::Validation { .. }
        ));
    }

    #[test]
    fn test_recent_query_params_defaults() {
        let json = "{}";
        let params: RecentQueryParams = serde_json::from_str(json).unwrap();
        assert!(params.limit.is_none());
    }

    #[test]
    fn test_timeseries_query_params_defaults() {
        let json = "{}";
        let params: TimeseriesQueryParams = serde_json::from_str(json).unwrap();
        assert!(params.from.is_none());
        assert!(params.to.is_none());
        assert!(params.bucket.is_none());
    }

    #[test]
    fn test_top_models_query_params_with_limit() {
        let json = r#"{"limit": 5}"#;
        let params: TopModelsQueryParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.limit, Some(5));
    }

    #[test]
    fn test_usage_query_params_with_times() {
        let json = r#"{"from": "2025-01-01T00:00:00Z", "to": "2025-01-31T23:59:59Z"}"#;
        let params: UsageQueryParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.from.as_deref(), Some("2025-01-01T00:00:00Z"));
        assert_eq!(params.to.as_deref(), Some("2025-01-31T23:59:59Z"));
    }
}
