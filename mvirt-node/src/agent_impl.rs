//! NodeAgent gRPC service implementation. Hosted by mvirt-node on the
//! reverse-tunnel socket and consumed by the api as a gRPC client.
//!
//! WatchEvents is the node→cplane status-push channel. mvirt-node
//! subscribes to each local daemon's own Watch* stream (vmm.WatchVms
//! today; zfs/ebpf to follow) and forwards every event into the gRPC
//! response stream the api is holding open. No polling — daemons
//! publish on their own mutator paths and on background watchers
//! (e.g. cloud-hypervisor child exit), and we fan-out from there.

use std::pin::Pin;

use mvirt_daemon_protos::vmm::vm_service_client::VmServiceClient;
use mvirt_daemon_protos::vmm::{VmEventType, VmState as VmmVmState, WatchVmsRequest};
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
    CurrentResourcesRequest, IdentifyRequest, IdentifyResponse, NodeEvent, NodeResources,
    NodeVmState, VmStateChanged, WatchEventsRequest,
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
    /// gRPC client for the local mvirt-vmm daemon. We open its WatchVms
    /// stream once per cplane WatchEvents call and forward events.
    pub vmm: VmServiceClient<Channel>,
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
        let vmm = self.vmm.clone();
        tokio::spawn(forward_vm_events(vmm, tx));
        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }

    async fn current_resources(
        &self,
        _request: Request<CurrentResourcesRequest>,
    ) -> Result<Response<NodeResources>, Status> {
        Ok(Response::new(self.resources))
    }
}

/// Subscribe to mvirt-vmm's WatchVms stream and forward each event onto the
/// cplane-facing WatchEvents stream. Reconnects automatically when the local
/// daemon is unavailable or the stream tears down (e.g. daemon restart) — we
/// own the gRPC client connection, lazy reconnect is handled by tonic.
async fn forward_vm_events(
    mut vmm: VmServiceClient<Channel>,
    tx: mpsc::Sender<Result<NodeEvent, Status>>,
) {
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
            let phase = vm_event_to_node_state(&ev);
            let node_event = NodeEvent {
                kind: Some(NodeEventKind::VmState(VmStateChanged {
                    vm_id: ev.vm_id,
                    state: phase as i32,
                    message: None,
                    vm: ev.vm,
                })),
            };
            if tx.send(Ok(node_event)).await.is_err() {
                return;
            }
        }
        tokio::time::sleep(RESUBSCRIBE_DELAY).await;
    }
}

/// Translate a vmm-side VmEvent (type + optional embedded Vm) into the
/// node-level NodeVmState the cplane consumes. The DELETED event maps to
/// Gone — the VM is no longer registered in vmm at all.
fn vm_event_to_node_state(ev: &mvirt_daemon_protos::vmm::VmEvent) -> NodeVmState {
    let ty = VmEventType::try_from(ev.r#type).unwrap_or(VmEventType::VmEventUnspecified);
    if ty == VmEventType::VmEventDeleted {
        return NodeVmState::Gone;
    }
    if let Some(vm) = &ev.vm {
        return match VmmVmState::try_from(vm.state).unwrap_or(VmmVmState::Unspecified) {
            VmmVmState::Stopped => NodeVmState::Stopped,
            VmmVmState::Starting => NodeVmState::Starting,
            VmmVmState::Running => NodeVmState::Running,
            VmmVmState::Stopping => NodeVmState::Stopping,
            VmmVmState::Unspecified => NodeVmState::Unspecified,
        };
    }
    match ty {
        VmEventType::VmEventCreated => NodeVmState::Stopped,
        VmEventType::VmEventStarted => NodeVmState::Running,
        VmEventType::VmEventStopped => NodeVmState::Stopped,
        _ => NodeVmState::Unspecified,
    }
}
