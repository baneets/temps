use chrono::Utc;
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryOrder, Set};
use std::sync::Arc;
use temps_entities::compose_stacks;
use thiserror::Error;
use tracing::debug;

#[derive(Error, Debug)]
pub enum ComposeError {
    #[error("Database error: {0}")]
    Database(sea_orm::DbErr),

    #[error("Stack {stack_id} not found")]
    NotFound { stack_id: i32 },

    #[error("Validation error: {message}")]
    Validation { message: String },

    #[error("Docker Compose error for stack {stack_id}: {reason}")]
    Docker { stack_id: i32, reason: String },

    #[error("Stack '{name}' is currently {state}, cannot perform {operation}")]
    InvalidState {
        name: String,
        state: String,
        operation: String,
    },
}

impl From<sea_orm::DbErr> for ComposeError {
    fn from(error: sea_orm::DbErr) -> Self {
        match error {
            sea_orm::DbErr::RecordNotFound(msg) => ComposeError::NotFound {
                stack_id: msg.parse().unwrap_or(0),
            },
            sea_orm::DbErr::RecordNotInserted => ComposeError::Validation {
                message: format!("Duplicate record: {}", error),
            },
            _ => ComposeError::Database(error),
        }
    }
}

pub struct ComposeService {
    db: Arc<DatabaseConnection>,
}

impl ComposeService {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }

    pub async fn list(
        &self,
        page: Option<u64>,
        page_size: Option<u64>,
    ) -> Result<(Vec<compose_stacks::Model>, u64), ComposeError> {
        let page = page.unwrap_or(1);
        let page_size = std::cmp::min(page_size.unwrap_or(20), 100);
        let paginator = compose_stacks::Entity::find()
            .order_by_desc(compose_stacks::Column::CreatedAt)
            .paginate(self.db.as_ref(), page_size);
        let total = paginator.num_items().await?;
        let items = paginator.fetch_page(page - 1).await?;
        Ok((items, total))
    }

    pub async fn get(&self, id: i32) -> Result<compose_stacks::Model, ComposeError> {
        compose_stacks::Entity::find_by_id(id)
            .one(self.db.as_ref())
            .await?
            .ok_or(ComposeError::NotFound { stack_id: id })
    }

    pub async fn create(
        &self,
        name: String,
        description: Option<String>,
        compose_content: String,
        env_content: Option<String>,
        node_id: Option<i32>,
    ) -> Result<compose_stacks::Model, ComposeError> {
        if name.is_empty() {
            return Err(ComposeError::Validation {
                message: "Stack name cannot be empty".into(),
            });
        }

        if compose_content.is_empty() {
            return Err(ComposeError::Validation {
                message: "Compose content cannot be empty".into(),
            });
        }

        let now = Utc::now();
        let model = compose_stacks::ActiveModel {
            name: Set(name.clone()),
            description: Set(description),
            compose_content: Set(compose_content),
            env_content: Set(env_content),
            node_id: Set(node_id),
            state: Set("stopped".to_string()),
            created_at: Set(now),
            updated_at: Set(now),
            ..Default::default()
        }
        .insert(self.db.as_ref())
        .await?;

        debug!(
            "Created compose stack '{}' with id {}",
            model.name, model.id
        );
        Ok(model)
    }

    pub async fn update(
        &self,
        id: i32,
        name: Option<String>,
        description: Option<Option<String>>,
        compose_content: Option<String>,
        env_content: Option<Option<String>>,
    ) -> Result<compose_stacks::Model, ComposeError> {
        let stack = self.get(id).await?;
        let mut active: compose_stacks::ActiveModel = stack.into();

        if let Some(name) = name {
            if name.is_empty() {
                return Err(ComposeError::Validation {
                    message: "Stack name cannot be empty".into(),
                });
            }
            active.name = Set(name);
        }

        if let Some(desc) = description {
            active.description = Set(desc);
        }

        if let Some(content) = compose_content {
            if content.is_empty() {
                return Err(ComposeError::Validation {
                    message: "Compose content cannot be empty".into(),
                });
            }
            active.compose_content = Set(content);
        }

        if let Some(env) = env_content {
            active.env_content = Set(env);
        }

        active.updated_at = Set(Utc::now());

        let model = active.update(self.db.as_ref()).await?;
        debug!("Updated compose stack '{}' (id: {})", model.name, model.id);
        Ok(model)
    }

    pub async fn delete(&self, id: i32) -> Result<(), ComposeError> {
        let stack = self.get(id).await?;

        if stack.state == "running" {
            return Err(ComposeError::InvalidState {
                name: stack.name,
                state: stack.state,
                operation: "delete".to_string(),
            });
        }

        compose_stacks::Entity::delete_by_id(id)
            .exec(self.db.as_ref())
            .await?;

        debug!("Deleted compose stack id {}", id);
        Ok(())
    }

    pub async fn set_state(
        &self,
        id: i32,
        state: &str,
    ) -> Result<compose_stacks::Model, ComposeError> {
        let stack = self.get(id).await?;
        let mut active: compose_stacks::ActiveModel = stack.into();
        active.state = Set(state.to_string());
        active.updated_at = Set(Utc::now());
        let model = active.update(self.db.as_ref()).await?;
        Ok(model)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::{DatabaseBackend, MockDatabase, MockExecResult};

    fn mock_stack() -> compose_stacks::Model {
        compose_stacks::Model {
            id: 1,
            name: "my-stack".to_string(),
            description: Some("Test stack".to_string()),
            compose_content: "version: '3'\nservices:\n  web:\n    image: nginx".to_string(),
            env_content: None,
            node_id: None,
            state: "stopped".to_string(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn test_get_stack_success() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![vec![mock_stack()]])
            .into_connection();

        let service = ComposeService::new(Arc::new(db));
        let result = service.get(1).await;
        assert!(result.is_ok());
        let stack = result.unwrap();
        assert_eq!(stack.name, "my-stack");
        assert_eq!(stack.state, "stopped");
    }

    #[tokio::test]
    async fn test_get_stack_not_found() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![Vec::<compose_stacks::Model>::new()])
            .into_connection();

        let service = ComposeService::new(Arc::new(db));
        let result = service.get(999).await;
        assert!(matches!(
            result.unwrap_err(),
            ComposeError::NotFound { stack_id: 999 }
        ));
    }

    #[tokio::test]
    async fn test_create_stack_empty_name() {
        let db = MockDatabase::new(DatabaseBackend::Postgres).into_connection();
        let service = ComposeService::new(Arc::new(db));

        let result = service
            .create("".to_string(), None, "content".to_string(), None, None)
            .await;
        assert!(matches!(
            result.unwrap_err(),
            ComposeError::Validation { .. }
        ));
    }

    #[tokio::test]
    async fn test_create_stack_empty_content() {
        let db = MockDatabase::new(DatabaseBackend::Postgres).into_connection();
        let service = ComposeService::new(Arc::new(db));

        let result = service
            .create("test".to_string(), None, "".to_string(), None, None)
            .await;
        assert!(matches!(
            result.unwrap_err(),
            ComposeError::Validation { .. }
        ));
    }

    #[tokio::test]
    async fn test_delete_running_stack_fails() {
        let running_stack = compose_stacks::Model {
            state: "running".to_string(),
            ..mock_stack()
        };

        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![vec![running_stack]])
            .into_connection();

        let service = ComposeService::new(Arc::new(db));
        let result = service.delete(1).await;
        assert!(matches!(
            result.unwrap_err(),
            ComposeError::InvalidState { .. }
        ));
    }

    #[tokio::test]
    async fn test_create_stack_success() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![vec![mock_stack()]])
            .append_exec_results(vec![MockExecResult {
                last_insert_id: 1,
                rows_affected: 1,
            }])
            .into_connection();

        let service = ComposeService::new(Arc::new(db));
        let result = service
            .create(
                "my-stack".to_string(),
                Some("Test".to_string()),
                "version: '3'".to_string(),
                None,
                None,
            )
            .await;
        assert!(result.is_ok());
    }
}
