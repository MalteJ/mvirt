//! mvirt-ebpf daemon: eBPF-based network service for mvirt VMs.

use mvirt_ebpf::audit::create_audit_logger;
use mvirt_ebpf::ebpf_loader::EbpfManager;
use mvirt_ebpf::grpc::proto::net_service_server::NetServiceServer;
use mvirt_ebpf::grpc::{EbpfNetServiceImpl, Storage};
use mvirt_ebpf::nat;
use mvirt_ebpf::proto_handler::ProtocolHandler;
use std::path::Path;
use std::sync::Arc;
use tokio::signal::unix::{SignalKind, signal};
use tonic::transport::Server;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

/// gRPC server address.
const GRPC_ADDR: &str = "[::1]:50054";

/// Database path.
const DB_PATH: &str = "/var/lib/mvirt/ebpf/networks.db";

/// Log service endpoint.
const LOG_ENDPOINT: &str = "http://[::1]:50052";

#[tokio::main(flavor = "current_thread")]
async fn main() {
    // Initialize logging with h2 filtered to warn level
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"))
        .add_directive("h2=warn".parse().unwrap());
    tracing_subscriber::fmt().with_env_filter(filter).init();

    info!("mvirt-ebpf starting...");

    // Ensure database directory exists
    if let Some(parent) = Path::new(DB_PATH).parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        error!(path = %parent.display(), error = %e, "Failed to create database directory");
        std::process::exit(1);
    }

    // Initialize storage
    let storage = match Storage::new(Path::new(DB_PATH)) {
        Ok(s) => Arc::new(s),
        Err(e) => {
            error!(error = %e, "Failed to initialize storage");
            std::process::exit(1);
        }
    };
    info!(path = DB_PATH, "Storage initialized");

    // Initialize nftables
    if let Err(e) = nat::init_nftables() {
        error!(error = %e, "Failed to initialize nftables");
        std::process::exit(1);
    }

    // Load eBPF programs
    let ebpf = match EbpfManager::load() {
        Ok(e) => Arc::new(e),
        Err(e) => {
            error!(error = %e, "Failed to load eBPF programs");
            std::process::exit(1);
        }
    };
    info!("eBPF programs loaded");

    // Create protocol handler
    let proto_handler = Arc::new(ProtocolHandler::new());

    // Create audit logger
    let audit = create_audit_logger(LOG_ENDPOINT);

    // Create gRPC service
    let service = EbpfNetServiceImpl::new(
        Arc::clone(&storage),
        Arc::clone(&ebpf),
        Arc::clone(&proto_handler),
        audit,
    );

    // Recover NICs from database
    if let Err(e) = service.recover_nics().await {
        error!(error = %e, "Failed to recover NICs");
        // Continue anyway - some NICs may have been recovered
    }

    // Parse address
    let addr = GRPC_ADDR.parse().expect("Invalid gRPC address");

    info!(addr = %addr, "Starting gRPC server");

    // Setup signal handlers
    let mut sigint = signal(SignalKind::interrupt()).expect("Failed to register SIGINT handler");
    let mut sigterm = signal(SignalKind::terminate()).expect("Failed to register SIGTERM handler");

    // Start server with graceful shutdown
    let server = Server::builder()
        .add_service(NetServiceServer::new(service))
        .serve_with_shutdown(addr, async {
            tokio::select! {
                _ = sigint.recv() => { info!("Received SIGINT"); }
                _ = sigterm.recv() => { info!("Received SIGTERM"); }
            }
        });

    if let Err(e) = server.await {
        error!(error = %e, "Server error");
        std::process::exit(1);
    }

    // Cleanup
    info!("Shutting down...");
    if let Err(e) = nat::cleanup_nftables() {
        error!(error = %e, "Failed to cleanup nftables");
    }

    info!("mvirt-ebpf stopped");
}
