//! Periodic job that checks node health, marks stale nodes as offline,
//! and triggers failover redeployment for affected environments.
//!
//! Runs on the control plane every 60 seconds. Nodes that haven't sent
//! a heartbeat in >90 seconds are marked offline. When a node transitions
//! to offline, its affected environments are automatically redeployed
//! to healthy nodes.

use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};

use temps_entities::nodes;

use crate::services::node_service::NodeService;
use crate::DeploymentService;

/// Threshold in seconds — nodes with older heartbeats are marked offline.
const HEARTBEAT_STALE_THRESHOLD_SECS: i64 = 90;

/// Runs a single health check pass across all active nodes.
///
/// This is designed to be called by a scheduler (e.g., every 60 seconds).
/// It does NOT run in a loop itself.
///
/// Returns the list of node IDs that were marked offline (for failover).
pub async fn check_node_health(node_service: &NodeService, db: &DatabaseConnection) -> Vec<i32> {
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
            return vec![];
        }
    };

    let mut marked_offline = Vec::new();

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
            marked_offline.push(node.id);
        }
    }

    if !marked_offline.is_empty() {
        tracing::info!(
            count = marked_offline.len(),
            "Node health check completed: marked {} node(s) offline",
            marked_offline.len()
        );
    }

    marked_offline
}

/// Notify operators that worker nodes went offline (ADR-020 / monitoring).
///
/// Called by the health-check loop right after `check_node_health` marks
/// nodes offline. Sends one Alert-priority notification per node through the
/// shared notification pipeline (email / Slack / webhook, per the operator's
/// configured channels). A node is only marked offline on the active->offline
/// transition, so this fires exactly once per outage — no repeat spam while a
/// node stays down. Best-effort: delivery failures are logged, never fatal.
pub async fn notify_nodes_offline(
    offline_node_ids: &[i32],
    node_service: &NodeService,
    notification_service: &std::sync::Arc<dyn temps_core::notifications::NotificationService>,
) {
    use temps_core::notifications::{NotificationData, NotificationPriority, NotificationType};

    for &node_id in offline_node_ids {
        let name = node_service
            .get_by_id(node_id)
            .await
            .map(|n| n.name)
            .unwrap_or_else(|_| format!("node-{}", node_id));

        let mut metadata = std::collections::HashMap::new();
        metadata.insert("event".to_string(), "node_offline".to_string());
        metadata.insert("node_id".to_string(), node_id.to_string());
        metadata.insert("node_name".to_string(), name.clone());

        let notification = NotificationData {
            title: format!("Worker node '{}' is offline", name),
            message: format!(
                "Node '{}' (id {}) stopped sending heartbeats for over {}s and was marked offline. \
                 Affected workloads are being failed over to healthy nodes.",
                name, node_id, HEARTBEAT_STALE_THRESHOLD_SECS
            ),
            notification_type: NotificationType::Alert,
            priority: NotificationPriority::Critical,
            ..Default::default()
        };

        match notification_service.send_notification(notification).await {
            Ok(()) => tracing::info!(node_id, node_name = %name, "Sent node-offline alert"),
            Err(e) => tracing::error!(
                node_id,
                node_name = %name,
                "Failed to send node-offline alert: {}",
                e
            ),
        }
    }
}

/// Notify operators that a worker node came back online (ADR-020 / monitoring).
///
/// Called from the heartbeat handler when a node transitions offline->active
/// (the `was_offline` signal). Sends one Info-priority notification through the
/// shared notification pipeline — the recovery counterpart to
/// [`notify_nodes_offline`]. Best-effort: failures are logged, never fatal.
pub async fn notify_node_recovered(
    node_id: i32,
    node_name: &str,
    notification_service: &std::sync::Arc<dyn temps_core::notifications::NotificationService>,
) {
    use temps_core::notifications::{NotificationData, NotificationPriority, NotificationType};

    let mut metadata = std::collections::HashMap::new();
    metadata.insert("event".to_string(), "node_recovered".to_string());
    metadata.insert("node_id".to_string(), node_id.to_string());
    metadata.insert("node_name".to_string(), node_name.to_string());

    let notification = NotificationData {
        title: format!("Worker node '{}' is back online", node_name),
        message: format!(
            "Node '{}' (id {}) resumed sending heartbeats and was marked active again.",
            node_name, node_id
        ),
        notification_type: NotificationType::Info,
        priority: NotificationPriority::Normal,
        metadata,
        ..Default::default()
    };

    match notification_service.send_notification(notification).await {
        Ok(()) => tracing::info!(node_id, node_name = %node_name, "Sent node-recovery alert"),
        Err(e) => tracing::error!(
            node_id,
            node_name = %node_name,
            "Failed to send node-recovery alert: {}",
            e
        ),
    }
}

/// Percent thresholds above which a node resource raises a Warning alert.
const NODE_CPU_ALERT_PERCENT: f64 = 90.0;
const NODE_MEMORY_ALERT_PERCENT: f64 = 90.0;
const NODE_DISK_ALERT_PERCENT: f64 = 90.0;
/// A still-"active" node whose last heartbeat is older than this (but under the
/// offline threshold) is lagging — an early "node is struggling / slow to
/// respond" signal short of a full outage.
const NODE_HEARTBEAT_LAG_ALERT_SECS: i64 = 60;

/// Send one resource-pressure alert. The title is STABLE (value lives in the
/// body) so the notification pipeline's batch-key throttle collapses repeated
/// alerts for the same node+metric instead of firing every 60s cycle.
async fn send_node_resource_alert(
    notification_service: &std::sync::Arc<dyn temps_core::notifications::NotificationService>,
    node_name: &str,
    node_id: i32,
    metric: &str,
    value: f64,
    threshold: f64,
) {
    use temps_core::notifications::{NotificationData, NotificationPriority, NotificationType};
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("event".to_string(), "node_resource_high".to_string());
    metadata.insert("node_id".to_string(), node_id.to_string());
    metadata.insert("node_name".to_string(), node_name.to_string());
    metadata.insert("metric".to_string(), metric.to_string());
    metadata.insert("value_percent".to_string(), format!("{:.1}", value));
    metadata.insert("threshold_percent".to_string(), format!("{:.0}", threshold));
    let notification = NotificationData {
        title: format!("Worker node '{}' {} usage is high", node_name, metric),
        message: format!(
            "{} on node '{}' (id {}) is at {:.1}% (alert threshold {:.0}%).",
            metric, node_name, node_id, value, threshold
        ),
        notification_type: NotificationType::Warning,
        priority: NotificationPriority::High,
        metadata,
        ..Default::default()
    };
    if let Err(e) = notification_service.send_notification(notification).await {
        tracing::error!(node_id, metric, "Failed to send node resource alert: {}", e);
    } else {
        tracing::info!(node_id, node_name = %node_name, metric, value, "Sent node resource alert");
    }
}

/// Check active nodes' resource usage (CPU / memory / disk from the heartbeat
/// `capacity`) and heartbeat freshness, alerting on threshold breaches
/// (ADR-020 / monitoring). Runs in the 60s health loop after the offline check.
/// Best-effort: query/delivery failures are logged, never fatal.
pub async fn check_node_resources(
    db: &DatabaseConnection,
    notification_service: &std::sync::Arc<dyn temps_core::notifications::NotificationService>,
) {
    let active = match nodes::Entity::find()
        .filter(nodes::Column::Status.eq("active"))
        .all(db)
        .await
    {
        Ok(n) => n,
        Err(e) => {
            tracing::error!("Node resource check: failed to query active nodes: {}", e);
            return;
        }
    };

    let now = chrono::Utc::now();
    for node in &active {
        let cap = &node.capacity;

        if let Some(cpu) = cap.get("cpu_percent").and_then(|v| v.as_f64()) {
            if cpu > NODE_CPU_ALERT_PERCENT {
                send_node_resource_alert(
                    notification_service,
                    &node.name,
                    node.id,
                    "CPU",
                    cpu,
                    NODE_CPU_ALERT_PERCENT,
                )
                .await;
            }
        }

        if let (Some(used), Some(total)) = (
            cap.get("memory_used_bytes").and_then(|v| v.as_f64()),
            cap.get("memory_total_bytes").and_then(|v| v.as_f64()),
        ) {
            if total > 0.0 {
                let pct = used / total * 100.0;
                if pct > NODE_MEMORY_ALERT_PERCENT {
                    send_node_resource_alert(
                        notification_service,
                        &node.name,
                        node.id,
                        "memory",
                        pct,
                        NODE_MEMORY_ALERT_PERCENT,
                    )
                    .await;
                }
            }
        }

        if let (Some(used), Some(total)) = (
            cap.get("disk_used_bytes").and_then(|v| v.as_f64()),
            cap.get("disk_total_bytes").and_then(|v| v.as_f64()),
        ) {
            if total > 0.0 {
                let pct = used / total * 100.0;
                if pct > NODE_DISK_ALERT_PERCENT {
                    send_node_resource_alert(
                        notification_service,
                        &node.name,
                        node.id,
                        "disk",
                        pct,
                        NODE_DISK_ALERT_PERCENT,
                    )
                    .await;
                }
            }
        }

        // Heartbeat lag — the node is still alive but slow to check in
        // (responsiveness / latency proxy short of a full outage).
        if let Some(hb) = node.last_heartbeat {
            let lag = (now - hb).num_seconds();
            if lag > NODE_HEARTBEAT_LAG_ALERT_SECS {
                use temps_core::notifications::{
                    NotificationData, NotificationPriority, NotificationType,
                };
                let mut metadata = std::collections::HashMap::new();
                metadata.insert("event".to_string(), "node_heartbeat_lag".to_string());
                metadata.insert("node_id".to_string(), node.id.to_string());
                metadata.insert("node_name".to_string(), node.name.clone());
                metadata.insert("lag_seconds".to_string(), lag.to_string());
                let notification = NotificationData {
                    title: format!("Worker node '{}' is slow to respond", node.name),
                    message: format!(
                        "Node '{}' (id {}) last sent a heartbeat {}s ago — it is lagging \
                         (marked offline at {}s). The node may be overloaded or network-degraded.",
                        node.name, node.id, lag, HEARTBEAT_STALE_THRESHOLD_SECS
                    ),
                    notification_type: NotificationType::Warning,
                    priority: NotificationPriority::High,
                    metadata,
                    ..Default::default()
                };
                if let Err(e) = notification_service.send_notification(notification).await {
                    tracing::error!(node_id = node.id, "Failed to send node-lag alert: {}", e);
                } else {
                    tracing::info!(node_id = node.id, node_name = %node.name, lag, "Sent node-lag alert");
                }
            }
        }
    }
}

/// Check all draining nodes for drain completion and transition them
/// to "drained" status when all containers have been migrated.
///
/// This is designed to be called after `check_node_health` in the same
/// periodic job (every 60 seconds). Returns the node IDs that completed.
pub async fn check_drain_completion(node_service: &NodeService) -> Vec<i32> {
    match node_service.check_all_drains().await {
        Ok(completed) => completed,
        Err(e) => {
            tracing::error!("Failed to check drain completion: {}", e);
            vec![]
        }
    }
}

/// Handle failover for nodes that just went offline.
///
/// For each affected deployment:
/// - If other nodes still have healthy replicas, just retire the containers
///   on the offline node (proxy stops routing to them on next refresh).
/// - If ALL replicas were on the offline node, trigger a full redeploy so
///   the workload is rescheduled to a healthy node.
pub async fn failover_offline_nodes(
    offline_node_ids: &[i32],
    node_service: &NodeService,
    deployment_service: &DeploymentService,
) {
    if offline_node_ids.is_empty() {
        return;
    }

    for &node_id in offline_node_ids {
        let affected = match node_service.affected_deployments(node_id).await {
            Ok(deps) => deps,
            Err(e) => {
                tracing::error!(
                    node_id,
                    "Failed to query affected deployments for failover: {}",
                    e
                );
                continue;
            }
        };

        if affected.is_empty() {
            tracing::debug!(node_id, "No affected deployments for offline node");
            continue;
        }

        tracing::warn!(
            node_id,
            affected_count = affected.len(),
            "Failover: processing {} deployment(s) on offline node",
            affected.len()
        );

        for dep in &affected {
            if dep.needs_redeploy() {
                // All replicas were on this node — must redeploy
                match deployment_service
                    .redeploy_environment(dep.project_id, dep.environment_id)
                    .await
                {
                    Ok(_) => {
                        tracing::info!(
                            node_id,
                            project_id = dep.project_id,
                            environment_id = dep.environment_id,
                            "Failover: triggered full redeploy (no healthy replicas elsewhere)"
                        );
                    }
                    Err(e) => {
                        tracing::error!(
                            node_id,
                            project_id = dep.project_id,
                            environment_id = dep.environment_id,
                            "Failover: failed to trigger redeploy: {}",
                            e
                        );
                    }
                }
            } else {
                // Other nodes have healthy replicas — just retire stale containers
                match node_service
                    .retire_containers_on_node(node_id, dep.deployment_id)
                    .await
                {
                    Ok(count) => {
                        tracing::info!(
                            node_id,
                            deployment_id = dep.deployment_id,
                            retired = count,
                            remaining = dep.total_active_containers - dep.containers_on_node,
                            "Failover: retired containers, healthy replicas remain"
                        );
                    }
                    Err(e) => {
                        tracing::error!(
                            node_id,
                            deployment_id = dep.deployment_id,
                            "Failover: failed to retire containers: {}",
                            e
                        );
                    }
                }
            }
        }
    }
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
            edge_public_key: None,
            compute_cidr: None,
            underlay_address: None,
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
        assert!(marked.is_empty());
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
        assert_eq!(marked.len(), 2);
        assert!(marked.contains(&1));
        assert!(marked.contains(&2));
    }

    #[tokio::test]
    async fn test_check_node_health_returns_offline_ids() {
        let stale_node = make_node(5, "worker-5", "active", 200);

        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![vec![stale_node]])
            .into_connection();

        let node_for_service = make_node(5, "worker-5", "active", 200);
        let node_updated = make_node(5, "worker-5", "offline", 200);

        let service_db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![vec![node_for_service]])
            .append_query_results(vec![vec![node_updated]])
            .into_connection();
        let node_service = NodeService::new(std::sync::Arc::new(service_db));

        let marked = check_node_health(&node_service, &db).await;
        assert_eq!(marked, vec![5]);
    }
}
