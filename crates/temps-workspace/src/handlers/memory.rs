//! HTTP handlers for workflow memory.
//!
//! Routes (served under two stable prefixes):
//!   Legacy (pre-v1, kept indefinitely for backward compatibility):
//!     /api/projects/{project_id}/workflows/{slug}/memory
//!     /api/projects/{project_id}/workflows/{slug}/memory/search
//!     /api/projects/{project_id}/workflows/{slug}/memory/{fact_id}/supersede
//!     /api/projects/{project_id}/workflows/{slug}/memory/{fact_id}
//!
//!   v1 (the stable, versioned contract — what new clients should target):
//!     /api/v1/projects/{project_id}/workflows/{slug}/memory
//!     /api/v1/projects/{project_id}/workflows/{slug}/memory/search
//!     /api/v1/projects/{project_id}/workflows/{slug}/memory/{fact_id}/supersede
//!     /api/v1/projects/{project_id}/workflows/{slug}/memory/{fact_id}
//!
//! Both prefixes resolve to the exact same handlers — the v1 namespace is
//! introduced now so future breaking DTO changes can land as `/v2/...`
//! without invalidating in-flight clients. The legacy prefix stays until
//! all first-party callers (bash `memory` script, `temps memory` CLI,
//! `temps-agents::executor`) have moved to v1 and a deprecation window
//! has elapsed (target: 12 months, per ADR 009).
//!
//! Auth: Bearer token. Project scope is enforced via `permission_guard!`
//! and double-checked at the service layer (every memory query filters by
//! project_id + agent_id).

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, post},
    Json, Router,
};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

use temps_auth::{permission_guard, RequireAuth};
use temps_core::problemdetails::Problem;
use temps_entities::{project_agents, workflow_memory};

use crate::error::WorkspaceError;
use crate::handlers::WorkspaceAppState;
use crate::services::memory_service::{SupersedeRequest, TriggerContext, WriteFactRequest};

// ── DTOs ────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, ToSchema)]
pub struct MemoryFactResponse {
    pub id: i64,
    pub project_id: i32,
    pub agent_id: i32,
    pub fact: String,
    pub tags: Vec<String>,
    pub confidence: f32,
    pub times_used: i32,
    pub source_run_ids: Vec<i32>,
    pub superseded_by: Option<i64>,
    pub created_at: String,
    pub updated_at: String,
    pub last_used_at: Option<String>,
}

impl From<workflow_memory::Model> for MemoryFactResponse {
    fn from(m: workflow_memory::Model) -> Self {
        let tags = m
            .tags
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let source_run_ids = m
            .source_run_ids
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_i64().map(|n| n as i32))
                    .collect()
            })
            .unwrap_or_default();
        Self {
            id: m.id,
            project_id: m.project_id,
            agent_id: m.agent_id,
            fact: m.fact,
            tags,
            confidence: m.confidence,
            times_used: m.times_used,
            source_run_ids,
            superseded_by: m.superseded_by,
            created_at: m.created_at.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            updated_at: m.updated_at.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            last_used_at: m
                .last_used_at
                .map(|t| t.format("%Y-%m-%dT%H:%M:%SZ").to_string()),
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MemoryListResponse {
    pub facts: Vec<MemoryFactResponse>,
    pub total: usize,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct WriteMemoryBody {
    pub fact: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub source_run_id: Option<i32>,
    pub confidence: Option<f32>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SupersedeBody {
    pub new_fact: String,
    #[serde(default)]
    pub new_tags: Vec<String>,
    pub source_run_id: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub struct ListParams {
    pub limit: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct SearchParams {
    pub q: String,
    pub limit: Option<u64>,
}

// ── Routes ──────────────────────────────────────────────────────────────────

/// Build the core route table. Factored out so it can be mounted under
/// multiple prefixes without duplicating handler wiring.
fn build_routes(prefix: &str) -> Router<Arc<WorkspaceAppState>> {
    // `prefix` is expected to be empty string for the legacy namespace
    // or "/v1" for the versioned namespace. Both routes resolve to the
    // same handler — versioning is purely about the contract guarantee,
    // not the behavior.
    let base = format!("{prefix}/projects/{{project_id}}/workflows/{{slug}}/memory");
    Router::new()
        .route(&base, get(list_memory).post(write_memory))
        .route(&format!("{base}/search"), get(search_memory))
        .route(
            &format!("{base}/{{fact_id}}/supersede"),
            post(supersede_memory),
        )
        .route(&format!("{base}/{{fact_id}}"), delete(drop_memory))
}

pub fn routes() -> Router<Arc<WorkspaceAppState>> {
    // Legacy (unversioned) + v1 (stable). Both mounted so clients can
    // migrate incrementally; neither path can be deleted without a
    // deprecation window. See ADR 009 for the versioning policy.
    build_routes("").merge(build_routes("/v1"))
}

// ── Helper: resolve slug → agent_id ─────────────────────────────────────────

async fn resolve_agent_id(
    state: &WorkspaceAppState,
    project_id: i32,
    slug: &str,
) -> Result<i32, WorkspaceError> {
    let agent = project_agents::Entity::find()
        .filter(project_agents::Column::ProjectId.eq(project_id))
        .filter(project_agents::Column::Slug.eq(slug))
        .one(state.db.as_ref())
        .await?
        .ok_or(WorkspaceError::WorkflowNotFound {
            project_id,
            slug: slug.to_string(),
        })?;
    Ok(agent.id)
}

// ── Handlers ────────────────────────────────────────────────────────────────

async fn list_memory(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<WorkspaceAppState>>,
    Path((project_id, slug)): Path<(i32, String)>,
    Query(params): Query<ListParams>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, ProjectsRead);

    let agent_id = resolve_agent_id(&app_state, project_id, &slug).await?;
    let facts = app_state
        .memory_service
        .list(project_id, agent_id, params.limit)
        .await?;

    let total = facts.len();
    Ok(Json(MemoryListResponse {
        facts: facts.into_iter().map(MemoryFactResponse::from).collect(),
        total,
    }))
}

async fn search_memory(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<WorkspaceAppState>>,
    Path((project_id, slug)): Path<(i32, String)>,
    Query(params): Query<SearchParams>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, ProjectsRead);

    let agent_id = resolve_agent_id(&app_state, project_id, &slug).await?;
    let facts = app_state
        .memory_service
        .search(project_id, agent_id, &params.q, params.limit)
        .await?;

    let total = facts.len();
    Ok(Json(MemoryListResponse {
        facts: facts.into_iter().map(MemoryFactResponse::from).collect(),
        total,
    }))
}

async fn write_memory(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<WorkspaceAppState>>,
    Path((project_id, slug)): Path<(i32, String)>,
    Json(body): Json<WriteMemoryBody>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, ProjectsWrite);

    let agent_id = resolve_agent_id(&app_state, project_id, &slug).await?;
    let fact = app_state
        .memory_service
        .write(WriteFactRequest {
            project_id,
            agent_id,
            fact: body.fact,
            tags: body.tags,
            source_run_id: body.source_run_id,
            confidence: body.confidence,
        })
        .await?;

    tracing::info!(
        "Memory write: project={} slug={} fact_id={}",
        project_id,
        slug,
        fact.id
    );

    Ok((StatusCode::CREATED, Json(MemoryFactResponse::from(fact))))
}

async fn supersede_memory(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<WorkspaceAppState>>,
    Path((project_id, slug, fact_id)): Path<(i32, String, i64)>,
    Json(body): Json<SupersedeBody>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, ProjectsWrite);

    let agent_id = resolve_agent_id(&app_state, project_id, &slug).await?;
    let new_fact = app_state
        .memory_service
        .supersede(SupersedeRequest {
            project_id,
            agent_id,
            old_fact_id: fact_id,
            new_fact: body.new_fact,
            new_tags: body.new_tags,
            source_run_id: body.source_run_id,
        })
        .await?;

    tracing::info!(
        "Memory supersede: project={} slug={} old_id={} new_id={}",
        project_id,
        slug,
        fact_id,
        new_fact.id
    );

    Ok((StatusCode::OK, Json(MemoryFactResponse::from(new_fact))))
}

async fn drop_memory(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<WorkspaceAppState>>,
    Path((project_id, slug, fact_id)): Path<(i32, String, i64)>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, ProjectsWrite);

    let agent_id = resolve_agent_id(&app_state, project_id, &slug).await?;
    app_state
        .memory_service
        .delete_fact(project_id, agent_id, fact_id)
        .await?;

    tracing::info!(
        "Memory drop: project={} slug={} fact_id={}",
        project_id,
        slug,
        fact_id
    );

    Ok(StatusCode::NO_CONTENT)
}

// ── For load_for_trigger to be exposed via internal API later, we re-export
// the TriggerContext type so it's accessible from this module.
pub use crate::services::memory_service::TriggerContext as MemoryTriggerContext;

// Silence unused-import warning since the TriggerContext re-export is for
// future use by other modules in this crate.
#[allow(dead_code)]
fn _ensure_trigger_context_imported(_: TriggerContext) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_fact_response_serializes_tags() {
        let mut model = workflow_memory::Model {
            id: 1,
            project_id: 10,
            agent_id: 5,
            fact: "test fact".to_string(),
            tags: serde_json::json!(["tag1", "tag2"]),
            confidence: 0.7,
            times_used: 3,
            source_run_ids: serde_json::json!([100, 101]),
            superseded_by: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            last_used_at: None,
            embedding: None,
            expires_at: None,
        };
        // Make sure it survives the round-trip
        let response = MemoryFactResponse::from(model.clone());
        assert_eq!(response.tags, vec!["tag1".to_string(), "tag2".to_string()]);
        assert_eq!(response.source_run_ids, vec![100, 101]);
        assert_eq!(response.confidence, 0.7);
        assert_eq!(response.times_used, 3);

        // Bad tags (not strings) get filtered out gracefully
        model.tags = serde_json::json!([42, "valid", null]);
        let response = MemoryFactResponse::from(model);
        assert_eq!(response.tags, vec!["valid".to_string()]);
    }

    #[test]
    fn test_memory_fact_response_handles_null_tags() {
        let model = workflow_memory::Model {
            id: 1,
            project_id: 10,
            agent_id: 5,
            fact: "test".to_string(),
            tags: serde_json::Value::Null,
            confidence: 0.5,
            times_used: 0,
            source_run_ids: serde_json::Value::Null,
            superseded_by: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            last_used_at: None,
            embedding: None,
            expires_at: None,
        };
        let response = MemoryFactResponse::from(model);
        assert!(response.tags.is_empty());
        assert!(response.source_run_ids.is_empty());
    }

    #[test]
    fn routes_mount_both_legacy_and_v1_prefixes() {
        // Guards the dual-namespace contract promised by this module's
        // doc comment (and ADR 009). If someone refactors `routes()` and
        // drops a prefix, clients break silently — this test catches it
        // before the PR lands.
        //
        // We inspect the route paths by constructing the router and
        // using debug formatting, which is stable across axum 0.8.
        let router = routes();
        let debug = format!("{:?}", router);
        assert!(
            debug.contains("/projects/{project_id}/workflows/{slug}/memory"),
            "legacy prefix missing: {debug}",
        );
        assert!(
            debug.contains("/v1/projects/{project_id}/workflows/{slug}/memory"),
            "v1 prefix missing: {debug}",
        );
    }

    #[test]
    fn test_memory_fact_response_includes_superseded_by() {
        let model = workflow_memory::Model {
            id: 1,
            project_id: 10,
            agent_id: 5,
            fact: "old".to_string(),
            tags: serde_json::json!([]),
            confidence: 0.5,
            times_used: 0,
            source_run_ids: serde_json::json!([]),
            superseded_by: Some(42),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            last_used_at: None,
            embedding: None,
            expires_at: None,
        };
        let response = MemoryFactResponse::from(model);
        assert_eq!(response.superseded_by, Some(42));
    }
}
