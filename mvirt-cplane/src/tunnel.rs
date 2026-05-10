//! Reverse-tunnel listener — mTLS edition (ADR-0006).
//!
//! mvirt-node dials in over TLS with a client cert issued by the internal
//! CA. We require the client cert (`WebPkiClientVerifier`), extract the
//! `(node_id, cluster_slug)` from the verified peer cert's SAN URIs, check
//! it against the revocation set and the Node table, and then hand the TLS
//! stream off to tonic as the byte-pipe for an inverted HTTP/2 channel.
//!
//! The gRPC roles still invert at this point: the node serves NodeAgent +
//! daemon proxies on its end; the cplane drives them as a tonic client.
//! Identity, however, is no longer self-claimed in `Identify` — it's pinned
//! to the cert.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex as StdMutex};

use anyhow::{Context, Result, anyhow};
use mvirt_daemon_protos::net::net_service_client::NetServiceClient;
use mvirt_daemon_protos::vmm::vm_service_client::VmServiceClient;
use mvirt_daemon_protos::zfs::zfs_service_client::ZfsServiceClient;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::RwLock;
use tokio_rustls::TlsAcceptor;
use tonic::transport::{Channel, Endpoint, Uri};
use tower::service_fn;
use tracing::{info, warn};

use crate::ca::extract_identity_from_der;
use crate::command::NodeStatus;
use crate::grpc::proto::WatchEventsRequest;
use crate::grpc::proto::node_agent_client::NodeAgentClient;
use crate::store::{DataStore, UpdateNodeStatusRequest};

/// Per-node connection state. All four clients share the same underlying
/// HTTP/2-over-TLS connection (the inverted tunnel socket).
pub struct NodeHandle {
    pub node_id: String,
    pub name: String,
    pub cluster_slug: String,
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

/// Bind a TCP listener, wrap each accepted socket in TLS, and spawn a
/// per-connection handler that performs identity extraction and channel
/// setup.
pub async fn listen(
    addr: SocketAddr,
    acceptor: Arc<TlsAcceptor>,
    registry: Arc<NodeRegistry>,
    store: Arc<dyn DataStore>,
) -> Result<()> {
    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding tunnel listener on {addr}"))?;
    info!(%addr, "tunnel listener up (mTLS)");

    loop {
        let (sock, peer) = listener.accept().await?;
        let acceptor = acceptor.clone();
        let registry = registry.clone();
        let store = store.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_connection(sock, peer, acceptor, registry, store).await {
                warn!(%peer, error = %e, "tunnel connection terminated");
            }
        });
    }
}

async fn handle_connection(
    sock: TcpStream,
    peer: SocketAddr,
    acceptor: Arc<TlsAcceptor>,
    registry: Arc<NodeRegistry>,
    store: Arc<dyn DataStore>,
) -> Result<()> {
    sock.set_nodelay(true).ok();

    // 1. TLS handshake (client cert required by ServerConfig).
    let tls = acceptor.accept(sock).await.context("TLS handshake")?;

    // 2. Identity from peer cert SAN URIs.
    let (node_id, cluster_slug) = {
        let (_, session) = tls.get_ref();
        let chain = session
            .peer_certificates()
            .ok_or_else(|| anyhow!("peer presented no client cert"))?;
        let leaf = chain
            .first()
            .ok_or_else(|| anyhow!("empty peer cert chain"))?;
        extract_identity_from_der(leaf.as_ref()).context("extract identity from peer cert")?
    };

    info!(
        %peer,
        node_id = %node_id,
        cluster_slug = %cluster_slug,
        "tunnel mTLS handshake ok"
    );

    // 3. Sanity-check against raft state.
    let node_row = match store
        .get_node(&node_id)
        .await
        .context("looking up node row")?
    {
        Some(n) if matches!(n.status, NodeStatus::Revoked) => {
            return Err(anyhow!(
                "node '{}' is revoked, refusing connection",
                node_id
            ));
        }
        Some(n) if n.cluster_slug.as_deref() != Some(cluster_slug.as_str()) => {
            return Err(anyhow!(
                "cert claims cluster '{}' but node row has '{:?}' — refusing",
                cluster_slug,
                n.cluster_slug
            ));
        }
        Some(n) => n,
        None => {
            return Err(anyhow!(
                "node '{}' (claimed via cert) is not in the node table",
                node_id
            ));
        }
    };

    // 4. Hand the TLS stream to tonic as a one-shot connector.
    let slot: Arc<StdMutex<Option<tokio_rustls::server::TlsStream<TcpStream>>>> =
        Arc::new(StdMutex::new(Some(tls)));
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
    let endpoint = Endpoint::from_static("https://reverse.tunnel")
        .keep_alive_while_idle(true)
        .http2_keep_alive_interval(std::time::Duration::from_secs(10))
        .keep_alive_timeout(std::time::Duration::from_secs(20));
    let channel = endpoint
        .connect_with_connector(connector)
        .await
        .context("building inverted channel from accepted TLS socket")?;

    if let Err(e) = store
        .update_node_status(
            &node_id,
            UpdateNodeStatusRequest {
                status: NodeStatus::Online,
                resources: None,
            },
        )
        .await
    {
        warn!(node_id = %node_id, error = %e, "raft update_node_status(Online) failed");
    }

    let agent = NodeAgentClient::new(channel.clone());
    let handle = Arc::new(NodeHandle {
        node_id: node_id.clone(),
        name: node_row.name,
        cluster_slug: cluster_slug.clone(),
        address: peer.to_string(),
        agent,
        vmm: VmServiceClient::new(channel.clone()),
        zfs: ZfsServiceClient::new(channel.clone()),
        net: NetServiceClient::new(channel),
    });

    registry.insert(handle.clone()).await;

    // 5. Hold an open WatchEvents stream for the lifetime of the tunnel.
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
                        // Phase 3 TODO: drive controller from node-emitted events.
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

    if let Err(e) = store
        .update_node_status(
            &node_id,
            UpdateNodeStatusRequest {
                status: NodeStatus::Offline,
                resources: None,
            },
        )
        .await
    {
        warn!(node_id = %node_id, error = %e, "raft update_node_status(Offline) failed");
    }
    Ok(())
}
