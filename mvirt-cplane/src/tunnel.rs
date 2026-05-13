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
use crate::command::{NodeResources, NodeStatus, VmPhase, VmStatus};
use crate::grpc::proto::node_agent_client::NodeAgentClient;
use crate::grpc::proto::node_event::Kind as NodeEventKind;
use crate::grpc::proto::{
    CurrentResourcesRequest, NodeResources as ProtoNodeResources, NodeVmState, VmStateChanged,
    WatchEventsRequest,
};
use crate::store::{DataStore, UpdateNodeStatusRequest};

/// How often to re-pull resource counters from a connected node.
const RESOURCE_PULL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

impl From<ProtoNodeResources> for NodeResources {
    fn from(r: ProtoNodeResources) -> Self {
        Self {
            cpu_cores: r.cpu_cores,
            memory_mb: r.memory_mb,
            storage_gb: r.storage_gb,
            available_cpu_cores: r.available_cpu_cores,
            available_memory_mb: r.available_memory_mb,
            available_storage_gb: r.available_storage_gb,
        }
    }
}

/// Pull `CurrentResources` from the node and persist them via raft.
///
/// Returns Ok on success; logs (does not propagate) any RPC or store error so
/// the caller's tunnel lifecycle is unaffected.
/// Translate a node-emitted `NodeEvent` into a raft `Command::UpdateVmStatus`.
/// Currently only `VmStateChanged` is consumed; resource and daemon-health
/// events are no-ops here because the periodic CurrentResources pull
/// already covers the former and there's no daemon-health column in raft
/// state yet.
async fn handle_node_event<S: DataStore + ?Sized>(
    event: crate::grpc::proto::NodeEvent,
    store: &Arc<S>,
    node_id: &str,
) {
    let Some(NodeEventKind::VmState(vs)) = event.kind else {
        return;
    };
    let VmStateChanged {
        vm_id,
        state,
        message,
        vm: _vm, // full vmm.Vm snapshot, currently unused — extract more
                 // observed fields (ip_address, etc.) here as we grow them.
    } = vs;
    let phase = match NodeVmState::try_from(state).unwrap_or(NodeVmState::Unspecified) {
        NodeVmState::Running => VmPhase::Running,
        NodeVmState::Starting => VmPhase::Creating,
        NodeVmState::Stopping => VmPhase::Stopping,
        NodeVmState::Stopped => VmPhase::Stopped,
        // The VM vanished from vmm (deleted out-of-band, or the process
        // exited without going through Stopping). Mark Failed so the UI
        // and any operator see "this is broken" rather than a forever-
        // STARTING state.
        NodeVmState::Gone => VmPhase::Failed,
        NodeVmState::Unspecified => return,
    };
    let req = crate::store::UpdateVmStatusRequest {
        status: VmStatus {
            phase,
            node_id: Some(node_id.to_string()),
            ip_address: None,
            message,
        },
    };
    if let Err(e) = store.update_vm_status(&vm_id, req).await {
        warn!(node_id = %node_id, vm = %vm_id, error = %e, "submit UpdateVmStatus from node event failed");
    }
}

async fn pull_resources<S: DataStore + ?Sized>(
    agent: &mut NodeAgentClient<Channel>,
    store: &Arc<S>,
    node_id: &str,
) {
    match agent
        .current_resources(tonic::Request::new(CurrentResourcesRequest {}))
        .await
    {
        Ok(resp) => {
            let resources: NodeResources = resp.into_inner().into();
            if let Err(e) = store
                .update_node_status(
                    node_id,
                    UpdateNodeStatusRequest {
                        status: NodeStatus::Online,
                        resources: Some(resources),
                    },
                )
                .await
            {
                warn!(node_id = %node_id, error = %e, "raft update_node_status(resources) failed");
            }
        }
        Err(e) => warn!(node_id = %node_id, error = %e, "CurrentResources RPC failed"),
    }
}

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
    serve(listener, acceptor, registry, store).await
}

/// Serve a pre-bound listener. Same as `listen` but lets the caller decide
/// when the bind happens — used by integration tests that need to know the
/// ephemeral port without racing against accept.
pub async fn serve(
    listener: TcpListener,
    acceptor: Arc<TlsAcceptor>,
    registry: Arc<NodeRegistry>,
    store: Arc<dyn DataStore>,
) -> Result<()> {
    let addr = listener.local_addr().ok();
    info!(?addr, "tunnel listener up (mTLS)");

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

    // Mark the node online + pull its initial resource snapshot. Without a
    // first pull, the cplane keeps a zero-resource view of the node and the
    // scheduler skips it for every placement decision.
    let agent = NodeAgentClient::new(channel.clone());
    {
        let mut agent = agent.clone();
        pull_resources(&mut agent, &store, &node_id).await;
    }

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

    // 5. Hold an open WatchEvents stream for the lifetime of the tunnel and,
    // in parallel, periodically refresh the node's resource counters.
    let mut agent = handle.agent.clone();
    let mut resource_tick = tokio::time::interval(RESOURCE_PULL_INTERVAL);
    resource_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    resource_tick.tick().await; // consume the immediate first tick
    let stream_result = agent
        .watch_events(tonic::Request::new(WatchEventsRequest {}))
        .await;
    match stream_result {
        Ok(resp) => {
            let mut stream = resp.into_inner();
            loop {
                tokio::select! {
                    msg = stream.message() => match msg {
                        Ok(Some(event)) => {
                            handle_node_event(event, &store, &node_id).await;
                        }
                        Ok(None) => {
                            info!(node_id = %node_id, "node closed event stream cleanly");
                            break;
                        }
                        Err(e) => {
                            warn!(node_id = %node_id, error = %e, "tunnel event stream broken");
                            break;
                        }
                    },
                    _ = resource_tick.tick() => {
                        let mut agent = handle.agent.clone();
                        pull_resources(&mut agent, &store, &node_id).await;
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
