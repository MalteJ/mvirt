//! vsock client for communicating with uos in MicroVMs.

#![allow(dead_code)]

use anyhow::{Result, anyhow};
use hyper_util::rt::TokioIo;
use std::time::{Duration, Instant};
use tokio_vsock::{VsockAddr, VsockStream};
use tonic::transport::{Channel, Endpoint, Uri};
use tower::service_fn;
use tracing::{debug, info, warn};

/// Port used by uos for its API.
const UOS_VSOCK_PORT: u32 = 1024;

/// Client for communicating with uos running inside a MicroVM.
pub struct UosClient {
    cid: u32,
    channel: Channel,
}

impl UosClient {
    /// Connect to uos in a MicroVM via vsock.
    pub async fn connect(cid: u32) -> Result<Self> {
        info!(cid = cid, "Connecting to uos via vsock");

        // Create a channel that uses vsock transport
        let channel = create_vsock_channel(cid, UOS_VSOCK_PORT).await?;

        Ok(Self { cid, channel })
    }

    /// Get the underlying gRPC channel.
    pub fn channel(&self) -> Channel {
        self.channel.clone()
    }

    /// Get the CID of the connected VM.
    pub fn cid(&self) -> u32 {
        self.cid
    }
}

/// Create a tonic channel over vsock.
async fn create_vsock_channel(cid: u32, port: u32) -> Result<Channel> {
    // Use a dummy URI - the actual connection is made via vsock
    let uri = Uri::builder()
        .scheme("http")
        .authority(format!("vsock-{}:{}", cid, port))
        .path_and_query("/")
        .build()
        .map_err(|e| anyhow!("Failed to build URI: {}", e))?;

    let endpoint = Endpoint::from(uri).connect_timeout(std::time::Duration::from_secs(5));

    // Create channel with custom connector that uses vsock
    let cid_clone = cid;
    let port_clone = port;
    let channel = endpoint
        .connect_with_connector(service_fn(move |_: Uri| {
            let cid = cid_clone;
            let port = port_clone;
            async move {
                debug!(cid = cid, port = port, "Creating vsock connection");
                let addr = VsockAddr::new(cid, port);
                let stream = VsockStream::connect(addr)
                    .await
                    .map_err(|e| std::io::Error::other(format!("vsock connect failed: {}", e)))?;
                // Wrap with TokioIo to implement hyper's Read/Write traits
                Ok::<_, std::io::Error>(TokioIo::new(stream))
            }
        }))
        .await
        .map_err(|e| anyhow!("Failed to connect via vsock: {}", e))?;

    info!(cid = cid, port = port, "Connected to uos via vsock");
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

/// Get the CID of a running VM process (deprecated, use vm_id_to_cid instead).
#[deprecated(note = "Use vm_id_to_cid instead")]
pub fn pid_to_cid(pid: u32) -> u32 {
    pid + 3
}

/// Wait for uos to become available on a MicroVM via vsock.
///
/// This function retries the connection until the timeout is reached.
/// Use this after starting a MicroVM to wait for the guest to boot.
pub async fn wait_for_uos(cid: u32, timeout: Duration) -> Result<UosClient> {
    let start = Instant::now();
    let retry_interval = Duration::from_millis(200);

    info!(
        cid = cid,
        timeout_secs = timeout.as_secs(),
        "Waiting for uos"
    );

    while start.elapsed() < timeout {
        match UosClient::connect(cid).await {
            Ok(client) => {
                info!(
                    cid = cid,
                    elapsed_ms = start.elapsed().as_millis(),
                    "Connected to uos"
                );
                return Ok(client);
            }
            Err(e) => {
                debug!(cid = cid, error = %e, "uos not ready yet, retrying...");
                tokio::time::sleep(retry_interval).await;
            }
        }
    }

    warn!(
        cid = cid,
        timeout_secs = timeout.as_secs(),
        "Timeout waiting for uos"
    );
    Err(anyhow!(
        "Timeout waiting for uos on CID {} after {:?}",
        cid,
        timeout
    ))
}
