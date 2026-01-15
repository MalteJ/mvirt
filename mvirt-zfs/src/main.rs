use std::sync::Arc;

use clap::Parser;
use tokio::signal;
use tonic::transport::Server;
use tracing::info;
use tracing_subscriber::EnvFilter;

use mvirt_zfs::audit::create_audit_logger;
use mvirt_zfs::grpc::ZfsServiceImpl;
use mvirt_zfs::import::ImportManager;
use mvirt_zfs::proto::zfs_service_server::ZfsServiceServer;
use mvirt_zfs::store::Store;
use mvirt_zfs::zfs::ZfsManager;

#[derive(Parser)]
#[command(name = "mvirt-zfs")]
#[command(about = "mvirt ZFS volume manager daemon")]
struct Args {
    /// ZFS pool name
    #[arg(short, long, default_value = "mvirt")]
    pool: String,

    /// gRPC listen address
    #[arg(short, long, default_value = "[::1]:50053")]
    listen: String,

    /// mvirt-log endpoint for audit logging
    #[arg(long, default_value = "http://[::1]:50052")]
    log_endpoint: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("mvirt_zfs=info".parse()?))
        .init();

    let args = Args::parse();

    info!(pool = %args.pool, "Initializing mvirt-zfs");

    // State directory following FHS
    let state_dir = "/var/lib/mvirt/zfs";
    tokio::fs::create_dir_all(state_dir).await?;

    // Initialize store
    let store = Arc::new(Store::new(state_dir).await?);

    // Initialize ZFS manager
    let zfs_manager = Arc::new(ZfsManager::new(args.pool.clone()));

    // Ensure pool structure exists (templates/, volumes/, .tmp/)
    let tmp_dir = format!("{}/tmp", state_dir);
    zfs_manager.ensure_pool_structure(&tmp_dir).await?;

    // Initialize audit logger (connects lazily to mvirt-log)
    let audit = create_audit_logger(&args.log_endpoint);

    // Initialize import manager
    let import_manager = Arc::new(ImportManager::new(
        args.pool.clone(),
        state_dir.to_string(),
        Arc::clone(&store),
        Arc::clone(&zfs_manager),
        Arc::clone(&audit),
    ));

    // Create gRPC service
    let service = ZfsServiceImpl::new(store, Arc::clone(&zfs_manager), import_manager, audit);

    let addr = args.listen.parse()?;
    info!(addr = %addr, pool = %args.pool, "Starting gRPC server");

    // Run server with graceful shutdown on SIGTERM/SIGINT
    Server::builder()
        .add_service(ZfsServiceServer::new(service))
        .serve_with_shutdown(addr, async {
            let ctrl_c = signal::ctrl_c();
            let mut sigterm = signal::unix::signal(signal::unix::SignalKind::terminate())
                .expect("Failed to install SIGTERM handler");

            tokio::select! {
                _ = ctrl_c => info!("Received SIGINT"),
                _ = sigterm.recv() => info!("Received SIGTERM"),
            }
        })
        .await?;

    // Cleanup: destroy temp dataset
    zfs_manager.destroy_tmp_dataset().await;

    info!("Shutdown complete");
    Ok(())
}
