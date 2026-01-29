//! Node agent - handles connection to mvirt-api and reconciliation.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::sync::mpsc;
use tokio::time::interval;
use tonic::transport::Channel;
use tracing::{debug, error, info, warn};

use crate::clients::{NetClient, VmmClient, ZfsClient};
use crate::proto::node_service_client::NodeServiceClient;
use crate::proto::{
    update_resource_status_request::Status, HeartbeatRequest, NodeResources as ProtoNodeResources,
    RegisterRequest, SpecEvent, UpdateResourceStatusRequest, WatchSpecsRequest,
};
use crate::reconciler::network::NetworkReconciler;
use crate::reconciler::nic::NicReconciler;
use crate::reconciler::route::RouteReconciler;
use crate::reconciler::security_group::SecurityGroupReconciler;
use crate::reconciler::template::TemplateReconciler;
use crate::reconciler::vm::VmReconciler;
use crate::reconciler::volume::VolumeReconciler;
use crate::reconciler::Reconciler;

/// Node resource information.
#[derive(Debug, Clone, Default)]
pub struct NodeResources {
    pub cpu_cores: u32,
    pub memory_mb: u64,
    pub storage_gb: u64,
    pub available_cpu_cores: u32,
    pub available_memory_mb: u64,
    pub available_storage_gb: u64,
}

impl From<&NodeResources> for ProtoNodeResources {
    fn from(r: &NodeResources) -> Self {
        ProtoNodeResources {
            cpu_cores: r.cpu_cores,
            memory_mb: r.memory_mb,
            storage_gb: r.storage_gb,
            available_cpu_cores: r.available_cpu_cores,
            available_memory_mb: r.available_memory_mb,
            available_storage_gb: r.available_storage_gb,
        }
    }
}

/// Audit logger for node events.
pub struct NodeAuditLogger {
    inner: Arc<mvirt_log::AuditLogger>,
}

impl NodeAuditLogger {
    pub fn new(log_endpoint: &str) -> Self {
        Self {
            inner: Arc::new(mvirt_log::AuditLogger::new(log_endpoint, "node")),
        }
    }

    pub fn registered(&self, node_id: &str, node_name: &str) {
        let inner = Arc::clone(&self.inner);
        let msg = format!("Node registered: {} ({})", node_name, node_id);
        let ids = vec![node_id.to_string()];
        tokio::spawn(async move {
            inner.log(mvirt_log::LogLevel::Audit, msg, ids).await;
        });
    }

    pub fn connected(&self, node_id: &str, api_endpoint: &str) {
        let inner = Arc::clone(&self.inner);
        let msg = format!("Connected to API: {}", api_endpoint);
        let ids = vec![node_id.to_string()];
        tokio::spawn(async move {
            inner.log(mvirt_log::LogLevel::Info, msg, ids).await;
        });
    }

    pub fn disconnected(&self, node_id: &str, reason: &str) {
        let inner = Arc::clone(&self.inner);
        let msg = format!("Disconnected from API: {}", reason);
        let ids = vec![node_id.to_string()];
        tokio::spawn(async move {
            inner.log(mvirt_log::LogLevel::Warn, msg, ids).await;
        });
    }

    pub fn spec_received(&self, node_id: &str, resource_type: &str, resource_id: &str) {
        let inner = Arc::clone(&self.inner);
        let msg = format!("Received spec: {} {}", resource_type, resource_id);
        let ids = vec![node_id.to_string(), resource_id.to_string()];
        tokio::spawn(async move {
            inner.log(mvirt_log::LogLevel::Debug, msg, ids).await;
        });
    }
}

/// Node agent that connects to mvirt-api and reconciles state.
pub struct NodeAgent {
    api_endpoint: String,
    node_name: String,
    node_id: Option<String>,
    heartbeat_interval: Duration,
    resources: NodeResources,
    audit: Arc<NodeAuditLogger>,
    last_revision: u64,
    // Reconcilers
    vm_reconciler: VmReconciler,
    network_reconciler: NetworkReconciler,
    nic_reconciler: NicReconciler,
    template_reconciler: TemplateReconciler,
    volume_reconciler: VolumeReconciler,
    security_group_reconciler: SecurityGroupReconciler,
    route_reconciler: RouteReconciler,
}

impl NodeAgent {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        api_endpoint: String,
        node_name: String,
        node_id: Option<String>,
        heartbeat_interval: Duration,
        resources: NodeResources,
        audit: Arc<NodeAuditLogger>,
        vmm_client: VmmClient,
        zfs_client: ZfsClient,
        net_client: NetClient,
    ) -> Self {
        // Clone clients for reconcilers that share the same backend
        Self {
            api_endpoint,
            node_name,
            node_id,
            heartbeat_interval,
            resources,
            audit,
            last_revision: 0,
            vm_reconciler: VmReconciler::new(vmm_client),
            network_reconciler: NetworkReconciler::new(net_client.clone()),
            nic_reconciler: NicReconciler::new(net_client.clone()),
            template_reconciler: TemplateReconciler::new(zfs_client.clone()),
            volume_reconciler: VolumeReconciler::new(zfs_client),
            security_group_reconciler: SecurityGroupReconciler::new(net_client),
            route_reconciler: RouteReconciler::new(),
        }
    }

    /// Connect to the API server.
    async fn connect(&self) -> Result<NodeServiceClient<Channel>> {
        info!("Connecting to API server: {}", self.api_endpoint);
        let channel = Channel::from_shared(self.api_endpoint.clone())
            .context("Invalid API endpoint")?
            .connect()
            .await
            .context("Failed to connect to API server")?;

        Ok(NodeServiceClient::new(channel))
    }

    /// Register with the API server.
    async fn register(&mut self, client: &mut NodeServiceClient<Channel>) -> Result<String> {
        let request = RegisterRequest {
            node_id: self.node_id.clone().unwrap_or_default(),
            name: self.node_name.clone(),
            address: format!("{}:0", self.node_name), // TODO: Get actual address
            resources: Some((&self.resources).into()),
            labels: std::collections::HashMap::new(),
        };

        let response = client
            .register(request)
            .await
            .context("Failed to register node")?
            .into_inner();

        if !response.success {
            anyhow::bail!("Registration failed: {}", response.message);
        }

        let node_id = response.node_id;
        self.node_id = Some(node_id.clone());
        self.last_revision = response.initial_revision;

        info!(
            "Registered as node: {} (revision {})",
            node_id, self.last_revision
        );
        self.audit.registered(&node_id, &self.node_name);

        Ok(node_id)
    }

    /// Report resource status back to API.
    async fn report_status(client: &mut NodeServiceClient<Channel>, node_id: &str, status: Status) {
        let request = UpdateResourceStatusRequest {
            node_id: node_id.to_string(),
            status: Some(status),
        };

        match client.update_resource_status(request).await {
            Ok(resp) => {
                let resp = resp.into_inner();
                if !resp.success {
                    warn!("Status update rejected: {}", resp.message);
                }
            }
            Err(e) => {
                error!("Failed to report status: {}", e);
            }
        }
    }

    /// Handle a spec event from the API server.
    async fn handle_spec_event(
        &self,
        client: &mut NodeServiceClient<Channel>,
        node_id: &str,
        event: SpecEvent,
    ) {
        use crate::proto::{spec_event::Spec, SpecEventType};

        let event_type =
            SpecEventType::try_from(event.r#type).unwrap_or(SpecEventType::Unspecified);
        let is_delete = event_type == SpecEventType::Delete;

        if let Some(spec) = event.spec {
            match spec {
                Spec::Network(net_spec) => {
                    let id = net_spec
                        .meta
                        .as_ref()
                        .map(|m| m.id.clone())
                        .unwrap_or_default();
                    self.audit.spec_received(node_id, "network", &id);

                    if is_delete {
                        if let Err(e) = self.network_reconciler.finalize(&id).await {
                            error!("Failed to finalize network {}: {}", id, e);
                        }
                    } else {
                        match self.network_reconciler.reconcile(&id, &net_spec).await {
                            Ok(_) => {}
                            Err(e) => {
                                error!("Network reconciliation failed for {}: {}", id, e)
                            }
                        }
                    }
                }
                Spec::Vm(vm_spec) => {
                    let id = vm_spec
                        .meta
                        .as_ref()
                        .map(|m| m.id.clone())
                        .unwrap_or_default();
                    self.audit.spec_received(node_id, "vm", &id);

                    if is_delete {
                        if let Err(e) = self.vm_reconciler.finalize(&id).await {
                            error!("Failed to finalize VM {}: {}", id, e);
                        }
                    } else {
                        match self.vm_reconciler.reconcile(&id, &vm_spec).await {
                            Ok(status) => {
                                Self::report_status(client, node_id, Status::VmStatus(status))
                                    .await;
                            }
                            Err(e) => error!("VM reconciliation failed for {}: {}", id, e),
                        }
                    }
                }
                Spec::Nic(nic_spec) => {
                    let id = nic_spec
                        .meta
                        .as_ref()
                        .map(|m| m.id.clone())
                        .unwrap_or_default();
                    self.audit.spec_received(node_id, "nic", &id);

                    if is_delete {
                        if let Err(e) = self.nic_reconciler.finalize(&id).await {
                            error!("Failed to finalize NIC {}: {}", id, e);
                        }
                    } else {
                        match self.nic_reconciler.reconcile(&id, &nic_spec).await {
                            Ok(status) => {
                                Self::report_status(client, node_id, Status::NicStatus(status))
                                    .await;
                            }
                            Err(e) => error!("NIC reconciliation failed for {}: {}", id, e),
                        }
                    }
                }
                Spec::Template(tpl_spec) => {
                    let id = tpl_spec
                        .meta
                        .as_ref()
                        .map(|m| m.id.clone())
                        .unwrap_or_default();
                    self.audit.spec_received(node_id, "template", &id);

                    if is_delete {
                        if let Err(e) = self.template_reconciler.finalize(&id).await {
                            error!("Failed to finalize template {}: {}", id, e);
                        }
                    } else {
                        match self.template_reconciler.reconcile(&id, &tpl_spec).await {
                            Ok(status) => {
                                Self::report_status(
                                    client,
                                    node_id,
                                    Status::TemplateStatus(status),
                                )
                                .await;
                            }
                            Err(e) => error!("Template reconciliation failed for {}: {}", id, e),
                        }
                    }
                }
                Spec::Volume(vol_spec) => {
                    let id = vol_spec
                        .meta
                        .as_ref()
                        .map(|m| m.id.clone())
                        .unwrap_or_default();
                    self.audit.spec_received(node_id, "volume", &id);

                    if is_delete {
                        if let Err(e) = self.volume_reconciler.finalize(&id).await {
                            error!("Failed to finalize volume {}: {}", id, e);
                        }
                    } else {
                        match self.volume_reconciler.reconcile(&id, &vol_spec).await {
                            Ok(status) => {
                                Self::report_status(client, node_id, Status::VolumeStatus(status))
                                    .await;
                            }
                            Err(e) => error!("Volume reconciliation failed for {}: {}", id, e),
                        }
                    }
                }
                Spec::SecurityGroup(sg_spec) => {
                    let id = sg_spec
                        .meta
                        .as_ref()
                        .map(|m| m.id.clone())
                        .unwrap_or_default();
                    self.audit.spec_received(node_id, "security_group", &id);

                    if is_delete {
                        if let Err(e) = self.security_group_reconciler.finalize(&id).await {
                            error!("Failed to finalize security group {}: {}", id, e);
                        }
                    } else {
                        match self
                            .security_group_reconciler
                            .reconcile(&id, &sg_spec)
                            .await
                        {
                            Ok(status) => {
                                Self::report_status(
                                    client,
                                    node_id,
                                    Status::SecurityGroupStatus(status),
                                )
                                .await;
                            }
                            Err(e) => {
                                error!("SecurityGroup reconciliation failed for {}: {}", id, e)
                            }
                        }
                    }
                }
                Spec::Route(route_spec) => {
                    let id = route_spec
                        .meta
                        .as_ref()
                        .map(|m| m.id.clone())
                        .unwrap_or_default();
                    self.audit.spec_received(node_id, "route", &id);

                    if is_delete {
                        if let Err(e) = self.route_reconciler.finalize(&id).await {
                            error!("Failed to finalize route {}: {}", id, e);
                        }
                    } else {
                        match self.route_reconciler.reconcile(&id, &route_spec).await {
                            Ok(status) => {
                                Self::report_status(client, node_id, Status::RouteStatus(status))
                                    .await;
                            }
                            Err(e) => error!("Route reconciliation failed for {}: {}", id, e),
                        }
                    }
                }
            }
        }
    }

    /// Main agent loop.
    pub async fn run(&mut self) -> Result<()> {
        // Connect to API
        let mut client = self.connect().await?;

        // Register
        let node_id = self.register(&mut client).await?;
        self.audit.connected(&node_id, &self.api_endpoint);

        // Start heartbeat task
        let heartbeat_client = client.clone();
        let heartbeat_node_id = node_id.clone();
        let heartbeat_interval = self.heartbeat_interval;
        let heartbeat_resources = self.resources.clone();

        let (heartbeat_tx, mut heartbeat_rx) = mpsc::channel::<()>(1);

        let heartbeat_handle = tokio::spawn(async move {
            let mut interval = interval(heartbeat_interval);
            let mut client = heartbeat_client;

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        let request = HeartbeatRequest {
                            node_id: heartbeat_node_id.clone(),
                            current_resources: Some((&heartbeat_resources).into()),
                        };

                        match client.heartbeat(request).await {
                            Ok(resp) => {
                                let resp = resp.into_inner();
                                if !resp.success {
                                    warn!("Heartbeat failed: {}", resp.message);
                                } else {
                                    debug!("Heartbeat sent");
                                }
                            }
                            Err(e) => {
                                error!("Heartbeat error: {}", e);
                            }
                        }
                    }
                    _ = heartbeat_rx.recv() => {
                        info!("Heartbeat task stopping");
                        break;
                    }
                }
            }
        });

        // Watch for spec changes
        let watch_request = WatchSpecsRequest {
            node_id: node_id.clone(),
            since_revision: self.last_revision,
        };

        info!("Starting spec watch from revision {}", self.last_revision);

        let mut stream = client
            .watch_specs(watch_request)
            .await
            .context("Failed to start spec watch")?
            .into_inner();

        // Process spec events
        loop {
            match tokio::time::timeout(Duration::from_secs(60), stream.message()).await {
                Ok(Ok(Some(event))) => {
                    self.last_revision = event.revision;
                    self.handle_spec_event(&mut client, &node_id, event).await;
                }
                Ok(Ok(None)) => {
                    info!("Spec stream ended");
                    break;
                }
                Ok(Err(e)) => {
                    error!("Spec stream error: {}", e);
                    break;
                }
                Err(_) => {
                    debug!("Spec stream timeout, continuing...");
                }
            }
        }

        // Stop heartbeat task
        drop(heartbeat_tx);
        let _ = heartbeat_handle.await;

        self.audit.disconnected(&node_id, "stream ended");

        Ok(())
    }
}
