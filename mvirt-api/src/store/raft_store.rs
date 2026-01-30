//! RaftStore implementation - bridges DataStore traits to RaftNode.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use mraft::RaftNode;
use tokio::sync::{RwLock, broadcast};

use crate::command::{
    Command, ImportJobData, ImportJobState, NetworkData, NicData, NodeData, ProjectData, Response,
    TemplateData, VmData, VmPhase, VmStatus, VolumeData,
};
use crate::scheduler::Scheduler;
use crate::state::ApiState;

use super::error::{Result, StoreError};
use super::event::Event;
use super::traits::{
    ControlplaneInfo, ControlplaneStore, CreateNetworkRequest, CreateNicRequest,
    CreateProjectRequest, CreateSecurityGroupRequest, CreateSecurityGroupRuleRequest,
    CreateSnapshotRequest, CreateTemplateRequest, CreateVmRequest, CreateVolumeRequest, DataStore,
    DeleteNetworkResult, ImportTemplateRequest, Membership, MembershipPeer, NetworkStore, NicStore,
    NodeStore, ProjectStore, RegisterNodeRequest, ResizeVolumeRequest, SecurityGroupStore,
    TemplateStore, UpdateNetworkRequest, UpdateNicRequest, UpdateNodeStatusRequest,
    UpdateVmSpecRequest, UpdateVmStatusRequest, VmStore, VolumeStore,
};

/// RaftStore wraps a RaftNode and implements the DataStore trait.
///
/// This provides a clean abstraction over Raft operations, hiding the
/// Command/Response types from handlers.
pub struct RaftStore {
    node: Arc<RwLock<RaftNode<Command, Response, ApiState>>>,
    events: broadcast::Sender<Event>,
    node_id: u64,
}

impl RaftStore {
    /// Create a new RaftStore wrapping the given RaftNode.
    pub fn new(
        node: Arc<RwLock<RaftNode<Command, Response, ApiState>>>,
        events: broadcast::Sender<Event>,
        node_id: u64,
    ) -> Self {
        Self {
            node,
            events,
            node_id,
        }
    }

    /// Get the event sender (for wiring up to the state machine).
    pub fn event_sender(&self) -> broadcast::Sender<Event> {
        self.events.clone()
    }

    /// Execute a write command through Raft.
    async fn write_command(&self, cmd: Command) -> Result<Response> {
        let node = self.node.read().await;
        node.write_or_forward(cmd)
            .await
            .map_err(|e| StoreError::Internal(e.to_string()))
    }
}

#[async_trait]
impl NodeStore for RaftStore {
    async fn list_nodes(&self) -> Result<Vec<NodeData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.list_nodes().into_iter().cloned().collect())
    }

    async fn list_online_nodes(&self) -> Result<Vec<NodeData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.list_online_nodes().into_iter().cloned().collect())
    }

    async fn get_node(&self, id: &str) -> Result<Option<NodeData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.get_node(id).cloned())
    }

    async fn get_node_by_name(&self, name: &str) -> Result<Option<NodeData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.get_node_by_name(name).cloned())
    }

    async fn register_node(&self, req: RegisterNodeRequest) -> Result<NodeData> {
        let cmd = Command::RegisterNode {
            request_id: uuid::Uuid::new_v4().to_string(),
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now().to_rfc3339(),
            name: req.name,
            address: req.address,
            resources: req.resources,
            labels: req.labels,
        };

        match self.write_command(cmd).await? {
            Response::Node(data) => Ok(data),
            Response::Error { code: 409, message } => Err(StoreError::Conflict(message)),
            Response::Error { message, .. } => Err(StoreError::Internal(message)),
            _ => Err(StoreError::Internal("unexpected response".into())),
        }
    }

    async fn update_node_status(&self, id: &str, req: UpdateNodeStatusRequest) -> Result<NodeData> {
        let cmd = Command::UpdateNodeStatus {
            request_id: uuid::Uuid::new_v4().to_string(),
            node_id: id.to_string(),
            timestamp: Utc::now().to_rfc3339(),
            status: req.status,
            resources: req.resources,
        };

        match self.write_command(cmd).await? {
            Response::Node(data) => Ok(data),
            Response::Error { code: 404, message } => Err(StoreError::NotFound(message)),
            Response::Error { message, .. } => Err(StoreError::Internal(message)),
            _ => Err(StoreError::Internal("unexpected response".into())),
        }
    }

    async fn deregister_node(&self, id: &str) -> Result<()> {
        let cmd = Command::DeregisterNode {
            request_id: uuid::Uuid::new_v4().to_string(),
            node_id: id.to_string(),
        };

        match self.write_command(cmd).await? {
            Response::Deleted { .. } => Ok(()),
            Response::Error { code: 404, message } => Err(StoreError::NotFound(message)),
            Response::Error { message, .. } => Err(StoreError::Internal(message)),
            _ => Err(StoreError::Internal("unexpected response".into())),
        }
    }
}

#[async_trait]
impl NetworkStore for RaftStore {
    async fn list_networks(&self) -> Result<Vec<NetworkData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.list_networks().into_iter().cloned().collect())
    }

    async fn list_networks_by_project(&self, project_id: &str) -> Result<Vec<NetworkData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state
            .list_networks_by_project(project_id)
            .into_iter()
            .cloned()
            .collect())
    }

    async fn get_network(&self, id: &str) -> Result<Option<NetworkData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.get_network(id).cloned())
    }

    async fn get_network_by_name(&self, name: &str) -> Result<Option<NetworkData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.get_network_by_name(name).cloned())
    }

    async fn create_network(&self, req: CreateNetworkRequest) -> Result<NetworkData> {
        let cmd = Command::CreateNetwork {
            request_id: uuid::Uuid::new_v4().to_string(),
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now().to_rfc3339(),
            name: req.name,
            ipv4_enabled: req.ipv4_enabled,
            project_id: req.project_id,
            ipv4_prefix: req.ipv4_prefix,
            ipv6_enabled: req.ipv6_enabled,
            ipv6_prefix: req.ipv6_prefix,
            dns_servers: req.dns_servers,
            ntp_servers: req.ntp_servers,
            is_public: req.is_public,
        };

        match self.write_command(cmd).await? {
            Response::Network(data) => Ok(data),
            Response::Error { code: 409, message } => Err(StoreError::Conflict(message)),
            Response::Error { message, .. } => Err(StoreError::Internal(message)),
            _ => Err(StoreError::Internal("unexpected response".into())),
        }
    }

    async fn update_network(&self, id: &str, req: UpdateNetworkRequest) -> Result<NetworkData> {
        let cmd = Command::UpdateNetwork {
            request_id: uuid::Uuid::new_v4().to_string(),
            id: id.to_string(),
            timestamp: Utc::now().to_rfc3339(),
            dns_servers: req.dns_servers,
            ntp_servers: req.ntp_servers,
        };

        match self.write_command(cmd).await? {
            Response::Network(data) => Ok(data),
            Response::Error { code: 404, message } => Err(StoreError::NotFound(message)),
            Response::Error { message, .. } => Err(StoreError::Internal(message)),
            _ => Err(StoreError::Internal("unexpected response".into())),
        }
    }

    async fn delete_network(&self, id: &str, force: bool) -> Result<DeleteNetworkResult> {
        let cmd = Command::DeleteNetwork {
            request_id: uuid::Uuid::new_v4().to_string(),
            id: id.to_string(),
            force,
        };

        match self.write_command(cmd).await? {
            Response::Deleted { .. } => Ok(DeleteNetworkResult { nics_deleted: 0 }),
            Response::DeletedWithCount { nics_deleted, .. } => {
                Ok(DeleteNetworkResult { nics_deleted })
            }
            Response::Error { code: 404, message } => Err(StoreError::NotFound(message)),
            Response::Error { code: 409, message } => Err(StoreError::Conflict(message)),
            Response::Error { message, .. } => Err(StoreError::Internal(message)),
            _ => Err(StoreError::Internal("unexpected response".into())),
        }
    }
}

#[async_trait]
impl NicStore for RaftStore {
    async fn list_nics(&self, network_id: Option<&str>) -> Result<Vec<NicData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.list_nics(network_id).into_iter().cloned().collect())
    }

    async fn list_nics_by_project(&self, project_id: &str) -> Result<Vec<NicData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state
            .list_nics_by_project(project_id)
            .into_iter()
            .cloned()
            .collect())
    }

    async fn get_nic(&self, id: &str) -> Result<Option<NicData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.get_nic(id).cloned())
    }

    async fn get_nic_by_name(&self, name: &str) -> Result<Option<NicData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.get_nic_by_name(name).cloned())
    }

    async fn create_nic(&self, req: CreateNicRequest) -> Result<NicData> {
        let cmd = Command::CreateNic {
            request_id: uuid::Uuid::new_v4().to_string(),
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now().to_rfc3339(),
            project_id: req.project_id,
            network_id: req.network_id,
            name: req.name,
            mac_address: req.mac_address,
            ipv4_address: req.ipv4_address,
            ipv6_address: req.ipv6_address,
            routed_ipv4_prefixes: req.routed_ipv4_prefixes,
            routed_ipv6_prefixes: req.routed_ipv6_prefixes,
            security_group_id: req.security_group_id,
        };

        match self.write_command(cmd).await? {
            Response::Nic(data) => Ok(data),
            Response::Error { code: 404, message } => Err(StoreError::NotFound(message)),
            Response::Error { code: 409, message } => Err(StoreError::Conflict(message)),
            Response::Error { message, .. } => Err(StoreError::Internal(message)),
            _ => Err(StoreError::Internal("unexpected response".into())),
        }
    }

    async fn update_nic(&self, id: &str, req: UpdateNicRequest) -> Result<NicData> {
        let cmd = Command::UpdateNic {
            request_id: uuid::Uuid::new_v4().to_string(),
            id: id.to_string(),
            timestamp: Utc::now().to_rfc3339(),
            routed_ipv4_prefixes: req.routed_ipv4_prefixes,
            routed_ipv6_prefixes: req.routed_ipv6_prefixes,
        };

        match self.write_command(cmd).await? {
            Response::Nic(data) => Ok(data),
            Response::Error { code: 404, message } => Err(StoreError::NotFound(message)),
            Response::Error { message, .. } => Err(StoreError::Internal(message)),
            _ => Err(StoreError::Internal("unexpected response".into())),
        }
    }

    async fn delete_nic(&self, id: &str) -> Result<()> {
        let cmd = Command::DeleteNic {
            request_id: uuid::Uuid::new_v4().to_string(),
            id: id.to_string(),
        };

        match self.write_command(cmd).await? {
            Response::Deleted { .. } => Ok(()),
            Response::Error { code: 404, message } => Err(StoreError::NotFound(message)),
            Response::Error { message, .. } => Err(StoreError::Internal(message)),
            _ => Err(StoreError::Internal("unexpected response".into())),
        }
    }

    async fn attach_nic(&self, id: &str, vm_id: &str) -> Result<NicData> {
        let cmd = Command::AttachNic {
            request_id: uuid::Uuid::new_v4().to_string(),
            id: id.to_string(),
            timestamp: Utc::now().to_rfc3339(),
            vm_id: vm_id.to_string(),
        };
        match self.write_command(cmd).await? {
            Response::Nic(data) => Ok(data),
            Response::Error { code: 404, message } => Err(StoreError::NotFound(message)),
            Response::Error { code: 409, message } => Err(StoreError::Conflict(message)),
            Response::Error { message, .. } => Err(StoreError::Internal(message)),
            _ => Err(StoreError::Internal("unexpected response".into())),
        }
    }

    async fn detach_nic(&self, id: &str) -> Result<NicData> {
        let cmd = Command::DetachNic {
            request_id: uuid::Uuid::new_v4().to_string(),
            id: id.to_string(),
            timestamp: Utc::now().to_rfc3339(),
        };
        match self.write_command(cmd).await? {
            Response::Nic(data) => Ok(data),
            Response::Error { code: 404, message } => Err(StoreError::NotFound(message)),
            Response::Error { message, .. } => Err(StoreError::Internal(message)),
            _ => Err(StoreError::Internal("unexpected response".into())),
        }
    }
}

#[async_trait]
impl VmStore for RaftStore {
    async fn list_vms(&self) -> Result<Vec<VmData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.list_vms(None).into_iter().cloned().collect())
    }

    async fn list_vms_by_project(&self, project_id: &str) -> Result<Vec<VmData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state
            .list_vms_by_project(project_id)
            .into_iter()
            .cloned()
            .collect())
    }

    async fn list_vms_by_node(&self, node_id: &str) -> Result<Vec<VmData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.list_vms(Some(node_id)).into_iter().cloned().collect())
    }

    async fn get_vm(&self, id: &str) -> Result<Option<VmData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.get_vm(id).cloned())
    }

    async fn get_vm_by_name(&self, name: &str) -> Result<Option<VmData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.get_vm_by_name(name).cloned())
    }

    async fn create_vm(&self, req: CreateVmRequest) -> Result<VmData> {
        let cmd = Command::CreateVm {
            request_id: uuid::Uuid::new_v4().to_string(),
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now().to_rfc3339(),
            spec: req.spec,
        };

        match self.write_command(cmd).await? {
            Response::Vm(data) => Ok(data),
            Response::Error { code: 404, message } => Err(StoreError::NotFound(message)),
            Response::Error { code: 409, message } => Err(StoreError::Conflict(message)),
            Response::Error { message, .. } => Err(StoreError::Internal(message)),
            _ => Err(StoreError::Internal("unexpected response".into())),
        }
    }

    async fn create_and_schedule_vm(&self, req: CreateVmRequest) -> Result<VmData> {
        // First, get all nodes to schedule
        let nodes = self.list_nodes().await?;

        // Use scheduler to pick a node
        let scheduler = Scheduler::new();
        let schedule_result = scheduler
            .select_node(&nodes, &req.spec)
            .map_err(|e| StoreError::ScheduleFailed(e.to_string()))?;

        // Create the VM
        let vm = self.create_vm(req).await?;

        // Update the VM status with the scheduled node
        let status_req = UpdateVmStatusRequest {
            status: VmStatus {
                phase: VmPhase::Scheduled,
                node_id: Some(schedule_result.node_id),
                ip_address: None,
                message: Some(schedule_result.reason),
            },
        };

        self.update_vm_status(&vm.id, status_req).await
    }

    async fn update_vm_spec(&self, id: &str, req: UpdateVmSpecRequest) -> Result<VmData> {
        let cmd = Command::UpdateVmSpec {
            request_id: uuid::Uuid::new_v4().to_string(),
            id: id.to_string(),
            timestamp: Utc::now().to_rfc3339(),
            desired_state: req.desired_state,
        };

        match self.write_command(cmd).await? {
            Response::Vm(data) => Ok(data),
            Response::Error { code: 404, message } => Err(StoreError::NotFound(message)),
            Response::Error { message, .. } => Err(StoreError::Internal(message)),
            _ => Err(StoreError::Internal("unexpected response".into())),
        }
    }

    async fn update_vm_status(&self, id: &str, req: UpdateVmStatusRequest) -> Result<VmData> {
        let cmd = Command::UpdateVmStatus {
            request_id: uuid::Uuid::new_v4().to_string(),
            id: id.to_string(),
            timestamp: Utc::now().to_rfc3339(),
            status: req.status,
        };

        match self.write_command(cmd).await? {
            Response::Vm(data) => Ok(data),
            Response::Error { code: 404, message } => Err(StoreError::NotFound(message)),
            Response::Error { message, .. } => Err(StoreError::Internal(message)),
            _ => Err(StoreError::Internal("unexpected response".into())),
        }
    }

    async fn delete_vm(&self, id: &str) -> Result<()> {
        let cmd = Command::DeleteVm {
            request_id: uuid::Uuid::new_v4().to_string(),
            id: id.to_string(),
        };

        match self.write_command(cmd).await? {
            Response::Deleted { .. } => Ok(()),
            Response::Error { code: 404, message } => Err(StoreError::NotFound(message)),
            Response::Error { message, .. } => Err(StoreError::Internal(message)),
            _ => Err(StoreError::Internal("unexpected response".into())),
        }
    }
}

#[async_trait]
impl ControlplaneStore for RaftStore {
    async fn get_controlplane_info(&self) -> Result<ControlplaneInfo> {
        let node = self.node.read().await;
        let metrics = node.metrics();

        Ok(ControlplaneInfo {
            cluster_id: "mvirt-cluster".to_string(),
            leader_id: metrics.current_leader,
            current_term: metrics.current_term,
            commit_index: metrics.last_applied.map(|l| l.index).unwrap_or(0),
            peer_id: self.node_id,
            is_leader: metrics.current_leader == Some(self.node_id),
        })
    }

    async fn get_membership(&self) -> Result<Membership> {
        let node = self.node.read().await;
        let membership = node.get_membership();

        let peers: Vec<MembershipPeer> = membership
            .nodes
            .iter()
            .map(|(id, addr)| {
                let role = if membership.voters.contains(id) {
                    "voter"
                } else if membership.learners.contains(id) {
                    "learner"
                } else {
                    "unknown"
                };
                MembershipPeer {
                    id: *id,
                    address: addr.clone(),
                    role: role.to_string(),
                }
            })
            .collect();

        Ok(Membership {
            voters: membership.voters.into_iter().collect(),
            learners: membership.learners.into_iter().collect(),
            peers,
        })
    }

    async fn create_join_token(&self, peer_id: u64, valid_for_secs: u64) -> Result<String> {
        let node = self.node.read().await;
        node.create_join_token(peer_id, valid_for_secs)
            .await
            .map_err(|e| match e {
                mraft::JoinError::NotLeader => StoreError::NotLeader { leader_id: None },
                mraft::JoinError::NoClusterSecret => {
                    StoreError::Internal("Cluster secret not configured".into())
                }
                _ => StoreError::Internal(e.to_string()),
            })
    }

    async fn remove_peer(&self, peer_id: u64) -> Result<()> {
        let node = self.node.read().await;
        node.remove_node(peer_id).await.map_err(|e| match e {
            mraft::JoinError::NotLeader => StoreError::NotLeader { leader_id: None },
            mraft::JoinError::NotMember(_) => {
                StoreError::NotFound(format!("Peer {} not found", peer_id))
            }
            _ => StoreError::Internal(e.to_string()),
        })
    }
}

#[async_trait]
impl ProjectStore for RaftStore {
    async fn list_projects(&self) -> Result<Vec<ProjectData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.list_projects().into_iter().cloned().collect())
    }

    async fn get_project(&self, id: &str) -> Result<Option<ProjectData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.get_project(id).cloned())
    }

    async fn get_project_by_name(&self, name: &str) -> Result<Option<ProjectData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.get_project_by_name(name).cloned())
    }

    async fn create_project(&self, req: CreateProjectRequest) -> Result<ProjectData> {
        let cmd = Command::CreateProject {
            request_id: uuid::Uuid::new_v4().to_string(),
            id: req.id,
            timestamp: Utc::now().to_rfc3339(),
            name: req.name,
            description: req.description,
        };

        match self.write_command(cmd).await? {
            Response::Project(data) => Ok(data),
            Response::Error { code: 409, message } => Err(StoreError::Conflict(message)),
            Response::Error { message, .. } => Err(StoreError::Internal(message)),
            _ => Err(StoreError::Internal("unexpected response".into())),
        }
    }

    async fn delete_project(&self, id: &str) -> Result<()> {
        let cmd = Command::DeleteProject {
            request_id: uuid::Uuid::new_v4().to_string(),
            id: id.to_string(),
        };

        match self.write_command(cmd).await? {
            Response::Deleted { .. } => Ok(()),
            Response::Error { code: 404, message } => Err(StoreError::NotFound(message)),
            Response::Error { message, .. } => Err(StoreError::Internal(message)),
            _ => Err(StoreError::Internal("unexpected response".into())),
        }
    }
}

#[async_trait]
impl VolumeStore for RaftStore {
    async fn list_volumes(
        &self,
        project_id: Option<&str>,
        node_id: Option<&str>,
    ) -> Result<Vec<VolumeData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state
            .list_volumes(project_id, node_id)
            .into_iter()
            .cloned()
            .collect())
    }

    async fn get_volume(&self, id: &str) -> Result<Option<VolumeData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.get_volume(id).cloned())
    }

    async fn create_volume(&self, req: CreateVolumeRequest) -> Result<VolumeData> {
        let cmd = Command::CreateVolume {
            request_id: uuid::Uuid::new_v4().to_string(),
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now().to_rfc3339(),
            project_id: req.project_id,
            node_id: req.node_id,
            name: req.name,
            size_bytes: req.size_bytes,
            template_id: req.template_id,
        };

        match self.write_command(cmd).await? {
            Response::Volume(data) => Ok(data),
            Response::Error { code: 404, message } => Err(StoreError::NotFound(message)),
            Response::Error { code: 409, message } => Err(StoreError::Conflict(message)),
            Response::Error { message, .. } => Err(StoreError::Internal(message)),
            _ => Err(StoreError::Internal("unexpected response".into())),
        }
    }

    async fn delete_volume(&self, id: &str) -> Result<()> {
        let cmd = Command::DeleteVolume {
            request_id: uuid::Uuid::new_v4().to_string(),
            id: id.to_string(),
        };

        match self.write_command(cmd).await? {
            Response::Deleted { .. } => Ok(()),
            Response::Error { code: 404, message } => Err(StoreError::NotFound(message)),
            Response::Error { message, .. } => Err(StoreError::Internal(message)),
            _ => Err(StoreError::Internal("unexpected response".into())),
        }
    }

    async fn resize_volume(&self, id: &str, req: ResizeVolumeRequest) -> Result<VolumeData> {
        let cmd = Command::ResizeVolume {
            request_id: uuid::Uuid::new_v4().to_string(),
            id: id.to_string(),
            timestamp: Utc::now().to_rfc3339(),
            size_bytes: req.size_bytes,
        };

        match self.write_command(cmd).await? {
            Response::Volume(data) => Ok(data),
            Response::Error { code: 404, message } => Err(StoreError::NotFound(message)),
            Response::Error { code: 400, message } => Err(StoreError::Conflict(message)),
            Response::Error { message, .. } => Err(StoreError::Internal(message)),
            _ => Err(StoreError::Internal("unexpected response".into())),
        }
    }

    async fn create_snapshot(
        &self,
        volume_id: &str,
        req: CreateSnapshotRequest,
    ) -> Result<VolumeData> {
        let cmd = Command::CreateSnapshot {
            request_id: uuid::Uuid::new_v4().to_string(),
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now().to_rfc3339(),
            volume_id: volume_id.to_string(),
            name: req.name,
        };

        match self.write_command(cmd).await? {
            Response::Volume(data) => Ok(data),
            Response::Error { code: 404, message } => Err(StoreError::NotFound(message)),
            Response::Error { code: 409, message } => Err(StoreError::Conflict(message)),
            Response::Error { message, .. } => Err(StoreError::Internal(message)),
            _ => Err(StoreError::Internal("unexpected response".into())),
        }
    }
}

#[async_trait]
impl TemplateStore for RaftStore {
    async fn list_templates(&self, node_id: Option<&str>) -> Result<Vec<TemplateData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.list_templates(node_id).into_iter().cloned().collect())
    }

    async fn list_templates_by_project(&self, project_id: &str) -> Result<Vec<TemplateData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state
            .list_templates_by_project(project_id)
            .into_iter()
            .cloned()
            .collect())
    }

    async fn get_template(&self, id: &str) -> Result<Option<TemplateData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.get_template(id).cloned())
    }

    async fn create_template(&self, req: CreateTemplateRequest) -> Result<TemplateData> {
        let cmd = Command::CreateTemplate {
            request_id: uuid::Uuid::new_v4().to_string(),
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now().to_rfc3339(),
            project_id: req.project_id,
            node_id: req.node_id,
            name: req.name,
            size_bytes: req.size_bytes,
        };

        match self.write_command(cmd).await? {
            Response::Template(data) => Ok(data),
            Response::Error { message, .. } => Err(StoreError::Internal(message)),
            _ => Err(StoreError::Internal("unexpected response".into())),
        }
    }

    async fn import_template(&self, req: ImportTemplateRequest) -> Result<ImportJobData> {
        let cmd = Command::CreateImportJob {
            request_id: uuid::Uuid::new_v4().to_string(),
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now().to_rfc3339(),
            project_id: req.project_id,
            node_id: req.node_id,
            template_name: req.name,
            url: req.url,
            total_bytes: req.total_bytes,
        };

        match self.write_command(cmd).await? {
            Response::ImportJob(data) => Ok(data),
            Response::Error { message, .. } => Err(StoreError::Internal(message)),
            _ => Err(StoreError::Internal("unexpected response".into())),
        }
    }

    async fn get_import_job(&self, id: &str) -> Result<Option<ImportJobData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.get_import_job(id).cloned())
    }

    async fn update_import_job(
        &self,
        id: &str,
        bytes_written: u64,
        state: ImportJobState,
        error: Option<String>,
    ) -> Result<ImportJobData> {
        let cmd = Command::UpdateImportJob {
            request_id: uuid::Uuid::new_v4().to_string(),
            id: id.to_string(),
            timestamp: Utc::now().to_rfc3339(),
            bytes_written,
            state,
            error,
        };

        match self.write_command(cmd).await? {
            Response::ImportJob(data) => Ok(data),
            Response::Error { code: 404, message } => Err(StoreError::NotFound(message)),
            Response::Error { message, .. } => Err(StoreError::Internal(message)),
            _ => Err(StoreError::Internal("unexpected response".into())),
        }
    }
}

#[async_trait]
impl SecurityGroupStore for RaftStore {
    async fn list_security_groups(
        &self,
        project_id: Option<&str>,
    ) -> Result<Vec<crate::command::SecurityGroupData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        match project_id {
            Some(pid) => Ok(state
                .list_security_groups_by_project(pid)
                .into_iter()
                .cloned()
                .collect()),
            None => Ok(state.list_security_groups().into_iter().cloned().collect()),
        }
    }

    async fn get_security_group(
        &self,
        id: &str,
    ) -> Result<Option<crate::command::SecurityGroupData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.get_security_group(id).cloned())
    }

    async fn create_security_group(
        &self,
        req: CreateSecurityGroupRequest,
    ) -> Result<crate::command::SecurityGroupData> {
        let cmd = Command::CreateSecurityGroup {
            request_id: uuid::Uuid::new_v4().to_string(),
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now().to_rfc3339(),
            project_id: req.project_id,
            name: req.name,
            description: req.description,
        };
        match self.write_command(cmd).await? {
            Response::SecurityGroup(sg) => Ok(sg),
            Response::Error { code, message } => match code {
                404 => Err(StoreError::NotFound(message)),
                409 => Err(StoreError::Conflict(message)),
                _ => Err(StoreError::Internal(message)),
            },
            _ => Err(StoreError::Internal("unexpected response".into())),
        }
    }

    async fn delete_security_group(&self, id: &str) -> Result<()> {
        let cmd = Command::DeleteSecurityGroup {
            request_id: uuid::Uuid::new_v4().to_string(),
            id: id.to_string(),
        };
        match self.write_command(cmd).await? {
            Response::Deleted { .. } => Ok(()),
            Response::Error { code, message } => match code {
                404 => Err(StoreError::NotFound(message)),
                409 => Err(StoreError::Conflict(message)),
                _ => Err(StoreError::Internal(message)),
            },
            _ => Err(StoreError::Internal("unexpected response".into())),
        }
    }

    async fn create_security_group_rule(
        &self,
        security_group_id: &str,
        req: CreateSecurityGroupRuleRequest,
    ) -> Result<crate::command::SecurityGroupData> {
        let cmd = Command::CreateSecurityGroupRule {
            request_id: uuid::Uuid::new_v4().to_string(),
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now().to_rfc3339(),
            security_group_id: security_group_id.to_string(),
            direction: req.direction,
            protocol: req.protocol,
            port_range_start: req.port_range_start,
            port_range_end: req.port_range_end,
            cidr: req.cidr,
            description: req.description,
        };
        match self.write_command(cmd).await? {
            Response::SecurityGroup(sg) => Ok(sg),
            Response::Error { code, message } => match code {
                404 => Err(StoreError::NotFound(message)),
                409 => Err(StoreError::Conflict(message)),
                _ => Err(StoreError::Internal(message)),
            },
            _ => Err(StoreError::Internal("unexpected response".into())),
        }
    }

    async fn delete_security_group_rule(
        &self,
        security_group_id: &str,
        rule_id: &str,
    ) -> Result<crate::command::SecurityGroupData> {
        let cmd = Command::DeleteSecurityGroupRule {
            request_id: uuid::Uuid::new_v4().to_string(),
            security_group_id: security_group_id.to_string(),
            rule_id: rule_id.to_string(),
        };
        match self.write_command(cmd).await? {
            Response::SecurityGroup(sg) => Ok(sg),
            Response::Error { code, message } => match code {
                404 => Err(StoreError::NotFound(message)),
                409 => Err(StoreError::Conflict(message)),
                _ => Err(StoreError::Internal(message)),
            },
            _ => Err(StoreError::Internal("unexpected response".into())),
        }
    }
}

impl DataStore for RaftStore {
    fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }
}
