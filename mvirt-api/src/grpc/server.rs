//! gRPC server implementation for NodeService (bidirectional Sync stream).

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

use tokio::sync::{RwLock, broadcast, mpsc};
use tokio_stream::{Stream, StreamExt};
use tonic::{Request, Response, Status, Streaming};

use crate::audit::ApiAuditLogger;
use crate::command::{
    ImportJobState, NodeResources as CommandNodeResources, NodeStatus as CommandNodeStatus,
};
use crate::store::{DataStore, Event, RegisterNodeRequest, StoreError, UpdateNodeStatusRequest};

use super::manifest::build_manifest;
use super::proto::{
    ApiMessage, NodeMessage, NodeResources, RegisterResult, api_message, node_message,
    node_service_server::NodeService,
};

/// Type alias for manifest sender channels (one per connected node).
type ManifestSenderMap = HashMap<String, mpsc::Sender<ApiMessage>>;

/// NodeService gRPC implementation.
pub struct NodeServiceImpl {
    store: Arc<dyn DataStore>,
    audit: Arc<ApiAuditLogger>,
    /// Channels for pushing manifests to connected nodes.
    manifest_senders: Arc<RwLock<ManifestSenderMap>>,
    /// Global revision counter for manifests.
    revision: Arc<RwLock<u64>>,
}

impl NodeServiceImpl {
    pub fn new(store: Arc<dyn DataStore>, audit: Arc<ApiAuditLogger>) -> Self {
        Self {
            store,
            audit,
            manifest_senders: Arc::new(RwLock::new(HashMap::new())),
            revision: Arc::new(RwLock::new(0)),
        }
    }

    /// Get the next revision number.
    async fn next_revision(&self) -> u64 {
        let mut rev = self.revision.write().await;
        *rev += 1;
        *rev
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

    /// Build and push a manifest to a specific node.
    async fn push_manifest_to_node(&self, node_id: &str) {
        let revision = self.next_revision().await;
        let manifest = build_manifest(&self.store, node_id, revision).await;

        let senders = self.manifest_senders.read().await;
        if let Some(sender) = senders.get(node_id) {
            let msg = ApiMessage {
                payload: Some(api_message::Payload::Manifest(manifest)),
            };
            if let Err(e) = sender.send(msg).await {
                tracing::warn!("Failed to push manifest to node {}: {}", node_id, e);
            }
        }
    }

    /// Build and push manifest to all connected nodes.
    async fn push_manifest_to_all(&self) {
        let node_ids: Vec<String> = {
            let senders = self.manifest_senders.read().await;
            senders.keys().cloned().collect()
        };

        for node_id in &node_ids {
            self.push_manifest_to_node(node_id).await;
        }
    }

    /// Start listening for state events and push manifests to affected nodes.
    pub fn start_event_listener(self: Arc<Self>, mut events: broadcast::Receiver<Event>) {
        tokio::spawn(async move {
            tracing::info!("Started manifest event listener");

            while let Ok(event) = events.recv().await {
                self.handle_state_event(event).await;
            }

            tracing::info!("Manifest event listener stopped");
        });
    }

    /// Determine affected nodes from an event and push updated manifests.
    async fn handle_state_event(&self, event: Event) {
        match event {
            // VM events — push to the node the VM is (or was) on
            Event::VmStatusUpdated { new, .. } | Event::VmUpdated { new, .. } => {
                if let Some(ref node_id) = new.status.node_id {
                    self.push_manifest_to_node(node_id).await;
                }
            }
            Event::VmCreated(vm) => {
                if let Some(ref node_id) = vm.status.node_id {
                    self.push_manifest_to_node(node_id).await;
                }
            }
            Event::VmDeleted { .. } => {
                // We don't know which node had the VM, push to all
                self.push_manifest_to_all().await;
            }

            // Network events — global, push to all nodes
            Event::NetworkCreated(_)
            | Event::NetworkUpdated { .. }
            | Event::NetworkDeleted { .. } => {
                self.push_manifest_to_all().await;
            }

            // NIC events — push to all (NIC's node depends on its VM)
            Event::NicCreated(_) | Event::NicUpdated { .. } | Event::NicDeleted { .. } => {
                self.push_manifest_to_all().await;
            }

            // Volume events — node-specific
            Event::VolumeCreated(vol) => {
                self.push_manifest_to_node(&vol.node_id).await;
            }
            Event::VolumeDeleted { node_id, .. } => {
                self.push_manifest_to_node(&node_id).await;
            }

            // Import job — broadcast to all nodes (templates are global)
            Event::ImportJobCreated(_) => {
                self.push_manifest_to_all().await;
            }

            // Security group events — global
            Event::SecurityGroupCreated { .. } | Event::SecurityGroupDeleted { .. } => {
                self.push_manifest_to_all().await;
            }

            // Node events — no manifest push needed
            Event::NodeRegistered(_)
            | Event::NodeUpdated { .. }
            | Event::NodeDeregistered { .. } => {}
        }
    }
}

#[tonic::async_trait]
impl NodeService for NodeServiceImpl {
    type SyncStream = Pin<Box<dyn Stream<Item = Result<ApiMessage, Status>> + Send + 'static>>;

    async fn sync(
        &self,
        request: Request<Streaming<NodeMessage>>,
    ) -> Result<Response<Self::SyncStream>, Status> {
        let mut inbound = request.into_inner();

        // 1. Read first message — must be Register
        let first_msg = inbound
            .next()
            .await
            .ok_or_else(|| Status::invalid_argument("Stream closed before register"))?
            .map_err(|e| Status::internal(format!("Stream error: {}", e)))?;

        let register = match first_msg.payload {
            Some(node_message::Payload::Register(r)) => r,
            _ => return Err(Status::invalid_argument("First message must be Register")),
        };

        // 2. Register node in store
        let node_id = if register.node_id.is_empty() {
            uuid::Uuid::new_v4().to_string()
        } else {
            register.node_id.clone()
        };

        let store_req = RegisterNodeRequest {
            name: register.name.clone(),
            address: register.address,
            resources: Self::from_proto_resources(register.resources),
            labels: register.labels,
        };

        let registered_id = match self.store.register_node(store_req).await {
            Ok(node) => {
                self.audit.hypervisor_node_registered(&node.id, &node.name);
                node.id
            }
            Err(StoreError::Conflict(msg)) => {
                // Send failure result and close
                let (tx, rx) = mpsc::channel(1);
                let fail_msg = ApiMessage {
                    payload: Some(api_message::Payload::RegisterResult(RegisterResult {
                        node_id,
                        success: false,
                        message: msg,
                    })),
                };
                let _ = tx.send(Ok(fail_msg)).await;
                drop(tx);
                return Ok(Response::new(Box::pin(
                    tokio_stream::wrappers::ReceiverStream::new(rx),
                )));
            }
            Err(e) => return Err(Status::internal(e.to_string())),
        };

        // 3. Create outbound channel
        let (out_tx, out_rx) = mpsc::channel::<Result<ApiMessage, Status>>(256);
        let (manifest_tx, mut manifest_rx) = mpsc::channel::<ApiMessage>(64);

        // Send RegisterResult
        let register_result = ApiMessage {
            payload: Some(api_message::Payload::RegisterResult(RegisterResult {
                node_id: registered_id.clone(),
                success: true,
                message: String::new(),
            })),
        };
        let _ = out_tx.send(Ok(register_result)).await;

        // 4. Build and send initial manifest
        let revision = self.next_revision().await;
        let initial_manifest = build_manifest(&self.store, &registered_id, revision).await;
        let _ = out_tx
            .send(Ok(ApiMessage {
                payload: Some(api_message::Payload::Manifest(initial_manifest)),
            }))
            .await;

        // 5. Register manifest sender
        {
            let mut senders = self.manifest_senders.write().await;
            senders.insert(registered_id.clone(), manifest_tx);
        }

        tracing::info!("Node {} connected via Sync stream", registered_id);

        // 6. Spawn reader task for incoming messages
        let store = self.store.clone();
        let manifest_senders = self.manifest_senders.clone();
        let node_id_clone = registered_id.clone();

        // Forward manifests from the manifest channel to the outbound stream
        let out_tx_manifest = out_tx;
        let node_id_for_manifest = registered_id.clone();

        tokio::spawn(async move {
            while let Some(msg) = manifest_rx.recv().await {
                if out_tx_manifest.send(Ok(msg)).await.is_err() {
                    break;
                }
            }
            tracing::info!(
                "Manifest forwarder stopped for node {}",
                node_id_for_manifest
            );
        });

        // Spawn inbound message reader
        tokio::spawn(async move {
            while let Some(result) = inbound.next().await {
                let msg = match result {
                    Ok(m) => m,
                    Err(e) => {
                        tracing::warn!("Inbound stream error for node {}: {}", node_id_clone, e);
                        break;
                    }
                };

                match msg.payload {
                    Some(node_message::Payload::Heartbeat(hb)) => {
                        let store_req = UpdateNodeStatusRequest {
                            status: CommandNodeStatus::Online,
                            resources: Some(Self::from_proto_resources(hb.current_resources)),
                        };
                        if let Err(e) = store.update_node_status(&node_id_clone, store_req).await {
                            tracing::warn!(
                                "Failed to update heartbeat for node {}: {}",
                                node_id_clone,
                                e
                            );
                        }
                    }
                    Some(node_message::Payload::Status(status)) => {
                        // Handle template status (import flow)
                        Self::handle_status_static(&store, &node_id_clone, status).await;
                    }
                    Some(node_message::Payload::Register(_)) => {
                        tracing::warn!("Unexpected Register message from node {}", node_id_clone);
                    }
                    None => {}
                }
            }

            // Stream closed — clean up
            tracing::info!("Node {} disconnected", node_id_clone);
            {
                let mut senders = manifest_senders.write().await;
                senders.remove(&node_id_clone);
            }

            // Mark node offline
            let _ = store
                .update_node_status(
                    &node_id_clone,
                    UpdateNodeStatusRequest {
                        status: CommandNodeStatus::Offline,
                        resources: None,
                    },
                )
                .await;
        });

        let stream = tokio_stream::wrappers::ReceiverStream::new(out_rx);
        Ok(Response::new(Box::pin(stream)))
    }
}

impl NodeServiceImpl {
    /// Static version of handle_status_update for use in spawned tasks.
    async fn handle_status_static(
        store: &Arc<dyn DataStore>,
        node_id: &str,
        status: super::proto::StatusUpdate,
    ) {
        use super::proto::ResourcePhase;
        use crate::store::CreateTemplateRequest;

        for ts in &status.templates {
            let phase = ResourcePhase::try_from(ts.phase).unwrap_or(ResourcePhase::Unspecified);

            if phase == ResourcePhase::Creating {
                let _ = store
                    .update_import_job(&ts.id, 0, ImportJobState::Running, None)
                    .await;
            } else if phase == ResourcePhase::Failed {
                let msg = ts.message.clone().unwrap_or_default();
                let _ = store
                    .update_import_job(&ts.id, 0, ImportJobState::Failed, Some(msg))
                    .await;
            } else if phase == ResourcePhase::Ready {
                let import_job = store.get_import_job(&ts.id).await.ok().flatten();

                if let Some(job) = import_job {
                    match store
                        .create_template(CreateTemplateRequest {
                            project_id: job.project_id.clone(),
                            node_id: node_id.to_string(),
                            name: job.template_name.clone(),
                            size_bytes: ts.size_bytes,
                        })
                        .await
                    {
                        Ok(tpl) => {
                            tracing::info!(
                                "Template {} ({}) created from import job {}",
                                tpl.name,
                                tpl.id,
                                ts.id
                            );
                            let _ = store
                                .update_import_job(
                                    &ts.id,
                                    ts.size_bytes,
                                    ImportJobState::Completed,
                                    None,
                                )
                                .await;
                        }
                        Err(e) => {
                            tracing::warn!(
                                "Failed to create template from import {}: {}",
                                ts.id,
                                e
                            );
                            let _ = store
                                .update_import_job(
                                    &ts.id,
                                    ts.size_bytes,
                                    ImportJobState::Completed,
                                    None,
                                )
                                .await;
                        }
                    }
                } else {
                    tracing::warn!("Import job {} not found for template status update", ts.id);
                }
            }
        }

        if !status.vms.is_empty() {
            tracing::debug!(
                "Received {} VM status updates from node {}",
                status.vms.len(),
                node_id
            );
        }
    }
}
