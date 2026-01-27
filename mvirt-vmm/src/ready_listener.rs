//! Listener for guest ready signals via vsock Unix socket.
//!
//! Guests (mvirt-one) connect to CID 2 (host) on port 1025 to signal they're ready.
//! Cloud-hypervisor proxies this as a connection to `<vsock_socket>_1025`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::net::UnixListener;
use tracing::{debug, info};

/// Port on which guests signal ready (used in socket path suffix).
const READY_SIGNAL_PORT: u32 = 1025;

/// A prepared ready signal listener.
///
/// Create this *before* starting the VM to avoid race conditions.
pub struct ReadySignalListener {
    listener: UnixListener,
    socket_path: PathBuf,
}

impl ReadySignalListener {
    /// Create a new ready signal listener for a VM.
    ///
    /// Call this BEFORE starting the VM, then call `wait()` after.
    pub async fn new(vsock_socket: &Path) -> anyhow::Result<Self> {
        let socket_path =
            PathBuf::from(format!("{}_{}", vsock_socket.display(), READY_SIGNAL_PORT));

        debug!(path = %socket_path.display(), "Creating ready signal listener");

        // Remove stale socket if it exists
        let _ = tokio::fs::remove_file(&socket_path).await;

        // Create Unix socket listener
        let listener = UnixListener::bind(&socket_path)?;
        info!(path = %socket_path.display(), "Ready signal listener bound");

        Ok(Self {
            listener,
            socket_path,
        })
    }

    /// Wait for the guest to signal ready.
    ///
    /// Consumes the listener and cleans up the socket file.
    pub async fn wait(self, timeout: Duration) -> anyhow::Result<()> {
        let result = tokio::time::timeout(timeout, async {
            match self.listener.accept().await {
                Ok((mut stream, _)) => {
                    info!(path = %self.socket_path.display(), "Guest connected to ready signal socket");

                    // Read any data the guest sends (e.g., "READY\n")
                    let mut buf = [0u8; 64];
                    let _ = stream.read(&mut buf).await;

                    Ok(())
                }
                Err(e) => Err(anyhow::anyhow!("Failed to accept: {}", e)),
            }
        })
        .await;

        // Clean up socket
        let _ = tokio::fs::remove_file(&self.socket_path).await;

        match result {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(anyhow::anyhow!("Timeout waiting for ready signal")),
        }
    }
}

impl Drop for ReadySignalListener {
    fn drop(&mut self) {
        // Best-effort cleanup if not already done
        let _ = std::fs::remove_file(&self.socket_path);
    }
}
