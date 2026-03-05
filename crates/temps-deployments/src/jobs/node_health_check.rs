//! Periodic job that checks node health and marks stale nodes as offline.
//!
//! Runs on the control plane every 60 seconds. Nodes that haven't sent
//! a heartbeat in >90 seconds are marked offline.

use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};

use temps_entities::nodes;

use crate::services::node_service::NodeService;

/// Threshold in seconds — nodes with older heartbeats are marked offline.
const HEARTBEAT_STALE_THRESHOLD_SECS: i64 = 90;

/// Runs a single health check pass across all active nodes.
///
/// This is designed to be called by a scheduler (e.g., every 60 seconds).
/// It does NOT run in a loop itself.
pub async fn check_node_health(node_service: &NodeService, db: &DatabaseConnection) -> u32 {
    let cutoff = chrono::Utc::now() - chrono::Duration::seconds(HEARTBEAT_STALE_THRESHOLD_SECS);

    // Find nodes that are still marked "active" but have a stale heartbeat
    let stale_nodes = match nodes::Entity::find()
        .filter(nodes::Column::Status.eq("active"))
        .filter(
            nodes::Column::LastHeartbeat
                .lt(cutoff)
                .or(nodes::Column::LastHeartbeat.is_null()),
        )
        .all(db)
        .await
    {
        Ok(nodes) => nodes,
        Err(e) => {
            tracing::error!("Failed to query nodes for health check: {}", e);
            return 0;
        }
    };

    let mut marked_offline = 0u32;

    for node in &stale_nodes {
        tracing::warn!(
            node_id = node.id,
            node_name = %node.name,
            last_heartbeat = ?node.last_heartbeat,
            "Node heartbeat stale, marking as offline"
        );

        if let Err(e) = node_service.mark_offline(node.id).await {
            tracing::error!(
                node_id = node.id,
                node_name = %node.name,
                "Failed to mark node as offline: {}",
                e
            );
        } else {
            marked_offline += 1;
        }
    }

    if marked_offline > 0 {
        tracing::info!(
            count = marked_offline,
            "Node health check completed: marked {} node(s) offline",
            marked_offline
        );
    }

    marked_offline
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::{DatabaseBackend, MockDatabase};

    fn make_node(id: i32, name: &str, status: &str, heartbeat_age_secs: i64) -> nodes::Model {
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
            status: status.to_string(),
            labels: serde_json::json!({}),
            capacity: serde_json::json!({}),
            last_heartbeat: Some(
                chrono::Utc::now() - chrono::Duration::seconds(heartbeat_age_secs),
            ),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn test_heartbeat_threshold_is_reasonable() {
        // Heartbeat threshold should be greater than the heartbeat interval (30s)
        // but not so long that stale nodes linger
        const { assert!(HEARTBEAT_STALE_THRESHOLD_SECS > 30) };
        const { assert!(HEARTBEAT_STALE_THRESHOLD_SECS <= 300) };
    }

    #[tokio::test]
    async fn test_check_node_health_no_stale_nodes() {
        // No stale nodes — query returns empty
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![Vec::<nodes::Model>::new()])
            .into_connection();
        let node_service = NodeService::new(std::sync::Arc::new(
            MockDatabase::new(DatabaseBackend::Postgres).into_connection(),
        ));

        let marked = check_node_health(&node_service, &db).await;
        assert_eq!(marked, 0);
    }

    #[tokio::test]
    async fn test_check_node_health_marks_stale_nodes() {
        // Two stale nodes returned by query
        let stale_node_1 = make_node(1, "worker-1", "active", 120);
        let stale_node_2 = make_node(2, "worker-2", "active", 200);

        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![vec![stale_node_1, stale_node_2]])
            .into_connection();

        // NodeService needs its own mock db for mark_offline calls:
        // - get_by_id for node 1, update for node 1
        // - get_by_id for node 2, update for node 2
        let node_1_for_service = make_node(1, "worker-1", "active", 120);
        let node_1_updated = make_node(1, "worker-1", "offline", 120);
        let node_2_for_service = make_node(2, "worker-2", "active", 200);
        let node_2_updated = make_node(2, "worker-2", "offline", 200);

        let service_db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![vec![node_1_for_service]])
            .append_query_results(vec![vec![node_1_updated]])
            .append_query_results(vec![vec![node_2_for_service]])
            .append_query_results(vec![vec![node_2_updated]])
            .into_connection();
        let node_service = NodeService::new(std::sync::Arc::new(service_db));

        let marked = check_node_health(&node_service, &db).await;
        assert_eq!(marked, 2);
    }
}
