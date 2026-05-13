//! Shared test utilities for mvirt-cplane integration tests.
//!
//! Each integration-test file is its own binary; if a helper is used by
//! some but not all of them the compiler flags it `dead_code` per binary.
//! We silence that here because the helpers are intentionally part of the
//! shared surface.
#![allow(dead_code)]

use mraft::{NodeConfig, RaftNode, StorageBackend};
use mvirt_cplane::rest::{AppState, create_router};
use mvirt_cplane::store::{DataStore, Event, RaftStore};
use mvirt_cplane::{ApiAuditLogger, ApiState, Command, NodeRegistry, Response, ca, tunnel};
use reqwest::{Client, Response as ReqwestResponse};
use serde::Serialize;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::{RwLock, broadcast};

/// Allocate an available port for testing.
pub fn allocate_port() -> u16 {
    portpicker::pick_unused_port().expect("No available port")
}

/// Test server wrapper that manages a single-node cluster with REST API.
pub struct TestServer {
    pub addr: SocketAddr,
    /// Local TCP address of the mTLS reverse-tunnel listener. `None` when
    /// the server was spawned with the simpler `spawn()` helper.
    pub tunnel_addr: Option<SocketAddr>,
    pub client: Client,
    pub raft_node: Arc<RwLock<RaftNode<Command, Response, ApiState>>>,
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
        let mut node: RaftNode<Command, Response, ApiState> =
            RaftNode::new(config, ApiState::default())
                .await
                .expect("Failed to create node");
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

        // Create event channel for state machine events
        let (event_tx, _) = broadcast::channel::<Event>(256);

        // Create RaftStore
        let store = Arc::new(RaftStore::new(node.clone(), event_tx, node_id));

        // Create app state with noop audit logger
        let app_state = Arc::new(AppState {
            store,
            audit: Arc::new(ApiAuditLogger::new_noop()),
            node_id,
            log_channel: None,
            log_advertise: Vec::new(),
            jwt_validator: None,
            initial_admin_email: None,
        });

        // Create router (auth off — tests run without OIDC).
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

        let server = Self {
            addr: actual_addr,
            tunnel_addr: None,
            client,
            raft_node: node,
            shutdown_tx,
        };

        // Bootstrap a default Org for tests. Pre-launch state machine starts empty;
        // every Project must live under an Org, so tests need one.
        let org_resp = server
            .post_json(
                "/orgs",
                &serde_json::json!({"slug": Self::DEFAULT_ORG_SLUG, "name": "Test Org"}),
            )
            .await;
        assert_eq!(
            org_resp.status(),
            200,
            "failed to bootstrap default test Org"
        );

        server
    }

    /// Spawn variant that additionally binds the mTLS reverse-tunnel
    /// listener on an ephemeral port. Used by the onboarding e2e tests
    /// that need to verify the tunnel handshake end-to-end.
    pub async fn spawn_with_tunnel() -> Self {
        let mut server = Self::spawn().await;

        ca::install_default_crypto_provider();
        // Reuse the same Raft node as the REST listener. A fresh broadcast
        // channel is fine — the tunnel doesn't subscribe.
        let (event_tx, _) = broadcast::channel::<Event>(256);
        let store: Arc<dyn DataStore> =
            Arc::new(RaftStore::new(server.raft_node.clone(), event_tx, 1));
        let ca_material = store
            .ensure_internal_ca("test")
            .await
            .expect("ensure_internal_ca");
        let server_cert = match store.get_server_cert().await.expect("get_server_cert") {
            Some(c) => c,
            None => store
                .rotate_server_cert(vec!["localhost".into(), "127.0.0.1".into()])
                .await
                .expect("rotate_server_cert"),
        };
        let cfg = ca::build_server_config(
            &ca_material.ca_cert_pem,
            &server_cert.cert_pem,
            &server_cert.key_pem,
        )
        .expect("build_server_config");
        let acceptor = Arc::new(tokio_rustls::TlsAcceptor::from(Arc::new(cfg)));

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind tunnel");
        let actual_addr = listener.local_addr().unwrap();
        let registry = Arc::new(NodeRegistry::new());
        let tunnel_store = store.clone();
        tokio::spawn(async move {
            let _ = tunnel::serve(listener, acceptor, registry, tunnel_store).await;
        });

        server.tunnel_addr = Some(actual_addr);
        server
    }

    /// Slug of the default Org auto-created by `spawn`. Tests POSTing Projects use
    /// `/orgs/{DEFAULT_ORG_SLUG}/projects` and pass `{"slug": "...", "name": "..."}`.
    pub const DEFAULT_ORG_SLUG: &'static str = "test";

    /// Get base URL for the REST API.
    pub fn base_url(&self) -> String {
        format!("http://{}/v1", self.addr)
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
