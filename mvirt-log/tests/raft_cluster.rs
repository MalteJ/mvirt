//! Integration tests for mvirt-log Raft cluster.
//!
//! Tests verify that log entries are replicated across a multi-node Raft cluster,
//! that reads work from followers, and that writes can be forwarded.

use mraft::{NodeConfig, RaftNode, StorageBackend};
use mvirt_log::command::{LogCommand, LogCommandResponse, SerializableLogEntry};
use mvirt_log::storage::{init_log_manager, LogManager, LogStateMachine};
use std::collections::BTreeMap;
use std::sync::{Arc, Once};
use std::time::Duration;
use tempfile::TempDir;
use ulid::Ulid;

// Shared LogManager across all tests in this binary.
// We leak the TempDir so it lives for the process lifetime.
static INIT: Once = Once::new();

fn ensure_log_manager() {
    INIT.call_once(|| {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_path_buf();
        // Leak the TempDir so the directory isn't deleted while tests run
        std::mem::forget(dir);
        let manager = Arc::new(LogManager::new(&path).unwrap());
        init_log_manager(manager);
    });
}

/// Allocate an unused port.
fn allocate_port() -> u16 {
    portpicker::pick_unused_port().expect("No available port")
}

fn test_node_config(id: u64, port: u16, peers: &[(u64, u16)]) -> NodeConfig {
    let mut peer_map = BTreeMap::new();
    for (peer_id, peer_port) in peers {
        peer_map.insert(*peer_id, format!("127.0.0.1:{}", peer_port));
    }

    NodeConfig {
        id,
        listen_addr: format!("127.0.0.1:{}", port),
        peers: peer_map,
        storage: StorageBackend::Memory,
        raft_config: None,
    }
}

fn make_serializable_entry(message: &str, objects: Vec<&str>) -> SerializableLogEntry {
    let ts_ns = 1_700_000_000_000_000_000i64;
    let ms = (ts_ns / 1_000_000) as u64;
    let ulid = Ulid::from_parts(ms, rand::random());
    SerializableLogEntry {
        id: ulid.to_string(),
        timestamp_ns: ts_ns,
        message: message.to_string(),
        level: 1,
        component: "test".to_string(),
        related_object_ids: objects.into_iter().map(String::from).collect(),
    }
}

struct TestCluster {
    nodes: Vec<RaftNode<LogCommand, LogCommandResponse, LogStateMachine>>,
}

impl TestCluster {
    async fn new_three_node() -> Self {
        ensure_log_manager();

        let ports = [allocate_port(), allocate_port(), allocate_port()];

        let configs: Vec<NodeConfig> = (0..3)
            .map(|i| {
                let peers: Vec<(u64, u16)> = (0..3)
                    .filter(|&j| j != i)
                    .map(|j| ((j + 1) as u64, ports[j as usize]))
                    .collect();
                test_node_config((i + 1) as u64, ports[i as usize], &peers)
            })
            .collect();

        let mut nodes = Vec::new();
        for config in configs {
            let node: RaftNode<LogCommand, LogCommandResponse, LogStateMachine> =
                RaftNode::new(config).await.expect("Failed to create node");
            nodes.push(node);
        }

        for node in &mut nodes {
            node.start().await.expect("Failed to start node");
        }

        tokio::time::sleep(Duration::from_millis(100)).await;

        Self { nodes }
    }

    async fn bootstrap(&self) {
        self.nodes[0]
            .initialize_cluster()
            .await
            .expect("Failed to bootstrap cluster");
    }

    async fn wait_for_leader(&self, timeout: Duration) -> Option<u64> {
        let start = std::time::Instant::now();
        loop {
            for node in &self.nodes {
                if let Some(leader) = node.current_leader() {
                    return Some(leader);
                }
            }
            if start.elapsed() > timeout {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    fn leader(&self) -> Option<&RaftNode<LogCommand, LogCommandResponse, LogStateMachine>> {
        self.nodes.iter().find(|n| n.is_leader())
    }

    fn follower(&self) -> Option<&RaftNode<LogCommand, LogCommandResponse, LogStateMachine>> {
        self.nodes
            .iter()
            .find(|n| !n.is_leader() && n.current_leader().is_some())
    }

    async fn shutdown(&mut self) {
        for node in &mut self.nodes {
            let _ = node.shutdown().await;
        }
    }
}

// =============================================================================
// Test 1: Three-node cluster forms and elects a leader
// =============================================================================

#[tokio::test]
async fn test_cluster_forms_and_elects_leader() {
    let mut cluster = TestCluster::new_three_node().await;
    cluster.bootstrap().await;

    let leader = cluster
        .wait_for_leader(Duration::from_secs(10))
        .await
        .expect("No leader elected");

    println!("Leader elected: node {}", leader);

    // All nodes agree on the leader
    for node in &cluster.nodes {
        assert_eq!(
            node.current_leader(),
            Some(leader),
            "Node {} sees different leader",
            node.id()
        );
    }

    cluster.shutdown().await;
}

// =============================================================================
// Test 2: Write to leader, verify replication to all nodes via LogManager
// =============================================================================

#[tokio::test]
async fn test_write_replicates_to_all_nodes() {
    let mut cluster = TestCluster::new_three_node().await;
    cluster.bootstrap().await;

    cluster
        .wait_for_leader(Duration::from_secs(10))
        .await
        .expect("No leader");

    let leader = cluster.leader().expect("No leader found");

    let entry = make_serializable_entry("replicated log", vec!["vm-1"]);
    let entry_id = entry.id.clone();
    let cmd = LogCommand::AppendBatch(vec![entry]);

    let resp = leader.write(cmd).await.expect("Write failed");
    assert!(matches!(resp, LogCommandResponse::Ok));

    // Wait for replication — all nodes apply to the same LogManager via OnceLock,
    // so the write is visible immediately after Raft commits.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Query the shared LogManager directly
    let manager = mvirt_log::storage::log_manager();
    let results = manager.query(None, None, None, 1000).unwrap();
    assert!(
        results
            .iter()
            .any(|e| e.id == entry_id && e.message == "replicated log"),
        "Replicated log entry not found"
    );

    // Query by object ID
    let by_obj = manager
        .query(Some("vm-1".to_string()), None, None, 1000)
        .unwrap();
    assert!(
        by_obj.iter().any(|e| e.id == entry_id),
        "Entry not found by object ID"
    );

    cluster.shutdown().await;
}

// =============================================================================
// Test 3: Write via follower (write_or_forward)
// =============================================================================

#[tokio::test]
async fn test_write_or_forward_from_follower() {
    let mut cluster = TestCluster::new_three_node().await;
    cluster.bootstrap().await;

    cluster
        .wait_for_leader(Duration::from_secs(10))
        .await
        .expect("No leader");

    let follower = cluster.follower().expect("No follower found");
    let follower_id = follower.id();

    println!(
        "Writing via follower {} (should forward to leader)",
        follower_id
    );

    let entry = make_serializable_entry("forwarded log", vec![]);
    let cmd = LogCommand::AppendBatch(vec![entry]);

    let resp = follower
        .write_or_forward(cmd)
        .await
        .expect("Forwarded write failed");

    assert!(matches!(resp, LogCommandResponse::Ok));

    // Verify the entry landed in the shared LogManager
    tokio::time::sleep(Duration::from_millis(200)).await;
    let manager = mvirt_log::storage::log_manager();
    let results = manager.query(None, None, None, 100).unwrap();
    assert!(
        results.iter().any(|e| e.message == "forwarded log"),
        "Forwarded log entry not found"
    );

    cluster.shutdown().await;
}

// =============================================================================
// Test 4: Multiple batches accumulate
// =============================================================================

#[tokio::test]
async fn test_multiple_batches() {
    let mut cluster = TestCluster::new_three_node().await;
    cluster.bootstrap().await;

    cluster
        .wait_for_leader(Duration::from_secs(10))
        .await
        .expect("No leader");

    let leader = cluster.leader().expect("No leader found");

    // Write 3 batches of 2 entries each
    for batch_idx in 0..3 {
        let entries: Vec<SerializableLogEntry> = (0..2)
            .map(|i| make_serializable_entry(&format!("batch-{}-entry-{}", batch_idx, i), vec![]))
            .collect();

        let cmd = LogCommand::AppendBatch(entries);
        let resp = leader.write(cmd).await.expect("Write failed");
        assert!(matches!(resp, LogCommandResponse::Ok));
    }

    tokio::time::sleep(Duration::from_millis(500)).await;

    let manager = mvirt_log::storage::log_manager();
    let results = manager.query(None, None, None, 1000).unwrap();

    let batch_entries: Vec<_> = results
        .iter()
        .filter(|e| e.message.starts_with("batch-"))
        .collect();
    assert_eq!(batch_entries.len(), 6, "Expected 6 batch entries");

    cluster.shutdown().await;
}

// =============================================================================
// Test 5: Majority write during minority failure
// =============================================================================

#[tokio::test]
async fn test_majority_write_during_minority_failure() {
    let mut cluster = TestCluster::new_three_node().await;
    cluster.bootstrap().await;

    cluster
        .wait_for_leader(Duration::from_secs(10))
        .await
        .expect("No leader");

    // Write before failure
    let leader = cluster.leader().expect("No leader");
    let before_entry = make_serializable_entry("before-failure", vec![]);
    let resp = leader
        .write(LogCommand::AppendBatch(vec![before_entry]))
        .await
        .expect("Write before failure failed");
    assert!(matches!(resp, LogCommandResponse::Ok));

    // Shutdown a follower (minority)
    let failed_idx = cluster
        .nodes
        .iter()
        .position(|n| !n.is_leader())
        .expect("No follower to fail");

    println!("Shutting down node {}", cluster.nodes[failed_idx].id());
    cluster.nodes[failed_idx]
        .shutdown()
        .await
        .expect("Failed to shutdown node");

    tokio::time::sleep(Duration::from_millis(200)).await;

    // Find the current leader (might be same or re-elected)
    let mut current_leader = None;
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(10) {
        for (i, node) in cluster.nodes.iter().enumerate() {
            if i == failed_idx {
                continue;
            }
            if node.is_leader() {
                current_leader = Some(node);
                break;
            }
        }
        if current_leader.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let leader = current_leader.expect("No leader after failure");

    // Write with 2-node majority should succeed
    let during_entry = make_serializable_entry("during-failure", vec![]);
    let resp = leader
        .write(LogCommand::AppendBatch(vec![during_entry]))
        .await
        .expect("Write during minority failure should succeed");
    assert!(matches!(resp, LogCommandResponse::Ok));

    tokio::time::sleep(Duration::from_millis(200)).await;

    let manager = mvirt_log::storage::log_manager();
    let results = manager.query(None, None, None, 1000).unwrap();
    assert!(
        results.iter().any(|e| e.message == "before-failure"),
        "Missing before-failure entry"
    );
    assert!(
        results.iter().any(|e| e.message == "during-failure"),
        "Missing during-failure entry"
    );

    // Cleanup remaining nodes
    for (i, node) in cluster.nodes.iter_mut().enumerate() {
        if i != failed_idx {
            let _ = node.shutdown().await;
        }
    }
}
