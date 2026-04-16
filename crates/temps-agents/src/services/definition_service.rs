use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, DatabaseConnection, EntityTrait, QueryFilter,
    QueryOrder, Set,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

use temps_entities::{project_mcp_definitions, project_skill_definitions};

use crate::error::AgentError;

/// Validate a slug used as a URL segment AND as a filesystem path component
/// (skills are extracted to `/workspace/.claude/skills/<slug>/`, MCP archives
/// will land in `/home/temps/.mcp/<slug>/`). Must reject `..`, `/`, and
/// other characters that could escape the intended directory.
fn validate_slug(slug: &str, kind: &str) -> Result<(), AgentError> {
    if slug.is_empty() {
        return Err(AgentError::Validation {
            message: format!("{kind} slug cannot be empty"),
        });
    }
    if slug.len() > 63 {
        return Err(AgentError::Validation {
            message: format!("{kind} slug must be 63 characters or fewer"),
        });
    }
    let first = slug.as_bytes()[0];
    if !(first.is_ascii_lowercase() || first.is_ascii_digit()) {
        return Err(AgentError::Validation {
            message: format!("{kind} slug must start with a lowercase letter or digit"),
        });
    }
    if !slug
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err(AgentError::Validation {
            message: format!(
                "{kind} slug must contain only lowercase letters, digits, and hyphens"
            ),
        });
    }
    Ok(())
}

// ── Skill Definitions ──

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct CreateSkillDefinitionRequest {
    pub slug: String,
    pub name: String,
    pub description: Option<String>,
    pub content: String,
    /// Tar.gz archive of the skill directory (SKILL.md + supporting files).
    #[serde(skip)]
    #[schema(value_type = Option<String>, format = "binary")]
    pub archive: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct UpdateSkillDefinitionRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub content: Option<String>,
    /// Tar.gz archive of the skill directory.
    #[serde(skip)]
    #[schema(value_type = Option<String>, format = "binary")]
    pub archive: Option<Vec<u8>>,
}

// ── MCP Definitions ──

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct CreateMcpDefinitionRequest {
    pub slug: String,
    pub name: String,
    pub description: Option<String>,
    /// MCP server config in Claude Code settings.json format.
    /// e.g. { "command": "npx", "args": ["-y", "@agentbrowser/mcp"] }
    pub config: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct UpdateMcpDefinitionRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub config: Option<serde_json::Value>,
}

pub struct DefinitionService {
    db: Arc<DatabaseConnection>,
}

impl DefinitionService {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }

    // ── Skills (project-scoped) ──

    pub async fn list_skills(
        &self,
        project_id: i32,
    ) -> Result<Vec<project_skill_definitions::Model>, AgentError> {
        let items = project_skill_definitions::Entity::find()
            .filter(project_skill_definitions::Column::ProjectId.eq(project_id))
            .order_by_asc(project_skill_definitions::Column::Name)
            .all(self.db.as_ref())
            .await?;
        Ok(items)
    }

    pub async fn get_skill(
        &self,
        project_id: i32,
        slug: &str,
    ) -> Result<project_skill_definitions::Model, AgentError> {
        project_skill_definitions::Entity::find()
            .filter(project_skill_definitions::Column::ProjectId.eq(project_id))
            .filter(project_skill_definitions::Column::Slug.eq(slug))
            .one(self.db.as_ref())
            .await?
            .ok_or(AgentError::SkillDefinitionNotFound {
                project_id,
                slug: slug.to_string(),
            })
    }

    pub async fn get_skills_by_slugs(
        &self,
        project_id: i32,
        slugs: &[String],
    ) -> Result<Vec<project_skill_definitions::Model>, AgentError> {
        if slugs.is_empty() {
            return Ok(vec![]);
        }
        let items = project_skill_definitions::Entity::find()
            .filter(project_skill_definitions::Column::ProjectId.eq(project_id))
            .filter(project_skill_definitions::Column::Slug.is_in(slugs.iter().map(|s| s.as_str())))
            .all(self.db.as_ref())
            .await?;
        Ok(items)
    }

    pub async fn create_skill(
        &self,
        project_id: i32,
        request: CreateSkillDefinitionRequest,
    ) -> Result<project_skill_definitions::Model, AgentError> {
        validate_slug(&request.slug, "Skill")?;
        if request.content.is_empty() {
            return Err(AgentError::Validation {
                message: "Skill content cannot be empty".into(),
            });
        }

        // Pre-check existence so we return a typed 409 instead of a 500 that
        // leaks the PG unique-constraint name. The `From<DbErr>` fallback
        // still catches the race where two inserts arrive concurrently.
        if project_skill_definitions::Entity::find()
            .filter(project_skill_definitions::Column::ProjectId.eq(project_id))
            .filter(project_skill_definitions::Column::Slug.eq(&request.slug))
            .one(self.db.as_ref())
            .await?
            .is_some()
        {
            return Err(AgentError::SkillDefinitionAlreadyExists {
                project_id: Some(project_id),
                slug: request.slug,
            });
        }

        let active = project_skill_definitions::ActiveModel {
            project_id: Set(Some(project_id)),
            slug: Set(request.slug.clone()),
            name: Set(request.name),
            description: Set(request.description),
            content: Set(request.content),
            archive: Set(request.archive),
            ..Default::default()
        };
        let model = active
            .insert(self.db.as_ref())
            .await
            .map_err(|e| fold_skill_dup(e, Some(project_id), &request.slug))?;
        Ok(model)
    }

    pub async fn update_skill(
        &self,
        project_id: i32,
        slug: &str,
        request: UpdateSkillDefinitionRequest,
    ) -> Result<project_skill_definitions::Model, AgentError> {
        let existing = self.get_skill(project_id, slug).await?;
        let mut active: project_skill_definitions::ActiveModel = existing.into();

        if let Some(name) = request.name {
            active.name = Set(name);
        }
        if let Some(description) = request.description {
            active.description = Set(Some(description));
        }
        if let Some(content) = request.content {
            active.content = Set(content);
        }
        if request.archive.is_some() {
            active.archive = Set(request.archive);
        }

        let model = active.update(self.db.as_ref()).await?;
        Ok(model)
    }

    pub async fn delete_skill(&self, project_id: i32, slug: &str) -> Result<(), AgentError> {
        let existing = self.get_skill(project_id, slug).await?;
        let active: project_skill_definitions::ActiveModel = existing.into();
        active.delete(self.db.as_ref()).await?;
        Ok(())
    }

    // ── Skills (global — project_id IS NULL) ──

    pub async fn list_global_skills(
        &self,
    ) -> Result<Vec<project_skill_definitions::Model>, AgentError> {
        let items = project_skill_definitions::Entity::find()
            .filter(project_skill_definitions::Column::ProjectId.is_null())
            .order_by_asc(project_skill_definitions::Column::Name)
            .all(self.db.as_ref())
            .await?;
        Ok(items)
    }

    pub async fn get_global_skill(
        &self,
        slug: &str,
    ) -> Result<project_skill_definitions::Model, AgentError> {
        project_skill_definitions::Entity::find()
            .filter(project_skill_definitions::Column::ProjectId.is_null())
            .filter(project_skill_definitions::Column::Slug.eq(slug))
            .one(self.db.as_ref())
            .await?
            .ok_or(AgentError::SkillDefinitionNotFound {
                project_id: 0,
                slug: slug.to_string(),
            })
    }

    pub async fn create_global_skill(
        &self,
        request: CreateSkillDefinitionRequest,
    ) -> Result<project_skill_definitions::Model, AgentError> {
        validate_slug(&request.slug, "Skill")?;
        if request.content.is_empty() {
            return Err(AgentError::Validation {
                message: "Skill content cannot be empty".into(),
            });
        }

        if project_skill_definitions::Entity::find()
            .filter(project_skill_definitions::Column::ProjectId.is_null())
            .filter(project_skill_definitions::Column::Slug.eq(&request.slug))
            .one(self.db.as_ref())
            .await?
            .is_some()
        {
            return Err(AgentError::SkillDefinitionAlreadyExists {
                project_id: None,
                slug: request.slug,
            });
        }

        let active = project_skill_definitions::ActiveModel {
            project_id: Set(None),
            slug: Set(request.slug.clone()),
            name: Set(request.name),
            description: Set(request.description),
            content: Set(request.content),
            archive: Set(request.archive),
            ..Default::default()
        };
        let model = active
            .insert(self.db.as_ref())
            .await
            .map_err(|e| fold_skill_dup(e, None, &request.slug))?;
        Ok(model)
    }

    pub async fn update_global_skill(
        &self,
        slug: &str,
        request: UpdateSkillDefinitionRequest,
    ) -> Result<project_skill_definitions::Model, AgentError> {
        let existing = self.get_global_skill(slug).await?;
        let mut active: project_skill_definitions::ActiveModel = existing.into();

        if let Some(name) = request.name {
            active.name = Set(name);
        }
        if let Some(description) = request.description {
            active.description = Set(Some(description));
        }
        if let Some(content) = request.content {
            active.content = Set(content);
        }
        if request.archive.is_some() {
            active.archive = Set(request.archive);
        }

        let model = active.update(self.db.as_ref()).await?;
        Ok(model)
    }

    pub async fn delete_global_skill(&self, slug: &str) -> Result<(), AgentError> {
        let existing = self.get_global_skill(slug).await?;
        let active: project_skill_definitions::ActiveModel = existing.into();
        active.delete(self.db.as_ref()).await?;
        Ok(())
    }

    /// Get all skills available to a project: project-scoped + global definitions.
    pub async fn get_all_available_skills(
        &self,
        project_id: i32,
        slugs: &[String],
    ) -> Result<Vec<project_skill_definitions::Model>, AgentError> {
        if slugs.is_empty() {
            return Ok(vec![]);
        }
        let items = project_skill_definitions::Entity::find()
            .filter(
                Condition::any()
                    .add(project_skill_definitions::Column::ProjectId.eq(project_id))
                    .add(project_skill_definitions::Column::ProjectId.is_null()),
            )
            .filter(project_skill_definitions::Column::Slug.is_in(slugs.iter().map(|s| s.as_str())))
            .all(self.db.as_ref())
            .await?;
        Ok(items)
    }

    // ── MCP Servers (project-scoped) ──

    pub async fn list_mcps(
        &self,
        project_id: i32,
    ) -> Result<Vec<project_mcp_definitions::Model>, AgentError> {
        let items = project_mcp_definitions::Entity::find()
            .filter(project_mcp_definitions::Column::ProjectId.eq(project_id))
            .order_by_asc(project_mcp_definitions::Column::Name)
            .all(self.db.as_ref())
            .await?;
        Ok(items)
    }

    pub async fn get_mcp(
        &self,
        project_id: i32,
        slug: &str,
    ) -> Result<project_mcp_definitions::Model, AgentError> {
        project_mcp_definitions::Entity::find()
            .filter(project_mcp_definitions::Column::ProjectId.eq(project_id))
            .filter(project_mcp_definitions::Column::Slug.eq(slug))
            .one(self.db.as_ref())
            .await?
            .ok_or(AgentError::McpDefinitionNotFound {
                project_id,
                slug: slug.to_string(),
            })
    }

    pub async fn get_mcps_by_slugs(
        &self,
        project_id: i32,
        slugs: &[String],
    ) -> Result<Vec<project_mcp_definitions::Model>, AgentError> {
        if slugs.is_empty() {
            return Ok(vec![]);
        }
        let items = project_mcp_definitions::Entity::find()
            .filter(project_mcp_definitions::Column::ProjectId.eq(project_id))
            .filter(project_mcp_definitions::Column::Slug.is_in(slugs.iter().map(|s| s.as_str())))
            .all(self.db.as_ref())
            .await?;
        Ok(items)
    }

    pub async fn create_mcp(
        &self,
        project_id: i32,
        request: CreateMcpDefinitionRequest,
    ) -> Result<project_mcp_definitions::Model, AgentError> {
        validate_slug(&request.slug, "MCP server")?;

        if project_mcp_definitions::Entity::find()
            .filter(project_mcp_definitions::Column::ProjectId.eq(project_id))
            .filter(project_mcp_definitions::Column::Slug.eq(&request.slug))
            .one(self.db.as_ref())
            .await?
            .is_some()
        {
            return Err(AgentError::McpDefinitionAlreadyExists {
                project_id: Some(project_id),
                slug: request.slug,
            });
        }

        let active = project_mcp_definitions::ActiveModel {
            project_id: Set(Some(project_id)),
            slug: Set(request.slug.clone()),
            name: Set(request.name),
            description: Set(request.description),
            config: Set(request.config),
            ..Default::default()
        };
        let model = active
            .insert(self.db.as_ref())
            .await
            .map_err(|e| fold_mcp_dup(e, Some(project_id), &request.slug))?;
        Ok(model)
    }

    pub async fn update_mcp(
        &self,
        project_id: i32,
        slug: &str,
        request: UpdateMcpDefinitionRequest,
    ) -> Result<project_mcp_definitions::Model, AgentError> {
        let existing = self.get_mcp(project_id, slug).await?;
        let mut active: project_mcp_definitions::ActiveModel = existing.into();

        if let Some(name) = request.name {
            active.name = Set(name);
        }
        if let Some(description) = request.description {
            active.description = Set(Some(description));
        }
        if let Some(config) = request.config {
            active.config = Set(config);
        }

        let model = active.update(self.db.as_ref()).await?;
        Ok(model)
    }

    pub async fn delete_mcp(&self, project_id: i32, slug: &str) -> Result<(), AgentError> {
        let existing = self.get_mcp(project_id, slug).await?;
        let active: project_mcp_definitions::ActiveModel = existing.into();
        active.delete(self.db.as_ref()).await?;
        Ok(())
    }

    // ── MCP Servers (global — project_id IS NULL) ──

    pub async fn list_global_mcps(
        &self,
    ) -> Result<Vec<project_mcp_definitions::Model>, AgentError> {
        let items = project_mcp_definitions::Entity::find()
            .filter(project_mcp_definitions::Column::ProjectId.is_null())
            .order_by_asc(project_mcp_definitions::Column::Name)
            .all(self.db.as_ref())
            .await?;
        Ok(items)
    }

    pub async fn get_global_mcp(
        &self,
        slug: &str,
    ) -> Result<project_mcp_definitions::Model, AgentError> {
        project_mcp_definitions::Entity::find()
            .filter(project_mcp_definitions::Column::ProjectId.is_null())
            .filter(project_mcp_definitions::Column::Slug.eq(slug))
            .one(self.db.as_ref())
            .await?
            .ok_or(AgentError::McpDefinitionNotFound {
                project_id: 0,
                slug: slug.to_string(),
            })
    }

    pub async fn create_global_mcp(
        &self,
        request: CreateMcpDefinitionRequest,
    ) -> Result<project_mcp_definitions::Model, AgentError> {
        validate_slug(&request.slug, "MCP server")?;

        if project_mcp_definitions::Entity::find()
            .filter(project_mcp_definitions::Column::ProjectId.is_null())
            .filter(project_mcp_definitions::Column::Slug.eq(&request.slug))
            .one(self.db.as_ref())
            .await?
            .is_some()
        {
            return Err(AgentError::McpDefinitionAlreadyExists {
                project_id: None,
                slug: request.slug,
            });
        }

        let active = project_mcp_definitions::ActiveModel {
            project_id: Set(None),
            slug: Set(request.slug.clone()),
            name: Set(request.name),
            description: Set(request.description),
            config: Set(request.config),
            ..Default::default()
        };
        let model = active
            .insert(self.db.as_ref())
            .await
            .map_err(|e| fold_mcp_dup(e, None, &request.slug))?;
        Ok(model)
    }

    pub async fn update_global_mcp(
        &self,
        slug: &str,
        request: UpdateMcpDefinitionRequest,
    ) -> Result<project_mcp_definitions::Model, AgentError> {
        let existing = self.get_global_mcp(slug).await?;
        let mut active: project_mcp_definitions::ActiveModel = existing.into();

        if let Some(name) = request.name {
            active.name = Set(name);
        }
        if let Some(description) = request.description {
            active.description = Set(Some(description));
        }
        if let Some(config) = request.config {
            active.config = Set(config);
        }

        let model = active.update(self.db.as_ref()).await?;
        Ok(model)
    }

    pub async fn delete_global_mcp(&self, slug: &str) -> Result<(), AgentError> {
        let existing = self.get_global_mcp(slug).await?;
        let active: project_mcp_definitions::ActiveModel = existing.into();
        active.delete(self.db.as_ref()).await?;
        Ok(())
    }

    /// Get all MCP servers available to a project: project-scoped + global definitions.
    pub async fn get_all_available_mcps(
        &self,
        project_id: i32,
        slugs: &[String],
    ) -> Result<Vec<project_mcp_definitions::Model>, AgentError> {
        if slugs.is_empty() {
            return Ok(vec![]);
        }
        let items = project_mcp_definitions::Entity::find()
            .filter(
                Condition::any()
                    .add(project_mcp_definitions::Column::ProjectId.eq(project_id))
                    .add(project_mcp_definitions::Column::ProjectId.is_null()),
            )
            .filter(project_mcp_definitions::Column::Slug.is_in(slugs.iter().map(|s| s.as_str())))
            .all(self.db.as_ref())
            .await?;
        Ok(items)
    }
}

/// Map a Sea-ORM insert error to a typed `AlreadyExists` variant when the
/// underlying cause is a Postgres unique-constraint violation. The generic
/// `From<DbErr>` impl already detects this but can't attach the authoritative
/// project_id/slug — do that here at the call site and fall through to the
/// default conversion for anything else.
fn fold_skill_dup(err: sea_orm::DbErr, project_id: Option<i32>, slug: &str) -> AgentError {
    if err
        .to_string()
        .contains("duplicate key value violates unique constraint")
    {
        return AgentError::SkillDefinitionAlreadyExists {
            project_id,
            slug: slug.to_string(),
        };
    }
    AgentError::from(err)
}

fn fold_mcp_dup(err: sea_orm::DbErr, project_id: Option<i32>, slug: &str) -> AgentError {
    if err
        .to_string()
        .contains("duplicate key value violates unique constraint")
    {
        return AgentError::McpDefinitionAlreadyExists {
            project_id,
            slug: slug.to_string(),
        };
    }
    AgentError::from(err)
}
