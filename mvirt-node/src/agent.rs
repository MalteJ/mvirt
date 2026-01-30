//! Node agent — connects to mvirt-api via bidirectional Sync stream,
//! receives full manifests, and reconciles desired state with local daemons.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::sync::mpsc;
use tokio::time::interval;
use tokio_stream::StreamExt;
use tonic::transport::Channel;
use tracing::{debug, error, info, warn};

use crate::clients::{NetClient, VmmClient, ZfsClient};
use crate::proto::node_service_client::NodeServiceClient;
use crate::proto::{
    api_message, node_message, ApiMessage, Heartbeat, NodeManifest, NodeMessage,
    NodeResources as ProtoNodeResources, RegisterNode, StatusUpdate,
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
    /// Tracks known resource IDs per type for pruning.
    known_ids: HashMap<String, HashSet<String>>,
    // Reconcilers
    vm_reconciler: VmReconciler,
    network_reconciler: NetworkReconciler,
    nic_reconciler: NicReconciler,
    template_reconciler: TemplateReconciler,
    volume_reconciler: VolumeReconciler,
    security_group_reconciler: SecurityGroupReconciler,
    route_reconciler: RouteReconciler,
    // Clients for init-from-reality
    vmm_client: VmmClient,
    net_client: NetClient,
    zfs_client: ZfsClient,
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
        Self {
            api_endpoint,
            node_name,
            node_id,
            heartbeat_interval,
            resources,
            audit,
            known_ids: HashMap::new(),
            vm_reconciler: VmReconciler::new(vmm_client.clone(), zfs_client.clone()),
            network_reconciler: NetworkReconciler::new(net_client.clone()),
            nic_reconciler: NicReconciler::new(net_client.clone()),
            template_reconciler: TemplateReconciler::new(zfs_client.clone()),
            volume_reconciler: VolumeReconciler::new(zfs_client.clone()),
            security_group_reconciler: SecurityGroupReconciler::new(net_client.clone()),
            route_reconciler: RouteReconciler::new(),
            vmm_client,
            net_client,
            zfs_client,
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

    /// Initialize known_ids from local sub-services (init from reality).
    async fn init_from_reality(&mut self) {
        // VMs from VMM
        match self.vmm_client.list_vms().await {
            Ok(vms) => {
                let ids: HashSet<String> = vms.iter().map(|v| v.id.clone()).collect();
                info!("Init from reality: {} VMs", ids.len());
                self.known_ids.insert("vm".to_string(), ids);
            }
            Err(e) => {
                warn!("Failed to list VMs from VMM: {}", e);
                self.known_ids.insert("vm".to_string(), HashSet::new());
            }
        }

        // Networks from net
        match self.net_client.list_networks().await {
            Ok(networks) => {
                let ids: HashSet<String> = networks.iter().map(|n| n.id.clone()).collect();
                info!("Init from reality: {} networks", ids.len());
                self.known_ids.insert("network".to_string(), ids);
            }
            Err(e) => {
                warn!("Failed to list networks from net: {}", e);
                self.known_ids.insert("network".to_string(), HashSet::new());
            }
        }

        // NICs from net
        match self.net_client.list_nics().await {
            Ok(nics) => {
                let ids: HashSet<String> = nics.iter().map(|n| n.id.clone()).collect();
                info!("Init from reality: {} NICs", ids.len());
                self.known_ids.insert("nic".to_string(), ids);
            }
            Err(e) => {
                warn!("Failed to list NICs from net: {}", e);
                self.known_ids.insert("nic".to_string(), HashSet::new());
            }
        }

        // Security groups from net
        match self.net_client.list_security_groups().await {
            Ok(sgs) => {
                let ids: HashSet<String> = sgs.iter().map(|s| s.id.clone()).collect();
                info!("Init from reality: {} security groups", ids.len());
                self.known_ids.insert("security_group".to_string(), ids);
            }
            Err(e) => {
                warn!("Failed to list security groups from net: {}", e);
                self.known_ids
                    .insert("security_group".to_string(), HashSet::new());
            }
        }

        // Volumes from ZFS
        match self.zfs_client.list_volumes().await {
            Ok(volumes) => {
                let ids: HashSet<String> = volumes.iter().map(|v| v.name.clone()).collect();
                info!("Init from reality: {} volumes", ids.len());
                self.known_ids.insert("volume".to_string(), ids);
            }
            Err(e) => {
                warn!("Failed to list volumes from ZFS: {}", e);
                self.known_ids.insert("volume".to_string(), HashSet::new());
            }
        }

        // Templates from ZFS
        match self.zfs_client.list_templates().await {
            Ok(templates) => {
                let ids: HashSet<String> = templates.iter().map(|t| t.name.clone()).collect();
                info!("Init from reality: {} templates", ids.len());
                self.known_ids.insert("template".to_string(), ids);
            }
            Err(e) => {
                warn!("Failed to list templates from ZFS: {}", e);
                self.known_ids
                    .insert("template".to_string(), HashSet::new());
            }
        }

        // Routes start empty (no local daemon to query)
        self.known_ids.entry("route".to_string()).or_default();
    }

    /// Main agent loop.
    pub async fn run(&mut self) -> Result<()> {
        // Init from reality before connecting
        self.init_from_reality().await;

        // Connect to API
        let mut client = self.connect().await?;

        // Create outbound channel for the bidirectional stream
        let (outbound_tx, outbound_rx) = mpsc::channel::<NodeMessage>(256);

        // Send Register as first message
        let register_msg = NodeMessage {
            payload: Some(node_message::Payload::Register(RegisterNode {
                node_id: self.node_id.clone().unwrap_or_default(),
                name: self.node_name.clone(),
                address: format!("{}:0", self.node_name),
                resources: Some((&self.resources).into()),
                labels: std::collections::HashMap::new(),
            })),
        };
        outbound_tx
            .send(register_msg)
            .await
            .context("Failed to send register message")?;

        // Open Sync stream
        let outbound_stream = tokio_stream::wrappers::ReceiverStream::new(outbound_rx);
        let mut inbound = client
            .sync(outbound_stream)
            .await
            .context("Failed to open Sync stream")?
            .into_inner();

        // Read RegisterResult
        let first_response = inbound
            .next()
            .await
            .ok_or_else(|| anyhow::anyhow!("Stream closed before register result"))?
            .context("Stream error reading register result")?;

        let register_result = match first_response.payload {
            Some(api_message::Payload::RegisterResult(r)) => r,
            _ => anyhow::bail!("Expected RegisterResult as first response"),
        };

        if !register_result.success {
            anyhow::bail!("Registration failed: {}", register_result.message);
        }

        let node_id = register_result.node_id;
        self.node_id = Some(node_id.clone());
        info!("Registered as node: {}", node_id);
        self.audit.registered(&node_id, &self.node_name);
        self.audit.connected(&node_id, &self.api_endpoint);

        // Spawn heartbeat task
        let heartbeat_tx = outbound_tx.clone();
        let heartbeat_interval = self.heartbeat_interval;
        let heartbeat_resources = self.resources.clone();
        let (stop_tx, mut stop_rx) = mpsc::channel::<()>(1);

        let heartbeat_handle = tokio::spawn(async move {
            let mut interval = interval(heartbeat_interval);
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        let msg = NodeMessage {
                            payload: Some(node_message::Payload::Heartbeat(Heartbeat {
                                current_resources: Some((&heartbeat_resources).into()),
                            })),
                        };
                        if heartbeat_tx.send(msg).await.is_err() {
                            break;
                        }
                        debug!("Heartbeat sent");
                    }
                    _ = stop_rx.recv() => {
                        info!("Heartbeat task stopping");
                        break;
                    }
                }
            }
        });

        // Main loop: receive manifests
        loop {
            match tokio::time::timeout(Duration::from_secs(60), inbound.next()).await {
                Ok(Some(Ok(msg))) => {
                    if let Some(api_message::Payload::Manifest(manifest)) = msg.payload {
                        self.apply_manifest(manifest, &outbound_tx, &node_id).await;
                    }
                }
                Ok(Some(Err(e))) => {
                    error!("Stream error: {}", e);
                    break;
                }
                Ok(None) => {
                    info!("Sync stream ended");
                    break;
                }
                Err(_) => {
                    debug!("Stream timeout, continuing...");
                }
            }
        }

        // Cleanup
        drop(stop_tx);
        let _ = heartbeat_handle.await;
        self.audit.disconnected(&node_id, "stream ended");

        Ok(())
    }

    /// Apply a manifest: reconcile all resources in dependency order, prune absent ones.
    async fn apply_manifest(
        &mut self,
        manifest: NodeManifest,
        outbound_tx: &mpsc::Sender<NodeMessage>,
        node_id: &str,
    ) {
        info!(
            "Applying manifest revision {} (vms={}, nets={}, nics={}, vols={}, tpls={}, sgs={})",
            manifest.revision,
            manifest.vms.len(),
            manifest.networks.len(),
            manifest.nics.len(),
            manifest.volumes.len(),
            manifest.templates.len(),
            manifest.security_groups.len(),
        );

        let mut status_update = StatusUpdate {
            vms: vec![],
            nics: vec![],
            templates: vec![],
            volumes: vec![],
            security_groups: vec![],
            routes: vec![],
        };

        // 1. Networks (global, no status report)
        {
            let desired: HashSet<String> = manifest
                .networks
                .iter()
                .filter_map(|n| n.meta.as_ref().map(|m| m.id.clone()))
                .collect();

            for spec in &manifest.networks {
                let id = spec.meta.as_ref().map(|m| m.id.clone()).unwrap_or_default();
                if let Err(e) = self.network_reconciler.reconcile(&id, spec).await {
                    error!("Network reconciliation failed for {}: {}", id, e);
                }
            }

            // Prune networks not in manifest
            let known = self.known_ids.entry("network".to_string()).or_default();
            for id in known.difference(&desired).cloned().collect::<Vec<_>>() {
                info!("Pruning network {}", id);
                if let Err(e) = self.network_reconciler.finalize(&id).await {
                    error!("Failed to prune network {}: {}", id, e);
                }
            }
            *known = desired;
        }

        // 2. Security Groups (global)
        {
            let desired: HashSet<String> = manifest
                .security_groups
                .iter()
                .filter_map(|s| s.meta.as_ref().map(|m| m.id.clone()))
                .collect();

            for spec in &manifest.security_groups {
                let id = spec.meta.as_ref().map(|m| m.id.clone()).unwrap_or_default();
                match self.security_group_reconciler.reconcile(&id, spec).await {
                    Ok(s) => status_update.security_groups.push(s),
                    Err(e) => error!("SecurityGroup reconciliation failed for {}: {}", id, e),
                }
            }

            let known = self
                .known_ids
                .entry("security_group".to_string())
                .or_default();
            for id in known.difference(&desired).cloned().collect::<Vec<_>>() {
                info!("Pruning security group {}", id);
                if let Err(e) = self.security_group_reconciler.finalize(&id).await {
                    error!("Failed to prune security group {}: {}", id, e);
                }
            }
            *known = desired;
        }

        // 3. Templates (report status)
        {
            let desired: HashSet<String> = manifest
                .templates
                .iter()
                .filter_map(|t| t.meta.as_ref().map(|m| m.id.clone()))
                .collect();

            for spec in &manifest.templates {
                let id = spec.meta.as_ref().map(|m| m.id.clone()).unwrap_or_default();
                match self.template_reconciler.reconcile(&id, spec).await {
                    Ok(s) => status_update.templates.push(s),
                    Err(e) => error!("Template reconciliation failed for {}: {}", id, e),
                }
            }

            let known = self.known_ids.entry("template".to_string()).or_default();
            for id in known.difference(&desired).cloned().collect::<Vec<_>>() {
                info!("Pruning template {}", id);
                if let Err(e) = self.template_reconciler.finalize(&id).await {
                    error!("Failed to prune template {}: {}", id, e);
                }
            }
            *known = desired;
        }

        // 4. Volumes (report status)
        {
            let desired: HashSet<String> = manifest
                .volumes
                .iter()
                .filter_map(|v| v.meta.as_ref().map(|m| m.id.clone()))
                .collect();

            for spec in &manifest.volumes {
                let id = spec.meta.as_ref().map(|m| m.id.clone()).unwrap_or_default();
                match self.volume_reconciler.reconcile(&id, spec).await {
                    Ok(s) => status_update.volumes.push(s),
                    Err(e) => error!("Volume reconciliation failed for {}: {}", id, e),
                }
            }

            let known = self.known_ids.entry("volume".to_string()).or_default();
            for id in known.difference(&desired).cloned().collect::<Vec<_>>() {
                info!("Pruning volume {}", id);
                if let Err(e) = self.volume_reconciler.finalize(&id).await {
                    error!("Failed to prune volume {}: {}", id, e);
                }
            }
            *known = desired;
        }

        // 5. NICs (report status)
        {
            let desired: HashSet<String> = manifest
                .nics
                .iter()
                .filter_map(|n| n.meta.as_ref().map(|m| m.id.clone()))
                .collect();

            for spec in &manifest.nics {
                let id = spec.meta.as_ref().map(|m| m.id.clone()).unwrap_or_default();
                match self.nic_reconciler.reconcile(&id, spec).await {
                    Ok(s) => status_update.nics.push(s),
                    Err(e) => error!("NIC reconciliation failed for {}: {}", id, e),
                }
            }

            let known = self.known_ids.entry("nic".to_string()).or_default();
            for id in known.difference(&desired).cloned().collect::<Vec<_>>() {
                info!("Pruning NIC {}", id);
                if let Err(e) = self.nic_reconciler.finalize(&id).await {
                    error!("Failed to prune NIC {}: {}", id, e);
                }
            }
            *known = desired;
        }

        // 6. VMs (report status)
        {
            let desired: HashSet<String> = manifest
                .vms
                .iter()
                .filter_map(|v| v.meta.as_ref().map(|m| m.id.clone()))
                .collect();

            for spec in &manifest.vms {
                let id = spec.meta.as_ref().map(|m| m.id.clone()).unwrap_or_default();
                match self.vm_reconciler.reconcile(&id, spec).await {
                    Ok(s) => status_update.vms.push(s),
                    Err(e) => error!("VM reconciliation failed for {}: {}", id, e),
                }
            }

            let known = self.known_ids.entry("vm".to_string()).or_default();
            for id in known.difference(&desired).cloned().collect::<Vec<_>>() {
                info!("Pruning VM {}", id);
                if let Err(e) = self.vm_reconciler.finalize(&id).await {
                    error!("Failed to prune VM {}: {}", id, e);
                }
            }
            *known = desired;
        }

        // 7. Routes (report status)
        {
            let desired: HashSet<String> = manifest
                .routes
                .iter()
                .filter_map(|r| r.meta.as_ref().map(|m| m.id.clone()))
                .collect();

            for spec in &manifest.routes {
                let id = spec.meta.as_ref().map(|m| m.id.clone()).unwrap_or_default();
                match self.route_reconciler.reconcile(&id, spec).await {
                    Ok(s) => status_update.routes.push(s),
                    Err(e) => error!("Route reconciliation failed for {}: {}", id, e),
                }
            }

            let known = self.known_ids.entry("route".to_string()).or_default();
            for id in known.difference(&desired).cloned().collect::<Vec<_>>() {
                info!("Pruning route {}", id);
                if let Err(e) = self.route_reconciler.finalize(&id).await {
                    error!("Failed to prune route {}: {}", id, e);
                }
            }
            *known = desired;
        }

        // Send status update
        let has_status = !status_update.vms.is_empty()
            || !status_update.nics.is_empty()
            || !status_update.templates.is_empty()
            || !status_update.volumes.is_empty()
            || !status_update.security_groups.is_empty()
            || !status_update.routes.is_empty();

        if has_status {
            let msg = NodeMessage {
                payload: Some(node_message::Payload::Status(status_update)),
            };
            if let Err(e) = outbound_tx.send(msg).await {
                error!("Failed to send status update: {}", e);
            }
        }
    }
}
