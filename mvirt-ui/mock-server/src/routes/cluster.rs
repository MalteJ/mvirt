use axum::Json;

use crate::state::{ClusterInfo, Node, NodeRole, NodeState};

pub async fn get_cluster_info() -> Json<ClusterInfo> {
    Json(ClusterInfo {
        id: "cluster-001".to_string(),
        name: "production".to_string(),
        node_count: 3,
        leader_node_id: "node-001".to_string(),
        term: 42,
        created_at: "2024-01-01T00:00:00Z".to_string(),
    })
}

pub async fn get_nodes() -> Json<Vec<Node>> {
    Json(vec![
        Node {
            id: "node-001".to_string(),
            name: "mvirt-node-01".to_string(),
            address: "10.0.0.1:50051".to_string(),
            state: NodeState::Online,
            role: NodeRole::Leader,
            version: "0.1.0".to_string(),
            cpu_count: 16,
            memory_total_bytes: 64 * 1024 * 1024 * 1024,
            memory_used_bytes: 24 * 1024 * 1024 * 1024,
            vm_count: 5,
            uptime: 86400 * 30,
            last_seen: chrono::Utc::now().to_rfc3339(),
        },
        Node {
            id: "node-002".to_string(),
            name: "mvirt-node-02".to_string(),
            address: "10.0.0.2:50051".to_string(),
            state: NodeState::Online,
            role: NodeRole::Follower,
            version: "0.1.0".to_string(),
            cpu_count: 16,
            memory_total_bytes: 64 * 1024 * 1024 * 1024,
            memory_used_bytes: 32 * 1024 * 1024 * 1024,
            vm_count: 8,
            uptime: 86400 * 25,
            last_seen: chrono::Utc::now().to_rfc3339(),
        },
        Node {
            id: "node-003".to_string(),
            name: "mvirt-node-03".to_string(),
            address: "10.0.0.3:50051".to_string(),
            state: NodeState::Maintenance,
            role: NodeRole::Follower,
            version: "0.1.0".to_string(),
            cpu_count: 8,
            memory_total_bytes: 32 * 1024 * 1024 * 1024,
            memory_used_bytes: 8 * 1024 * 1024 * 1024,
            vm_count: 0,
            uptime: 3600,
            last_seen: chrono::Utc::now().to_rfc3339(),
        },
    ])
}
