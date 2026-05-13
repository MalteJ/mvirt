//! mvirt-ebpf daemon: eBPF-based network service for mvirt VMs.

use mvirt_ebpf::audit::create_audit_logger;
use mvirt_ebpf::ebpf_loader::EbpfManager;
use mvirt_ebpf::grpc::proto::net_service_server::NetServiceServer;
use mvirt_ebpf::grpc::{EbpfNetServiceImpl, Storage};
use mvirt_ebpf::nat;
use mvirt_ebpf::proto_handler::ProtocolHandler;
use mvirt_log::tls_config_from_paths;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::signal::unix::{SignalKind, signal};
use tonic::transport::Server;
use tracing::{error, info, warn};

/// gRPC server address.
const GRPC_ADDR: &str = "[::1]:50054";

/// Database path.
const DB_PATH: &str = "/var/lib/mvirt/ebpf/networks.db";

/// mvirt-log endpoints (override with `MVIRT_LOG_ENDPOINTS=https://h1,https://h2`).
const LOG_ENDPOINTS_ENV: &str = "MVIRT_LOG_ENDPOINTS";
const LOG_ENDPOINTS_DEFAULT: &str = "https://[::1]:50052";

/// TLS material — node-cert paths, see `/var/lib/mvirt-node/`.
const TLS_CA_ENV: &str = "MVIRT_TLS_CA";
const TLS_CERT_ENV: &str = "MVIRT_TLS_CERT";
const TLS_KEY_ENV: &str = "MVIRT_TLS_KEY";
const TLS_CA_DEFAULT: &str = "/var/lib/mvirt-node/ca.pem";
const TLS_CERT_DEFAULT: &str = "/var/lib/mvirt-node/cert.pem";
const TLS_KEY_DEFAULT: &str = "/var/lib/mvirt-node/key.pem";

#[tokio::main(flavor = "current_thread")]
async fn main() {
    mvirt_log::tracing_setup::init("info", &["h2=warn"]);

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
    let log_endpoints: Vec<String> = std::env::var(LOG_ENDPOINTS_ENV)
        .unwrap_or_else(|_| LOG_ENDPOINTS_DEFAULT.to_string())
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let tls_ca = PathBuf::from(std::env::var(TLS_CA_ENV).unwrap_or_else(|_| TLS_CA_DEFAULT.into()));
    let tls_cert =
        PathBuf::from(std::env::var(TLS_CERT_ENV).unwrap_or_else(|_| TLS_CERT_DEFAULT.into()));
    let tls_key =
        PathBuf::from(std::env::var(TLS_KEY_ENV).unwrap_or_else(|_| TLS_KEY_DEFAULT.into()));
    let tls = match tls_config_from_paths(&tls_ca, &tls_cert, &tls_key) {
        Ok(t) => Some(t),
        Err(e) => {
            warn!(error = %e, "TLS config for audit logger failed; running without remote audit");
            None
        }
    };
    let audit = create_audit_logger(log_endpoints, tls);

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
