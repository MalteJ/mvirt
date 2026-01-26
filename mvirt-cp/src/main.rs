use clap::Parser;
use mraft::{NodeConfig, RaftNode, StorageBackend};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::signal;
use tokio::sync::RwLock;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

use mvirt_cp::audit::create_audit_logger;
use mvirt_cp::rest::{AppState, create_router};
use mvirt_cp::{Command, CpAuditLogger, CpState, NodeId, Response};

#[derive(Parser)]
#[command(name = "mvirt-cp")]
#[command(about = "mvirt Cluster Control Plane - Raft-based distributed state management")]
struct Args {
    /// Node ID for this instance
    #[arg(long, default_value = "1")]
    node_id: NodeId,

    /// Node name for display
    #[arg(long, default_value = "node1")]
    name: String,

    /// Listen address for Raft gRPC (node-to-node)
    #[arg(long, default_value = "127.0.0.1:6001")]
    raft_listen: String,

    /// Listen address for REST API (client)
    #[arg(short, long, default_value = "[::1]:50055")]
    listen: String,

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
        .with_env_filter(EnvFilter::from_default_env().add_directive("mvirt_cp=info".parse()?))
        .init();

    let args = Args::parse();

    // Validate arguments
    if !args.dev && !args.bootstrap {
        warn!("Neither --bootstrap nor --dev specified. Node will wait for cluster membership.");
    }

    // Create data directory
    if !args.dev {
        tokio::fs::create_dir_all(&args.data_dir).await?;
    }

    info!(
        "Starting mvirt-cp node {} ({}) - Raft: {}, REST: {}",
        args.node_id, args.name, args.raft_listen, args.listen
    );

    // Build peers map
    let peers: BTreeMap<NodeId, String> = args.peer.into_iter().collect();

    // Create node configuration
    let config = NodeConfig {
        id: args.node_id,
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
    let mut node: RaftNode<Command, Response, CpState> = RaftNode::new(config).await?;
    node.start().await?;

    // Bootstrap or wait
    if args.bootstrap || args.dev {
        info!("Bootstrapping new cluster");
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
        Arc::new(CpAuditLogger::new_noop())
    } else {
        create_audit_logger(&args.log_endpoint)
    };

    // Create REST API state
    let app_state = Arc::new(AppState {
        node: Arc::new(RwLock::new(node)),
        audit,
        node_id: args.node_id,
    });

    // Create REST router
    let router = create_router(app_state.clone());

    // Start REST server
    let listener = tokio::net::TcpListener::bind(&args.listen).await?;
    info!("REST API listening on {}", args.listen);

    // Run server with graceful shutdown
    axum::serve(listener, router)
        .with_graceful_shutdown(async {
            let ctrl_c = signal::ctrl_c();
            let mut sigterm = signal::unix::signal(signal::unix::SignalKind::terminate())
                .expect("Failed to install SIGTERM handler");

            tokio::select! {
                _ = ctrl_c => info!("Received SIGINT"),
                _ = sigterm.recv() => info!("Received SIGTERM"),
            }
        })
        .await?;

    // Shutdown Raft node
    info!("Shutting down Raft node...");
    let mut node = app_state.node.write().await;
    node.shutdown().await?;

    info!("Shutdown complete");
    Ok(())
}
