use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

use temps_entities::{project_mcp_definitions, project_skill_definitions};

use crate::error::AgentError;

// ── Skill Definitions ──

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct CreateSkillDefinitionRequest {
    pub slug: String,
    pub name: String,
    pub description: Option<String>,
    pub content: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct UpdateSkillDefinitionRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub content: Option<String>,
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

    // ── Skills ──

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
        if request.slug.is_empty() {
            return Err(AgentError::Validation {
                message: "Skill slug cannot be empty".into(),
            });
        }
        if request.content.is_empty() {
            return Err(AgentError::Validation {
                message: "Skill content cannot be empty".into(),
            });
        }

        let active = project_skill_definitions::ActiveModel {
            project_id: Set(project_id),
            slug: Set(request.slug),
            name: Set(request.name),
            description: Set(request.description),
            content: Set(request.content),
            ..Default::default()
        };
        let model = active.insert(self.db.as_ref()).await?;
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

        let model = active.update(self.db.as_ref()).await?;
        Ok(model)
    }

    pub async fn delete_skill(&self, project_id: i32, slug: &str) -> Result<(), AgentError> {
        let existing = self.get_skill(project_id, slug).await?;
        let active: project_skill_definitions::ActiveModel = existing.into();
        active.delete(self.db.as_ref()).await?;
        Ok(())
    }

    // ── MCP Servers ──

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
        if request.slug.is_empty() {
            return Err(AgentError::Validation {
                message: "MCP server slug cannot be empty".into(),
            });
        }

        let active = project_mcp_definitions::ActiveModel {
            project_id: Set(project_id),
            slug: Set(request.slug),
            name: Set(request.name),
            description: Set(request.description),
            config: Set(request.config),
            ..Default::default()
        };
        let model = active.insert(self.db.as_ref()).await?;
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
}
