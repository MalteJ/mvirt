use clap::Parser;
use mraft::{JoinToken, NodeConfig, RaftNode, StorageBackend};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::signal;
use tokio::sync::{RwLock, broadcast};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

use tonic::transport::Server as TonicServer;

use mvirt_api::audit::create_audit_logger;
use mvirt_api::rest::{AppState, create_router};
use mvirt_api::store::{Event, RaftStore};
use mvirt_api::{
    ApiAuditLogger, ApiState, Command, DataStore, NodeId, NodeServiceImpl, NodeServiceServer,
    Response,
};

#[derive(Parser)]
#[command(name = "mvirt-api")]
#[command(about = "mvirt API Server - Raft-based distributed control plane")]
struct Args {
    /// Node ID for this instance (auto-detected from token when using --join)
    #[arg(long)]
    node_id: Option<NodeId>,

    /// Node name for display
    #[arg(long)]
    name: Option<String>,

    /// Listen address for Raft gRPC (node-to-node)
    #[arg(long, default_value = "127.0.0.1:6001")]
    raft_listen: String,

    /// Listen address for REST API (client)
    #[arg(short, long, default_value = "[::]:8080")]
    listen: String,

    /// Listen address for gRPC API (mvirt-node agents)
    #[arg(long, default_value = "[::1]:50056")]
    grpc_listen: String,

    /// Peer nodes (format: id:addr, can be repeated)
    #[arg(long, value_parser = parse_peer)]
    peer: Vec<(NodeId, String)>,

    /// Data directory for persistent storage
    #[arg(short, long, default_value = "/var/lib/mvirt/cp")]
    data_dir: PathBuf,

    /// Bootstrap a new cluster (only for the first node)
    #[arg(long)]
    bootstrap: bool,

    /// Run in development mode (single-node, ephemeral storage)
    #[arg(long)]
    dev: bool,

    /// Log service endpoint for audit logging
    #[arg(long, default_value = "http://[::1]:50052")]
    log_endpoint: String,

    /// Join an existing cluster (leader's Raft gRPC address)
    #[arg(long)]
    join: Option<String>,

    /// Join token (required with --join)
    #[arg(long)]
    token: Option<String>,
}

fn parse_peer(s: &str) -> Result<(NodeId, String), String> {
    let (id_str, addr) = s
        .split_once(':')
        .ok_or("Expected format: id:addr".to_string())?;
    let id: NodeId = id_str.parse().map_err(|_| "Invalid node ID".to_string())?;
    Ok((id, addr.to_string()))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("mvirt_api=info".parse()?))
        .init();

    let args = Args::parse();

    // Resolve node_id: from --node-id, or extract from token when joining
    let node_id: NodeId = if let Some(id) = args.node_id {
        id
    } else if let Some(token) = &args.token {
        JoinToken::peek_node_id(token).ok_or("Invalid token format: cannot extract node ID")?
    } else {
        return Err("--node-id is required (or use --join with --token)".into());
    };

    // Default name if not provided
    let name = args.name.unwrap_or_else(|| format!("node-{}", node_id));

    // Validate arguments
    if args.join.is_none() && !args.dev && !args.bootstrap {
        warn!(
            "Neither --bootstrap, --dev, nor --join specified. Node will wait for cluster membership."
        );
    }

    // Create data directory
    if !args.dev {
        tokio::fs::create_dir_all(&args.data_dir).await?;
    }

    info!(
        "Starting mvirt-api node {} ({}) - Raft: {}, REST: {}, gRPC: {}",
        node_id, name, args.raft_listen, args.listen, args.grpc_listen
    );

    // Build peers map
    let peers: BTreeMap<NodeId, String> = args.peer.into_iter().collect();

    // Create node configuration
    let config = NodeConfig {
        id: node_id,
        listen_addr: args.raft_listen.clone(),
        peers,
        storage: if args.dev {
            StorageBackend::Memory
        } else {
            StorageBackend::Persistent {
                path: args.data_dir.join("raft.db"),
            }
        },
        raft_config: None,
    };

    // Create and start the Raft node
    let mut node: RaftNode<Command, Response, ApiState> = RaftNode::new(config).await?;
    node.start().await?;

    // Handle cluster membership
    if let Some(leader_addr) = &args.join {
        // Join existing cluster
        let token = args
            .token
            .as_ref()
            .ok_or("--token is required when using --join")?;

        info!("Joining cluster via {}", leader_addr);
        node.join_cluster(leader_addr, token)
            .await
            .map_err(|e| format!("Failed to join cluster: {}", e))?;
        info!("Successfully joined cluster");
    } else if args.bootstrap || args.dev {
        // Bootstrap new cluster
        info!("Bootstrapping new cluster");
        node.generate_cluster_secret();
        node.initialize_cluster().await?;
    }

    // Wait for leader election
    info!("Waiting for leader election...");
    if let Some(leader) = node.wait_for_leader(Duration::from_secs(10)).await {
        info!("Leader elected: node {}", leader);
    } else {
        warn!("No leader elected within timeout");
    }

    // Create audit logger
    let audit = if args.dev {
        Arc::new(ApiAuditLogger::new_noop())
    } else {
        create_audit_logger(&args.log_endpoint)
    };

    // Create event channel for state machine events
    // IMPORTANT: Create before wrapping node so we can wire up the event sink
    let (event_tx, _) = broadcast::channel::<Event>(256);

    // Wire up event sink to mraft's Store (events emitted during apply())
    node.set_event_sink(event_tx.clone());

    // Wrap RaftNode with RaftStore for DataStore interface
    let raft_node = Arc::new(RwLock::new(node));
    let store = Arc::new(RaftStore::new(raft_node.clone(), event_tx, node_id));

    // Create REST API state
    let app_state = Arc::new(AppState {
        store: store.clone(),
        audit: audit.clone(),
        node_id,
        log_endpoint: args.log_endpoint.clone(),
    });

    // Create REST router
    let router = create_router(app_state.clone());

    // Create gRPC NodeService (wrapped in Arc for sharing with event listener)
    let node_service = Arc::new(NodeServiceImpl::new(store.clone(), audit));

    // Start event listener to forward specs to nodes
    let event_rx = store.subscribe();
    Arc::clone(&node_service).start_event_listener(event_rx);

    // Create gRPC server with the Arc-wrapped service
    let grpc_service = NodeServiceServer::from_arc(node_service);

    // Start REST server
    let listener = tokio::net::TcpListener::bind(&args.listen).await?;
    info!("REST API listening on {}", args.listen);

    // Parse gRPC address
    let grpc_addr = args.grpc_listen.parse()?;

    // Create shutdown signal channel
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
    let mut shutdown_rx2 = shutdown_tx.subscribe();

    // Spawn REST server
    let rest_handle = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async move {
                shutdown_rx.changed().await.ok();
            })
            .await
    });

    // Spawn gRPC server
    let grpc_handle = tokio::spawn(async move {
        info!("gRPC API listening on {}", grpc_addr);
        TonicServer::builder()
            .add_service(grpc_service)
            .serve_with_shutdown(grpc_addr, async move {
                shutdown_rx2.changed().await.ok();
            })
            .await
    });

    // Wait for shutdown signal
    let ctrl_c = signal::ctrl_c();
    let mut sigterm = signal::unix::signal(signal::unix::SignalKind::terminate())
        .expect("Failed to install SIGTERM handler");

    tokio::select! {
        _ = ctrl_c => info!("Received SIGINT"),
        _ = sigterm.recv() => info!("Received SIGTERM"),
    }

    // Signal shutdown to both servers
    let _ = shutdown_tx.send(true);

    // Wait for servers to finish
    let _ = rest_handle.await;
    let _ = grpc_handle.await;

    // Shutdown Raft node
    info!("Shutting down Raft node...");
    let mut node = raft_node.write().await;
    node.shutdown().await?;

    info!("Shutdown complete");
    Ok(())
}
