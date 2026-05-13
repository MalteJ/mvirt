//! NodeAgent gRPC service implementation. Hosted by mvirt-node on the
//! reverse-tunnel socket and consumed by the api as a gRPC client.
//!
//! WatchEvents is the node→cplane status-push channel. mvirt-node
//! subscribes to each local daemon's own Watch* stream (vmm / zfs /
//! ebpf) and forwards every event into the gRPC response stream the
//! api is holding open. K8s-style envelopes — each NodeEvent carries
//! the full current resource snapshot (or None when the resource is
//! gone). No transition enum, no polling.

use std::pin::Pin;

use mvirt_daemon_protos::net::net_service_client::NetServiceClient;
use mvirt_daemon_protos::net::{WatchNetworksRequest, WatchNicsRequest};
use mvirt_daemon_protos::vmm::vm_service_client::VmServiceClient;
use mvirt_daemon_protos::vmm::WatchVmsRequest;
use mvirt_daemon_protos::zfs::zfs_service_client::ZfsServiceClient;
use mvirt_daemon_protos::zfs::{WatchTemplatesRequest, WatchVolumesRequest};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::Stream;
use tokio_stream::StreamExt;
use tonic::transport::Channel;
use tonic::{Request, Response, Status};
use tracing::{debug, info};

use crate::proto::node::node_agent_server::NodeAgent;
use crate::proto::node_event::Kind as NodeEventKind;
use crate::proto::{
    CurrentResourcesRequest, IdentifyRequest, IdentifyResponse, NetworkStateChanged, NicStateChanged,
    NodeEvent, NodeResources, TemplateStateChanged, VmStateChanged, VolumeStateChanged,
    WatchEventsRequest,
};

const EVENT_CHANNEL_CAPACITY: usize = 64;
const RESUBSCRIBE_DELAY: std::time::Duration = std::time::Duration::from_secs(2);

#[derive(Clone)]
pub struct NodeAgentService {
    pub node_id: String,
    pub name: String,
    pub address: String,
    pub resources: NodeResources,
    pub agent_version: String,
    /// Typed gRPC clients for each local daemon. We subscribe to each
    /// daemon's Watch* stream per cplane WatchEvents call.
    pub vmm: VmServiceClient<Channel>,
    pub zfs: ZfsServiceClient<Channel>,
    pub net: NetServiceClient<Channel>,
}

#[tonic::async_trait]
impl NodeAgent for NodeAgentService {
    type WatchEventsStream =
        Pin<Box<dyn Stream<Item = Result<NodeEvent, Status>> + Send + 'static>>;

    async fn identify(
        &self,
        _request: Request<IdentifyRequest>,
    ) -> Result<Response<IdentifyResponse>, Status> {
        Ok(Response::new(IdentifyResponse {
            node_id: self.node_id.clone(),
            name: self.name.clone(),
            address: self.address.clone(),
            resources: Some(self.resources),
            labels: Default::default(),
            agent_version: self.agent_version.clone(),
        }))
    }

    async fn watch_events(
        &self,
        _request: Request<WatchEventsRequest>,
    ) -> Result<Response<Self::WatchEventsStream>, Status> {
        let (tx, rx) = mpsc::channel::<Result<NodeEvent, Status>>(EVENT_CHANNEL_CAPACITY);
        tokio::spawn(forward_vm_events(self.vmm.clone(), tx.clone()));
        tokio::spawn(forward_volume_events(self.zfs.clone(), tx.clone()));
        tokio::spawn(forward_template_events(self.zfs.clone(), tx.clone()));
        tokio::spawn(forward_nic_events(self.net.clone(), tx.clone()));
        tokio::spawn(forward_network_events(self.net.clone(), tx));
        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }

    async fn current_resources(
        &self,
        _request: Request<CurrentResourcesRequest>,
    ) -> Result<Response<NodeResources>, Status> {
        Ok(Response::new(self.resources))
    }
}

/// Macro-equivalent helper: given a closure that subscribes to a daemon
/// Watch stream and a closure that wraps each event into a NodeEvent,
/// run the resubscribe-on-error loop.
async fn forward_vm_events(
    mut vmm: VmServiceClient<Channel>,
    tx: mpsc::Sender<Result<NodeEvent, Status>>,
) {
    use mvirt_daemon_protos::vmm::VmEventType;
    loop {
        if tx.is_closed() {
            return;
        }
        let stream = match vmm.watch_vms(WatchVmsRequest { vm_id: None }).await {
            Ok(resp) => resp.into_inner(),
            Err(s) => {
                debug!(error = %s, "vmm.WatchVms subscribe failed; will retry");
                tokio::time::sleep(RESUBSCRIBE_DELAY).await;
                continue;
            }
        };
        info!("vmm WatchVms stream open");
        let mut stream = stream;
        while let Some(item) = stream.next().await {
            let ev = match item {
                Ok(ev) => ev,
                Err(s) => {
                    debug!(error = %s, "vmm.WatchVms stream errored; will resubscribe");
                    break;
                }
            };
            let is_deleted = VmEventType::try_from(ev.r#type)
                .map(|t| t == VmEventType::VmEventDeleted)
                .unwrap_or(false);
            let vm = if is_deleted { None } else { ev.vm };
            let node_event = NodeEvent {
                kind: Some(NodeEventKind::VmState(VmStateChanged {
                    vm_id: ev.vm_id,
                    vm,
                })),
            };
            if tx.send(Ok(node_event)).await.is_err() {
                return;
            }
        }
        tokio::time::sleep(RESUBSCRIBE_DELAY).await;
    }
}

async fn forward_volume_events(
    mut zfs: ZfsServiceClient<Channel>,
    tx: mpsc::Sender<Result<NodeEvent, Status>>,
) {
    loop {
        if tx.is_closed() {
            return;
        }
        let stream = match zfs.watch_volumes(WatchVolumesRequest {}).await {
            Ok(resp) => resp.into_inner(),
            Err(s) => {
                debug!(error = %s, "zfs.WatchVolumes subscribe failed; will retry");
                tokio::time::sleep(RESUBSCRIBE_DELAY).await;
                continue;
            }
        };
        info!("zfs WatchVolumes stream open");
        let mut stream = stream;
        while let Some(item) = stream.next().await {
            let ev = match item {
                Ok(ev) => ev,
                Err(s) => {
                    debug!(error = %s, "zfs.WatchVolumes stream errored; will resubscribe");
                    break;
                }
            };
            let node_event = NodeEvent {
                kind: Some(NodeEventKind::VolumeState(VolumeStateChanged {
                    volume_id: ev.volume_id,
                    volume: ev.volume,
                })),
            };
            if tx.send(Ok(node_event)).await.is_err() {
                return;
            }
        }
        tokio::time::sleep(RESUBSCRIBE_DELAY).await;
    }
}

async fn forward_template_events(
    mut zfs: ZfsServiceClient<Channel>,
    tx: mpsc::Sender<Result<NodeEvent, Status>>,
) {
    loop {
        if tx.is_closed() {
            return;
        }
        let stream = match zfs.watch_templates(WatchTemplatesRequest {}).await {
            Ok(resp) => resp.into_inner(),
            Err(s) => {
                debug!(error = %s, "zfs.WatchTemplates subscribe failed; will retry");
                tokio::time::sleep(RESUBSCRIBE_DELAY).await;
                continue;
            }
        };
        info!("zfs WatchTemplates stream open");
        let mut stream = stream;
        while let Some(item) = stream.next().await {
            let ev = match item {
                Ok(ev) => ev,
                Err(s) => {
                    debug!(error = %s, "zfs.WatchTemplates stream errored; will resubscribe");
                    break;
                }
            };
            let node_event = NodeEvent {
                kind: Some(NodeEventKind::TemplateState(TemplateStateChanged {
                    template_id: ev.template_id,
                    template: ev.template,
                    import_job: ev.import_job,
                })),
            };
            if tx.send(Ok(node_event)).await.is_err() {
                return;
            }
        }
        tokio::time::sleep(RESUBSCRIBE_DELAY).await;
    }
}

async fn forward_nic_events(
    mut net: NetServiceClient<Channel>,
    tx: mpsc::Sender<Result<NodeEvent, Status>>,
) {
    loop {
        if tx.is_closed() {
            return;
        }
        let stream = match net.watch_nics(WatchNicsRequest {}).await {
            Ok(resp) => resp.into_inner(),
            Err(s) => {
                debug!(error = %s, "net.WatchNics subscribe failed; will retry");
                tokio::time::sleep(RESUBSCRIBE_DELAY).await;
                continue;
            }
        };
        info!("net WatchNics stream open");
        let mut stream = stream;
        while let Some(item) = stream.next().await {
            let ev = match item {
                Ok(ev) => ev,
                Err(s) => {
                    debug!(error = %s, "net.WatchNics stream errored; will resubscribe");
                    break;
                }
            };
            let node_event = NodeEvent {
                kind: Some(NodeEventKind::NicState(NicStateChanged {
                    nic_id: ev.nic_id,
                    nic: ev.nic,
                })),
            };
            if tx.send(Ok(node_event)).await.is_err() {
                return;
            }
        }
        tokio::time::sleep(RESUBSCRIBE_DELAY).await;
    }
}

async fn forward_network_events(
    mut net: NetServiceClient<Channel>,
    tx: mpsc::Sender<Result<NodeEvent, Status>>,
) {
    loop {
        if tx.is_closed() {
            return;
        }
        let stream = match net.watch_networks(WatchNetworksRequest {}).await {
            Ok(resp) => resp.into_inner(),
            Err(s) => {
                debug!(error = %s, "net.WatchNetworks subscribe failed; will retry");
                tokio::time::sleep(RESUBSCRIBE_DELAY).await;
                continue;
            }
        };
        info!("net WatchNetworks stream open");
        let mut stream = stream;
        while let Some(item) = stream.next().await {
            let ev = match item {
                Ok(ev) => ev,
                Err(s) => {
                    debug!(error = %s, "net.WatchNetworks stream errored; will resubscribe");
                    break;
                }
            };
            let node_event = NodeEvent {
                kind: Some(NodeEventKind::NetworkState(NetworkStateChanged {
                    network_id: ev.network_id,
                    network: ev.network,
                })),
            };
            if tx.send(Ok(node_event)).await.is_err() {
                return;
            }
        }
        tokio::time::sleep(RESUBSCRIBE_DELAY).await;
    }
}
