//! Node management service — CRUD operations for the `nodes` table.

use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
    QueryOrder,
};
use std::sync::Arc;
use thiserror::Error;

use temps_entities::nodes;

#[derive(Error, Debug)]
pub enum NodeError {
    #[error("Node '{name}' not found")]
    NotFound { name: String },

    #[error("Node with id {node_id} not found")]
    NotFoundById { node_id: i32 },

    #[error("Node '{name}' already exists")]
    AlreadyExists { name: String },

    #[error("Invalid node configuration: {message}")]
    Validation { message: String },

    #[error("Database error: {0}")]
    Database(#[from] sea_orm::DbErr),
}

/// Request to register a new worker node.
#[derive(Debug, Clone)]
pub struct RegisterNodeRequest {
    pub name: String,
    pub token_hash: String,
    /// Encrypted plaintext token for control plane → agent auth
    pub token_encrypted: Option<String>,
    pub address: String,
    pub private_address: String,
    pub public_endpoint: Option<String>,
    pub wg_public_key: Option<String>,
    pub role: String,
    pub labels: serde_json::Value,
}

/// Request to update a node's heartbeat.
#[derive(Debug, Clone)]
pub struct HeartbeatRequest {
    pub capacity: serde_json::Value,
}

pub struct NodeService {
    db: Arc<DatabaseConnection>,
}

impl NodeService {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }

    /// Register a new node in the cluster.
    pub async fn register(&self, request: RegisterNodeRequest) -> Result<nodes::Model, NodeError> {
        if request.name.is_empty() {
            return Err(NodeError::Validation {
                message: "Node name cannot be empty".into(),
            });
        }

        if request.address.is_empty() {
            return Err(NodeError::Validation {
                message: "Node address cannot be empty".into(),
            });
        }

        // Check for existing node with the same name
        let existing = nodes::Entity::find()
            .filter(nodes::Column::Name.eq(&request.name))
            .one(self.db.as_ref())
            .await?;

        if let Some(existing_node) = existing {
            // Reconnection: update the existing node and set it back to active
            let mut active: nodes::ActiveModel = existing_node.into();
            active.token_hash = Set(request.token_hash);
            active.token_encrypted = Set(request.token_encrypted.clone());
            active.address = Set(request.address);
            active.private_address = Set(request.private_address);
            active.public_endpoint = Set(request.public_endpoint);
            active.wg_public_key = Set(request.wg_public_key);
            active.labels = Set(request.labels);
            active.status = Set("active".to_string());
            active.last_heartbeat = Set(Some(chrono::Utc::now()));

            let node = active.update(self.db.as_ref()).await?;

            tracing::info!(
                node_id = node.id,
                node_name = %node.name,
                address = %node.address,
                "Node reconnected"
            );

            return Ok(node);
        }

        let model = nodes::ActiveModel {
            name: Set(request.name),
            token_hash: Set(request.token_hash),
            token_encrypted: Set(request.token_encrypted),
            address: Set(request.address),
            private_address: Set(request.private_address),
            public_endpoint: Set(request.public_endpoint),
            wg_public_key: Set(request.wg_public_key),
            role: Set(request.role),
            status: Set("active".to_string()),
            labels: Set(request.labels),
            capacity: Set(serde_json::json!({})),
            last_heartbeat: Set(Some(chrono::Utc::now())),
            ..Default::default()
        };

        let node = model.insert(self.db.as_ref()).await?;

        tracing::info!(
            node_id = node.id,
            node_name = %node.name,
            address = %node.address,
            private_address = %node.private_address,
            "Node registered"
        );

        Ok(node)
    }

    /// Update a node's heartbeat timestamp and capacity metrics.
    pub async fn heartbeat(
        &self,
        node_id: i32,
        request: HeartbeatRequest,
    ) -> Result<(), NodeError> {
        let node = nodes::Entity::find_by_id(node_id)
            .one(self.db.as_ref())
            .await?
            .ok_or(NodeError::NotFoundById { node_id })?;

        let mut active: nodes::ActiveModel = node.into();
        active.last_heartbeat = Set(Some(chrono::Utc::now()));
        active.capacity = Set(request.capacity);
        active.status = Set("active".to_string());
        active.update(self.db.as_ref()).await?;

        Ok(())
    }

    /// Get a node by its ID.
    pub async fn get_by_id(&self, node_id: i32) -> Result<nodes::Model, NodeError> {
        nodes::Entity::find_by_id(node_id)
            .one(self.db.as_ref())
            .await?
            .ok_or(NodeError::NotFoundById { node_id })
    }

    /// Get a node by its name.
    pub async fn get_by_name(&self, name: &str) -> Result<nodes::Model, NodeError> {
        nodes::Entity::find()
            .filter(nodes::Column::Name.eq(name))
            .one(self.db.as_ref())
            .await?
            .ok_or(NodeError::NotFound {
                name: name.to_string(),
            })
    }

    /// List all nodes, ordered by name.
    pub async fn list_all(&self) -> Result<Vec<nodes::Model>, NodeError> {
        let nodes = nodes::Entity::find()
            .order_by_asc(nodes::Column::Name)
            .all(self.db.as_ref())
            .await?;
        Ok(nodes)
    }

    /// List only active nodes (heartbeat within the threshold).
    pub async fn list_active(
        &self,
        heartbeat_threshold_secs: i64,
    ) -> Result<Vec<nodes::Model>, NodeError> {
        let cutoff = chrono::Utc::now() - chrono::Duration::seconds(heartbeat_threshold_secs);

        let nodes = nodes::Entity::find()
            .filter(nodes::Column::Status.eq("active"))
            .filter(nodes::Column::LastHeartbeat.gte(cutoff))
            .order_by_asc(nodes::Column::Name)
            .all(self.db.as_ref())
            .await?;

        Ok(nodes)
    }

    /// Mark a node as offline.
    pub async fn mark_offline(&self, node_id: i32) -> Result<(), NodeError> {
        let node = nodes::Entity::find_by_id(node_id)
            .one(self.db.as_ref())
            .await?
            .ok_or(NodeError::NotFoundById { node_id })?;

        let mut active: nodes::ActiveModel = node.into();
        active.status = Set("offline".to_string());
        active.update(self.db.as_ref()).await?;

        tracing::warn!(node_id = node_id, "Node marked as offline");

        Ok(())
    }

    /// Mark a node as draining (no new deployments, existing continue).
    pub async fn mark_draining(&self, node_id: i32) -> Result<(), NodeError> {
        let node = nodes::Entity::find_by_id(node_id)
            .one(self.db.as_ref())
            .await?
            .ok_or(NodeError::NotFoundById { node_id })?;

        let mut active: nodes::ActiveModel = node.into();
        active.status = Set("draining".to_string());
        active.update(self.db.as_ref()).await?;

        tracing::info!(node_id = node_id, "Node marked as draining");

        Ok(())
    }

    /// Remove a node from the cluster.
    pub async fn remove(&self, node_id: i32) -> Result<(), NodeError> {
        let result = nodes::Entity::delete_by_id(node_id)
            .exec(self.db.as_ref())
            .await?;

        if result.rows_affected == 0 {
            return Err(NodeError::NotFoundById { node_id });
        }

        tracing::info!(node_id = node_id, "Node removed from cluster");

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::{DatabaseBackend, MockDatabase};

    fn sample_node() -> nodes::Model {
        nodes::Model {
            id: 1,
            name: "worker-1".to_string(),
            token_hash: "hash123".to_string(),
            token_encrypted: None,
            address: "https://10.100.0.2:3100".to_string(),
            private_address: "10.100.0.2".to_string(),
            public_endpoint: Some("203.0.113.50:51820".to_string()),
            wg_public_key: Some("pubkey123".to_string()),
            role: "worker".to_string(),
            status: "active".to_string(),
            labels: serde_json::json!({}),
            capacity: serde_json::json!({}),
            last_heartbeat: Some(chrono::Utc::now()),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    #[tokio::test]
    async fn test_register_validates_empty_name() {
        let db = MockDatabase::new(DatabaseBackend::Postgres).into_connection();
        let service = NodeService::new(Arc::new(db));

        let result = service
            .register(RegisterNodeRequest {
                name: "".to_string(),
                token_hash: "hash".to_string(),
                token_encrypted: None,
                address: "https://10.100.0.2:3100".to_string(),
                private_address: "10.100.0.2".to_string(),
                public_endpoint: None,
                wg_public_key: None,
                role: "worker".to_string(),
                labels: serde_json::json!({}),
            })
            .await;

        assert!(matches!(result.unwrap_err(), NodeError::Validation { .. }));
    }

    #[tokio::test]
    async fn test_register_validates_empty_address() {
        let db = MockDatabase::new(DatabaseBackend::Postgres).into_connection();
        let service = NodeService::new(Arc::new(db));

        let result = service
            .register(RegisterNodeRequest {
                name: "worker-1".to_string(),
                token_hash: "hash".to_string(),
                token_encrypted: None,
                address: "".to_string(),
                private_address: "10.100.0.2".to_string(),
                public_endpoint: None,
                wg_public_key: None,
                role: "worker".to_string(),
                labels: serde_json::json!({}),
            })
            .await;

        assert!(matches!(result.unwrap_err(), NodeError::Validation { .. }));
    }

    #[tokio::test]
    async fn test_register_reconnects_existing_node() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            // First query: find existing node by name
            .append_query_results(vec![vec![sample_node()]])
            // Second query: update returns the updated node
            .append_query_results(vec![vec![sample_node()]])
            .into_connection();
        let service = NodeService::new(Arc::new(db));

        let result = service
            .register(RegisterNodeRequest {
                name: "worker-1".to_string(),
                token_hash: "new-hash".to_string(),
                token_encrypted: None,
                address: "https://10.100.0.3:3100".to_string(),
                private_address: "10.100.0.3".to_string(),
                public_endpoint: None,
                wg_public_key: None,
                role: "worker".to_string(),
                labels: serde_json::json!({}),
            })
            .await;

        assert!(result.is_ok());
        let node = result.unwrap();
        assert_eq!(node.name, "worker-1");
    }

    #[tokio::test]
    async fn test_list_all() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![vec![sample_node()]])
            .into_connection();
        let service = NodeService::new(Arc::new(db));

        let nodes = service.list_all().await.unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].name, "worker-1");
    }

    #[tokio::test]
    async fn test_get_by_id_not_found() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![Vec::<nodes::Model>::new()])
            .into_connection();
        let service = NodeService::new(Arc::new(db));

        let result = service.get_by_id(999).await;
        assert!(matches!(
            result.unwrap_err(),
            NodeError::NotFoundById { node_id: 999 }
        ));
    }
}
