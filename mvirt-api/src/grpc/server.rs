//! gRPC server implementation for NodeService.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

use tokio::sync::{RwLock, broadcast, mpsc};
use tokio_stream::Stream;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

use crate::audit::ApiAuditLogger;
use crate::command::{NodeResources as CommandNodeResources, NodeStatus as CommandNodeStatus};
use crate::store::{DataStore, Event, RegisterNodeRequest, StoreError, UpdateNodeStatusRequest};

use super::proto::{
    DeregisterRequest, DeregisterResponse, HeartbeatRequest, HeartbeatResponse, NodeResources,
    RegisterRequest, RegisterResponse, SpecEvent, UpdateResourceStatusRequest,
    UpdateResourceStatusResponse, WatchSpecsRequest, node_service_server::NodeService,
};

/// Type alias for the spec sender channel map.
type SpecSenderMap = HashMap<String, mpsc::Sender<Result<SpecEvent, Status>>>;

/// NodeService gRPC implementation.
pub struct NodeServiceImpl {
    store: Arc<dyn DataStore>,
    audit: Arc<ApiAuditLogger>,
    /// Channels for pushing specs to connected nodes
    /// Key: node_id, Value: sender for SpecEvent stream
    spec_senders: Arc<RwLock<SpecSenderMap>>,
    /// Global revision counter for spec events
    revision: Arc<RwLock<u64>>,
}

impl NodeServiceImpl {
    /// Create a new NodeServiceImpl.
    pub fn new(store: Arc<dyn DataStore>, audit: Arc<ApiAuditLogger>) -> Self {
        Self {
            store,
            audit,
            spec_senders: Arc::new(RwLock::new(HashMap::new())),
            revision: Arc::new(RwLock::new(0)),
        }
    }

    /// Convert store NodeResources to proto NodeResources.
    #[allow(dead_code)]
    fn to_proto_resources(r: &CommandNodeResources) -> NodeResources {
        NodeResources {
            cpu_cores: r.cpu_cores,
            memory_mb: r.memory_mb,
            storage_gb: r.storage_gb,
            available_cpu_cores: r.available_cpu_cores,
            available_memory_mb: r.available_memory_mb,
            available_storage_gb: r.available_storage_gb,
        }
    }

    /// Convert proto NodeResources to command NodeResources.
    fn from_proto_resources(r: Option<NodeResources>) -> CommandNodeResources {
        match r {
            Some(r) => CommandNodeResources {
                cpu_cores: r.cpu_cores,
                memory_mb: r.memory_mb,
                storage_gb: r.storage_gb,
                available_cpu_cores: r.available_cpu_cores,
                available_memory_mb: r.available_memory_mb,
                available_storage_gb: r.available_storage_gb,
            },
            None => CommandNodeResources::default(),
        }
    }

    /// Get the next revision number.
    #[allow(dead_code)]
    async fn next_revision(&self) -> u64 {
        let mut rev = self.revision.write().await;
        *rev += 1;
        *rev
    }

    /// Send a spec event to a specific node (by ID or name).
    pub async fn send_spec_to_node(&self, node_id: &str, event: SpecEvent) -> Result<(), Status> {
        let senders = self.spec_senders.read().await;
        // Try direct ID lookup first
        if let Some(sender) = senders.get(node_id) {
            sender.send(Ok(event)).await.map_err(|_| {
                Status::internal(format!("Failed to send spec to node {}", node_id))
            })?;
            return Ok(());
        }
        // Try resolving name to ID
        if let Ok(Some(node)) = self.store.get_node_by_name(node_id).await
            && let Some(sender) = senders.get(&node.id) {
                sender.send(Ok(event)).await.map_err(|_| {
                    Status::internal(format!("Failed to send spec to node {}", node_id))
                })?;
            }
        Ok(())
    }

    /// Broadcast a spec event to all connected nodes.
    pub async fn broadcast_spec(&self, event: SpecEvent) {
        let senders = self.spec_senders.read().await;
        for (node_id, sender) in senders.iter() {
            if let Err(e) = sender.send(Ok(event.clone())).await {
                tracing::warn!("Failed to send spec to node {}: {}", node_id, e);
            }
        }
    }

    /// Start listening for state events and forward specs to nodes.
    ///
    /// This spawns a background task that converts state machine events
    /// to SpecEvents and sends them to the appropriate nodes.
    pub fn start_event_listener(self: Arc<Self>, mut events: broadcast::Receiver<Event>) {
        tokio::spawn(async move {
            tracing::info!("Started spec event listener");

            while let Ok(event) = events.recv().await {
                if let Err(e) = self.handle_state_event(event).await {
                    tracing::error!("Failed to handle state event: {}", e);
                }
            }

            tracing::info!("Spec event listener stopped");
        });
    }

    /// Handle a state machine event and send specs to nodes.
    async fn handle_state_event(&self, event: Event) -> Result<(), Status> {
        use super::proto::{
            NetworkSpec as ProtoNetworkSpec, NicSpec as ProtoNicSpec, ResourceMeta, SpecEvent,
            SpecEventType, VmDesiredState as ProtoVmDesiredState, VmSpec as ProtoVmSpec,
            VolumeSpec as ProtoVolumeSpec, spec_event,
        };

        let revision = self.next_revision().await;

        match event {
            // VM scheduled to a node - send spec to that node
            Event::VmStatusUpdated { id, new, .. } => {
                if let Some(ref node_id) = new.status.node_id {
                    let desired_state = match new.spec.desired_state {
                        crate::command::VmDesiredState::Running => {
                            ProtoVmDesiredState::Running as i32
                        }
                        crate::command::VmDesiredState::Stopped => {
                            ProtoVmDesiredState::Stopped as i32
                        }
                    };

                    let spec_event = SpecEvent {
                        revision,
                        r#type: SpecEventType::Create as i32,
                        spec: Some(spec_event::Spec::Vm(ProtoVmSpec {
                            meta: Some(ResourceMeta {
                                id: id.clone(),
                                name: new.spec.name.clone(),
                                project_id: new.spec.project_id.clone(),
                                node_id: Some(node_id.clone()),
                                labels: Default::default(),
                            }),
                            cpu_cores: new.spec.cpu_cores,
                            memory_mb: new.spec.memory_mb,
                            volume_id: new.spec.volume_id.clone(),
                            nic_id: new.spec.nic_id.clone(),
                            image: new.spec.image.clone(),
                            desired_state,
                        })),
                    };

                    tracing::info!("Sending VM spec {} to node {}", id, node_id);
                    self.send_spec_to_node(node_id, spec_event).await?;
                }
            }

            // Network created - broadcast to all nodes (they'll create if needed)
            Event::NetworkCreated(network) => {
                let spec_event = SpecEvent {
                    revision,
                    r#type: SpecEventType::Create as i32,
                    spec: Some(spec_event::Spec::Network(ProtoNetworkSpec {
                        meta: Some(ResourceMeta {
                            id: network.id.clone(),
                            name: network.name.clone(),
                            project_id: network.project_id.clone(),
                            node_id: None,
                            labels: Default::default(),
                        }),
                        ipv4_enabled: network.ipv4_enabled,
                        ipv4_prefix: network.ipv4_prefix.clone().unwrap_or_default(),
                        ipv6_enabled: network.ipv6_enabled,
                        ipv6_prefix: network.ipv6_prefix.clone().unwrap_or_default(),
                        dns_servers: network.dns_servers.clone(),
                        ntp_servers: network.ntp_servers.clone(),
                        is_public: network.is_public,
                    })),
                };

                tracing::info!("Broadcasting network spec {}", network.id);
                self.broadcast_spec(spec_event).await;
            }

            // Network deleted - broadcast delete to all nodes
            Event::NetworkDeleted { id } => {
                let spec_event = SpecEvent {
                    revision,
                    r#type: SpecEventType::Delete as i32,
                    spec: Some(spec_event::Spec::Network(ProtoNetworkSpec {
                        meta: Some(ResourceMeta {
                            id: id.clone(),
                            ..Default::default()
                        }),
                        ..Default::default()
                    })),
                };

                tracing::info!("Broadcasting network delete {}", id);
                self.broadcast_spec(spec_event).await;
            }

            // NIC created - send to nodes that need it
            Event::NicCreated(nic) => {
                let spec_event = SpecEvent {
                    revision,
                    r#type: SpecEventType::Create as i32,
                    spec: Some(spec_event::Spec::Nic(ProtoNicSpec {
                        meta: Some(ResourceMeta {
                            id: nic.id.clone(),
                            name: nic.name.clone().unwrap_or_default(),
                            project_id: nic.project_id.clone(),
                            node_id: None,
                            labels: Default::default(),
                        }),
                        network_id: nic.network_id.clone(),
                        mac_address: nic.mac_address.clone(),
                        ipv4_address: nic.ipv4_address.clone(),
                        ipv6_address: nic.ipv6_address.clone(),
                        routed_ipv4_prefixes: nic.routed_ipv4_prefixes.clone(),
                        routed_ipv6_prefixes: nic.routed_ipv6_prefixes.clone(),
                        security_group_id: nic.security_group_id.clone().unwrap_or_default(),
                    })),
                };

                // Broadcast NIC creation (nodes will filter by relevance)
                tracing::info!("Broadcasting NIC spec {}", nic.id);
                self.broadcast_spec(spec_event).await;
            }

            // NIC deleted
            Event::NicDeleted { id, .. } => {
                let spec_event = SpecEvent {
                    revision,
                    r#type: SpecEventType::Delete as i32,
                    spec: Some(spec_event::Spec::Nic(ProtoNicSpec {
                        meta: Some(ResourceMeta {
                            id: id.clone(),
                            ..Default::default()
                        }),
                        ..Default::default()
                    })),
                };

                tracing::info!("Broadcasting NIC delete {}", id);
                self.broadcast_spec(spec_event).await;
            }

            // VM deleted
            Event::VmDeleted { id } => {
                let spec_event = SpecEvent {
                    revision,
                    r#type: SpecEventType::Delete as i32,
                    spec: Some(spec_event::Spec::Vm(ProtoVmSpec {
                        meta: Some(ResourceMeta {
                            id: id.clone(),
                            ..Default::default()
                        }),
                        ..Default::default()
                    })),
                };

                tracing::info!("Broadcasting VM delete {}", id);
                self.broadcast_spec(spec_event).await;
            }

            // Volume created - send to specific node
            Event::VolumeCreated(volume) => {
                let spec_event = SpecEvent {
                    revision,
                    r#type: SpecEventType::Create as i32,
                    spec: Some(spec_event::Spec::Volume(ProtoVolumeSpec {
                        meta: Some(ResourceMeta {
                            id: volume.id.clone(),
                            name: volume.name.clone(),
                            project_id: volume.project_id.clone(),
                            node_id: Some(volume.node_id.clone()),
                            ..Default::default()
                        }),
                        size_gb: volume.size_bytes / (1024 * 1024 * 1024),
                        template_id: volume.template_id.clone(),
                        attached_vm_id: None,
                    })),
                };

                tracing::info!(
                    "Sending volume spec {} to node {}",
                    volume.id,
                    volume.node_id
                );
                self.send_spec_to_node(&volume.node_id, spec_event).await?;
            }

            // Volume deleted - send to specific node
            Event::VolumeDeleted { id, node_id } => {
                let spec_event = SpecEvent {
                    revision,
                    r#type: SpecEventType::Delete as i32,
                    spec: Some(spec_event::Spec::Volume(ProtoVolumeSpec {
                        meta: Some(ResourceMeta {
                            id: id.clone(),
                            node_id: Some(node_id.clone()),
                            ..Default::default()
                        }),
                        ..Default::default()
                    })),
                };

                tracing::info!("Sending volume delete {} to node {}", id, node_id);
                self.send_spec_to_node(&node_id, spec_event).await?;
            }

            // Other events don't require spec distribution
            _ => {}
        }

        Ok(())
    }
}

#[tonic::async_trait]
impl NodeService for NodeServiceImpl {
    /// Register a new node with the API server.
    async fn register(
        &self,
        request: Request<RegisterRequest>,
    ) -> Result<Response<RegisterResponse>, Status> {
        let req = request.into_inner();

        // Generate node_id if not provided
        let node_id = if req.node_id.is_empty() {
            uuid::Uuid::new_v4().to_string()
        } else {
            req.node_id.clone()
        };

        let store_req = RegisterNodeRequest {
            name: req.name.clone(),
            address: req.address,
            resources: Self::from_proto_resources(req.resources),
            labels: req.labels,
        };

        match self.store.register_node(store_req).await {
            Ok(node) => {
                self.audit.hypervisor_node_registered(&node.id, &node.name);
                let revision = *self.revision.read().await;
                Ok(Response::new(RegisterResponse {
                    node_id: node.id,
                    success: true,
                    message: String::new(),
                    initial_revision: revision,
                }))
            }
            Err(StoreError::Conflict(msg)) => Ok(Response::new(RegisterResponse {
                node_id,
                success: false,
                message: msg,
                initial_revision: 0,
            })),
            Err(e) => Err(Status::internal(e.to_string())),
        }
    }

    /// Handle heartbeat from a node.
    async fn heartbeat(
        &self,
        request: Request<HeartbeatRequest>,
    ) -> Result<Response<HeartbeatResponse>, Status> {
        let req = request.into_inner();

        let store_req = UpdateNodeStatusRequest {
            status: CommandNodeStatus::Online,
            resources: Some(Self::from_proto_resources(req.current_resources)),
        };

        match self.store.update_node_status(&req.node_id, store_req).await {
            Ok(_) => Ok(Response::new(HeartbeatResponse {
                success: true,
                message: String::new(),
            })),
            Err(StoreError::NotFound(msg)) => Ok(Response::new(HeartbeatResponse {
                success: false,
                message: msg,
            })),
            Err(e) => Err(Status::internal(e.to_string())),
        }
    }

    /// Deregister a node from the API server.
    async fn deregister(
        &self,
        request: Request<DeregisterRequest>,
    ) -> Result<Response<DeregisterResponse>, Status> {
        let req = request.into_inner();

        // Remove from spec senders
        {
            let mut senders = self.spec_senders.write().await;
            senders.remove(&req.node_id);
        }

        match self.store.deregister_node(&req.node_id).await {
            Ok(()) => {
                self.audit.hypervisor_node_deregistered(&req.node_id);
                Ok(Response::new(DeregisterResponse {
                    success: true,
                    message: String::new(),
                }))
            }
            Err(StoreError::NotFound(msg)) => Ok(Response::new(DeregisterResponse {
                success: false,
                message: msg,
            })),
            Err(e) => Err(Status::internal(e.to_string())),
        }
    }

    type WatchSpecsStream = Pin<Box<dyn Stream<Item = Result<SpecEvent, Status>> + Send + 'static>>;

    /// Stream spec changes to a node for reconciliation.
    async fn watch_specs(
        &self,
        request: Request<WatchSpecsRequest>,
    ) -> Result<Response<Self::WatchSpecsStream>, Status> {
        let req = request.into_inner();
        let node_id = req.node_id.clone();

        // Verify node exists
        let node_exists = self
            .store
            .get_node(&node_id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;
        if node_exists.is_none() {
            return Err(Status::not_found(format!("Node {} not found", node_id)));
        }

        // Create a channel for this node's spec stream
        let (tx, rx) = mpsc::channel::<Result<SpecEvent, Status>>(256);

        // Register the sender
        {
            let mut senders = self.spec_senders.write().await;
            senders.insert(node_id.clone(), tx);
        }

        // Convert the receiver into a stream
        let stream = ReceiverStream::new(rx);

        tracing::info!(
            "Node {} started watching specs from revision {}",
            node_id,
            req.since_revision
        );

        Ok(Response::new(Box::pin(stream)))
    }

    /// Update the status of a resource from a node.
    async fn update_resource_status(
        &self,
        request: Request<UpdateResourceStatusRequest>,
    ) -> Result<Response<UpdateResourceStatusResponse>, Status> {
        let req = request.into_inner();

        // TODO: Handle status updates and update the state machine
        // For now, just acknowledge receipt
        tracing::debug!(
            "Received status update from node {}: {:?}",
            req.node_id,
            req.status
        );

        Ok(Response::new(UpdateResourceStatusResponse {
            success: true,
            message: String::new(),
        }))
    }
}
