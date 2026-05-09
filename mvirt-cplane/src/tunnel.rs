//! Reverse-tunnel listener.
//!
//! mvirt-node dials in over plain TCP. The roles invert at the gRPC layer:
//! the node hosts gRPC services on its end of the dialed socket, the api here
//! consumes them as a regular tonic Channel client. Each accepted connection
//! produces a NodeHandle bundling the per-node daemon clients, registered in
//! the shared NodeRegistry so api-side reconcilers can dispatch RPCs by node id.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex as StdMutex};

use anyhow::{Context, Result, anyhow};
use mvirt_daemon_protos::net::net_service_client::NetServiceClient;
use mvirt_daemon_protos::vmm::vm_service_client::VmServiceClient;
use mvirt_daemon_protos::zfs::zfs_service_client::ZfsServiceClient;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::RwLock;
use tonic::transport::{Channel, Endpoint, Uri};
use tower::service_fn;
use tracing::{info, warn};

use crate::grpc::proto::node_agent_client::NodeAgentClient;
use crate::grpc::proto::{IdentifyRequest, WatchEventsRequest};

/// Per-node connection state. All four clients share the same underlying
/// HTTP/2 connection (the inverted tunnel socket).
pub struct NodeHandle {
    pub node_id: String,
    pub name: String,
    pub address: String,
    pub agent: NodeAgentClient<Channel>,
    pub vmm: VmServiceClient<Channel>,
    pub zfs: ZfsServiceClient<Channel>,
    pub net: NetServiceClient<Channel>,
}

#[derive(Default)]
pub struct NodeRegistry {
    nodes: RwLock<HashMap<String, Arc<NodeHandle>>>,
}

impl NodeRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn insert(&self, handle: Arc<NodeHandle>) {
        self.nodes
            .write()
            .await
            .insert(handle.node_id.clone(), handle);
    }

    pub async fn remove(&self, node_id: &str) {
        self.nodes.write().await.remove(node_id);
    }

    pub async fn get(&self, node_id: &str) -> Option<Arc<NodeHandle>> {
        self.nodes.read().await.get(node_id).cloned()
    }

    pub async fn list(&self) -> Vec<Arc<NodeHandle>> {
        self.nodes.read().await.values().cloned().collect()
    }
}

/// Bind a TCP listener and accept reverse-tunnel connections from nodes.
/// Each accepted socket spawns a task that performs the Identify handshake
/// and registers the node.
pub async fn listen(addr: SocketAddr, registry: Arc<NodeRegistry>) -> Result<()> {
    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding tunnel listener on {addr}"))?;
    info!(%addr, "tunnel listener up");

    loop {
        let (sock, peer) = listener.accept().await?;
        let registry = registry.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_connection(sock, peer, registry).await {
                warn!(%peer, error = %e, "tunnel connection terminated");
            }
        });
    }
}

async fn handle_connection(
    sock: TcpStream,
    peer: SocketAddr,
    registry: Arc<NodeRegistry>,
) -> Result<()> {
    sock.set_nodelay(true).ok();

    // Hand the accepted socket to tonic via a one-shot connector. The Channel
    // reuses this single HTTP/2 connection for all RPCs back to the node.
    let slot = Arc::new(StdMutex::new(Some(sock)));
    let connector = service_fn(move |_uri: Uri| {
        let slot = slot.clone();
        async move {
            slot.lock()
                .expect("connector poisoned")
                .take()
                .map(hyper_util::rt::TokioIo::new)
                .ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::ConnectionAborted,
                        "tunnel socket already consumed",
                    )
                })
        }
    });

    // The URI authority is irrelevant — the connector ignores it. Keep-alive
    // is required because the api-side Channel sits idle between reconciliation
    // RPCs and hyper-util's pooled client would otherwise close the inverted
    // connection after the first request completes.
    let endpoint = Endpoint::from_static("http://reverse.tunnel")
        .keep_alive_while_idle(true)
        .http2_keep_alive_interval(std::time::Duration::from_secs(10))
        .keep_alive_timeout(std::time::Duration::from_secs(20));
    let channel = endpoint
        .connect_with_connector(connector)
        .await
        .context("building inverted channel from accepted socket")?;

    let mut agent = NodeAgentClient::new(channel.clone());
    let identify = agent
        .identify(tonic::Request::new(IdentifyRequest {}))
        .await
        .context("calling NodeAgent.Identify")?
        .into_inner();

    if identify.node_id.is_empty() {
        return Err(anyhow!("node returned empty node_id during Identify"));
    }

    info!(
        node_id = %identify.node_id,
        name = %identify.name,
        version = %identify.agent_version,
        %peer,
        "node connected via reverse tunnel"
    );

    let handle = Arc::new(NodeHandle {
        node_id: identify.node_id.clone(),
        name: identify.name,
        address: identify.address,
        agent,
        vmm: VmServiceClient::new(channel.clone()),
        zfs: ZfsServiceClient::new(channel.clone()),
        net: NetServiceClient::new(channel),
    });

    let node_id = handle.node_id.clone();
    registry.insert(handle.clone()).await;

    // Hold an open WatchEvents stream for the lifetime of the tunnel. This
    // doubles as our liveness signal — when the underlying HTTP/2 connection
    // dies (peer disconnect, network failure, h2 keep-alive timeout) the stream
    // errors and we deregister the node. Phase 3 will also drain
    // node-originated events from this stream.
    let mut agent = handle.agent.clone();
    let stream_result = agent
        .watch_events(tonic::Request::new(WatchEventsRequest {}))
        .await;
    match stream_result {
        Ok(resp) => {
            let mut stream = resp.into_inner();
            loop {
                match stream.message().await {
                    Ok(Some(_event)) => {
                        // TODO Phase 3: forward event to controller for state-machine update
                    }
                    Ok(None) => {
                        info!(node_id = %node_id, "node closed event stream cleanly");
                        break;
                    }
                    Err(e) => {
                        warn!(node_id = %node_id, error = %e, "tunnel event stream broken");
                        break;
                    }
                }
            }
        }
        Err(e) => {
            warn!(node_id = %node_id, error = %e, "could not open WatchEvents stream");
        }
    }
    registry.remove(&node_id).await;
    Ok(())
}
