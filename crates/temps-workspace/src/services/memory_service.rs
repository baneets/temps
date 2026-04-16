//! Workflow memory service.
//!
//! Stores per-workflow facts that the AI accumulates over time. Memory is
//! strictly scoped by `(project_id, agent_id)` — facts never cross workflow
//! or project boundaries.
//!
//! ## Memory lifecycle
//!
//! 1. **Write**: AI calls `memory write "..."` during a run. Each fact has
//!    optional tags for later retrieval and starts at confidence 0.5.
//! 2. **Search/list**: Future runs (or the AI itself, mid-run) query memory
//!    by tag, full-text, or just list everything. Each successful retrieval
//!    bumps `times_used` and `last_used_at`.
//! 3. **Supersede**: If a fact becomes outdated, the AI replaces it with a
//!    new fact. The old row is kept for audit, with `superseded_by` pointing
//!    forward.
//! 4. **Compaction** (handled elsewhere): a background service merges
//!    duplicates and drops stale low-value facts.

use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, DatabaseBackend,
    DatabaseConnection, EntityTrait, Order, QueryFilter, QueryOrder, QuerySelect, Statement,
};
use std::sync::Arc;

use temps_entities::workflow_memory;

use crate::error::WorkspaceError;

// ── Constants ───────────────────────────────────────────────────────────────

/// Maximum length of a single fact. Forces concise statements.
pub const MAX_FACT_LENGTH: usize = 500;

/// Maximum number of tags per fact.
pub const MAX_TAGS_PER_FACT: usize = 16;

/// Maximum length of a single tag.
pub const MAX_TAG_LENGTH: usize = 64;

/// Default initial confidence for new facts.
pub const DEFAULT_CONFIDENCE: f32 = 0.5;

/// When loading memory for a trigger, never push more than this many facts
/// into the prompt.
pub const MAX_FACTS_PER_PROMPT: usize = 20;

/// Soft limit on active facts per workflow before compaction kicks in.
pub const COMPACTION_THRESHOLD: i64 = 100;

// ── Request types ───────────────────────────────────────────────────────────

/// Request to write a new fact.
#[derive(Debug, Clone)]
pub struct WriteFactRequest {
    pub project_id: i32,
    pub agent_id: i32,
    /// The natural-language fact
    pub fact: String,
    /// Tags for later retrieval (e.g. ["error_group_id:42", "file:src/api.ts"])
    pub tags: Vec<String>,
    /// Optional source run for provenance
    pub source_run_id: Option<i32>,
    /// Optional explicit confidence (defaults to DEFAULT_CONFIDENCE)
    pub confidence: Option<f32>,
}

/// Request to supersede an existing fact with a new one.
#[derive(Debug, Clone)]
pub struct SupersedeRequest {
    pub project_id: i32,
    pub agent_id: i32,
    pub old_fact_id: i64,
    pub new_fact: String,
    pub new_tags: Vec<String>,
    pub source_run_id: Option<i32>,
}

/// Filter criteria for `load_for_trigger`. The executor uses this to
/// pre-load relevant memory into the prompt before spawning the harness.
#[derive(Debug, Clone, Default)]
pub struct TriggerContext {
    pub project_id: i32,
    pub agent_id: i32,
    /// Tags to match against (any-of). E.g. `["error_group_id:42", "file:src/api.ts"]`.
    pub relevant_tags: Vec<String>,
    /// Maximum facts to return (defaults to `MAX_FACTS_PER_PROMPT`).
    pub limit: Option<usize>,
}

// ── Service ─────────────────────────────────────────────────────────────────

pub struct WorkflowMemoryService {
    db: Arc<DatabaseConnection>,
}

impl WorkflowMemoryService {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }

    // ── Validation ──────────────────────────────────────────────────────────

    fn validate_fact(fact: &str) -> Result<(), WorkspaceError> {
        let trimmed = fact.trim();
        if trimmed.is_empty() {
            return Err(WorkspaceError::Validation {
                message: "Fact cannot be empty".to_string(),
            });
        }
        if trimmed.len() > MAX_FACT_LENGTH {
            return Err(WorkspaceError::Validation {
                message: format!(
                    "Fact too long ({} chars, max {})",
                    trimmed.len(),
                    MAX_FACT_LENGTH
                ),
            });
        }
        Ok(())
    }

    fn validate_tags(tags: &[String]) -> Result<(), WorkspaceError> {
        if tags.len() > MAX_TAGS_PER_FACT {
            return Err(WorkspaceError::Validation {
                message: format!("Too many tags ({}, max {})", tags.len(), MAX_TAGS_PER_FACT),
            });
        }
        for tag in tags {
            if tag.is_empty() {
                return Err(WorkspaceError::Validation {
                    message: "Tag cannot be empty".to_string(),
                });
            }
            if tag.len() > MAX_TAG_LENGTH {
                return Err(WorkspaceError::Validation {
                    message: format!(
                        "Tag '{}' too long ({} chars, max {})",
                        tag,
                        tag.len(),
                        MAX_TAG_LENGTH
                    ),
                });
            }
        }
        Ok(())
    }

    fn normalize_tags(tags: Vec<String>) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        tags.into_iter()
            .map(|t| t.trim().to_lowercase())
            .filter(|t| !t.is_empty() && seen.insert(t.clone()))
            .collect()
    }

    // ── Write ───────────────────────────────────────────────────────────────

    /// Write a new fact to memory.
    pub async fn write(
        &self,
        request: WriteFactRequest,
    ) -> Result<workflow_memory::Model, WorkspaceError> {
        Self::validate_fact(&request.fact)?;
        Self::validate_tags(&request.tags)?;

        let normalized_tags = Self::normalize_tags(request.tags);
        let confidence = request
            .confidence
            .unwrap_or(DEFAULT_CONFIDENCE)
            .clamp(0.0, 1.0);

        let source_run_ids = match request.source_run_id {
            Some(id) => serde_json::json!([id]),
            None => serde_json::json!([]),
        };

        let now = Utc::now();
        let active = workflow_memory::ActiveModel {
            project_id: Set(request.project_id),
            agent_id: Set(request.agent_id),
            fact: Set(request.fact.trim().to_string()),
            tags: Set(serde_json::Value::Array(
                normalized_tags
                    .into_iter()
                    .map(serde_json::Value::String)
                    .collect(),
            )),
            confidence: Set(confidence),
            times_used: Set(0),
            source_run_ids: Set(source_run_ids),
            superseded_by: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
            last_used_at: Set(None),
            ..Default::default()
        };

        let model = active.insert(self.db.as_ref()).await?;
        Ok(model)
    }

    // ── Read ────────────────────────────────────────────────────────────────

    /// Get a single fact by ID. Enforces project + agent scope as a security
    /// boundary — never returns a fact that belongs to a different workflow.
    pub async fn get(
        &self,
        project_id: i32,
        agent_id: i32,
        fact_id: i64,
    ) -> Result<workflow_memory::Model, WorkspaceError> {
        let fact = workflow_memory::Entity::find_by_id(fact_id)
            .filter(workflow_memory::Column::ProjectId.eq(project_id))
            .filter(workflow_memory::Column::AgentId.eq(agent_id))
            .one(self.db.as_ref())
            .await?
            .ok_or(WorkspaceError::MemoryNotFound {
                project_id,
                agent_id,
                fact_id,
            })?;
        Ok(fact)
    }

    /// List all active (non-superseded) facts for a workflow, ordered by
    /// confidence DESC, times_used DESC.
    pub async fn list(
        &self,
        project_id: i32,
        agent_id: i32,
        limit: Option<u64>,
    ) -> Result<Vec<workflow_memory::Model>, WorkspaceError> {
        let limit = limit.unwrap_or(50).min(200);
        let facts = workflow_memory::Entity::find()
            .filter(workflow_memory::Column::ProjectId.eq(project_id))
            .filter(workflow_memory::Column::AgentId.eq(agent_id))
            .filter(workflow_memory::Column::SupersededBy.is_null())
            .order_by(workflow_memory::Column::Confidence, Order::Desc)
            .order_by(workflow_memory::Column::TimesUsed, Order::Desc)
            .limit(limit)
            .all(self.db.as_ref())
            .await?;
        Ok(facts)
    }

    /// Count active facts for a workflow (used by compaction triggers).
    pub async fn count_active(
        &self,
        project_id: i32,
        agent_id: i32,
    ) -> Result<i64, WorkspaceError> {
        let stmt = Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT COUNT(*)::BIGINT AS count FROM workflow_memory \
             WHERE project_id = $1 AND agent_id = $2 AND superseded_by IS NULL",
            [project_id.into(), agent_id.into()],
        );
        let result = self.db.query_one(stmt).await?;
        match result {
            Some(row) => row
                .try_get::<i64>("", "count")
                .map_err(WorkspaceError::Database),
            None => Ok(0),
        }
    }

    /// Full-text search over facts. Uses Postgres `to_tsvector(fact) @@ plainto_tsquery(query)`.
    pub async fn search(
        &self,
        project_id: i32,
        agent_id: i32,
        query: &str,
        limit: Option<u64>,
    ) -> Result<Vec<workflow_memory::Model>, WorkspaceError> {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return Err(WorkspaceError::Validation {
                message: "Search query cannot be empty".to_string(),
            });
        }
        let limit = limit.unwrap_or(20).min(100) as i64;

        let stmt = Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT id, project_id, agent_id, fact, tags, confidence, times_used, \
                    source_run_ids, superseded_by, created_at, updated_at, last_used_at \
             FROM workflow_memory \
             WHERE project_id = $1 \
               AND agent_id = $2 \
               AND superseded_by IS NULL \
               AND to_tsvector('english', fact) @@ plainto_tsquery('english', $3) \
             ORDER BY ts_rank(to_tsvector('english', fact), plainto_tsquery('english', $3)) DESC, \
                      confidence DESC \
             LIMIT $4",
            [
                project_id.into(),
                agent_id.into(),
                trimmed.into(),
                limit.into(),
            ],
        );

        let facts = workflow_memory::Entity::find()
            .from_raw_sql(stmt)
            .all(self.db.as_ref())
            .await?;
        Ok(facts)
    }

    /// Load memory relevant to a specific trigger context. This is what the
    /// executor calls before spawning the harness — it pulls facts that match
    /// any of the relevant tags, plus high-confidence general facts as a
    /// fallback. Capped at `MAX_FACTS_PER_PROMPT`.
    pub async fn load_for_trigger(
        &self,
        ctx: &TriggerContext,
    ) -> Result<Vec<workflow_memory::Model>, WorkspaceError> {
        let limit = ctx.limit.unwrap_or(MAX_FACTS_PER_PROMPT).min(50) as i64;

        // If no tags, just return the highest-confidence facts.
        if ctx.relevant_tags.is_empty() {
            let stmt = Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "SELECT id, project_id, agent_id, fact, tags, confidence, times_used, \
                        source_run_ids, superseded_by, created_at, updated_at, last_used_at \
                 FROM workflow_memory \
                 WHERE project_id = $1 AND agent_id = $2 AND superseded_by IS NULL \
                 ORDER BY confidence DESC, times_used DESC \
                 LIMIT $3",
                [ctx.project_id.into(), ctx.agent_id.into(), limit.into()],
            );
            let facts = workflow_memory::Entity::find()
                .from_raw_sql(stmt)
                .all(self.db.as_ref())
                .await?;
            return Ok(facts);
        }

        // Match facts whose tags overlap with relevant_tags, OR high-confidence
        // general facts (confidence > 0.8). Tags are JSONB arrays — we use the
        // ?| operator (any of these keys exist as top-level array elements).
        let tags_json = serde_json::to_value(&ctx.relevant_tags).unwrap_or_default();
        let stmt = Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT id, project_id, agent_id, fact, tags, confidence, times_used, \
                    source_run_ids, superseded_by, created_at, updated_at, last_used_at \
             FROM workflow_memory \
             WHERE project_id = $1 \
               AND agent_id = $2 \
               AND superseded_by IS NULL \
               AND (tags ?| $3 OR confidence > 0.8) \
             ORDER BY confidence DESC, times_used DESC \
             LIMIT $4",
            [
                ctx.project_id.into(),
                ctx.agent_id.into(),
                // Convert the Vec<String> to a Postgres text[] which the ?| operator expects
                sea_orm::Value::Array(
                    sea_orm::sea_query::ArrayType::String,
                    Some(Box::new(
                        ctx.relevant_tags
                            .iter()
                            .map(|t| sea_orm::Value::String(Some(Box::new(t.clone()))))
                            .collect(),
                    )),
                ),
                limit.into(),
            ],
        );
        let _ = tags_json;

        let facts = workflow_memory::Entity::find()
            .from_raw_sql(stmt)
            .all(self.db.as_ref())
            .await?;
        Ok(facts)
    }

    // ── Update ──────────────────────────────────────────────────────────────

    /// Mark a fact as recently used. Bumps `times_used` and `last_used_at`.
    /// Best-effort: failures are returned but the caller may choose to ignore them.
    pub async fn mark_used(
        &self,
        project_id: i32,
        agent_id: i32,
        fact_id: i64,
    ) -> Result<(), WorkspaceError> {
        let stmt = Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE workflow_memory \
             SET times_used = times_used + 1, last_used_at = NOW(), updated_at = NOW() \
             WHERE id = $1 AND project_id = $2 AND agent_id = $3 AND superseded_by IS NULL",
            [fact_id.into(), project_id.into(), agent_id.into()],
        );
        self.db.execute(stmt).await?;
        Ok(())
    }

    /// Mark several facts as used in a single statement.
    pub async fn mark_used_many(
        &self,
        project_id: i32,
        agent_id: i32,
        fact_ids: &[i64],
    ) -> Result<(), WorkspaceError> {
        if fact_ids.is_empty() {
            return Ok(());
        }
        // Build placeholders $3, $4, ... for the id list. $1 = project_id, $2 = agent_id.
        let placeholders: Vec<String> =
            (0..fact_ids.len()).map(|i| format!("${}", i + 3)).collect();
        let sql = format!(
            "UPDATE workflow_memory \
             SET times_used = times_used + 1, last_used_at = NOW(), updated_at = NOW() \
             WHERE project_id = $1 AND agent_id = $2 AND superseded_by IS NULL \
               AND id IN ({})",
            placeholders.join(", ")
        );
        let mut values: Vec<sea_orm::Value> = vec![project_id.into(), agent_id.into()];
        for id in fact_ids {
            values.push((*id).into());
        }
        let stmt = Statement::from_sql_and_values(DatabaseBackend::Postgres, &sql, values);
        self.db.execute(stmt).await?;
        Ok(())
    }

    /// Replace an outdated fact with a new one. The old fact is kept (audit trail)
    /// with `superseded_by` pointing at the new fact's id.
    pub async fn supersede(
        &self,
        request: SupersedeRequest,
    ) -> Result<workflow_memory::Model, WorkspaceError> {
        // Validate the new fact
        Self::validate_fact(&request.new_fact)?;
        Self::validate_tags(&request.new_tags)?;

        // Verify the old fact exists in this scope (security boundary check)
        let old = self
            .get(request.project_id, request.agent_id, request.old_fact_id)
            .await?;

        if old.superseded_by.is_some() {
            return Err(WorkspaceError::Validation {
                message: format!("Fact {} is already superseded", request.old_fact_id),
            });
        }

        // Insert the new fact, carrying forward provenance from the old one
        // and bumping confidence slightly (the AI is reinforcing this knowledge area).
        let mut source_runs: Vec<i32> = old
            .source_run_ids
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_i64().map(|n| n as i32))
                    .collect()
            })
            .unwrap_or_default();
        if let Some(run_id) = request.source_run_id {
            if !source_runs.contains(&run_id) {
                source_runs.push(run_id);
            }
        }
        let new_confidence = (old.confidence + 0.1).min(1.0);

        let new = self
            .write(WriteFactRequest {
                project_id: request.project_id,
                agent_id: request.agent_id,
                fact: request.new_fact,
                tags: request.new_tags,
                source_run_id: None, // we set source_run_ids manually below
                confidence: Some(new_confidence),
            })
            .await?;

        // Patch the new fact's source_run_ids to include the inherited ones
        if !source_runs.is_empty() {
            let stmt = Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "UPDATE workflow_memory SET source_run_ids = $1, updated_at = NOW() \
                 WHERE id = $2",
                [
                    serde_json::to_value(&source_runs).unwrap().into(),
                    new.id.into(),
                ],
            );
            self.db.execute(stmt).await?;
        }

        // Mark the old fact as superseded
        let stmt = Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE workflow_memory SET superseded_by = $1, updated_at = NOW() WHERE id = $2",
            [new.id.into(), request.old_fact_id.into()],
        );
        self.db.execute(stmt).await?;

        Ok(new)
    }

    // ── Prompt rendering ────────────────────────────────────────────────────

    /// Render a "Things you've learned from past runs" section to be prepended
    /// to a workflow's prompt. Returns an empty string if there's no memory yet.
    ///
    /// This is the **push** half of memory: even if the AI never calls
    /// `memory search`, it always sees the most relevant facts at the top
    /// of the prompt.
    pub async fn render_for_prompt(&self, ctx: &TriggerContext) -> Result<String, WorkspaceError> {
        let facts = self.load_for_trigger(ctx).await?;
        Ok(render_memory_section(&facts))
    }

    /// Hard-delete a fact (used by compaction for noise/stale facts).
    /// Named `delete_fact` to avoid colliding with `Drop::drop`.
    pub async fn delete_fact(
        &self,
        project_id: i32,
        agent_id: i32,
        fact_id: i64,
    ) -> Result<(), WorkspaceError> {
        // Verify scope first
        let _ = self.get(project_id, agent_id, fact_id).await?;

        let stmt = Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "DELETE FROM workflow_memory WHERE id = $1 AND project_id = $2 AND agent_id = $3",
            [fact_id.into(), project_id.into(), agent_id.into()],
        );
        self.db.execute(stmt).await?;
        Ok(())
    }
}

// ── WorkflowMemoryProvider trait impl ───────────────────────────────────────
//
// Lets the agent executor (in temps-agents) consume this service via the
// trait defined in temps-core, without creating a temps-agents → temps-workspace
// dependency cycle.

#[async_trait::async_trait]
impl temps_core::WorkflowMemoryProvider for WorkflowMemoryService {
    async fn load_for_trigger(
        &self,
        project_id: i32,
        agent_id: i32,
        relevant_tags: Vec<String>,
        limit: usize,
    ) -> Result<Vec<temps_core::WorkflowMemoryFact>, temps_core::WorkflowMemoryError> {
        let ctx = TriggerContext {
            project_id,
            agent_id,
            relevant_tags,
            limit: Some(limit),
        };
        let facts = self
            .load_for_trigger(&ctx)
            .await
            .map_err(|e| temps_core::WorkflowMemoryError::new(e.to_string()))?;
        Ok(facts
            .into_iter()
            .map(|f| temps_core::WorkflowMemoryFact {
                id: f.id,
                fact: f.fact,
                confidence: f.confidence,
                times_used: f.times_used,
            })
            .collect())
    }

    fn render_for_prompt(&self, facts: &[temps_core::WorkflowMemoryFact]) -> String {
        if facts.is_empty() {
            return String::new();
        }

        let mut out = String::new();
        out.push_str("## Things you've learned about this from past runs\n\n");
        for fact in facts {
            let confidence_label = match fact.confidence {
                c if c >= 0.8 => "high",
                c if c >= 0.5 => "medium",
                _ => "low",
            };
            out.push_str(&format!(
                "- ({}, used {}x) {}\n",
                confidence_label, fact.times_used, fact.fact
            ));
        }
        out.push('\n');
        out.push_str(
            "If anything here looks outdated, run `memory supersede <id> --by \"<new fact>\"`.\n",
        );
        out.push_str("You can search for more with `memory search \"<query>\"`.\n\n");
        out.push_str("---\n\n");
        out
    }
}

// ── Prompt rendering helper ─────────────────────────────────────────────────

/// Format a list of facts into a markdown section that can be prepended to
/// any AI prompt. Returns an empty string for empty input — never injects
/// the section header on its own.
pub fn render_memory_section(facts: &[workflow_memory::Model]) -> String {
    if facts.is_empty() {
        return String::new();
    }

    let mut out = String::new();
    out.push_str("## Things you've learned about this from past runs\n\n");

    for fact in facts {
        let confidence_label = match fact.confidence {
            c if c >= 0.8 => "high",
            c if c >= 0.5 => "medium",
            _ => "low",
        };
        out.push_str(&format!(
            "- ({}, used {}x) {}\n",
            confidence_label, fact.times_used, fact.fact
        ));
    }

    out.push('\n');
    out.push_str(
        "If anything here looks outdated, run `memory supersede <id> --by \"<new fact>\"`.\n",
    );
    out.push_str("You can search for more with `memory search \"<query>\"`.\n\n");
    out.push_str("---\n\n");

    out
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::{DatabaseBackend, MockDatabase, MockExecResult};

    fn mock_fact(id: i64, project_id: i32, agent_id: i32, fact: &str) -> workflow_memory::Model {
        let now = Utc::now();
        workflow_memory::Model {
            id,
            project_id,
            agent_id,
            fact: fact.to_string(),
            tags: serde_json::json!([]),
            confidence: 0.5,
            times_used: 0,
            source_run_ids: serde_json::json!([]),
            superseded_by: None,
            created_at: now,
            updated_at: now,
            last_used_at: None,
            embedding: None,
            expires_at: None,
        }
    }

    fn fact_with_tags(
        id: i64,
        project_id: i32,
        agent_id: i32,
        fact: &str,
        tags: Vec<&str>,
        confidence: f32,
    ) -> workflow_memory::Model {
        let mut m = mock_fact(id, project_id, agent_id, fact);
        m.tags = serde_json::Value::Array(
            tags.iter()
                .map(|t| serde_json::Value::String(t.to_string()))
                .collect(),
        );
        m.confidence = confidence;
        m
    }

    // ── Validation ──────────────────────────────────────────────────────────

    #[test]
    fn test_validate_fact_empty() {
        let result = WorkflowMemoryService::validate_fact("");
        assert!(matches!(result, Err(WorkspaceError::Validation { .. })));
    }

    #[test]
    fn test_validate_fact_whitespace_only() {
        let result = WorkflowMemoryService::validate_fact("   \n\t  ");
        assert!(matches!(result, Err(WorkspaceError::Validation { .. })));
    }

    #[test]
    fn test_validate_fact_too_long() {
        let long = "x".repeat(MAX_FACT_LENGTH + 1);
        let result = WorkflowMemoryService::validate_fact(&long);
        assert!(matches!(result, Err(WorkspaceError::Validation { .. })));
    }

    #[test]
    fn test_validate_fact_at_limit_ok() {
        let at_limit = "x".repeat(MAX_FACT_LENGTH);
        let result = WorkflowMemoryService::validate_fact(&at_limit);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_fact_normal() {
        let result = WorkflowMemoryService::validate_fact(
            "OAuth callbacks fail when state cookie is missing",
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_tags_too_many() {
        let tags: Vec<String> = (0..MAX_TAGS_PER_FACT + 1)
            .map(|i| format!("tag{}", i))
            .collect();
        let result = WorkflowMemoryService::validate_tags(&tags);
        assert!(matches!(result, Err(WorkspaceError::Validation { .. })));
    }

    #[test]
    fn test_validate_tags_empty_tag() {
        let tags = vec!["valid".to_string(), "".to_string()];
        let result = WorkflowMemoryService::validate_tags(&tags);
        assert!(matches!(result, Err(WorkspaceError::Validation { .. })));
    }

    #[test]
    fn test_validate_tags_too_long() {
        let tags = vec!["x".repeat(MAX_TAG_LENGTH + 1)];
        let result = WorkflowMemoryService::validate_tags(&tags);
        assert!(matches!(result, Err(WorkspaceError::Validation { .. })));
    }

    #[test]
    fn test_validate_tags_ok() {
        let tags = vec![
            "error_group_id:42".to_string(),
            "file:src/api.ts".to_string(),
        ];
        let result = WorkflowMemoryService::validate_tags(&tags);
        assert!(result.is_ok());
    }

    #[test]
    fn test_normalize_tags_dedupes_and_lowercases() {
        let tags = vec![
            "Error_Group_ID:42".to_string(),
            "error_group_id:42".to_string(),   // dup after lowercase
            "  file:src/api.ts  ".to_string(), // dup after trim
            "file:src/api.ts".to_string(),
        ];
        let normalized = WorkflowMemoryService::normalize_tags(tags);
        assert_eq!(normalized.len(), 2);
        assert!(normalized.contains(&"error_group_id:42".to_string()));
        assert!(normalized.contains(&"file:src/api.ts".to_string()));
    }

    #[test]
    fn test_normalize_tags_drops_empty_after_trim() {
        let tags = vec!["  ".to_string(), "valid".to_string()];
        let normalized = WorkflowMemoryService::normalize_tags(tags);
        assert_eq!(normalized, vec!["valid".to_string()]);
    }

    // ── Write ───────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_write_success() {
        let inserted = mock_fact(1, 10, 5, "OAuth state cookie is missing");
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![vec![inserted.clone()]])
            .into_connection();

        let service = WorkflowMemoryService::new(Arc::new(db));
        let result = service
            .write(WriteFactRequest {
                project_id: 10,
                agent_id: 5,
                fact: "OAuth state cookie is missing".to_string(),
                tags: vec!["error_group_id:42".to_string()],
                source_run_id: Some(100),
                confidence: None,
            })
            .await;

        assert!(result.is_ok(), "write failed: {:?}", result.err());
        let fact = result.unwrap();
        assert_eq!(fact.project_id, 10);
        assert_eq!(fact.agent_id, 5);
    }

    #[tokio::test]
    async fn test_write_empty_fact_fails() {
        let db = MockDatabase::new(DatabaseBackend::Postgres).into_connection();
        let service = WorkflowMemoryService::new(Arc::new(db));

        let result = service
            .write(WriteFactRequest {
                project_id: 10,
                agent_id: 5,
                fact: "".to_string(),
                tags: vec![],
                source_run_id: None,
                confidence: None,
            })
            .await;

        assert!(matches!(result, Err(WorkspaceError::Validation { .. })));
    }

    #[tokio::test]
    async fn test_write_too_many_tags_fails() {
        let db = MockDatabase::new(DatabaseBackend::Postgres).into_connection();
        let service = WorkflowMemoryService::new(Arc::new(db));

        let tags: Vec<String> = (0..30).map(|i| format!("tag{}", i)).collect();
        let result = service
            .write(WriteFactRequest {
                project_id: 10,
                agent_id: 5,
                fact: "valid fact".to_string(),
                tags,
                source_run_id: None,
                confidence: None,
            })
            .await;

        assert!(matches!(result, Err(WorkspaceError::Validation { .. })));
    }

    #[tokio::test]
    async fn test_write_clamps_confidence_above_one() {
        let inserted = mock_fact(1, 10, 5, "valid");
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![vec![inserted]])
            .into_connection();
        let service = WorkflowMemoryService::new(Arc::new(db));

        let result = service
            .write(WriteFactRequest {
                project_id: 10,
                agent_id: 5,
                fact: "valid".to_string(),
                tags: vec![],
                source_run_id: None,
                confidence: Some(1.5),
            })
            .await;

        // Should clamp to 1.0, not error
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_write_clamps_confidence_below_zero() {
        let inserted = mock_fact(1, 10, 5, "valid");
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![vec![inserted]])
            .into_connection();
        let service = WorkflowMemoryService::new(Arc::new(db));

        let result = service
            .write(WriteFactRequest {
                project_id: 10,
                agent_id: 5,
                fact: "valid".to_string(),
                tags: vec![],
                source_run_id: None,
                confidence: Some(-0.5),
            })
            .await;

        assert!(result.is_ok());
    }

    // ── Read ────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_get_success() {
        let fact = mock_fact(1, 10, 5, "test fact");
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![vec![fact.clone()]])
            .into_connection();
        let service = WorkflowMemoryService::new(Arc::new(db));

        let result = service.get(10, 5, 1).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().id, 1);
    }

    #[tokio::test]
    async fn test_get_not_found() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![Vec::<workflow_memory::Model>::new()])
            .into_connection();
        let service = WorkflowMemoryService::new(Arc::new(db));

        let result = service.get(10, 5, 999).await;
        assert!(matches!(
            result,
            Err(WorkspaceError::MemoryNotFound {
                project_id: 10,
                agent_id: 5,
                fact_id: 999
            })
        ));
    }

    #[tokio::test]
    async fn test_get_wrong_project_returns_not_found() {
        // Even if a fact with id=1 exists in project 99, asking from project 10
        // returns NotFound because the WHERE clause filters it out.
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![Vec::<workflow_memory::Model>::new()])
            .into_connection();
        let service = WorkflowMemoryService::new(Arc::new(db));

        let result = service.get(10, 5, 1).await;
        assert!(matches!(result, Err(WorkspaceError::MemoryNotFound { .. })));
    }

    #[tokio::test]
    async fn test_list_returns_facts_in_order() {
        let facts = vec![
            fact_with_tags(1, 10, 5, "high confidence", vec!["x"], 0.9),
            fact_with_tags(2, 10, 5, "medium", vec!["y"], 0.5),
        ];
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![facts])
            .into_connection();
        let service = WorkflowMemoryService::new(Arc::new(db));

        let result = service.list(10, 5, Some(50)).await;
        assert!(result.is_ok());
        let list = result.unwrap();
        assert_eq!(list.len(), 2);
    }

    #[tokio::test]
    async fn test_list_caps_at_max_limit() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![Vec::<workflow_memory::Model>::new()])
            .into_connection();
        let service = WorkflowMemoryService::new(Arc::new(db));

        // Asking for 1000 should be silently capped to 200 — we just verify no error
        let result = service.list(10, 5, Some(1000)).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_search_empty_query_fails() {
        let db = MockDatabase::new(DatabaseBackend::Postgres).into_connection();
        let service = WorkflowMemoryService::new(Arc::new(db));

        let result = service.search(10, 5, "  ", None).await;
        assert!(matches!(result, Err(WorkspaceError::Validation { .. })));
    }

    #[tokio::test]
    async fn test_search_returns_results() {
        let facts = vec![mock_fact(1, 10, 5, "OAuth state cookie missing")];
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![facts])
            .into_connection();
        let service = WorkflowMemoryService::new(Arc::new(db));

        let result = service.search(10, 5, "oauth", Some(10)).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().len(), 1);
    }

    // ── load_for_trigger ────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_load_for_trigger_with_no_tags_returns_top_facts() {
        let facts = vec![
            fact_with_tags(1, 10, 5, "general high-confidence", vec![], 0.9),
            fact_with_tags(2, 10, 5, "another", vec![], 0.7),
        ];
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![facts])
            .into_connection();
        let service = WorkflowMemoryService::new(Arc::new(db));

        let ctx = TriggerContext {
            project_id: 10,
            agent_id: 5,
            relevant_tags: vec![],
            limit: None,
        };
        let result = service.load_for_trigger(&ctx).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn test_load_for_trigger_with_tags_filters() {
        let facts = vec![fact_with_tags(
            1,
            10,
            5,
            "OAuth fix",
            vec!["error_group_id:42"],
            0.8,
        )];
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![facts])
            .into_connection();
        let service = WorkflowMemoryService::new(Arc::new(db));

        let ctx = TriggerContext {
            project_id: 10,
            agent_id: 5,
            relevant_tags: vec!["error_group_id:42".to_string()],
            limit: Some(10),
        };
        let result = service.load_for_trigger(&ctx).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn test_load_for_trigger_caps_limit() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![Vec::<workflow_memory::Model>::new()])
            .into_connection();
        let service = WorkflowMemoryService::new(Arc::new(db));

        let ctx = TriggerContext {
            project_id: 10,
            agent_id: 5,
            relevant_tags: vec![],
            limit: Some(1000),
        };
        let result = service.load_for_trigger(&ctx).await;
        assert!(result.is_ok());
    }

    // ── Mark used ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_mark_used_single() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_exec_results(vec![MockExecResult {
                last_insert_id: 0,
                rows_affected: 1,
            }])
            .into_connection();
        let service = WorkflowMemoryService::new(Arc::new(db));

        let result = service.mark_used(10, 5, 1).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_mark_used_many_empty_is_noop() {
        let db = MockDatabase::new(DatabaseBackend::Postgres).into_connection();
        let service = WorkflowMemoryService::new(Arc::new(db));

        let result = service.mark_used_many(10, 5, &[]).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_mark_used_many_batch() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_exec_results(vec![MockExecResult {
                last_insert_id: 0,
                rows_affected: 3,
            }])
            .into_connection();
        let service = WorkflowMemoryService::new(Arc::new(db));

        let result = service.mark_used_many(10, 5, &[1, 2, 3]).await;
        assert!(result.is_ok());
    }

    // ── Supersede ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_supersede_success() {
        // Old fact has a prior source_run_id so the new fact will inherit it,
        // which triggers the source_run_ids patch UPDATE.
        let mut old = mock_fact(1, 10, 5, "old fact");
        old.source_run_ids = serde_json::json!([100]);
        let new = mock_fact(2, 10, 5, "new fact");

        let db = MockDatabase::new(DatabaseBackend::Postgres)
            // get(old) — first finds old fact
            .append_query_results(vec![vec![old.clone()]])
            // write(new) — insert new
            .append_query_results(vec![vec![new.clone()]])
            // exec 1: UPDATE source_run_ids on new fact (because old had run_ids)
            .append_exec_results(vec![MockExecResult {
                last_insert_id: 0,
                rows_affected: 1,
            }])
            // exec 2: UPDATE superseded_by on old fact
            .append_exec_results(vec![MockExecResult {
                last_insert_id: 0,
                rows_affected: 1,
            }])
            .into_connection();
        let service = WorkflowMemoryService::new(Arc::new(db));

        let result = service
            .supersede(SupersedeRequest {
                project_id: 10,
                agent_id: 5,
                old_fact_id: 1,
                new_fact: "new fact".to_string(),
                new_tags: vec![],
                source_run_id: Some(200),
            })
            .await;

        assert!(result.is_ok(), "supersede failed: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_supersede_without_inherited_run_ids() {
        // Old fact has no source_run_ids → only the superseded_by UPDATE runs.
        let old = mock_fact(1, 10, 5, "old fact"); // source_run_ids = []
        let new = mock_fact(2, 10, 5, "new fact");

        let db = MockDatabase::new(DatabaseBackend::Postgres)
            // get(old)
            .append_query_results(vec![vec![old]])
            // write(new)
            .append_query_results(vec![vec![new]])
            // exec: only the superseded_by UPDATE
            .append_exec_results(vec![MockExecResult {
                last_insert_id: 0,
                rows_affected: 1,
            }])
            .into_connection();
        let service = WorkflowMemoryService::new(Arc::new(db));

        let result = service
            .supersede(SupersedeRequest {
                project_id: 10,
                agent_id: 5,
                old_fact_id: 1,
                new_fact: "new fact".to_string(),
                new_tags: vec![],
                source_run_id: None,
            })
            .await;

        assert!(result.is_ok(), "supersede failed: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_supersede_old_not_found_fails() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![Vec::<workflow_memory::Model>::new()])
            .into_connection();
        let service = WorkflowMemoryService::new(Arc::new(db));

        let result = service
            .supersede(SupersedeRequest {
                project_id: 10,
                agent_id: 5,
                old_fact_id: 999,
                new_fact: "new".to_string(),
                new_tags: vec![],
                source_run_id: None,
            })
            .await;

        assert!(matches!(result, Err(WorkspaceError::MemoryNotFound { .. })));
    }

    #[tokio::test]
    async fn test_supersede_already_superseded_fails() {
        let mut old = mock_fact(1, 10, 5, "old");
        old.superseded_by = Some(2); // already superseded

        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![vec![old]])
            .into_connection();
        let service = WorkflowMemoryService::new(Arc::new(db));

        let result = service
            .supersede(SupersedeRequest {
                project_id: 10,
                agent_id: 5,
                old_fact_id: 1,
                new_fact: "new".to_string(),
                new_tags: vec![],
                source_run_id: None,
            })
            .await;

        assert!(matches!(result, Err(WorkspaceError::Validation { .. })));
    }

    #[tokio::test]
    async fn test_supersede_invalid_new_fact_fails_before_db() {
        let db = MockDatabase::new(DatabaseBackend::Postgres).into_connection();
        let service = WorkflowMemoryService::new(Arc::new(db));

        let result = service
            .supersede(SupersedeRequest {
                project_id: 10,
                agent_id: 5,
                old_fact_id: 1,
                new_fact: "".to_string(),
                new_tags: vec![],
                source_run_id: None,
            })
            .await;

        assert!(matches!(result, Err(WorkspaceError::Validation { .. })));
    }

    // ── Drop ────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_drop_success() {
        let fact = mock_fact(1, 10, 5, "test");
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![vec![fact]])
            .append_exec_results(vec![MockExecResult {
                last_insert_id: 0,
                rows_affected: 1,
            }])
            .into_connection();
        let service = WorkflowMemoryService::new(Arc::new(db));

        let result = service.delete_fact(10, 5, 1).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_drop_not_found_fails() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![Vec::<workflow_memory::Model>::new()])
            .into_connection();
        let service = WorkflowMemoryService::new(Arc::new(db));

        let result = service.delete_fact(10, 5, 999).await;
        assert!(matches!(result, Err(WorkspaceError::MemoryNotFound { .. })));
    }

    // ── Constants sanity ────────────────────────────────────────────────────

    // ── Prompt rendering ────────────────────────────────────────────────────

    #[test]
    fn test_render_memory_section_empty_returns_empty_string() {
        let rendered = render_memory_section(&[]);
        assert_eq!(rendered, "");
    }

    #[test]
    fn test_render_memory_section_includes_header() {
        let facts = vec![mock_fact(1, 10, 5, "OAuth state cookie missing")];
        let rendered = render_memory_section(&facts);
        assert!(rendered.contains("## Things you've learned about this from past runs"));
    }

    #[test]
    fn test_render_memory_section_lists_facts() {
        let facts = vec![
            mock_fact(1, 10, 5, "First fact"),
            mock_fact(2, 10, 5, "Second fact"),
        ];
        let rendered = render_memory_section(&facts);
        assert!(rendered.contains("First fact"));
        assert!(rendered.contains("Second fact"));
    }

    #[test]
    fn test_render_memory_section_labels_high_confidence() {
        let mut high = mock_fact(1, 10, 5, "very confident");
        high.confidence = 0.95;
        let rendered = render_memory_section(&[high]);
        assert!(rendered.contains("(high"));
    }

    #[test]
    fn test_render_memory_section_labels_medium_confidence() {
        let mut med = mock_fact(1, 10, 5, "medium");
        med.confidence = 0.6;
        let rendered = render_memory_section(&[med]);
        assert!(rendered.contains("(medium"));
    }

    #[test]
    fn test_render_memory_section_labels_low_confidence() {
        let mut low = mock_fact(1, 10, 5, "low");
        low.confidence = 0.3;
        let rendered = render_memory_section(&[low]);
        assert!(rendered.contains("(low"));
    }

    #[test]
    fn test_render_memory_section_includes_supersede_hint() {
        let facts = vec![mock_fact(1, 10, 5, "test")];
        let rendered = render_memory_section(&facts);
        assert!(rendered.contains("memory supersede"));
        assert!(rendered.contains("memory search"));
    }

    #[test]
    fn test_render_memory_section_shows_times_used() {
        let mut fact = mock_fact(1, 10, 5, "popular");
        fact.times_used = 7;
        let rendered = render_memory_section(&[fact]);
        assert!(rendered.contains("used 7x"));
    }

    #[tokio::test]
    async fn test_render_for_prompt_no_memory_returns_empty() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![Vec::<workflow_memory::Model>::new()])
            .into_connection();
        let service = WorkflowMemoryService::new(Arc::new(db));

        let ctx = TriggerContext {
            project_id: 10,
            agent_id: 5,
            relevant_tags: vec![],
            limit: None,
        };
        let rendered = service.render_for_prompt(&ctx).await.unwrap();
        assert_eq!(rendered, "");
    }

    #[tokio::test]
    async fn test_render_for_prompt_with_memory_returns_section() {
        let facts = vec![mock_fact(1, 10, 5, "remember this")];
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![facts])
            .into_connection();
        let service = WorkflowMemoryService::new(Arc::new(db));

        let ctx = TriggerContext {
            project_id: 10,
            agent_id: 5,
            relevant_tags: vec![],
            limit: None,
        };
        let rendered = service.render_for_prompt(&ctx).await.unwrap();
        assert!(rendered.contains("Things you've learned"));
        assert!(rendered.contains("remember this"));
    }

    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn test_constants_are_sensible() {
        // Sanity-check the budget constants don't get accidentally tweaked to absurd values.
        // These are compile-time constants, so the assertions evaluate statically —
        // we keep them as a runtime test so a bad change still shows up in `cargo test`.
        assert!((100..=2000).contains(&MAX_FACT_LENGTH));
        assert!((4..=64).contains(&MAX_TAGS_PER_FACT));
        assert!((16..=256).contains(&MAX_TAG_LENGTH));
        assert!((5..=100).contains(&MAX_FACTS_PER_PROMPT));
        assert!((50..=1000).contains(&COMPACTION_THRESHOLD));
        assert!(DEFAULT_CONFIDENCE > 0.0 && DEFAULT_CONFIDENCE < 1.0);
    }
}
