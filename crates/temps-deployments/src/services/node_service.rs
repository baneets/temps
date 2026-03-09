//! Node management service — CRUD operations for the `nodes` table.

use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
    QueryOrder,
};
use std::sync::Arc;
use thiserror::Error;

use temps_entities::{deployment_containers, deployments, nodes};

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
    /// Updated labels from the agent (allows runtime label changes without re-registration).
    pub labels: Option<serde_json::Value>,
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
        if let Some(labels) = request.labels {
            active.labels = Set(labels);
        }
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

    /// Check if a draining node has completed its drain (no remaining containers).
    /// If complete, transitions the node status from "draining" to "drained".
    /// Returns `true` if the drain is now complete.
    pub async fn check_drain_complete(&self, node_id: i32) -> Result<bool, NodeError> {
        let node = nodes::Entity::find_by_id(node_id)
            .one(self.db.as_ref())
            .await?
            .ok_or(NodeError::NotFoundById { node_id })?;

        if node.status != "draining" {
            return Ok(node.status == "drained");
        }

        let containers = self.list_containers_for_node(node_id).await?;

        if containers.is_empty() {
            let mut active: nodes::ActiveModel = node.into();
            active.status = Set("drained".to_string());
            active.update(self.db.as_ref()).await?;

            tracing::info!(
                node_id = node_id,
                "Node drain complete — all containers migrated, status set to drained"
            );
            return Ok(true);
        }

        tracing::debug!(
            node_id = node_id,
            remaining_containers = containers.len(),
            "Node drain in progress"
        );
        Ok(false)
    }

    /// Check all draining nodes for drain completion.
    /// Returns the list of node IDs that transitioned to "drained".
    pub async fn check_all_drains(&self) -> Result<Vec<i32>, NodeError> {
        let draining_nodes = nodes::Entity::find()
            .filter(nodes::Column::Status.eq("draining"))
            .all(self.db.as_ref())
            .await?;

        let mut completed = Vec::new();
        for node in draining_nodes {
            match self.check_drain_complete(node.id).await {
                Ok(true) => completed.push(node.id),
                Ok(false) => {}
                Err(e) => {
                    tracing::error!(node_id = node.id, "Failed to check drain completion: {}", e);
                }
            }
        }

        if !completed.is_empty() {
            tracing::info!(
                count = completed.len(),
                "Drain check: {} node(s) completed drain",
                completed.len()
            );
        }

        Ok(completed)
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

    /// List active (non-deleted) containers running on a specific node.
    pub async fn list_containers_for_node(
        &self,
        node_id: i32,
    ) -> Result<Vec<deployment_containers::Model>, NodeError> {
        let containers = deployment_containers::Entity::find()
            .filter(deployment_containers::Column::NodeId.eq(node_id))
            .filter(deployment_containers::Column::DeletedAt.is_null())
            .all(self.db.as_ref())
            .await?;
        Ok(containers)
    }

    /// Get the set of unique (project_id, environment_id) pairs affected by
    /// containers running on the given node. Each pair represents an environment
    /// that has at least one live container on this node and may need redeployment.
    pub async fn affected_environments(&self, node_id: i32) -> Result<Vec<(i32, i32)>, NodeError> {
        let containers = self.list_containers_for_node(node_id).await?;

        // Gather unique deployment IDs
        let deployment_ids: Vec<i32> = containers
            .iter()
            .map(|c| c.deployment_id)
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();

        if deployment_ids.is_empty() {
            return Ok(vec![]);
        }

        let deploys = deployments::Entity::find()
            .filter(deployments::Column::Id.is_in(deployment_ids))
            .all(self.db.as_ref())
            .await?;

        let pairs: Vec<(i32, i32)> = deploys
            .iter()
            .map(|d| (d.project_id, d.environment_id))
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();

        Ok(pairs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::{DatabaseBackend, MockDatabase};
    use temps_entities::deployments;

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

    fn sample_container(id: i32, deployment_id: i32, node_id: i32) -> deployment_containers::Model {
        deployment_containers::Model {
            id,
            deployment_id,
            container_id: format!("container-{}", id),
            container_name: format!("app-{}", id),
            container_port: 8080,
            host_port: Some(30000 + id),
            image_name: Some("myapp:latest".to_string()),
            status: Some("running".to_string()),
            created_at: chrono::Utc::now(),
            deployed_at: chrono::Utc::now(),
            ready_at: Some(chrono::Utc::now()),
            deleted_at: None,
            node_id: Some(node_id),
        }
    }

    fn sample_deployment(id: i32, project_id: i32, environment_id: i32) -> deployments::Model {
        deployments::Model {
            id,
            project_id,
            environment_id,
            slug: format!("deploy-{}", id),
            state: "deployed".to_string(),
            metadata: None,
            deploying_at: None,
            ready_at: None,
            started_at: Some(chrono::Utc::now()),
            finished_at: Some(chrono::Utc::now()),
            context_vars: None,
            branch_ref: Some("main".to_string()),
            tag_ref: None,
            commit_sha: None,
            commit_message: None,
            commit_author: None,
            commit_json: None,
            cancelled_reason: None,
            static_dir_location: None,
            screenshot_location: None,
            image_name: None,
            deployment_config: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    #[tokio::test]
    async fn test_list_containers_for_node_returns_active_containers() {
        let c1 = sample_container(1, 10, 5);
        let c2 = sample_container(2, 11, 5);

        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![vec![c1.clone(), c2.clone()]])
            .into_connection();
        let service = NodeService::new(Arc::new(db));

        let containers = service.list_containers_for_node(5).await.unwrap();
        assert_eq!(containers.len(), 2);
        assert_eq!(containers[0].container_id, "container-1");
        assert_eq!(containers[1].container_id, "container-2");
    }

    #[tokio::test]
    async fn test_list_containers_for_node_empty() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![Vec::<deployment_containers::Model>::new()])
            .into_connection();
        let service = NodeService::new(Arc::new(db));

        let containers = service.list_containers_for_node(99).await.unwrap();
        assert!(containers.is_empty());
    }

    #[tokio::test]
    async fn test_affected_environments_returns_unique_pairs() {
        // Two containers on the same deployment, one on a different deployment
        let c1 = sample_container(1, 10, 5);
        let c2 = sample_container(2, 10, 5); // same deployment
        let c3 = sample_container(3, 20, 5); // different deployment

        let d1 = sample_deployment(10, 100, 200);
        let d2 = sample_deployment(20, 100, 201);

        let db = MockDatabase::new(DatabaseBackend::Postgres)
            // list_containers_for_node
            .append_query_results(vec![vec![c1, c2, c3]])
            // deployments query
            .append_query_results(vec![vec![d1, d2]])
            .into_connection();
        let service = NodeService::new(Arc::new(db));

        let affected = service.affected_environments(5).await.unwrap();
        // Should have 2 unique (project_id, environment_id) pairs
        assert_eq!(affected.len(), 2);
        assert!(affected.contains(&(100, 200)));
        assert!(affected.contains(&(100, 201)));
    }

    #[tokio::test]
    async fn test_affected_environments_empty_when_no_containers() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![Vec::<deployment_containers::Model>::new()])
            .into_connection();
        let service = NodeService::new(Arc::new(db));

        let affected = service.affected_environments(99).await.unwrap();
        assert!(affected.is_empty());
    }

    #[tokio::test]
    async fn test_check_drain_complete_transitions_to_drained() {
        // Node is "draining" with 0 containers → should transition to "drained"
        let mut draining_node = sample_node();
        draining_node.status = "draining".to_string();

        let mut drained_node = sample_node();
        drained_node.status = "drained".to_string();

        let db = MockDatabase::new(DatabaseBackend::Postgres)
            // check_drain_complete: find_by_id
            .append_query_results(vec![vec![draining_node.clone()]])
            // list_containers_for_node: empty → drain complete
            .append_query_results(vec![Vec::<deployment_containers::Model>::new()])
            // update status to "drained"
            .append_query_results(vec![vec![drained_node]])
            .into_connection();
        let service = NodeService::new(Arc::new(db));

        let result = service.check_drain_complete(1).await.unwrap();
        assert!(result, "Should return true when drain completes");
    }

    #[tokio::test]
    async fn test_check_drain_complete_still_has_containers() {
        // Node is "draining" with containers still running → should not transition
        let mut draining_node = sample_node();
        draining_node.status = "draining".to_string();

        let container = sample_container(1, 10, 1);

        let db = MockDatabase::new(DatabaseBackend::Postgres)
            // check_drain_complete: find_by_id
            .append_query_results(vec![vec![draining_node]])
            // list_containers_for_node: has containers
            .append_query_results(vec![vec![container]])
            .into_connection();
        let service = NodeService::new(Arc::new(db));

        let result = service.check_drain_complete(1).await.unwrap();
        assert!(!result, "Should return false when containers remain");
    }

    #[tokio::test]
    async fn test_check_drain_complete_already_drained() {
        // Node is already "drained" → should return true immediately
        let mut drained_node = sample_node();
        drained_node.status = "drained".to_string();

        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![vec![drained_node]])
            .into_connection();
        let service = NodeService::new(Arc::new(db));

        let result = service.check_drain_complete(1).await.unwrap();
        assert!(result, "Should return true for already-drained node");
    }

    #[tokio::test]
    async fn test_check_drain_complete_active_node() {
        // Node is "active" (not draining) → should return false
        let active_node = sample_node(); // status = "active"

        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![vec![active_node]])
            .into_connection();
        let service = NodeService::new(Arc::new(db));

        let result = service.check_drain_complete(1).await.unwrap();
        assert!(!result, "Should return false for active node");
    }

    #[tokio::test]
    async fn test_check_drain_complete_not_found() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![Vec::<nodes::Model>::new()])
            .into_connection();
        let service = NodeService::new(Arc::new(db));

        let result = service.check_drain_complete(999).await;
        assert!(matches!(
            result.unwrap_err(),
            NodeError::NotFoundById { node_id: 999 }
        ));
    }

    #[tokio::test]
    async fn test_check_all_drains() {
        let mut draining_node = sample_node();
        draining_node.status = "draining".to_string();

        let mut drained_result = sample_node();
        drained_result.status = "drained".to_string();

        let db = MockDatabase::new(DatabaseBackend::Postgres)
            // check_all_drains: find draining nodes
            .append_query_results(vec![vec![draining_node.clone()]])
            // check_drain_complete: find_by_id for node 1
            .append_query_results(vec![vec![draining_node]])
            // list_containers_for_node: empty
            .append_query_results(vec![Vec::<deployment_containers::Model>::new()])
            // update: transitions to drained
            .append_query_results(vec![vec![drained_result]])
            .into_connection();
        let service = NodeService::new(Arc::new(db));

        let completed = service.check_all_drains().await.unwrap();
        assert_eq!(completed, vec![1]);
    }
}
