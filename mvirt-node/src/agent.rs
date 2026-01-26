//! Node agent - handles connection to mvirt-api and reconciliation.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::sync::mpsc;
use tokio::time::interval;
use tonic::transport::Channel;
use tracing::{debug, error, info, warn};

use crate::proto::node_service_client::NodeServiceClient;
use crate::proto::{
    HeartbeatRequest, NodeResources as ProtoNodeResources, RegisterRequest, SpecEvent,
    WatchSpecsRequest,
};

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
}

impl NodeAgent {
    pub fn new(
        api_endpoint: String,
        node_name: String,
        node_id: Option<String>,
        heartbeat_interval: Duration,
        resources: NodeResources,
        audit: Arc<NodeAuditLogger>,
    ) -> Self {
        Self {
            api_endpoint,
            node_name,
            node_id,
            heartbeat_interval,
            resources,
            audit,
            last_revision: 0,
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

    /// Send a heartbeat to the API server.
    async fn heartbeat(
        &self,
        client: &mut NodeServiceClient<Channel>,
        node_id: &str,
    ) -> Result<()> {
        let request = HeartbeatRequest {
            node_id: node_id.to_string(),
            current_resources: Some((&self.resources).into()),
        };

        let response = client
            .heartbeat(request)
            .await
            .context("Failed to send heartbeat")?
            .into_inner();

        if !response.success {
            warn!("Heartbeat failed: {}", response.message);
        } else {
            debug!("Heartbeat sent successfully");
        }

        Ok(())
    }

    /// Handle a spec event from the API server.
    async fn handle_spec_event(&mut self, node_id: &str, event: SpecEvent) -> Result<()> {
        use crate::proto::{spec_event::Spec, SpecEventType};

        self.last_revision = event.revision;
        let event_type =
            SpecEventType::try_from(event.r#type).unwrap_or(SpecEventType::Unspecified);

        if let Some(spec) = event.spec {
            match spec {
                Spec::Network(net_spec) => {
                    let action = match event_type {
                        SpecEventType::Delete => "delete",
                        SpecEventType::Create => "create",
                        SpecEventType::Update => "update",
                        _ => "unknown",
                    };
                    info!("Received network {} event: {}", action, net_spec.id);
                    self.audit.spec_received(node_id, "network", &net_spec.id);
                    // TODO: Reconcile with mvirt-net based on event_type
                }
                Spec::Vm(vm_spec) => {
                    let action = match event_type {
                        SpecEventType::Delete => "delete",
                        SpecEventType::Create => "create",
                        SpecEventType::Update => "update",
                        _ => "unknown",
                    };
                    info!("Received VM {} event: {}", action, vm_spec.id);
                    self.audit.spec_received(node_id, "vm", &vm_spec.id);
                    // TODO: Reconcile with mvirt-vmm based on event_type
                }
                Spec::Nic(nic_spec) => {
                    let action = match event_type {
                        SpecEventType::Delete => "delete",
                        SpecEventType::Create => "create",
                        SpecEventType::Update => "update",
                        _ => "unknown",
                    };
                    info!("Received NIC {} event: {}", action, nic_spec.id);
                    self.audit.spec_received(node_id, "nic", &nic_spec.id);
                    // TODO: Reconcile with mvirt-net based on event_type
                }
            }
        }

        Ok(())
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
                    if let Err(e) = self.handle_spec_event(&node_id, event).await {
                        error!("Failed to handle spec event: {}", e);
                    }
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
                    // Timeout - continue to allow reconnection handling
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
