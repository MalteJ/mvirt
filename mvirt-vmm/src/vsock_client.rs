//! vsock client for communicating with one in MicroVMs.
//!
//! Cloud-hypervisor exposes vsock via a Unix socket proxy. To connect:
//! 1. Connect to the vsock.sock Unix socket
//! 2. Send "CONNECT <port>\n"
//! 3. Receive "OK <cid>\n"
//! 4. Stream is now connected to the guest's vsock port

use anyhow::{Result, anyhow};
use hyper_util::rt::TokioIo;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tonic::transport::{Channel, Endpoint, Uri};
use tower::service_fn;
use tracing::{debug, info, warn};

/// Port used by one for its API.
const ONE_VSOCK_PORT: u32 = 1024;

/// Client for communicating with one running inside a MicroVM.
pub struct OneClient {
    channel: Channel,
}

impl OneClient {
    /// Connect to one in a MicroVM via vsock Unix socket proxy.
    pub async fn connect(vsock_socket: &Path) -> Result<Self> {
        info!(socket = %vsock_socket.display(), "Connecting to one via vsock");

        let channel = create_vsock_channel(vsock_socket, ONE_VSOCK_PORT).await?;

        Ok(Self { channel })
    }

    /// Get the underlying gRPC channel.
    pub fn channel(&self) -> Channel {
        self.channel.clone()
    }
}

/// Perform the vsock CONNECT handshake over a Unix stream.
async fn vsock_connect_handshake(stream: &mut UnixStream, port: u32) -> Result<()> {
    // Send CONNECT command
    let connect_cmd = format!("CONNECT {}\n", port);
    stream.write_all(connect_cmd.as_bytes()).await?;

    // Read response
    let mut reader = BufReader::new(stream);
    let mut response = String::new();
    reader.read_line(&mut response).await?;

    if response.starts_with("OK") {
        debug!(response = %response.trim(), "vsock handshake successful");
        Ok(())
    } else {
        Err(anyhow!("vsock handshake failed: {}", response.trim()))
    }
}

/// Create a tonic channel over vsock Unix socket proxy.
async fn create_vsock_channel(vsock_socket: &Path, port: u32) -> Result<Channel> {
    // Use a dummy URI - the actual connection is made via Unix socket
    let uri = Uri::builder()
        .scheme("http")
        .authority("vsock.local")
        .path_and_query("/")
        .build()
        .map_err(|e| anyhow!("Failed to build URI: {}", e))?;

    let endpoint = Endpoint::from(uri).connect_timeout(Duration::from_secs(5));

    let socket_path = vsock_socket.to_path_buf();
    let channel = endpoint
        .connect_with_connector(service_fn(move |_: Uri| {
            let socket_path = socket_path.clone();
            async move {
                debug!(socket = %socket_path.display(), port = port, "Connecting to vsock socket");

                // Connect to Unix socket
                let mut stream = UnixStream::connect(&socket_path).await?;

                // Perform vsock CONNECT handshake
                vsock_connect_handshake(&mut stream, port)
                    .await
                    .map_err(|e| std::io::Error::other(e.to_string()))?;

                // Wrap with TokioIo for hyper compatibility
                Ok::<_, std::io::Error>(TokioIo::new(stream))
            }
        }))
        .await
        .map_err(|e| anyhow!("Failed to connect via vsock: {}", e))?;

    info!(socket = %vsock_socket.display(), port = port, "Connected to one via vsock");
    Ok(channel)
}

/// Calculate CID from VM ID.
///
/// The CID must be > 2 (0 = hypervisor, 1 = local, 2 = host are reserved).
/// We use a hash of the VM ID to generate a unique CID.
pub fn vm_id_to_cid(vm_id: &str) -> u32 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    vm_id.hash(&mut hasher);
    // CID must be > 2, and we limit to 32-bit range minus reserved values
    let hash = hasher.finish();
    ((hash % (u32::MAX as u64 - 3)) + 3) as u32
}

/// Get the vsock socket path for a VM.
pub fn vsock_socket_path(data_dir: &Path, vm_id: &str) -> PathBuf {
    data_dir.join("vm").join(vm_id).join("vsock.sock")
}

/// Wait for one to become available on a MicroVM via vsock.
///
/// This function retries the connection until the timeout is reached.
/// Use this after starting a MicroVM to wait for the guest to boot.
pub async fn wait_for_one(vsock_socket: &Path, timeout: Duration) -> Result<OneClient> {
    let start = Instant::now();
    let retry_interval = Duration::from_millis(200);

    info!(
        socket = %vsock_socket.display(),
        timeout_secs = timeout.as_secs(),
        "Waiting for one"
    );

    while start.elapsed() < timeout {
        match OneClient::connect(vsock_socket).await {
            Ok(client) => {
                info!(
                    socket = %vsock_socket.display(),
                    elapsed_ms = start.elapsed().as_millis(),
                    "Connected to one"
                );
                return Ok(client);
            }
            Err(e) => {
                debug!(socket = %vsock_socket.display(), error = %e, "one not ready yet, retrying...");
                tokio::time::sleep(retry_interval).await;
            }
        }
    }

    warn!(
        socket = %vsock_socket.display(),
        timeout_secs = timeout.as_secs(),
        "Timeout waiting for one"
    );
    Err(anyhow!(
        "Timeout waiting for one on {} after {:?}",
        vsock_socket.display(),
        timeout
    ))
}
