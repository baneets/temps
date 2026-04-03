use axum::{
    extract::{Path, Query, State},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse,
    },
    routing::get,
    Json, Router,
};
use futures::stream::Stream;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

use temps_auth::{permission_guard, RequireAuth};
use temps_core::problemdetails::Problem;
use temps_entities::{agent_run_logs, agent_runs};

use crate::handlers::AppState;

// ── Response DTOs ─────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct AgentRunResponse {
    pub id: i32,
    pub project_id: i32,
    pub config_id: i32,
    /// Slug of the agent that created this run, if available.
    pub agent_slug: Option<String>,
    /// Name of the agent that created this run, if available.
    pub agent_name: Option<String>,
    pub trigger_type: String,
    pub trigger_source_id: Option<i32>,
    pub trigger_source_type: Option<String>,
    pub status: String,
    pub branch_name: Option<String>,
    pub commit_sha: Option<String>,
    pub pr_url: Option<String>,
    pub pr_number: Option<i32>,
    pub preview_url: Option<String>,
    pub error_message: Option<String>,
    pub ai_output: Option<String>,
    pub ai_reasoning: Option<String>,
    pub ai_model: Option<String>,
    pub tokens_input: i32,
    pub tokens_output: i32,
    pub estimated_cost_cents: i32,
    pub files_changed: i32,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub created_at: String,
    /// Whether this run executed inside a sandbox.
    pub sandbox_enabled: bool,
}

impl From<agent_runs::Model> for AgentRunResponse {
    fn from(model: agent_runs::Model) -> Self {
        Self {
            id: model.id,
            project_id: model.project_id,
            config_id: model.config_id,
            agent_slug: None,
            agent_name: None,
            trigger_type: model.trigger_type,
            trigger_source_id: model.trigger_source_id,
            trigger_source_type: model.trigger_source_type,
            status: model.status,
            branch_name: model.branch_name,
            commit_sha: model.commit_sha,
            pr_url: model.pr_url,
            pr_number: model.pr_number,
            preview_url: model.preview_url,
            error_message: model.error_message,
            ai_output: model.ai_output,
            ai_reasoning: model.ai_reasoning,
            ai_model: model.ai_model,
            tokens_input: model.tokens_input,
            tokens_output: model.tokens_output,
            estimated_cost_cents: model.estimated_cost_cents,
            files_changed: model.files_changed,
            started_at: model.started_at.map(|t| t.to_rfc3339()),
            completed_at: model.completed_at.map(|t| t.to_rfc3339()),
            created_at: model.created_at.to_rfc3339(),
            sandbox_enabled: false,
        }
    }
}

impl AgentRunResponse {
    /// Build from a run model, enriched with the agent's slug and name.
    pub fn from_with_agent(
        model: agent_runs::Model,
        agent_slug: Option<String>,
        agent_name: Option<String>,
        sandbox_enabled: bool,
    ) -> Self {
        let mut resp = Self::from(model);
        resp.agent_slug = agent_slug;
        resp.agent_name = agent_name;
        resp.sandbox_enabled = sandbox_enabled;
        resp
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct AgentRunLogResponse {
    pub id: i64,
    pub run_id: i32,
    pub level: String,
    pub message: String,
    pub metadata: Option<serde_json::Value>,
    pub created_at: String,
}

impl From<agent_run_logs::Model> for AgentRunLogResponse {
    fn from(model: agent_run_logs::Model) -> Self {
        Self {
            id: model.id,
            run_id: model.run_id,
            level: model.level,
            message: model.message,
            metadata: model.metadata,
            created_at: model.created_at.to_rfc3339(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct AgentRunWithLogsResponse {
    pub run: AgentRunResponse,
    pub logs: Vec<AgentRunLogResponse>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ListRunsResponse {
    pub items: Vec<AgentRunResponse>,
    pub total: u64,
    pub page: u64,
    pub page_size: u64,
}

#[derive(Debug, Deserialize)]
pub struct ListRunsQuery {
    pub page: Option<u64>,
    pub page_size: Option<u64>,
}

// ── Routes ────────────────────────────────────────────────────────────────────

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        // All runs for a project (across all agents)
        .route("/projects/{project_id}/agents/runs", get(list_all_runs))
        // All runs for a specific agent
        .route(
            "/projects/{project_id}/agents/{slug}/runs",
            get(list_agent_runs),
        )
        // Single run detail by run ID
        .route(
            "/projects/{project_id}/agents/runs/{run_id}",
            get(get_run_with_logs),
        )
        // Cancel a running run
        .route(
            "/projects/{project_id}/agents/runs/{run_id}/cancel",
            axum::routing::post(cancel_run),
        )
        // SSE stream for real-time run events
        .route(
            "/projects/{project_id}/agents/runs/{run_id}/stream",
            get(stream_run_events),
        )
}

// ── Handlers ──────────────────────────────────────────────────────────────────

#[utoipa::path(
    tag = "Agents",
    get,
    path = "/projects/{project_id}/agents/runs",
    params(
        ("project_id" = i32, Path, description = "Project ID"),
        ("page" = Option<u64>, Query, description = "Page number (1-based)"),
        ("page_size" = Option<u64>, Query, description = "Page size (max 100)"),
    ),
    responses(
        (status = 200, description = "List of all agent runs for a project", body = ListRunsResponse),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Insufficient permissions"),
        (status = 500, description = "Internal server error"),
    ),
    security(("bearer_auth" = []))
)]
async fn list_all_runs(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<AppState>>,
    Path(project_id): Path<i32>,
    Query(query): Query<ListRunsQuery>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, ProjectsRead);

    let page = query.page.unwrap_or(1);
    let page_size = query.page_size.unwrap_or(20);

    let (runs, total) = app_state
        .run_service
        .list_runs(project_id, Some(page), Some(page_size))
        .await
        .map_err(Problem::from)?;

    // Enrich with agent slug/name by looking up the config_id
    let agents = app_state
        .config_service
        .list_agents(project_id)
        .await
        .map_err(Problem::from)?;

    let items = runs
        .into_iter()
        .map(|run| {
            let agent = agents.iter().find(|a| a.id == run.config_id);
            AgentRunResponse::from_with_agent(
                run,
                agent.map(|a| a.slug.clone()),
                agent.map(|a| a.name.clone()),
                agent.map(|a| a.sandbox_enabled).unwrap_or(false),
            )
        })
        .collect();

    Ok(Json(ListRunsResponse {
        items,
        total,
        page,
        page_size,
    }))
}

#[utoipa::path(
    tag = "Agents",
    get,
    path = "/projects/{project_id}/agents/{slug}/runs",
    params(
        ("project_id" = i32, Path, description = "Project ID"),
        ("slug" = String, Path, description = "Agent slug"),
        ("page" = Option<u64>, Query, description = "Page number (1-based)"),
        ("page_size" = Option<u64>, Query, description = "Page size (max 100)"),
    ),
    responses(
        (status = 200, description = "List of runs for a specific agent", body = ListRunsResponse),
        (status = 404, description = "Agent not found"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Insufficient permissions"),
        (status = 500, description = "Internal server error"),
    ),
    security(("bearer_auth" = []))
)]
async fn list_agent_runs(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<AppState>>,
    Path((project_id, slug)): Path<(i32, String)>,
    Query(query): Query<ListRunsQuery>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, ProjectsRead);

    let agent = app_state
        .config_service
        .get_agent_by_slug(project_id, &slug)
        .await
        .map_err(Problem::from)?
        .ok_or_else(|| {
            Problem::from(crate::error::AgentError::AgentNotFound {
                project_id,
                slug: slug.clone(),
            })
        })?;

    let page = query.page.unwrap_or(1);
    let page_size = query.page_size.unwrap_or(20);

    let (runs, total) = app_state
        .run_service
        .list_runs_for_agent(agent.id, Some(page), Some(page_size))
        .await
        .map_err(Problem::from)?;

    let items = runs
        .into_iter()
        .map(|run| {
            AgentRunResponse::from_with_agent(
                run,
                Some(agent.slug.clone()),
                Some(agent.name.clone()),
                agent.sandbox_enabled,
            )
        })
        .collect();

    Ok(Json(ListRunsResponse {
        items,
        total,
        page,
        page_size,
    }))
}

#[utoipa::path(
    tag = "Agents",
    get,
    path = "/projects/{project_id}/agents/runs/{run_id}",
    params(
        ("project_id" = i32, Path, description = "Project ID"),
        ("run_id" = i32, Path, description = "Run ID"),
    ),
    responses(
        (status = 200, description = "Run with logs", body = AgentRunWithLogsResponse),
        (status = 404, description = "Run not found"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Insufficient permissions"),
        (status = 500, description = "Internal server error"),
    ),
    security(("bearer_auth" = []))
)]
async fn get_run_with_logs(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<AppState>>,
    Path((_project_id, run_id)): Path<(i32, i32)>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, ProjectsRead);

    let run_with_logs = app_state
        .run_service
        .get_run_with_logs(run_id)
        .await
        .map_err(Problem::from)?;

    // Enrich with agent slug/name
    let agent = app_state
        .config_service
        .get_agent_by_id(run_with_logs.run.config_id)
        .await
        .map_err(Problem::from)?;

    let run_resp = AgentRunResponse::from_with_agent(
        run_with_logs.run,
        agent.as_ref().map(|a| a.slug.clone()),
        agent.as_ref().map(|a| a.name.clone()),
        agent.as_ref().map(|a| a.sandbox_enabled).unwrap_or(false),
    );

    Ok(Json(AgentRunWithLogsResponse {
        run: run_resp,
        logs: run_with_logs
            .logs
            .into_iter()
            .map(AgentRunLogResponse::from)
            .collect(),
    }))
}

/// SSE endpoint for real-time streaming of run events.
/// Polls the agent_run_logs table every 500ms for new entries and streams them.
/// Closes when the run reaches a terminal status.
async fn stream_run_events(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<AppState>>,
    Path((_project_id, run_id)): Path<(i32, i32)>,
) -> Result<Sse<impl Stream<Item = Result<Event, std::convert::Infallible>>>, Problem> {
    permission_guard!(auth, ProjectsRead);

    let run_service = app_state.run_service.clone();

    let stream = async_stream::stream! {
        let mut last_log_id: i64 = 0;
        let terminal = ["completed", "failed", "no_fix", "cancelled"];

        loop {
            // Fetch new logs since last_log_id
            match run_service.get_logs_after(run_id, last_log_id).await {
                Ok(logs) => {
                    for log in logs {
                        if log.id > last_log_id {
                            last_log_id = log.id;
                        }
                        let data = serde_json::json!({
                            "id": log.id,
                            "level": log.level,
                            "message": log.message,
                            "metadata": log.metadata,
                            "created_at": log.created_at.to_rfc3339(),
                        });
                        yield Ok(Event::default().data(data.to_string()));
                    }
                }
                Err(e) => {
                    tracing::warn!("SSE stream error for run {}: {}", run_id, e);
                }
            }

            // Check if run is in terminal state
            match run_service.get_run(run_id).await {
                Ok(run) => {
                    if terminal.contains(&run.status.as_str()) {
                        // Send final status event and close
                        let status_event = serde_json::json!({
                            "type": "run_status",
                            "status": run.status,
                        });
                        yield Ok(Event::default().event("status").data(status_event.to_string()));
                        break;
                    }
                }
                Err(_) => break,
            }

            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        }
    };

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

async fn cancel_run(
    RequireAuth(auth): RequireAuth,
    State(app_state): State<Arc<AppState>>,
    Path((_project_id, run_id)): Path<(i32, i32)>,
) -> Result<impl IntoResponse, Problem> {
    permission_guard!(auth, ProjectsWrite);

    let run = app_state
        .run_service
        .cancel_run(run_id)
        .await
        .map_err(Problem::from)?;

    Ok(Json(AgentRunResponse::from(run)))
}
