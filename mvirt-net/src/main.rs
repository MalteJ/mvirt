use std::sync::Arc;

use clap::Parser;
use tonic::transport::Server;
use tracing::info;
use tracing_subscriber::EnvFilter;

use mvirt_net::audit::create_audit_logger;
use mvirt_net::grpc::NetServiceImpl;
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
    #[arg(long, default_value = "/run/mvirt-net")]
    socket_dir: String,

    /// Directory for metadata storage (SQLite)
    #[arg(long, default_value = "/var/lib/mvirt-net")]
    metadata_dir: String,

    /// mvirt-log endpoint for audit logging
    #[arg(long, default_value = "http://[::1]:50052")]
    log_endpoint: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("mvirt_net=info".parse()?))
        .init();

    let args = Args::parse();

    info!("Initializing mvirt-net");

    // Ensure directories exist
    tokio::fs::create_dir_all(&args.socket_dir).await?;
    tokio::fs::create_dir_all(&args.metadata_dir).await?;

    // Initialize store
    let store = Arc::new(Store::new(&args.metadata_dir).await?);

    // Initialize audit logger (connects lazily to mvirt-log)
    let audit = create_audit_logger(&args.log_endpoint);

    // Create gRPC service
    let service = NetServiceImpl::new(args.socket_dir.clone(), store, audit);

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
