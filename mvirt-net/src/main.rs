use mvirt_net::audit::create_audit_logger;
use mvirt_net::grpc::proto::net_service_server::NetServiceServer;
use mvirt_net::grpc::{NetServiceImpl, NetworkManager, Storage};
use mvirt_net::{ping, router};
use std::net::Ipv4Addr;
use std::path::Path;
use std::sync::Arc;
use tokio::signal::unix::{SignalKind, signal};
use tonic::transport::Server;
use tracing::{error, info};

/// Default gRPC listen address.
const GRPC_ADDR: &str = "[::1]:50054";

/// Default database path.
const DB_PATH: &str = "/var/lib/mvirt/net/networks.db";

/// Default log service endpoint.
const LOG_ENDPOINT: &str = "http://[::1]:50052";

/// Default TUN device name.
const TUN_NAME: &str = "mvirt0";

#[tokio::main(flavor = "current_thread")]
async fn main() {
    tracing_subscriber::fmt::init();

    // Parse command line args
    let args: Vec<String> = std::env::args().collect();
    let mode = args.get(1).map(|s| s.as_str()).unwrap_or("grpc");

    match mode {
        "grpc" => run_grpc_server().await,
        "ping" => run_ping_mode().await,
        _ => {
            eprintln!("Usage: {} [grpc|ping]", args[0]);
            eprintln!("  grpc  - Run gRPC server (default)");
            eprintln!("  ping  - Run ping test mode");
            std::process::exit(1);
        }
    }
}

async fn run_grpc_server() {
    info!("Starting mvirt-net gRPC server");

    // Ensure database directory exists
    if let Some(parent) = Path::new(DB_PATH).parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        error!(error = %e, path = %parent.display(), "Failed to create database directory");
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

    // Initialize network manager
    let manager = Arc::new(NetworkManager::new(Arc::clone(&storage)));

    // Initialize global TUN device
    if let Err(e) = manager.init_tun(TUN_NAME).await {
        error!(error = %e, "Failed to initialize TUN device");
        error!("Do you have root privileges? Try running with 'sudo'.");
        std::process::exit(1);
    }

    // Create audit logger
    let audit = create_audit_logger(LOG_ENDPOINT);

    // Create gRPC service
    let service = NetServiceImpl::new(Arc::clone(&storage), Arc::clone(&manager), audit);

    // Parse listen address
    let addr = GRPC_ADDR.parse().expect("Invalid listen address");

    info!(addr = %addr, "Starting gRPC server");

    // Set up signal handlers
    let mut sigint = signal(SignalKind::interrupt()).expect("Failed to set up SIGINT handler");
    let mut sigterm = signal(SignalKind::terminate()).expect("Failed to set up SIGTERM handler");

    // Run server with graceful shutdown
    let server = Server::builder()
        .add_service(NetServiceServer::new(service))
        .serve_with_shutdown(addr, async {
            tokio::select! {
                _ = sigint.recv() => {
                    info!("Received SIGINT, shutting down...");
                }
                _ = sigterm.recv() => {
                    info!("Received SIGTERM, shutting down...");
                }
            }
        });

    if let Err(e) = server.await {
        error!(error = %e, "gRPC server error");
    }

    // Shutdown manager
    if let Err(e) = manager.shutdown().await {
        error!(error = %e, "Failed to shutdown network manager");
    }

    info!("Server stopped");
}

async fn run_ping_mode() {
    let local_ip = Ipv4Addr::new(192, 168, 1, 1);

    let router = match router::Router::with_config("tun0", local_ip, 24, 4096, 4096, 4096).await {
        Ok(r) => r,
        Err(e) => {
            error!("Failed to start router: {}", e);
            error!("Do you have root privileges? Try running with 'sudo'.");
            std::process::exit(1);
        }
    };

    // Set up signal handlers
    let mut sigint = signal(SignalKind::interrupt()).expect("Failed to set up SIGINT handler");
    let mut sigterm = signal(SignalKind::terminate()).expect("Failed to set up SIGTERM handler");

    info!("Starting ping loop to TUN IP... (Ctrl+C to stop)");
    let mut seq = 0u16;

    loop {
        tokio::select! {
            _ = sigint.recv() => {
                info!("Received SIGINT, shutting down...");
                break;
            }
            _ = sigterm.recv() => {
                info!("Received SIGTERM, shutting down...");
                break;
            }
            _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {
                match ping::send(local_ip, seq) {
                    Ok(rtt) => info!(seq, rtt_ms = rtt.as_secs_f64() * 1000.0, "ping OK"),
                    Err(e) => error!(seq, error = %e, "ping FAILED"),
                }
                seq = seq.wrapping_add(1);
            }
        }
    }

    if let Err(e) = router.shutdown().await {
        error!(error = %e, "Failed to shutdown router");
    }
}
