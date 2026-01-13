use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use tonic::transport::Server;
use tracing::info;
use tracing_subscriber::EnvFilter;

mod grpc;
mod hypervisor;
mod store;

pub mod proto {
    tonic::include_proto!("mvirt");
}

use grpc::VmServiceImpl;
use hypervisor::Hypervisor;
use proto::vm_service_server::VmServiceServer;
use store::VmStore;

#[derive(Parser)]
#[command(name = "mvirt-vmm")]
#[command(about = "mvirt Virtual Machine Manager daemon")]
struct Args {
    /// Data directory for SQLite database and VM runtime files
    #[arg(short, long, default_value = "/var/lib/mvirt")]
    data_dir: PathBuf,

    /// gRPC listen address
    #[arg(short, long, default_value = "[::1]:50051")]
    listen: String,

    /// Bridge to attach VM TAP devices to (created if not exists)
    #[arg(short, long, default_value = "br0")]
    bridge: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("mvirt_vmm=info".parse()?))
        .init();

    let args = Args::parse();

    // Ensure data directory exists
    tokio::fs::create_dir_all(&args.data_dir).await?;

    info!(data_dir = %args.data_dir.display(), "Initializing mvirt-vmm");

    // Initialize store
    let store = Arc::new(VmStore::new(&args.data_dir).await?);

    // Initialize hypervisor
    let hypervisor =
        Arc::new(Hypervisor::new(args.data_dir.clone(), store.clone(), args.bridge).await?);

    // Recover VMs from previous run
    hypervisor.recover_vms().await?;

    // Spawn process watcher
    let _watcher_shutdown = hypervisor.clone().spawn_watcher();

    // Create gRPC service
    let service = VmServiceImpl::new(store, hypervisor);

    let addr = args.listen.parse()?;
    info!(addr = %addr, "Starting gRPC server");

    Server::builder()
        .add_service(VmServiceServer::new(service))
        .serve(addr)
        .await?;

    Ok(())
}
