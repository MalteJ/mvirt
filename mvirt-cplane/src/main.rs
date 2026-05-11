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

use mvirt_cplane::JwtValidator;
use mvirt_cplane::audit::create_audit_logger;
use mvirt_cplane::reconciler::Controller;
use mvirt_cplane::rest::{AppState, create_router};
use mvirt_cplane::store::{Event, RaftStore};
use mvirt_cplane::{
    ApiAuditLogger, ApiState, Command, DataStore, NodeId, NodeRegistry, Response, ca, tunnel,
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

    /// Hostnames + IPs the tunnel server cert covers. Repeatable.
    /// Defaults to localhost + 127.0.0.1 + ::1 so dev nodes Just Work.
    #[arg(long = "tunnel-san", value_name = "DNS-OR-IP")]
    tunnel_san: Vec<String>,

    /// Peer nodes (format: id:addr, can be repeated)
    #[arg(long, value_parser = parse_peer)]
    peer: Vec<(NodeId, String)>,

    /// Data directory for persistent storage. Defaults to
    /// `/var/lib/mvirt/cplane` in production and `/tmp/mvirt-cplane-dev`
    /// when `--dev` is set.
    #[arg(short, long)]
    data_dir: Option<PathBuf>,

    /// Bootstrap a new cluster (only for the first node)
    #[arg(long)]
    bootstrap: bool,

    /// Run in development mode (single-node, auto-bootstrap, in-memory by
    /// default — see also `--dev-persist`).
    #[arg(long)]
    dev: bool,

    /// In `--dev` mode, persist state to disk under `--data-dir`
    /// (default `/tmp/mvirt-cplane-dev`) so restarts don't wipe orgs,
    /// clusters, accounts, and the internal CA. Ignored without `--dev`.
    #[arg(long)]
    dev_persist: bool,

    /// Delete the data directory before starting. Combine with
    /// `--dev --dev-persist` to start from a known-empty state. Safety:
    /// only fires when the path looks like a dev dir (contains "dev" or
    /// "tmp") OR `--dev` is set — never wipes production data dirs.
    #[arg(long)]
    reset: bool,

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

    // Effective data dir:
    //  - explicit `--data-dir` always wins
    //  - else `--dev` defaults to `/tmp/mvirt-cplane-dev` (writable, easy to wipe)
    //  - else production default `/var/lib/mvirt/cplane`
    let data_dir = args.data_dir.unwrap_or_else(|| {
        if args.dev {
            PathBuf::from("/tmp/mvirt-cplane-dev")
        } else {
            PathBuf::from("/var/lib/mvirt/cplane")
        }
    });

    // --reset wipes the data dir before opening it. Guard against
    // accidentally nuking a production path: only allow when --dev is set
    // OR the path obviously looks like a dev/tmp location.
    if args.reset {
        let path_str = data_dir.to_string_lossy();
        let looks_dev = path_str.contains("/tmp/") || path_str.contains("dev");
        if !(args.dev || looks_dev) {
            return Err(format!(
                "--reset refused on non-dev data dir {}; pass --dev or use a path containing 'tmp' or 'dev'",
                data_dir.display()
            )
            .into());
        }
        if data_dir.exists() {
            info!(path = %data_dir.display(), "--reset: removing data dir");
            tokio::fs::remove_dir_all(&data_dir).await?;
        }
    }

    // Both --dev (in-memory) and --dev-persist (on-disk) skip the explicit
    // create_dir_all dance for prod; ApiState::open / RaftNode handle it.
    let persistent = !args.dev || args.dev_persist;
    if persistent {
        tokio::fs::create_dir_all(&data_dir).await?;
    }

    info!(
        "Starting mvirt-cplane node {} ({}) - Raft: {}, REST: {}, tunnel: {}, storage: {}",
        node_id,
        name,
        args.raft_listen,
        args.listen,
        args.tunnel_listen,
        if persistent {
            format!("persistent at {}", data_dir.display())
        } else {
            "in-memory".into()
        }
    );

    let peers: BTreeMap<NodeId, String> = args.peer.into_iter().collect();

    let config = NodeConfig {
        id: node_id,
        listen_addr: args.raft_listen.clone(),
        peers,
        storage: if persistent {
            StorageBackend::Persistent {
                path: data_dir.join("raft.db"),
            }
        } else {
            StorageBackend::Memory
        },
        // Snapshot every 1000 applied commands (see ADR-0002 §1).
        // openraft default is 100; we go higher because a redb-backed snapshot
        // is cheap and the only cost of waiting is log size + replay window.
        raft_config: Some(config_with_snapshot_threshold(1000)),
    };

    let api_state = if persistent {
        ApiState::open(&data_dir.join("state.redb"))?
    } else {
        ApiState::default()
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
        // With persistent storage, a second start finds the cluster already
        // initialized (membership log entry present). openraft documents
        // that re-calling `initialize` returns InitializeError::NotAllowed
        // and that it's safe to ignore. mraft surfaces the raft instance
        // via `node.raft()` — check up-front so we don't log a misleading
        // "Bootstrapping new cluster" on every dev-persist restart.
        if node.raft().is_initialized().await? {
            info!("Cluster already initialized — resuming from persistent storage");
        } else {
            info!("Bootstrapping new cluster");
            node.generate_cluster_secret();
            node.initialize_cluster().await?;
        }
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

    let jwt_validator = JwtValidator::from_env().await?;
    let initial_admin_email = std::env::var("MVIRT_INITIAL_ADMIN_EMAIL").ok();
    if let Some(email) = initial_admin_email.as_deref() {
        info!(
            email = %email,
            "initial-admin bootstrap armed — first OIDC login with this email + no existing platform-admin → granted Platform/PlatformAdmin"
        );
    }
    let app_state = Arc::new(AppState {
        store: store.clone(),
        audit: audit.clone(),
        node_id,
        log_endpoint: args.log_endpoint.clone(),
        jwt_validator,
        initial_admin_email,
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

    // Bootstrap PKI: ensure the internal CA exists, generate a server cert
    // for the tunnel listener. Both are raft-replicated, so on join we end
    // up with whatever the cluster already has.
    ca::install_default_crypto_provider();
    let san = if args.tunnel_san.is_empty() {
        vec![
            "localhost".to_string(),
            "127.0.0.1".to_string(),
            "::1".to_string(),
        ]
    } else {
        args.tunnel_san.clone()
    };
    let pki_store: Arc<dyn DataStore> = store.clone();
    let _ = pki_store.ensure_internal_ca("mvirt").await.map_err(|e| {
        warn!(error = %e, "ensure_internal_ca failed; tunnel will be retried");
        e
    });
    let server_cert = match pki_store.get_server_cert().await? {
        Some(c) => c,
        None => pki_store.rotate_server_cert(san).await?,
    };
    let internal_ca = pki_store.ensure_internal_ca("mvirt").await?;
    let server_config = ca::build_server_config(
        &internal_ca.ca_cert_pem,
        &server_cert.cert_pem,
        &server_cert.key_pem,
    )?;
    let acceptor = Arc::new(tokio_rustls::TlsAcceptor::from(Arc::new(server_config)));

    let tunnel_handle = {
        let registry = registry.clone();
        let tunnel_store: Arc<dyn DataStore> = store.clone();
        tokio::spawn(
            async move { tunnel::listen(tunnel_addr, acceptor, registry, tunnel_store).await },
        )
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
