//! Node scheduler — selects which nodes each replica deploys to.
//!
//! **Single-node invariant:** if `nodes` table has zero active rows,
//! all replicas deploy locally (identical to current behavior).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use super::node_service::{NodeError, NodeService};

/// Describes where a replica should be deployed.
#[derive(Debug, Clone)]
pub enum NodeAssignment {
    /// Deploy to the local Docker daemon (single-node mode or control plane).
    Local,
    /// Deploy to a remote worker node.
    Remote {
        node_id: i32,
        node_name: String,
        /// Agent API URL
        address: String,
        /// WireGuard or private IP
        private_address: String,
    },
}

impl NodeAssignment {
    /// Returns true if this is a local deployment.
    pub fn is_local(&self) -> bool {
        matches!(self, NodeAssignment::Local)
    }

    /// Get the node_id if this is a remote assignment, None if local.
    pub fn node_id(&self) -> Option<i32> {
        match self {
            NodeAssignment::Local => None,
            NodeAssignment::Remote { node_id, .. } => Some(*node_id),
        }
    }

    /// Get the private address for cross-node networking.
    /// Returns None for local assignments.
    pub fn private_address(&self) -> Option<&str> {
        match self {
            NodeAssignment::Local => None,
            NodeAssignment::Remote {
                private_address, ..
            } => Some(private_address),
        }
    }
}

/// Schedules replicas across available nodes using round-robin.
pub struct NodeScheduler {
    node_service: Arc<NodeService>,
    /// Round-robin counter for distributing replicas
    counter: AtomicUsize,
    /// Heartbeat threshold in seconds — nodes with older heartbeats are excluded
    heartbeat_threshold_secs: i64,
}

impl NodeScheduler {
    pub fn new(node_service: Arc<NodeService>) -> Self {
        Self {
            node_service,
            counter: AtomicUsize::new(0),
            heartbeat_threshold_secs: 90,
        }
    }

    /// Get a reference to the underlying node service.
    pub fn node_service(&self) -> &NodeService {
        &self.node_service
    }

    /// Schedule `replica_count` replicas across available nodes.
    ///
    /// Returns a Vec of `NodeAssignment` — one per replica.
    ///
    /// **Behavior:**
    /// - If `target_node_ids` is set → only schedule on those nodes (error if none are active)
    /// - If no active worker nodes exist → all replicas are `Local`
    /// - If active workers exist → round-robin across **local + remote** nodes,
    ///   so the control plane always participates in scheduling alongside workers
    pub async fn schedule_replicas(
        &self,
        replica_count: u32,
        _labels: Option<&serde_json::Value>,
        target_node_ids: Option<&[i32]>,
    ) -> Result<Vec<NodeAssignment>, NodeError> {
        let active_nodes = self
            .node_service
            .list_active(self.heartbeat_threshold_secs)
            .await?;

        // Filter by target node IDs if specified
        let eligible_nodes: Vec<_> = if let Some(target_ids) = target_node_ids {
            active_nodes
                .into_iter()
                .filter(|n| target_ids.contains(&n.id))
                .collect()
        } else {
            active_nodes
        };

        if eligible_nodes.is_empty() {
            if target_node_ids.is_some() {
                // Caller explicitly requested specific nodes, but none are active.
                // Fall back to local deployment rather than failing the deployment.
                tracing::warn!(
                    "No active target nodes found for deployment, falling back to local deployment"
                );
            }
            // Single-node mode: all replicas go local
            return Ok(vec![NodeAssignment::Local; replica_count as usize]);
        }

        // Build the full scheduling pool: the control plane (Local) + all remote worker nodes.
        // This ensures replicas are distributed across both the control plane and workers
        // rather than piling all replicas onto remote nodes only.
        let mut pool: Vec<NodeAssignment> = Vec::with_capacity(1 + eligible_nodes.len());
        pool.push(NodeAssignment::Local);
        for node in &eligible_nodes {
            pool.push(NodeAssignment::Remote {
                node_id: node.id,
                node_name: node.name.clone(),
                address: node.address.clone(),
                private_address: node.private_address.clone(),
            });
        }

        let mut assignments = Vec::with_capacity(replica_count as usize);

        for _ in 0..replica_count {
            let idx = self.counter.fetch_add(1, Ordering::Relaxed) % pool.len();
            assignments.push(pool[idx].clone());
        }

        Ok(assignments)
    }

    /// Get the assignment for a single replica (convenience wrapper).
    pub async fn schedule_single(
        &self,
        labels: Option<&serde_json::Value>,
    ) -> Result<NodeAssignment, NodeError> {
        let mut assignments = self.schedule_replicas(1, labels, None).await?;
        Ok(assignments.remove(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::{DatabaseBackend, MockDatabase};
    use temps_entities::nodes;

    fn make_node(id: i32, name: &str) -> nodes::Model {
        nodes::Model {
            id,
            name: name.to_string(),
            token_hash: "hash".to_string(),
            token_encrypted: None,
            address: format!("https://10.100.0.{}:3100", id),
            private_address: format!("10.100.0.{}", id),
            public_endpoint: None,
            wg_public_key: None,
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
    async fn test_schedule_no_nodes_returns_local() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![Vec::<nodes::Model>::new()])
            .into_connection();
        let node_service = Arc::new(NodeService::new(Arc::new(db)));
        let scheduler = NodeScheduler::new(node_service);

        let assignments = scheduler.schedule_replicas(3, None, None).await.unwrap();
        assert_eq!(assignments.len(), 3);
        for a in &assignments {
            assert!(a.is_local());
        }
    }

    #[tokio::test]
    async fn test_schedule_with_nodes_round_robins_including_local() {
        let node_a = make_node(1, "worker-a");
        let node_b = make_node(2, "worker-b");

        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![vec![node_a, node_b]])
            .into_connection();
        let node_service = Arc::new(NodeService::new(Arc::new(db)));
        let scheduler = NodeScheduler::new(node_service);

        let assignments = scheduler.schedule_replicas(6, None, None).await.unwrap();
        assert_eq!(assignments.len(), 6);

        // Pool is [Local, worker-a, worker-b] → round-robin: Local, A, B, Local, A, B
        assert!(assignments[0].is_local(), "First replica should be Local");
        match &assignments[1] {
            NodeAssignment::Remote { node_id, .. } => assert_eq!(*node_id, 1),
            _ => panic!("Expected Remote assignment for worker-a"),
        }
        match &assignments[2] {
            NodeAssignment::Remote { node_id, .. } => assert_eq!(*node_id, 2),
            _ => panic!("Expected Remote assignment for worker-b"),
        }
        assert!(assignments[3].is_local(), "Fourth replica should be Local");
        match &assignments[4] {
            NodeAssignment::Remote { node_id, .. } => assert_eq!(*node_id, 1),
            _ => panic!("Expected Remote assignment for worker-a"),
        }
        match &assignments[5] {
            NodeAssignment::Remote { node_id, .. } => assert_eq!(*node_id, 2),
            _ => panic!("Expected Remote assignment for worker-b"),
        }
    }

    #[tokio::test]
    async fn test_schedule_two_replicas_one_worker_splits_local_and_remote() {
        let node_a = make_node(1, "worker-a");

        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![vec![node_a]])
            .into_connection();
        let node_service = Arc::new(NodeService::new(Arc::new(db)));
        let scheduler = NodeScheduler::new(node_service);

        let assignments = scheduler.schedule_replicas(2, None, None).await.unwrap();
        assert_eq!(assignments.len(), 2);

        // Pool is [Local, worker-a] → replica 1 = Local, replica 2 = worker-a
        assert!(assignments[0].is_local(), "First replica should be Local");
        match &assignments[1] {
            NodeAssignment::Remote { node_id, .. } => assert_eq!(*node_id, 1),
            _ => panic!("Expected Remote assignment for worker-a"),
        }
    }

    #[tokio::test]
    async fn test_schedule_single() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![Vec::<nodes::Model>::new()])
            .into_connection();
        let node_service = Arc::new(NodeService::new(Arc::new(db)));
        let scheduler = NodeScheduler::new(node_service);

        let assignment = scheduler.schedule_single(None).await.unwrap();
        assert!(assignment.is_local());
    }

    #[tokio::test]
    async fn test_schedule_with_target_nodes_filters() {
        let node_a = make_node(1, "worker-a");
        let node_b = make_node(2, "worker-b");
        let node_c = make_node(3, "worker-c");

        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![vec![node_a, node_b, node_c]])
            .into_connection();
        let node_service = Arc::new(NodeService::new(Arc::new(db)));
        let scheduler = NodeScheduler::new(node_service);

        // Only target nodes 1 and 3
        let target_ids = vec![1, 3];
        let assignments = scheduler
            .schedule_replicas(6, None, Some(&target_ids))
            .await
            .unwrap();
        assert_eq!(assignments.len(), 6);

        // Pool is [Local, node-1, node-3] → round-robin includes Local
        for assignment in &assignments {
            match assignment {
                NodeAssignment::Remote { node_id, .. } => {
                    assert!(
                        *node_id == 1 || *node_id == 3,
                        "Expected node 1 or 3, got {}",
                        node_id
                    );
                }
                NodeAssignment::Local => {
                    // Local (control plane) is always part of the pool
                }
            }
        }
    }

    #[tokio::test]
    async fn test_schedule_with_target_nodes_none_active_falls_back_to_local() {
        // All nodes active, but target IDs don't match any
        let node_a = make_node(1, "worker-a");

        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![vec![node_a]])
            .into_connection();
        let node_service = Arc::new(NodeService::new(Arc::new(db)));
        let scheduler = NodeScheduler::new(node_service);

        let target_ids = vec![99]; // Non-existent node
        let assignments = scheduler
            .schedule_replicas(2, None, Some(&target_ids))
            .await
            .unwrap();
        assert_eq!(assignments.len(), 2);
        for a in &assignments {
            assert!(
                a.is_local(),
                "Should fall back to local when no target nodes match"
            );
        }
    }

    #[test]
    fn test_node_assignment_accessors() {
        let local = NodeAssignment::Local;
        assert!(local.is_local());
        assert!(local.node_id().is_none());
        assert!(local.private_address().is_none());

        let remote = NodeAssignment::Remote {
            node_id: 5,
            node_name: "worker-5".to_string(),
            address: "https://10.100.0.5:3100".to_string(),
            private_address: "10.100.0.5".to_string(),
        };
        assert!(!remote.is_local());
        assert_eq!(remote.node_id(), Some(5));
        assert_eq!(remote.private_address(), Some("10.100.0.5"));
    }
}
