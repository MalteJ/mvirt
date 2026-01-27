//! mvirt-one - MicroVM Init System for isolated Pods.
//!
//! Runs as PID 1 inside MicroVMs or locally for development.

use anyhow::Result;
use clap::Parser;
use log::{error, info};
use mvirt_one::proto::uos_service_server::UosServiceServer;
use mvirt_one::utils::{mount, network, signals};
use mvirt_one::{Config, create_api_handler, initialize_services};
use nix::sys::prctl;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;
use tokio::net::TcpListener;
use tonic::transport::Server;

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// MicroVM Init System for isolated Pods.
#[derive(Parser)]
#[command(name = "mvirt-one")]
#[command(version = VERSION)]
#[command(about = "MicroVM Init System for isolated Pods")]
struct Args {
    /// Path to youki binary
    #[arg(long)]
    youki: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let is_pid1 = std::process::id() == 1;

    // Redirect stdin/stdout/stderr to console when running as PID 1
    if is_pid1 {
        setup_console();
    }

    // Initialize logging
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();

    info!("mvirt-one v{} starting", VERSION);

    // Parse CLI args (only in non-PID1 mode)
    let args = if is_pid1 {
        Args { youki: None }
    } else {
        Args::parse()
    };

    if is_pid1 {
        info!("Running as PID 1 (init mode)");
        run_as_init().await
    } else {
        info!("Running in local development mode");
        run_local(args).await
    }
}

/// Setup console for init mode - redirect stdin/stdout/stderr to serial console
fn setup_console() {
    use std::fs::OpenOptions;
    use std::os::unix::io::AsRawFd;

    // Use /dev/ttyS0 (serial console) - this is what cloud-hypervisor exposes via --serial file=...
    let console_path = "/dev/ttyS0";

    if let Ok(console) = OpenOptions::new().read(true).write(true).open(console_path) {
        let fd = console.as_raw_fd();
        unsafe {
            // Redirect stdin, stdout, stderr to serial console
            libc::dup2(fd, 0); // stdin
            libc::dup2(fd, 1); // stdout
            libc::dup2(fd, 2); // stderr
        }
        // console file handle dropped here, but fd 0/1/2 keep it open
    }
}

/// Run as PID 1 inside a MicroVM.
async fn run_as_init() -> Result<()> {
    // Phase 1: Mount filesystems
    info!("Phase 1: Mounting filesystems");
    mount::mount_all();

    // Phase 2: Setup signal handling
    info!("Phase 2: Setting up signal handlers");
    signals::setup_signal_handlers();

    // Phase 3: Configure network
    info!("Phase 3: Configuring network");
    network::configure_all().await;

    // Phase 4: Initialize services
    info!("Phase 4: Initializing services");
    let config = Config::default();
    let services = initialize_services(config).await?;

    // Phase 5: Start vsock server
    info!("Phase 5: Starting vsock server");
    let api_handler = create_api_handler(services.pod_tx, services.shutdown_tx);
    let _vsock_handle = start_vsock_server(api_handler).await?;

    // Phase 6: Signal ready to host
    info!("Phase 6: Signaling ready to host");
    if let Err(e) = signal_ready_to_host().await {
        error!("Failed to signal ready to host: {} (continuing anyway)", e);
    }

    // Main loop
    info!("mvirt-one ready, entering main loop");
    let mut shutdown_rx = services.shutdown_rx;

    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => {
                info!("Shutdown signal received");
                break;
            }
            _ = tokio::time::sleep(Duration::from_secs(1)) => {
                // Reap zombie children periodically
                signals::reap_children();
            }
        }
    }

    info!("mvirt-one shutting down");
    Ok(())
}

/// Run locally for development/testing.
async fn run_local(args: Args) -> Result<()> {
    // Set as child subreaper so we can wait for grandchildren
    prctl::set_child_subreaper(true)
        .map_err(|e| anyhow::anyhow!("Failed to set as child subreaper: {}", e))?;

    // Initialize services
    info!("Initializing services");
    let mut config = Config::default();

    // Override youki path if provided via CLI
    if let Some(youki_path) = args.youki {
        info!("Using custom youki path: {}", youki_path.display());
        config.youki_path = youki_path;
    }

    let services = initialize_services(config).await?;

    // Start TCP server for local testing (instead of vsock)
    info!("Starting TCP server on 127.0.0.1:50051");
    let api_handler = create_api_handler(services.pod_tx, services.shutdown_tx);

    let addr: SocketAddr = "127.0.0.1:50051".parse()?;
    let listener = TcpListener::bind(addr).await?;

    info!("mvirt-one ready, listening on {}", addr);

    Server::builder()
        .add_service(UosServiceServer::new(api_handler))
        .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
        .await?;

    Ok(())
}

/// Start the vsock server for host communication.
async fn start_vsock_server(
    api_handler: mvirt_one::services::pod::PodApiHandler,
) -> Result<tokio::task::JoinHandle<()>> {
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
    use tokio_vsock::{VsockAddr, VsockListener, VsockStream};
    use tonic::transport::server::Connected;

    // CID_ANY (u32::MAX) means accept connections from any CID
    // Port 1024 is our chosen port for the uos API
    const VSOCK_PORT: u32 = 1024;

    let addr = VsockAddr::new(libc::VMADDR_CID_ANY, VSOCK_PORT);
    let mut listener =
        VsockListener::bind(addr).map_err(|e| anyhow::anyhow!("Failed to bind vsock: {}", e))?;

    info!("vsock server listening on port {}", VSOCK_PORT);

    // Wrapper for VsockStream that implements Connected
    struct VsockConnection {
        inner: VsockStream,
        peer_cid: u32,
    }

    impl AsyncRead for VsockConnection {
        fn poll_read(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            Pin::new(&mut self.inner).poll_read(cx, buf)
        }
    }

    impl AsyncWrite for VsockConnection {
        fn poll_write(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            Pin::new(&mut self.inner).poll_write(cx, buf)
        }

        fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Pin::new(&mut self.inner).poll_flush(cx)
        }

        fn poll_shutdown(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<std::io::Result<()>> {
            Pin::new(&mut self.inner).poll_shutdown(cx)
        }
    }

    #[derive(Clone)]
    #[allow(dead_code)]
    struct VsockConnectInfo {
        peer_cid: u32,
    }

    impl Connected for VsockConnection {
        type ConnectInfo = VsockConnectInfo;

        fn connect_info(&self) -> Self::ConnectInfo {
            VsockConnectInfo {
                peer_cid: self.peer_cid,
            }
        }
    }

    let handle = tokio::spawn(async move {
        // Convert vsock listener to a stream of connections
        let incoming = async_stream::stream! {
            loop {
                match listener.accept().await {
                    Ok((stream, addr)) => {
                        info!("vsock connection from CID {}", addr.cid());
                        let conn = VsockConnection {
                            inner: stream,
                            peer_cid: addr.cid(),
                        };
                        yield Ok::<_, std::io::Error>(conn);
                    }
                    Err(e) => {
                        error!("vsock accept error: {}", e);
                    }
                }
            }
        };

        if let Err(e) = Server::builder()
            .add_service(UosServiceServer::new(api_handler))
            .serve_with_incoming(incoming)
            .await
        {
            error!("vsock server error: {}", e);
        }
    });

    Ok(handle)
}

/// Signal to the host that mvirt-one is ready.
///
/// Connects to the host (CID 2) on port 1025 to notify that
/// the guest is ready to receive gRPC calls.
async fn signal_ready_to_host() -> Result<()> {
    use tokio::io::AsyncWriteExt;
    use tokio_vsock::{VsockAddr, VsockStream};

    const HOST_CID: u32 = 2; // VMADDR_CID_HOST
    const READY_PORT: u32 = 1025;

    let addr = VsockAddr::new(HOST_CID, READY_PORT);

    info!(
        "Connecting to host CID {} port {} to signal ready",
        HOST_CID, READY_PORT
    );

    let mut stream = VsockStream::connect(addr).await?;

    // Send a simple ready message (the connection itself is the signal)
    stream.write_all(b"READY\n").await?;
    // Connection will be closed when stream is dropped

    info!("Ready signal sent to host");
    Ok(())
}
