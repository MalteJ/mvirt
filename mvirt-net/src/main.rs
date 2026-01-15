use std::sync::Arc;

use clap::Parser;
use tonic::transport::Server;
use tracing::{debug, info};
use tracing_subscriber::EnvFilter;

use mvirt_net::audit::create_audit_logger;
use mvirt_net::grpc::{NetServiceImpl, reconcile_routes};
use mvirt_net::proto::net_service_server::NetServiceServer;
use mvirt_net::store::Store;

#[derive(Parser)]
#[command(name = "mvirt-net")]
#[command(about = "mvirt virtual network daemon")]
struct Args {
    /// gRPC listen address
    #[arg(short, long, default_value = "[::1]:50054")]
    listen: String,

    /// Directory for vhost-user sockets
    #[arg(long, default_value = "/run/mvirt/net")]
    socket_dir: String,

    /// Directory for metadata storage (SQLite)
    #[arg(long, default_value = "/var/lib/mvirt/net")]
    metadata_dir: String,

    /// mvirt-log endpoint for audit logging
    #[arg(long, default_value = "http://[::1]:50052")]
    log_endpoint: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Use RUST_LOG if set, otherwise default to info for mvirt_net
    // Always suppress noisy h2 codec logs
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("mvirt_net=info"));
    let env_filter = env_filter.add_directive("h2::codec=info".parse().unwrap());

    tracing_subscriber::fmt().with_env_filter(env_filter).init();

    let args = Args::parse();

    info!("Initializing mvirt-net");
    debug!("DEBUG logging enabled");

    // Ensure directories exist
    tokio::fs::create_dir_all(&args.socket_dir).await?;
    tokio::fs::create_dir_all(&args.metadata_dir).await?;

    // Initialize store
    let store = Arc::new(Store::new(&args.metadata_dir).await?);

    // Initialize audit logger (connects lazily to mvirt-log)
    let audit = create_audit_logger(&args.log_endpoint);

    // Create gRPC service (also creates TUN device)
    let service = NetServiceImpl::new(args.socket_dir.clone(), store.clone(), audit)
        .map_err(|e| format!("Failed to create service: {e}"))?;

    // Recover workers for existing NICs
    service.recover_nics().await;

    // Initial route reconciliation
    info!("Reconciling routes for public networks");
    reconcile_routes(&store).await;

    // Spawn background route reconciliation loop (every 10 seconds)
    let store_for_reconcile = store.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(10));
        loop {
            interval.tick().await;
            reconcile_routes(&store_for_reconcile).await;
        }
    });

    let addr = args.listen.parse()?;
    info!(
        addr = %addr,
        socket_dir = %args.socket_dir,
        metadata_dir = %args.metadata_dir,
        "Starting gRPC server"
    );

    Server::builder()
        .add_service(NetServiceServer::new(service))
        .serve(addr)
        .await?;

    Ok(())
}
