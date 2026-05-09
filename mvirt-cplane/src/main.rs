use clap::Parser;
use mraft::{JoinToken, NodeConfig, RaftNode, StorageBackend, config_with_snapshot_threshold};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::signal;
use tokio::sync::{RwLock, broadcast};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

use mvirt_cplane::audit::create_audit_logger;
use mvirt_cplane::reconciler::Controller;
use mvirt_cplane::rest::{AppState, create_router};
use mvirt_cplane::store::{Event, RaftStore};
use mvirt_cplane::{
    ApiAuditLogger, ApiState, Command, DataStore, NodeId, NodeRegistry, Response, tunnel,
};

#[derive(Parser)]
#[command(name = "mvirt-cplane")]
#[command(
    about = "mvirt control plane - Raft consensus, REST API, scheduler, reconciler, and node tunnel acceptor"
)]
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
    #[arg(short, long, default_value = "[::1]:8080")]
    listen: String,

    /// Listen address for the reverse-tunnel (mvirt-node agents dial here)
    #[arg(long, default_value = "[::]:50056")]
    tunnel_listen: String,

    /// Peer nodes (format: id:addr, can be repeated)
    #[arg(long, value_parser = parse_peer)]
    peer: Vec<(NodeId, String)>,

    /// Data directory for persistent storage
    #[arg(short, long, default_value = "/var/lib/mvirt/cplane")]
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
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("mvirt_cplane=info".parse()?))
        .init();

    let args = Args::parse();

    let node_id: NodeId = if let Some(id) = args.node_id {
        id
    } else if let Some(token) = &args.token {
        JoinToken::peek_node_id(token).ok_or("Invalid token format: cannot extract node ID")?
    } else {
        return Err("--node-id is required (or use --join with --token)".into());
    };

    let name = args.name.unwrap_or_else(|| format!("node-{}", node_id));

    if args.join.is_none() && !args.dev && !args.bootstrap {
        warn!(
            "Neither --bootstrap, --dev, nor --join specified. Node will wait for cluster membership."
        );
    }

    if !args.dev {
        tokio::fs::create_dir_all(&args.data_dir).await?;
    }

    info!(
        "Starting mvirt-cplane node {} ({}) - Raft: {}, REST: {}, tunnel: {}",
        node_id, name, args.raft_listen, args.listen, args.tunnel_listen
    );

    let peers: BTreeMap<NodeId, String> = args.peer.into_iter().collect();

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
        // Snapshot every 1000 applied commands (see ADR-0002 §1).
        // openraft default is 100; we go higher because a redb-backed snapshot
        // is cheap and the only cost of waiting is log size + replay window.
        raft_config: Some(config_with_snapshot_threshold(1000)),
    };

    let api_state = if args.dev {
        ApiState::default()
    } else {
        ApiState::open(&args.data_dir.join("state.redb"))?
    };

    let mut node: RaftNode<Command, Response, ApiState> = RaftNode::new(config, api_state).await?;
    node.start().await?;

    if let Some(leader_addr) = &args.join {
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
        info!("Bootstrapping new cluster");
        node.generate_cluster_secret();
        node.initialize_cluster().await?;
    }

    info!("Waiting for leader election...");
    if let Some(leader) = node.wait_for_leader(Duration::from_secs(10)).await {
        info!("Leader elected: node {}", leader);
        if leader == node_id {
            let flag = node.is_leader_flag();
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    } else {
        warn!("No leader elected within timeout");
    }

    let audit = if args.dev {
        Arc::new(ApiAuditLogger::new_noop())
    } else {
        create_audit_logger(&args.log_endpoint)
    };

    let (event_tx, _) = broadcast::channel::<Event>(256);
    node.set_event_sink(event_tx.clone());

    let raft_node = Arc::new(RwLock::new(node));
    let store = Arc::new(RaftStore::new(raft_node.clone(), event_tx, node_id));

    let app_state = Arc::new(AppState {
        store: store.clone(),
        audit: audit.clone(),
        node_id,
        log_endpoint: args.log_endpoint.clone(),
    });

    let router = create_router(app_state.clone());

    // Reverse-tunnel listener: nodes dial in, we get a Channel per connection.
    let registry = Arc::new(NodeRegistry::new());
    let tunnel_addr: std::net::SocketAddr = args.tunnel_listen.parse()?;

    // Reconciler controller: subscribes to raft events + periodic resync,
    // dispatches per-resource RPCs against the daemon channels in the registry.
    Controller::new(store.clone(), registry.clone(), audit.clone()).spawn(store.subscribe());

    let listener = tokio::net::TcpListener::bind(&args.listen).await?;
    info!("REST API listening on {}", args.listen);

    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);

    let rest_handle = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async move {
                shutdown_rx.changed().await.ok();
            })
            .await
    });

    let tunnel_handle = {
        let registry = registry.clone();
        tokio::spawn(async move { tunnel::listen(tunnel_addr, registry).await })
    };

    let ctrl_c = signal::ctrl_c();
    let mut sigterm = signal::unix::signal(signal::unix::SignalKind::terminate())
        .expect("Failed to install SIGTERM handler");

    tokio::select! {
        _ = ctrl_c => info!("Received SIGINT"),
        _ = sigterm.recv() => info!("Received SIGTERM"),
    }

    let _ = shutdown_tx.send(true);

    let _ = rest_handle.await;
    tunnel_handle.abort();

    info!("Shutting down Raft node...");
    let mut node = raft_node.write().await;
    node.shutdown().await?;

    info!("Shutdown complete");
    Ok(())
}
