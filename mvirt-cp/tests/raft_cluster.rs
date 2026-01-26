//! Integration tests for Raft cluster functionality.
//!
//! These tests verify that the Raft consensus works correctly with mvirt-cp's
//! Command/Response types and CpState state machine.

use mraft::{NodeConfig, RaftNode, StorageBackend};
use mvirt_cp::{Command, CpState, Response};
use std::collections::BTreeMap;
use std::time::Duration;

/// Helper to create a node configuration for testing.
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

/// Helper struct to manage a test cluster.
struct TestCluster {
    nodes: Vec<RaftNode<Command, Response, CpState>>,
}

impl TestCluster {
    /// Create and start a 3-node cluster.
    async fn new_three_node(base_port: u16) -> Self {
        let ports = [base_port, base_port + 1, base_port + 2];

        // Create configs - each node knows about all others
        let configs: Vec<NodeConfig> = (0..3)
            .map(|i| {
                let peers: Vec<(u64, u16)> = (0..3)
                    .filter(|&j| j != i)
                    .map(|j| ((j + 1) as u64, ports[j as usize]))
                    .collect();
                test_node_config((i + 1) as u64, ports[i as usize], &peers)
            })
            .collect();

        // Create nodes
        let mut nodes = Vec::new();
        for config in configs {
            let node: RaftNode<Command, Response, CpState> =
                RaftNode::new(config).await.expect("Failed to create node");
            nodes.push(node);
        }

        // Start all nodes
        for node in &mut nodes {
            node.start().await.expect("Failed to start node");
        }

        // Small delay to let servers bind
        tokio::time::sleep(Duration::from_millis(100)).await;

        Self { nodes }
    }

    /// Bootstrap the cluster from node 0.
    async fn bootstrap(&self) {
        self.nodes[0]
            .initialize_cluster()
            .await
            .expect("Failed to bootstrap cluster");
    }

    /// Wait for a leader to be elected across any node.
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

    /// Get the leader node (if any).
    fn leader(&self) -> Option<&RaftNode<Command, Response, CpState>> {
        for node in &self.nodes {
            if node.is_leader() {
                return Some(node);
            }
        }
        None
    }

    /// Get a follower node (any non-leader).
    fn follower(&self) -> Option<&RaftNode<Command, Response, CpState>> {
        for node in &self.nodes {
            if !node.is_leader() && node.current_leader().is_some() {
                return Some(node);
            }
        }
        None
    }

    /// Wait for a network to be replicated to all nodes.
    async fn wait_for_replication(&self, network_id: &str, timeout: Duration) -> bool {
        let start = std::time::Instant::now();
        loop {
            let mut all_have_it = true;
            for node in &self.nodes {
                let state = node.get_state().await;
                if state.get_network(network_id).is_none() {
                    all_have_it = false;
                    break;
                }
            }
            if all_have_it {
                return true;
            }
            if start.elapsed() > timeout {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Shutdown all nodes.
    async fn shutdown(&mut self) {
        for node in &mut self.nodes {
            let _ = node.shutdown().await;
        }
    }
}

// =============================================================================
// Test 1: Three Node Cluster Forms
// =============================================================================

#[tokio::test]
async fn test_three_node_cluster_forms() {
    // Start 3 nodes with in-memory storage
    let mut cluster = TestCluster::new_three_node(16001).await;

    // Bootstrap from node 1
    cluster.bootstrap().await;

    // Wait for leader election
    let leader = cluster
        .wait_for_leader(Duration::from_secs(10))
        .await
        .expect("No leader elected within timeout");

    println!("Leader elected: node {}", leader);

    // Verify all nodes agree on the leader
    for node in &cluster.nodes {
        let node_leader = node.current_leader();
        assert_eq!(
            node_leader,
            Some(leader),
            "Node {} sees different leader: {:?}",
            node.id(),
            node_leader
        );
    }

    cluster.shutdown().await;
}

// =============================================================================
// Test 2: Leader Election on Failure
// =============================================================================

#[tokio::test]
async fn test_leader_election_on_failure() {
    let mut cluster = TestCluster::new_three_node(16011).await;
    cluster.bootstrap().await;

    // Wait for initial leader
    let old_leader = cluster
        .wait_for_leader(Duration::from_secs(10))
        .await
        .expect("No initial leader");

    println!("Initial leader: node {}", old_leader);

    // Find and shutdown the leader
    let leader_idx = cluster
        .nodes
        .iter()
        .position(|n| n.id() == old_leader)
        .unwrap();

    cluster.nodes[leader_idx]
        .shutdown()
        .await
        .expect("Failed to shutdown leader");

    println!("Leader node {} shut down", old_leader);

    // Wait for new leader election (from remaining nodes)
    let start = std::time::Instant::now();
    let mut new_leader = None;
    while start.elapsed() < Duration::from_secs(10) {
        for (i, node) in cluster.nodes.iter().enumerate() {
            if i == leader_idx {
                continue; // Skip the shut down node
            }
            if let Some(leader) = node.current_leader() {
                if leader != old_leader {
                    new_leader = Some(leader);
                    break;
                }
            }
        }
        if new_leader.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let new_leader = new_leader.expect("No new leader elected after old leader failed");
    println!("New leader elected: node {}", new_leader);

    assert_ne!(new_leader, old_leader, "New leader should be different");

    // Cleanup remaining nodes
    for (i, node) in cluster.nodes.iter_mut().enumerate() {
        if i != leader_idx {
            let _ = node.shutdown().await;
        }
    }
}

// =============================================================================
// Test 3: Log Replication
// =============================================================================

#[tokio::test]
async fn test_log_replication() {
    let mut cluster = TestCluster::new_three_node(16021).await;
    cluster.bootstrap().await;

    // Wait for leader
    cluster
        .wait_for_leader(Duration::from_secs(10))
        .await
        .expect("No leader");

    // Write to leader
    let leader = cluster.leader().expect("No leader found");
    let leader_id = leader.id();
    println!("Writing to leader node {}", leader_id);

    let network_id = uuid::Uuid::new_v4().to_string();
    let response = leader
        .write(Command::CreateNetwork {
            request_id: "req-1".to_string(),
            id: network_id.clone(),
            name: "test-network".to_string(),
            ipv4_enabled: true,
            ipv4_subnet: Some("10.0.0.0/24".to_string()),
            ipv6_enabled: false,
            ipv6_prefix: None,
            dns_servers: vec![],
            ntp_servers: vec![],
            is_public: false,
        })
        .await
        .expect("Write failed");

    // Verify response
    match response {
        Response::Network(data) => {
            assert_eq!(data.name, "test-network");
            assert_eq!(data.id, network_id);
            println!("Created network: {}", data.id);
        }
        other => panic!("Unexpected response: {:?}", other),
    };

    // Debug: Print metrics for each node
    for node in &cluster.nodes {
        let metrics = node.metrics();
        println!(
            "Node {} - leader: {:?}, last_applied: {:?}, last_log: {:?}",
            node.id(),
            metrics.current_leader,
            metrics.last_applied,
            metrics.last_log_index
        );
    }

    // Wait for replication to all nodes with extended timeout
    let replicated = cluster
        .wait_for_replication(&network_id, Duration::from_secs(10))
        .await;

    // Debug: Print state after wait
    for node in &cluster.nodes {
        let state = node.get_state().await;
        let has_network = state.get_network(&network_id).is_some();
        let metrics = node.metrics();
        println!(
            "After wait - Node {} has network: {}, last_applied: {:?}",
            node.id(),
            has_network,
            metrics.last_applied
        );
    }

    assert!(replicated, "Network should be replicated to all nodes");

    // Double-check all nodes have the network
    for node in &cluster.nodes {
        let state = node.get_state().await;
        let network = state.get_network(&network_id);
        assert!(
            network.is_some(),
            "Node {} missing network {}",
            node.id(),
            network_id
        );
        assert_eq!(network.unwrap().name, "test-network");
    }

    println!("Network replicated to all nodes");

    cluster.shutdown().await;
}

// =============================================================================
// Test 4: Read from Follower
// =============================================================================

#[tokio::test]
async fn test_read_from_follower() {
    let mut cluster = TestCluster::new_three_node(16031).await;
    cluster.bootstrap().await;

    cluster
        .wait_for_leader(Duration::from_secs(10))
        .await
        .expect("No leader");

    // Write via leader
    let leader = cluster.leader().expect("No leader");
    let network_id = uuid::Uuid::new_v4().to_string();
    let response = leader
        .write(Command::CreateNetwork {
            request_id: "req-follower-read".to_string(),
            id: network_id.clone(),
            name: "follower-read-test".to_string(),
            ipv4_enabled: true,
            ipv4_subnet: Some("10.1.0.0/24".to_string()),
            ipv6_enabled: false,
            ipv6_prefix: None,
            dns_servers: vec!["8.8.8.8".to_string()],
            ntp_servers: vec![],
            is_public: true,
        })
        .await
        .expect("Write failed");

    match response {
        Response::Network(data) => assert_eq!(data.id, network_id),
        other => panic!("Unexpected response: {:?}", other),
    };

    // Wait for replication
    let replicated = cluster
        .wait_for_replication(&network_id, Duration::from_secs(5))
        .await;
    assert!(replicated, "Network should be replicated");

    // Read from follower
    let follower = cluster.follower().expect("No follower found");
    println!(
        "Reading from follower node {} (leader is {})",
        follower.id(),
        follower.current_leader().unwrap()
    );

    let state = follower.get_state().await;
    let network = state
        .get_network(&network_id)
        .expect("Network not found on follower");

    assert_eq!(network.name, "follower-read-test");
    assert_eq!(network.ipv4_subnet, Some("10.1.0.0/24".to_string()));
    assert!(network.is_public);
    assert_eq!(network.dns_servers, vec!["8.8.8.8".to_string()]);

    println!("Successfully read consistent state from follower");

    cluster.shutdown().await;
}

// =============================================================================
// Test 5: Majority Write During Minority Failure
// =============================================================================

#[tokio::test]
async fn test_majority_write_during_minority_failure() {
    // This test verifies that writes succeed as long as a majority is available
    let mut cluster = TestCluster::new_three_node(16041).await;
    cluster.bootstrap().await;

    cluster
        .wait_for_leader(Duration::from_secs(10))
        .await
        .expect("No leader");

    // Initial write
    let leader = cluster.leader().expect("No leader");
    let before_id = uuid::Uuid::new_v4().to_string();
    let response = leader
        .write(Command::CreateNetwork {
            request_id: "req-before-failure".to_string(),
            id: before_id.clone(),
            name: "before-failure".to_string(),
            ipv4_enabled: true,
            ipv4_subnet: None,
            ipv6_enabled: false,
            ipv6_prefix: None,
            dns_servers: vec![],
            ntp_servers: vec![],
            is_public: false,
        })
        .await
        .expect("Initial write failed");

    match response {
        Response::Network(data) => assert_eq!(data.id, before_id),
        other => panic!("Unexpected response: {:?}", other),
    };

    // Wait for replication
    cluster
        .wait_for_replication(&before_id, Duration::from_secs(5))
        .await;

    // Find a non-leader node to shut down
    let failed_id = cluster
        .nodes
        .iter()
        .find(|n| !n.is_leader())
        .map(|n| n.id())
        .expect("No follower to fail");

    let failed_idx = cluster
        .nodes
        .iter()
        .position(|n| n.id() == failed_id)
        .unwrap();

    println!("Shutting down node {}", failed_id);

    // Shutdown one follower
    cluster.nodes[failed_idx]
        .shutdown()
        .await
        .expect("Failed to shutdown node");

    // Small delay
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Find the current leader (might have changed if we shut down the leader)
    let mut current_leader = None;
    for (i, node) in cluster.nodes.iter().enumerate() {
        if i == failed_idx {
            continue;
        }
        if node.is_leader() {
            current_leader = Some(node);
            break;
        }
    }

    // If no leader, wait for one to be elected
    if current_leader.is_none() {
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
    }

    let leader = current_leader.expect("No leader after failure");
    println!("Leader after failure: node {}", leader.id());

    // Write should succeed with 2-node majority
    let during_id = uuid::Uuid::new_v4().to_string();
    let response = leader
        .write(Command::CreateNetwork {
            request_id: "req-during-failure".to_string(),
            id: during_id.clone(),
            name: "during-failure".to_string(),
            ipv4_enabled: true,
            ipv4_subnet: Some("192.168.0.0/24".to_string()),
            ipv6_enabled: false,
            ipv6_prefix: None,
            dns_servers: vec![],
            ntp_servers: vec![],
            is_public: false,
        })
        .await
        .expect("Write during minority failure should succeed");

    match response {
        Response::Network(data) => {
            println!("Created network during failure: {}", data.id);
            assert_eq!(data.id, during_id);
        }
        other => panic!("Unexpected response: {:?}", other),
    };

    // Verify the remaining nodes have both networks
    for (i, node) in cluster.nodes.iter().enumerate() {
        if i == failed_idx {
            continue;
        }
        let state = node.get_state().await;
        assert!(
            state.get_network(&before_id).is_some(),
            "Node {} should have network created before failure",
            node.id()
        );
        assert!(
            state.get_network(&during_id).is_some(),
            "Node {} should have network created during failure",
            node.id()
        );
    }

    println!("Majority write test passed");

    // Cleanup remaining nodes
    for (i, node) in cluster.nodes.iter_mut().enumerate() {
        if i != failed_idx {
            let _ = node.shutdown().await;
        }
    }
}

// =============================================================================
// Bonus: Test write_or_forward functionality
// =============================================================================

#[tokio::test]
async fn test_write_or_forward() {
    let mut cluster = TestCluster::new_three_node(16051).await;
    cluster.bootstrap().await;

    cluster
        .wait_for_leader(Duration::from_secs(10))
        .await
        .expect("No leader");

    // Get a follower
    let follower = cluster.follower().expect("No follower");
    let follower_id = follower.id();

    println!(
        "Writing via follower {} (should forward to leader)",
        follower_id
    );

    // Write via follower using write_or_forward
    let network_id = uuid::Uuid::new_v4().to_string();
    let response = follower
        .write_or_forward(Command::CreateNetwork {
            request_id: "req-forwarded".to_string(),
            id: network_id.clone(),
            name: "forwarded-write".to_string(),
            ipv4_enabled: true,
            ipv4_subnet: Some("172.16.0.0/16".to_string()),
            ipv6_enabled: false,
            ipv6_prefix: None,
            dns_servers: vec![],
            ntp_servers: vec![],
            is_public: false,
        })
        .await
        .expect("Forwarded write failed");

    match response {
        Response::Network(data) => {
            assert_eq!(data.name, "forwarded-write");
            assert_eq!(data.id, network_id);
            println!("Network created via forwarding: {}", data.id);
        }
        other => panic!("Unexpected response: {:?}", other),
    };

    // Verify leader has the data
    let leader = cluster.leader().expect("No leader");
    let state = leader.get_state().await;
    assert!(
        state.get_network(&network_id).is_some(),
        "Leader should have the forwarded network"
    );

    println!("write_or_forward test passed");

    cluster.shutdown().await;
}
