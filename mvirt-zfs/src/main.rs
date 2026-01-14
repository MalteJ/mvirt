use std::sync::Arc;

use clap::Parser;
use tonic::transport::Server;
use tracing::info;
use tracing_subscriber::EnvFilter;

mod grpc;
mod import;
mod store;
mod zfs;

pub mod proto {
    tonic::include_proto!("mvirt.zfs");
}

use grpc::ZfsServiceImpl;
use import::ImportManager;
use proto::zfs_service_server::ZfsServiceServer;
use store::Store;
use zfs::ZfsManager;

#[derive(Parser)]
#[command(name = "mvirt-zfs")]
#[command(about = "mvirt ZFS volume manager daemon")]
struct Args {
    /// ZFS pool name
    #[arg(short, long, default_value = "vmpool")]
    pool: String,

    /// gRPC listen address
    #[arg(short, long, default_value = "[::1]:50052")]
    listen: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("mvirt_zfs=info".parse()?))
        .init();

    let args = Args::parse();

    info!(pool = %args.pool, "Initializing mvirt-zfs");

    // Get pool mountpoint for metadata storage
    // For now, assume the pool is mounted at /{pool_name}
    let pool_mountpoint = format!("/{}", args.pool);
    let metadata_dir = format!("{}/.mvirt-zfs", pool_mountpoint);

    // Ensure metadata directory exists
    tokio::fs::create_dir_all(&metadata_dir).await?;

    // Initialize store
    let store = Arc::new(Store::new(&metadata_dir).await?);

    // Initialize ZFS manager
    let zfs_manager = Arc::new(ZfsManager::new(args.pool.clone()));

    // Initialize import manager
    let import_manager = Arc::new(ImportManager::new(
        args.pool.clone(),
        pool_mountpoint.clone(),
        Arc::clone(&store),
        Arc::clone(&zfs_manager),
    ));

    // Create gRPC service
    let service = ZfsServiceImpl::new(args.pool.clone(), store, zfs_manager, import_manager);

    let addr = args.listen.parse()?;
    info!(addr = %addr, pool = %args.pool, "Starting gRPC server");

    Server::builder()
        .add_service(ZfsServiceServer::new(service))
        .serve(addr)
        .await?;

    Ok(())
}
