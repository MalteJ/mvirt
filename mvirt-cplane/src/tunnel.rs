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
    CurrentResourcesRequest, NodeResources as ProtoNodeResources, VmStateChanged,
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
    use crate::command::{NicPhase, VolumePhase};
    let Some(kind) = event.kind else {
        return;
    };
    let vs = match kind {
        NodeEventKind::VmState(vs) => vs,
        NodeEventKind::VolumeState(v) => {
            // K8s envelope: None payload ⇒ deletion. We currently mark
            // surviving cplane rows Failed in that case so the reconciler
            // sees drift; future finalizer work will delete instead.
            let (phase, path, used_bytes, error) = match v.volume.as_ref() {
                Some(vol) => {
                    let phase = if vol.path.is_empty() {
                        VolumePhase::Creating
                    } else {
                        VolumePhase::Ready
                    };
                    (phase, Some(vol.path.clone()), vol.used_bytes, None)
                }
                None => (
                    VolumePhase::Failed,
                    None,
                    0,
                    Some("daemon reports gone".to_string()),
                ),
            };
            let req = crate::store::UpdateVolumeStatusRequest {
                phase,
                path,
                used_bytes,
                error,
            };
            if let Err(e) = store.update_volume_status(&v.volume_id, req).await {
                warn!(node_id, volume = %v.volume_id, error = %e, "update_volume_status from node event failed");
            }
            return;
        }
        NodeEventKind::TemplateState(t) => {
            // Template updates funnel through update_template_status — same
            // request DTO the template reconciler uses. Pull phase from the
            // ImportJob if present, otherwise treat presence-only as Ready.
            use crate::command::TemplatePhase;
            use mvirt_daemon_protos::zfs::ImportJobState;
            let (phase, bytes_written, size_bytes, error) = match (&t.template, &t.import_job) {
                (_, Some(job)) => {
                    let phase = match ImportJobState::try_from(job.state)
                        .unwrap_or(ImportJobState::Unspecified)
                    {
                        ImportJobState::Completed => TemplatePhase::Ready,
                        ImportJobState::Failed | ImportJobState::Cancelled => TemplatePhase::Failed,
                        _ => TemplatePhase::Importing,
                    };
                    (
                        phase,
                        job.bytes_written,
                        job.total_bytes,
                        job.error.clone().filter(|s| !s.is_empty()),
                    )
                }
                (Some(tpl), None) => (TemplatePhase::Ready, tpl.size_bytes, tpl.size_bytes, None),
                (None, None) => (
                    TemplatePhase::Failed,
                    0,
                    0,
                    Some("daemon reports gone".to_string()),
                ),
            };
            let req = crate::store::UpdateTemplateStatusRequest {
                phase,
                bytes_written,
                size_bytes,
                error,
            };
            if let Err(e) = store.update_template_status(&t.template_id, req).await {
                warn!(node_id, template = %t.template_id, error = %e, "update_template_status from node event failed");
            }
            return;
        }
        NodeEventKind::NicState(n) => {
            use mvirt_daemon_protos::net::NicState as DaemonNicState;
            let (phase, socket_path) = match n.nic.as_ref() {
                Some(nic) => {
                    let phase = match DaemonNicState::try_from(nic.state)
                        .unwrap_or(DaemonNicState::Unspecified)
                    {
                        DaemonNicState::Active | DaemonNicState::Created => NicPhase::Active,
                        DaemonNicState::Error => NicPhase::Failed,
                        DaemonNicState::Unspecified => NicPhase::Pending,
                    };
                    (phase, nic.socket_path.clone())
                }
                None => (NicPhase::Failed, String::new()),
            };
            let req = crate::store::UpdateNicStatusRequest {
                phase,
                socket_path,
                message: None,
            };
            if let Err(e) = store.update_nic_status(&n.nic_id, req).await {
                warn!(node_id, nic = %n.nic_id, error = %e, "update_nic_status from node event failed");
            }
            return;
        }
        NodeEventKind::NetworkState(n) => {
            // Network state today has nothing for the api to write back —
            // call update_network_status purely to keep the wire active
            // (it's a fetch+return) and as a placeholder for when we grow
            // observed-on-network fields.
            let req = crate::store::UpdateNetworkStatusRequest::default();
            if let Err(e) = store.update_network_status(&n.network_id, req).await {
                warn!(node_id, network = %n.network_id, error = %e, "update_network_status from node event failed");
            }
            return;
        }
        NodeEventKind::Resources(_) | NodeEventKind::DaemonHealth(_) => return,
    };
    let VmStateChanged { vm_id, vm } = vs;
    // K8s-style: vm = None means the daemon no longer has this VM.
    // Derive the api-side phase from the embedded daemon state.
    use mvirt_daemon_protos::vmm::VmState as VmmVmState;
    let phase = match vm
        .as_ref()
        .map(|v| VmmVmState::try_from(v.state).unwrap_or(VmmVmState::Unspecified))
    {
        Some(VmmVmState::Running) => VmPhase::Running,
        Some(VmmVmState::Starting) => VmPhase::Creating,
        Some(VmmVmState::Stopping) => VmPhase::Stopping,
        Some(VmmVmState::Stopped) => VmPhase::Stopped,
        Some(VmmVmState::Unspecified) => return,
        // None: vmm has no record of this VM. If our spec still wants
        // it Running this is a failure; the cplane's reconciler will
        // re-create or fail-mark on next pass.
        None => VmPhase::Failed,
    };
    let req = crate::store::UpdateVmStatusRequest {
        status: VmStatus {
            phase,
            node_id: Some(node_id.to_string()),
            ip_address: None,
            message: None,
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
