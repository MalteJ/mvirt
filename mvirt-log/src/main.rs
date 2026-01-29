use clap::Parser;
use mraft::{NodeConfig, NodeId, RaftNode, StorageBackend};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, RwLock};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{transport::Server, Request, Response, Status};
use tracing::{info, warn};

use mvirt_log::command::{LogCommand, LogCommandResponse};
use mvirt_log::distributed::DistributedLogStore;
use mvirt_log::proto::{GetVersionRequest, VersionInfo};
use mvirt_log::storage::{init_log_manager, LogManager, LogStateMachine};
use mvirt_log::{LogEntry, LogRequest, LogResponse, LogService, LogServiceServer, QueryRequest};

mod batcher;
use batcher::Batcher;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Listen address (e.g., [::1]:50052)
    #[arg(short, long, default_value = "[::1]:50052")]
    listen: String,

    /// Data directory for logs
    #[arg(short, long, default_value = "/var/lib/mvirt/log")]
    data_dir: PathBuf,

    /// Node ID for this instance
    #[arg(long)]
    node_id: Option<NodeId>,

    /// Listen address for Raft gRPC (node-to-node)
    #[arg(long, default_value = "127.0.0.1:7001")]
    raft_listen: String,

    /// Peer nodes (format: id:addr, can be repeated)
    #[arg(long, value_parser = parse_peer)]
    peer: Vec<(NodeId, String)>,

    /// Bootstrap a new cluster (only for the first node)
    #[arg(long)]
    bootstrap: bool,

    /// Run in development mode (single-node, ephemeral storage)
    #[arg(long)]
    dev: bool,
}

fn parse_peer(s: &str) -> Result<(NodeId, String), String> {
    let (id_str, addr) = s
        .split_once(':')
        .ok_or("Expected format: id:addr".to_string())?;
    let id: NodeId = id_str.parse().map_err(|_| "Invalid node ID".to_string())?;
    Ok((id, addr.to_string()))
}

pub struct MyLogService {
    store: Arc<DistributedLogStore>,
    batcher: Arc<Batcher>,
}

#[tonic::async_trait]
impl LogService for MyLogService {
    async fn get_version(
        &self,
        _request: Request<GetVersionRequest>,
    ) -> Result<Response<VersionInfo>, Status> {
        Ok(Response::new(VersionInfo {
            version: env!("CARGO_PKG_VERSION").to_string(),
        }))
    }

    async fn log(&self, request: Request<LogRequest>) -> Result<Response<LogResponse>, Status> {
        let req = request.into_inner();
        let entry = req
            .entry
            .ok_or_else(|| Status::invalid_argument("Missing entry"))?;

        self.batcher.submit(entry);
        Ok(Response::new(LogResponse { id: String::new() }))
    }

    type QueryStream = ReceiverStream<Result<LogEntry, Status>>;

    async fn query(
        &self,
        request: Request<QueryRequest>,
    ) -> Result<Response<Self::QueryStream>, Status> {
        let req = request.into_inner();
        let (tx, rx) = mpsc::channel(64);
        let store = self.store.clone();

        tokio::spawn(async move {
            let limit = if req.limit == 0 {
                100
            } else {
                req.limit as usize
            };

            let start = req.start_time_ns;
            let end = req.end_time_ns;

            let obj = match req.object_id {
                Some(o) if o.is_empty() => None,
                Some(o) => Some(o),
                None => None,
            };

            match store.query(obj, start, end, limit).await {
                Ok(logs) => {
                    for log in logs {
                        if tx.send(Ok(log)).await.is_err() {
                            break;
                        }
                    }
                }
                Err(e) => {
                    let _ = tx
                        .send(Err(Status::internal(format!("Query failed: {}", e))))
                        .await;
                }
            }
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    let node_id = args.node_id.unwrap_or(1);

    std::fs::create_dir_all(&args.data_dir)?;

    info!("Opening log storage at {:?}", args.data_dir);
    let manager = Arc::new(LogManager::new(&args.data_dir)?);
    init_log_manager(manager);

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
        raft_config: None,
    };

    let mut node: RaftNode<LogCommand, LogCommandResponse, LogStateMachine> =
        RaftNode::new(config).await?;
    node.start().await?;

    if args.bootstrap || args.dev {
        info!("Bootstrapping new cluster");
        node.generate_cluster_secret();
        node.initialize_cluster().await?;
    }

    info!("Waiting for leader election...");
    if let Some(leader) = node.wait_for_leader(Duration::from_secs(10)).await {
        info!("Leader elected: node {}", leader);
    } else {
        warn!("No leader elected within timeout");
    }

    let raft_node = Arc::new(RwLock::new(node));
    let store = Arc::new(DistributedLogStore::new(raft_node));
    let batcher = Arc::new(Batcher::new(store.clone()));

    let addr = args.listen.parse()?;
    let service = MyLogService { store, batcher };

    info!("mvirt-log listening on {}", addr);

    Server::builder()
        .add_service(LogServiceServer::new(service))
        .serve(addr)
        .await?;

    Ok(())
}
