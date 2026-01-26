//! Shared test utilities for mvirt-cp integration tests.

use mraft::{NodeConfig, RaftNode, StorageBackend};
use mvirt_cp::rest::{AppState, create_router};
use mvirt_cp::{Command, CpAuditLogger, CpState, Response};
use reqwest::{Client, Response as ReqwestResponse};
use serde::Serialize;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::RwLock;

/// Allocate an available port for testing.
pub fn allocate_port() -> u16 {
    portpicker::pick_unused_port().expect("No available port")
}

/// Test server wrapper that manages a single-node cluster with REST API.
pub struct TestServer {
    pub addr: SocketAddr,
    pub client: Client,
    pub raft_node: Arc<RwLock<RaftNode<Command, Response, CpState>>>,
    shutdown_tx: tokio::sync::oneshot::Sender<()>,
}

impl TestServer {
    /// Spawn a single-node test server with in-memory storage.
    pub async fn spawn() -> Self {
        let raft_port = allocate_port();
        let node_id = 1u64;

        // Create Raft node config
        let config = NodeConfig {
            id: node_id,
            listen_addr: format!("127.0.0.1:{}", raft_port),
            peers: std::collections::BTreeMap::new(),
            storage: StorageBackend::Memory,
            raft_config: None,
        };

        // Create and start Raft node
        let mut node: RaftNode<Command, Response, CpState> =
            RaftNode::new(config).await.expect("Failed to create node");
        node.start().await.expect("Failed to start node");

        // Generate cluster secret and bootstrap
        node.generate_cluster_secret();
        node.initialize_cluster()
            .await
            .expect("Failed to bootstrap cluster");

        // Wait for leader election
        node.wait_for_leader(std::time::Duration::from_secs(5))
            .await
            .expect("No leader elected");

        let node = Arc::new(RwLock::new(node));

        // Create app state with noop audit logger
        let app_state = Arc::new(AppState {
            node: node.clone(),
            audit: Arc::new(CpAuditLogger::new_noop()),
            node_id,
        });

        // Create router
        let router = create_router(app_state);

        // Bind to REST port - use port 0 to let OS choose available port
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let listener = TcpListener::bind(&addr).await.expect("Failed to bind");
        let actual_addr = listener.local_addr().unwrap();

        // Create shutdown channel
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

        // Spawn server task
        tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await
                .expect("Server error");
        });

        // Small delay to ensure server is ready
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let client = Client::new();

        Self {
            addr: actual_addr,
            client,
            raft_node: node,
            shutdown_tx,
        }
    }

    /// Get base URL for the REST API.
    pub fn base_url(&self) -> String {
        format!("http://{}/api/v1", self.addr)
    }

    /// Perform a GET request.
    pub async fn get(&self, path: &str) -> ReqwestResponse {
        self.client
            .get(format!("{}{}", self.base_url(), path))
            .send()
            .await
            .expect("Request failed")
    }

    /// Perform a POST request with JSON body.
    pub async fn post_json<T: Serialize>(&self, path: &str, body: &T) -> ReqwestResponse {
        self.client
            .post(format!("{}{}", self.base_url(), path))
            .json(body)
            .send()
            .await
            .expect("Request failed")
    }

    /// Perform a PATCH request with JSON body.
    pub async fn patch_json<T: Serialize>(&self, path: &str, body: &T) -> ReqwestResponse {
        self.client
            .patch(format!("{}{}", self.base_url(), path))
            .json(body)
            .send()
            .await
            .expect("Request failed")
    }

    /// Perform a DELETE request.
    pub async fn delete(&self, path: &str) -> ReqwestResponse {
        self.client
            .delete(format!("{}{}", self.base_url(), path))
            .send()
            .await
            .expect("Request failed")
    }

    /// Shutdown the server.
    pub async fn shutdown(self) {
        let _ = self.shutdown_tx.send(());
        // Shutdown Raft node
        let mut node = self.raft_node.write().await;
        let _ = node.shutdown().await;
    }
}
