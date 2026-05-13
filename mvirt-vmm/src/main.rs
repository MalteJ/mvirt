use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use mvirt_log::{create_audit_logger, tls_config_from_paths};
use mvirt_vmm::grpc::VmServiceImpl;
use mvirt_vmm::hypervisor::Hypervisor;
use mvirt_vmm::pod_service::PodServiceImpl;
use mvirt_vmm::proto::pod_service_server::PodServiceServer;
use mvirt_vmm::proto::vm_service_server::VmServiceServer;
use mvirt_vmm::store::VmStore;
use tonic::transport::Server;
use tracing::{info, warn};

#[derive(Parser)]
#[command(name = "mvirt-vmm")]
#[command(about = "mvirt Virtual Machine Manager daemon")]
struct Args {
    /// Data directory for SQLite database and VM runtime files
    #[arg(short, long, default_value = "/var/lib/mvirt/vmm")]
    data_dir: PathBuf,

    /// gRPC listen address
    #[arg(short, long, default_value = "[::1]:50051")]
    listen: String,

    /// mvirt-log endpoints (comma-separated), cplane-side. Reads from
    /// `MVIRT_LOG_ENDPOINTS` if set — populated by mvirt-node's env sidecar
    /// after onboarding.
    #[arg(
        long,
        env = "MVIRT_LOG_ENDPOINTS",
        default_value = "https://[::1]:50052",
        value_delimiter = ','
    )]
    log_endpoint: Vec<String>,

    /// Path to the internal CA cert (PEM).
    #[arg(
        long,
        env = "MVIRT_TLS_CA",
        default_value = "/var/lib/mvirt-node/ca.pem"
    )]
    tls_ca: PathBuf,

    /// Path to this daemon's client cert (PEM).
    #[arg(
        long,
        env = "MVIRT_TLS_CERT",
        default_value = "/var/lib/mvirt-node/cert.pem"
    )]
    tls_cert: PathBuf,

    /// Path to this daemon's client key (PEM).
    #[arg(
        long,
        env = "MVIRT_TLS_KEY",
        default_value = "/var/lib/mvirt-node/key.pem"
    )]
    tls_key: PathBuf,

    /// Disable mTLS to mvirt-log (talk plain h2c). Dev/loopback only.
    #[arg(long, env = "MVIRT_LOG_INSECURE")]
    log_insecure: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    mvirt_log::tracing_setup::init("mvirt_vmm=info", &[]);

    let args = Args::parse();

    // Ensure data directory exists
    tokio::fs::create_dir_all(&args.data_dir).await?;

    info!(data_dir = %args.data_dir.display(), "Initializing mvirt-vmm");

    // Initialize store
    let store = Arc::new(VmStore::new(&args.data_dir).await?);

    // Broadcast bus for VM lifecycle events — fans out to WatchVms gRPC
    // subscribers. Buffer of 64 absorbs short-lived bursts (boot storms,
    // batch deletes); on overflow late subscribers see lag errors and the
    // 30s cplane resync covers gaps.
    let (vm_events_tx, _) = tokio::sync::broadcast::channel::<mvirt_vmm::VmEvent>(64);

    // Initialize hypervisor
    let hypervisor = Arc::new(
        Hypervisor::new(args.data_dir.clone(), store.clone(), vm_events_tx.clone()).await?,
    );

    // Recover VMs from previous run
    hypervisor.recover_vms().await?;

    // Spawn process watcher
    let _watcher_shutdown = hypervisor.clone().spawn_watcher();

    // Create audit logger (connects lazily to mvirt-log)
    let tls = if args.log_insecure {
        None
    } else {
        match tls_config_from_paths(&args.tls_ca, &args.tls_cert, &args.tls_key) {
            Ok(t) => Some(t),
            Err(e) => {
                warn!(error = %e, "TLS config for audit logger failed; running without remote audit");
                None
            }
        }
    };
    let audit = create_audit_logger(args.log_endpoint.clone(), "vmm", tls);

    // Create gRPC services
    let vm_service = VmServiceImpl::new(
        store.clone(),
        hypervisor.clone(),
        audit.clone(),
        vm_events_tx,
    );
    let pod_service = PodServiceImpl::new(store, hypervisor, audit);

    let addr = args.listen.parse()?;
    info!(addr = %addr, "Starting gRPC server");

    Server::builder()
        .add_service(VmServiceServer::new(vm_service))
        .add_service(PodServiceServer::new(pod_service))
        .serve(addr)
        .await?;

    Ok(())
}
