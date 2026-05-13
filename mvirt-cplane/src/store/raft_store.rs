//! RaftStore implementation - bridges DataStore traits to RaftNode.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use mraft::RaftNode;
use tokio::sync::{RwLock, broadcast};

use crate::command::{
    AccountData, ClusterData, Command, MembershipData, MembershipScope, NetworkData, NicData,
    NodeData, OrgContact, OrgData, ProjectData, Response, TemplateData, VmData, VmPhase, VmStatus,
    VolumeData,
};
use crate::scheduler::Scheduler;
use crate::state::ApiState;

use super::error::{Result, StoreError};
use super::event::Event;
use super::traits::{
    AccountStore, BootstrapOutcome, ClusterStore, ControlplaneInfo, ControlplaneStore,
    CreateClusterRequest, CreateMembershipRequest, CreateNetworkRequest, CreateNicRequest,
    CreateOnboardingTokenRequest, CreateOrgRequest, CreateProjectRequest,
    CreateSecurityGroupRequest, CreateSecurityGroupRuleRequest, CreateSnapshotRequest,
    CreateTemplateRequest, CreateVmRequest, CreateVolumeRequest, DataStore, DeleteNetworkResult,
    EnsureAccountRequest, Membership, MembershipPeer, NetworkStore, NicStore, NodeStore,
    OnboardingStore, OrgStore, ProjectStore, RedeemOnboardingTokenRequest, RegisterNodeRequest,
    ResizeVolumeRequest, SecurityGroupStore, TemplateStore, UpdateClusterRequest,
    UpdateNetworkRequest, UpdateNetworkStatusRequest, UpdateNicRequest, UpdateNicStatusRequest,
    UpdateNodeStatusRequest, UpdateOrgRequest, UpdateSecurityGroupRequest,
    UpdateSecurityGroupRuleRequest, UpdateTemplateStatusRequest, UpdateVmSpecRequest,
    UpdateVmStatusRequest, UpdateVolumeStatusRequest, VmStore, VolumeStore,
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

    /// Submit an arbitrary command through Raft. Used by reconcilers to push
    /// status updates back into the state machine.
    pub async fn submit(&self, cmd: Command) -> Result<Response> {
        self.write_command(cmd).await
    }

    /// Read the current ApiState snapshot. Used by reconcilers for resync.
    pub async fn snapshot(&self) -> ApiState {
        let node = self.node.read().await;
        node.get_state().await.clone()
    }
}

#[async_trait]
impl NodeStore for RaftStore {
    async fn list_nodes(&self) -> Result<Vec<NodeData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.list_nodes())
    }

    async fn list_online_nodes(&self) -> Result<Vec<NodeData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.list_online_nodes())
    }

    async fn get_node(&self, id: &str) -> Result<Option<NodeData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.get_node(id))
    }

    async fn get_node_by_name(&self, name: &str) -> Result<Option<NodeData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.get_node_by_name(name))
    }

    async fn register_node(&self, req: RegisterNodeRequest) -> Result<NodeData> {
        self.upsert_node(&uuid::Uuid::new_v4().to_string(), req)
            .await
    }

    async fn upsert_node(&self, id: &str, req: RegisterNodeRequest) -> Result<NodeData> {
        let cmd = Command::RegisterNode {
            request_id: uuid::Uuid::new_v4().to_string(),
            id: id.to_string(),
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
        Ok(state.list_networks())
    }

    async fn list_networks_by_project(&self, project_slug: &str) -> Result<Vec<NetworkData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.list_networks_by_project(project_slug))
    }

    async fn get_network(&self, id: &str) -> Result<Option<NetworkData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.get_network(id))
    }

    async fn get_network_by_name(&self, name: &str) -> Result<Option<NetworkData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.get_network_by_name(name))
    }

    async fn create_network(&self, req: CreateNetworkRequest) -> Result<NetworkData> {
        let cmd = Command::CreateNetwork {
            request_id: uuid::Uuid::new_v4().to_string(),
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now().to_rfc3339(),
            name: req.name,
            ipv4_enabled: req.ipv4_enabled,
            project_slug: req.project_slug,
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

    async fn update_network_status(
        &self,
        id: &str,
        _req: UpdateNetworkStatusRequest,
    ) -> Result<NetworkData> {
        // No Command::UpdateNetworkStatus on raft today — NetworkData is
        // spec-only. We accept the call (so the trait surface is uniform
        // across all five resources) but treat it as a no-op: just fetch
        // and return the current row. When NetworkData grows status
        // fields, swap in a real Command + Response variant.
        let node = self.node.read().await;
        let state = node.get_state().await;
        state
            .get_network(id)
            .ok_or_else(|| StoreError::NotFound(format!("Network '{}' not found", id)))
    }
}

#[async_trait]
impl NicStore for RaftStore {
    async fn list_nics(&self, network_id: Option<&str>) -> Result<Vec<NicData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.list_nics(network_id))
    }

    async fn list_nics_by_project(&self, project_slug: &str) -> Result<Vec<NicData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.list_nics_by_project(project_slug))
    }

    async fn get_nic(&self, id: &str) -> Result<Option<NicData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.get_nic(id))
    }

    async fn get_nic_by_name(&self, name: &str) -> Result<Option<NicData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.get_nic_by_name(name))
    }

    async fn create_nic(&self, req: CreateNicRequest) -> Result<NicData> {
        let cmd = Command::CreateNic {
            request_id: uuid::Uuid::new_v4().to_string(),
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now().to_rfc3339(),
            project_slug: req.project_slug,
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

    async fn update_nic_status(&self, id: &str, req: UpdateNicStatusRequest) -> Result<NicData> {
        let cmd = Command::UpdateNicStatus {
            request_id: uuid::Uuid::new_v4().to_string(),
            id: id.to_string(),
            timestamp: Utc::now().to_rfc3339(),
            phase: req.phase,
            socket_path: req.socket_path,
            message: req.message,
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
        Ok(state.list_vms(None))
    }

    async fn list_vms_by_project(&self, project_slug: &str) -> Result<Vec<VmData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.list_vms_by_project(project_slug))
    }

    async fn list_vms_by_node(&self, node_id: &str) -> Result<Vec<VmData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.list_vms(Some(node_id)))
    }

    async fn get_vm(&self, id: &str) -> Result<Option<VmData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.get_vm(id))
    }

    async fn get_vm_by_name(&self, name: &str) -> Result<Option<VmData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.get_vm_by_name(name))
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
impl OrgStore for RaftStore {
    async fn list_orgs(&self) -> Result<Vec<OrgData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.list_orgs())
    }

    async fn get_org(&self, slug: &str) -> Result<Option<OrgData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.get_org(slug))
    }

    async fn create_org(&self, req: CreateOrgRequest) -> Result<OrgData> {
        let cmd = Command::CreateOrg {
            request_id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now().to_rfc3339(),
            slug: req.slug,
            name: req.name,
            contact: OrgContact::default(),
        };

        match self.write_command(cmd).await? {
            Response::Org(data) => Ok(data),
            Response::Error { code: 409, message } => Err(StoreError::Conflict(message)),
            Response::Error { message, .. } => Err(StoreError::Internal(message)),
            _ => Err(StoreError::Internal("unexpected response".into())),
        }
    }

    async fn update_org(&self, slug: &str, req: UpdateOrgRequest) -> Result<OrgData> {
        let cmd = Command::UpdateOrg {
            request_id: uuid::Uuid::new_v4().to_string(),
            slug: slug.to_string(),
            timestamp: Utc::now().to_rfc3339(),
            name: req.name,
            contact: req.contact,
        };

        match self.write_command(cmd).await? {
            Response::Org(data) => Ok(data),
            Response::Error { code: 404, message } => Err(StoreError::NotFound(message)),
            Response::Error { message, .. } => Err(StoreError::Internal(message)),
            _ => Err(StoreError::Internal("unexpected response".into())),
        }
    }

    async fn delete_org(&self, slug: &str) -> Result<()> {
        let cmd = Command::DeleteOrg {
            request_id: uuid::Uuid::new_v4().to_string(),
            slug: slug.to_string(),
        };

        match self.write_command(cmd).await? {
            Response::Deleted { .. } => Ok(()),
            Response::Error { code: 404, message } => Err(StoreError::NotFound(message)),
            Response::Error { code: 409, message } => Err(StoreError::Conflict(message)),
            Response::Error { message, .. } => Err(StoreError::Internal(message)),
            _ => Err(StoreError::Internal("unexpected response".into())),
        }
    }
}

#[async_trait]
impl ProjectStore for RaftStore {
    async fn list_projects(&self) -> Result<Vec<ProjectData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.list_projects())
    }

    async fn list_projects_by_org(&self, org_slug: &str) -> Result<Vec<ProjectData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.list_projects_by_org(org_slug))
    }

    async fn get_project(&self, slug: &str) -> Result<Option<ProjectData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.get_project(slug))
    }

    async fn create_project(&self, req: CreateProjectRequest) -> Result<ProjectData> {
        let cmd = Command::CreateProject {
            request_id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now().to_rfc3339(),
            org_slug: req.org_slug,
            slug: req.slug,
            name: req.name,
            description: req.description,
        };

        match self.write_command(cmd).await? {
            Response::Project(data) => Ok(data),
            Response::Error { code: 404, message } => Err(StoreError::NotFound(message)),
            Response::Error { code: 409, message } => Err(StoreError::Conflict(message)),
            Response::Error { message, .. } => Err(StoreError::Internal(message)),
            _ => Err(StoreError::Internal("unexpected response".into())),
        }
    }

    async fn delete_project(&self, slug: &str) -> Result<()> {
        let cmd = Command::DeleteProject {
            request_id: uuid::Uuid::new_v4().to_string(),
            slug: slug.to_string(),
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
impl ClusterStore for RaftStore {
    async fn list_clusters(&self) -> Result<Vec<ClusterData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.list_clusters())
    }

    async fn list_clusters_by_org(&self, org_slug: &str) -> Result<Vec<ClusterData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.list_clusters_by_org(org_slug))
    }

    async fn get_cluster(&self, slug: &str) -> Result<Option<ClusterData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.get_cluster(slug))
    }

    async fn create_cluster(&self, req: CreateClusterRequest) -> Result<ClusterData> {
        let cmd = Command::CreateCluster {
            request_id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now().to_rfc3339(),
            org_slug: req.org_slug,
            slug: req.slug,
            name: req.name,
            description: req.description,
            location: req.location,
        };

        match self.write_command(cmd).await? {
            Response::Cluster(data) => Ok(data),
            Response::Error { code: 404, message } => Err(StoreError::NotFound(message)),
            Response::Error { code: 409, message } => Err(StoreError::Conflict(message)),
            Response::Error { message, .. } => Err(StoreError::Internal(message)),
            _ => Err(StoreError::Internal("unexpected response".into())),
        }
    }

    async fn update_cluster(&self, slug: &str, req: UpdateClusterRequest) -> Result<ClusterData> {
        let cmd = Command::UpdateCluster {
            request_id: uuid::Uuid::new_v4().to_string(),
            slug: slug.to_string(),
            timestamp: Utc::now().to_rfc3339(),
            name: req.name,
            description: req.description,
            location: req.location,
        };

        match self.write_command(cmd).await? {
            Response::Cluster(data) => Ok(data),
            Response::Error { code: 404, message } => Err(StoreError::NotFound(message)),
            Response::Error { message, .. } => Err(StoreError::Internal(message)),
            _ => Err(StoreError::Internal("unexpected response".into())),
        }
    }

    async fn delete_cluster(&self, slug: &str) -> Result<()> {
        let cmd = Command::DeleteCluster {
            request_id: uuid::Uuid::new_v4().to_string(),
            slug: slug.to_string(),
        };

        match self.write_command(cmd).await? {
            Response::Deleted { .. } => Ok(()),
            Response::Error { code: 404, message } => Err(StoreError::NotFound(message)),
            Response::Error { code: 409, message } => Err(StoreError::Conflict(message)),
            Response::Error { message, .. } => Err(StoreError::Internal(message)),
            _ => Err(StoreError::Internal("unexpected response".into())),
        }
    }

    async fn add_node_to_cluster(&self, cluster_slug: &str, node_id: &str) -> Result<ClusterData> {
        let cmd = Command::AddNodeToCluster {
            request_id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now().to_rfc3339(),
            cluster_slug: cluster_slug.to_string(),
            node_id: node_id.to_string(),
        };

        match self.write_command(cmd).await? {
            Response::Cluster(data) => Ok(data),
            Response::Error { code: 404, message } => Err(StoreError::NotFound(message)),
            Response::Error { message, .. } => Err(StoreError::Internal(message)),
            _ => Err(StoreError::Internal("unexpected response".into())),
        }
    }

    async fn remove_node_from_cluster(
        &self,
        cluster_slug: &str,
        node_id: &str,
    ) -> Result<ClusterData> {
        let cmd = Command::RemoveNodeFromCluster {
            request_id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now().to_rfc3339(),
            cluster_slug: cluster_slug.to_string(),
            node_id: node_id.to_string(),
        };

        match self.write_command(cmd).await? {
            Response::Cluster(data) => Ok(data),
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
        project_slug: Option<&str>,
        node_id: Option<&str>,
    ) -> Result<Vec<VolumeData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.list_volumes(project_slug, node_id))
    }

    async fn get_volume(&self, id: &str) -> Result<Option<VolumeData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.get_volume(id))
    }

    async fn create_volume(&self, req: CreateVolumeRequest) -> Result<VolumeData> {
        let cmd = Command::CreateVolume {
            request_id: uuid::Uuid::new_v4().to_string(),
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now().to_rfc3339(),
            project_slug: req.project_slug,
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

    async fn update_volume_status(
        &self,
        id: &str,
        req: UpdateVolumeStatusRequest,
    ) -> Result<VolumeData> {
        let cmd = Command::UpdateVolumeStatus {
            request_id: uuid::Uuid::new_v4().to_string(),
            id: id.to_string(),
            timestamp: Utc::now().to_rfc3339(),
            phase: req.phase,
            path: req.path,
            used_bytes: req.used_bytes,
            error: req.error,
        };
        match self.write_command(cmd).await? {
            Response::Volume(data) => Ok(data),
            Response::Error { code: 404, message } => Err(StoreError::NotFound(message)),
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
        Ok(state.list_templates(node_id))
    }

    async fn list_templates_by_project(&self, project_slug: &str) -> Result<Vec<TemplateData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.list_templates_by_project(project_slug))
    }

    async fn get_template(&self, id: &str) -> Result<Option<TemplateData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.get_template(id))
    }

    async fn create_template(&self, req: CreateTemplateRequest) -> Result<TemplateData> {
        let cmd = Command::CreateTemplate {
            request_id: uuid::Uuid::new_v4().to_string(),
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now().to_rfc3339(),
            project_slug: req.project_slug,
            node_id: req.node_id,
            name: req.name,
            size_bytes: req.size_bytes,
            source_url: req.source_url,
            total_bytes: req.total_bytes,
        };

        match self.write_command(cmd).await? {
            Response::Template(data) => Ok(data),
            Response::Error { code: 409, message } => Err(StoreError::Conflict(message)),
            Response::Error { message, .. } => Err(StoreError::Internal(message)),
            _ => Err(StoreError::Internal("unexpected response".into())),
        }
    }

    async fn update_template_status(
        &self,
        id: &str,
        req: UpdateTemplateStatusRequest,
    ) -> Result<TemplateData> {
        let cmd = Command::UpdateTemplateStatus {
            request_id: uuid::Uuid::new_v4().to_string(),
            id: id.to_string(),
            timestamp: Utc::now().to_rfc3339(),
            phase: req.phase,
            bytes_written: req.bytes_written,
            size_bytes: req.size_bytes,
            error: req.error,
        };

        match self.write_command(cmd).await? {
            Response::Template(data) => Ok(data),
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
        project_slug: Option<&str>,
    ) -> Result<Vec<crate::command::SecurityGroupData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        match project_slug {
            Some(pid) => Ok(state.list_security_groups_by_project(pid)),
            None => Ok(state.list_security_groups()),
        }
    }

    async fn get_security_group(
        &self,
        id: &str,
    ) -> Result<Option<crate::command::SecurityGroupData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.get_security_group(id))
    }

    async fn create_security_group(
        &self,
        req: CreateSecurityGroupRequest,
    ) -> Result<crate::command::SecurityGroupData> {
        let cmd = Command::CreateSecurityGroup {
            request_id: uuid::Uuid::new_v4().to_string(),
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now().to_rfc3339(),
            project_slug: req.project_slug,
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

    async fn update_security_group(
        &self,
        id: &str,
        req: UpdateSecurityGroupRequest,
    ) -> Result<crate::command::SecurityGroupData> {
        let cmd = Command::UpdateSecurityGroup {
            request_id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now().to_rfc3339(),
            id: id.to_string(),
            name: req.name,
            description: req.description,
        };
        match self.write_command(cmd).await? {
            Response::SecurityGroup(sg) => Ok(sg),
            Response::Error { code, message } => match code {
                404 => Err(StoreError::NotFound(message)),
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

    async fn update_security_group_rule(
        &self,
        security_group_id: &str,
        rule_id: &str,
        req: UpdateSecurityGroupRuleRequest,
    ) -> Result<crate::command::SecurityGroupData> {
        let cmd = Command::UpdateSecurityGroupRule {
            request_id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now().to_rfc3339(),
            security_group_id: security_group_id.to_string(),
            rule_id: rule_id.to_string(),
            description: req.description,
        };
        match self.write_command(cmd).await? {
            Response::SecurityGroup(sg) => Ok(sg),
            Response::Error { code, message } => match code {
                404 => Err(StoreError::NotFound(message)),
                _ => Err(StoreError::Internal(message)),
            },
            _ => Err(StoreError::Internal("unexpected response".into())),
        }
    }
}

#[async_trait]
impl OnboardingStore for RaftStore {
    async fn ensure_internal_ca(&self, deployment_name: &str) -> Result<crate::ca::InternalCa> {
        // Fast path: CA already exists.
        {
            let node = self.node.read().await;
            let state = node.get_state().await;
            if let Some(ca) = state.get_internal_ca() {
                return Ok(ca);
            }
        }
        // Slow path: generate locally on this leader, send through raft so
        // every replica writes the same bytes. Subsequent EnsureInternalCa
        // applies are noops via the idempotency check.
        let ca = crate::ca::generate_root_ca(deployment_name)
            .map_err(|e| StoreError::Internal(format!("generate CA: {e}")))?;
        let cmd = Command::EnsureInternalCa {
            request_id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now().to_rfc3339(),
            ca: ca.clone(),
        };
        match self.write_command(cmd).await? {
            Response::Ack => {}
            Response::Error { message, .. } => return Err(StoreError::Internal(message)),
            _ => return Err(StoreError::Internal("unexpected response".into())),
        }
        // Re-read so the caller sees whatever the cluster agreed on (in case
        // a concurrent ensure raced ahead).
        let node = self.node.read().await;
        let state = node.get_state().await;
        state
            .get_internal_ca()
            .ok_or_else(|| StoreError::Internal("CA missing after EnsureInternalCa".into()))
    }

    async fn get_server_cert(&self) -> Result<Option<crate::command::ServerCertData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.get_server_cert())
    }

    async fn rotate_server_cert(
        &self,
        dns_names: Vec<String>,
    ) -> Result<crate::command::ServerCertData> {
        let ca = self.ensure_internal_ca("mvirt").await?;
        let (signed, key_pem) =
            crate::ca::sign_server_cert(&ca, dns_names, crate::ca::new_serial())
                .map_err(|e| StoreError::Internal(format!("sign server cert: {e}")))?;
        let cmd = Command::UpdateServerCert {
            request_id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now().to_rfc3339(),
            cert_pem: signed.cert_pem.clone(),
            key_pem: key_pem.clone(),
            serial_hex: signed.serial_hex.clone(),
            not_after: signed.not_after.clone(),
        };
        match self.write_command(cmd).await? {
            Response::Ack => Ok(crate::command::ServerCertData {
                cert_pem: signed.cert_pem,
                key_pem,
                serial_hex: signed.serial_hex,
                not_after: signed.not_after,
                created_at: Utc::now().to_rfc3339(),
            }),
            Response::Error { message, .. } => Err(StoreError::Internal(message)),
            _ => Err(StoreError::Internal("unexpected response".into())),
        }
    }

    async fn create_onboarding_token(
        &self,
        req: CreateOnboardingTokenRequest,
    ) -> Result<(crate::command::OnboardingTokenData, String)> {
        // Cap TTL at 7 days (ADR-0006), floor at 1 minute.
        let ttl = req.ttl_seconds.clamp(60, 7 * 24 * 3600);
        let expires = Utc::now() + chrono::Duration::seconds(ttl as i64);

        // Generate the bare token (32 bytes, base64url) and hash it.
        let bare = generate_bare_token();
        let hash_hex = sha256_hex(bare.as_bytes());

        let id = format!("tok_{}", short_id());
        let node_id = format!("node_{}", uuid::Uuid::new_v4().simple());

        let cmd = Command::CreateOnboardingToken {
            request_id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now().to_rfc3339(),
            id: id.clone(),
            token_hash_hex: hash_hex,
            cluster_slug: req.cluster_slug,
            node_id,
            hostname: req.hostname,
            description: req.description,
            expires_at: expires.to_rfc3339(),
            created_by_account: req.created_by_account,
        };
        match self.write_command(cmd).await? {
            Response::OnboardingToken(t) => Ok((t, bare)),
            Response::Error { code: 404, message } => Err(StoreError::NotFound(message)),
            Response::Error { code: 409, message } => Err(StoreError::Conflict(message)),
            Response::Error { message, .. } => Err(StoreError::Internal(message)),
            _ => Err(StoreError::Internal("unexpected response".into())),
        }
    }

    async fn list_onboarding_tokens_by_cluster(
        &self,
        cluster_slug: &str,
    ) -> Result<Vec<crate::command::OnboardingTokenData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.list_onboarding_tokens_by_cluster(cluster_slug))
    }

    async fn delete_onboarding_token(&self, cluster_slug: &str, id: &str) -> Result<()> {
        let cmd = Command::DeleteOnboardingToken {
            request_id: uuid::Uuid::new_v4().to_string(),
            cluster_slug: cluster_slug.to_string(),
            id: id.to_string(),
        };
        match self.write_command(cmd).await? {
            Response::Deleted { .. } => Ok(()),
            Response::Error { code: 404, message } => Err(StoreError::NotFound(message)),
            Response::Error { message, .. } => Err(StoreError::Internal(message)),
            _ => Err(StoreError::Internal("unexpected response".into())),
        }
    }

    async fn redeem_onboarding_token(
        &self,
        req: RedeemOnboardingTokenRequest,
    ) -> Result<BootstrapOutcome> {
        let hash_hex = sha256_hex(req.token.as_bytes());
        let cmd = Command::RedeemOnboardingToken {
            request_id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now().to_rfc3339(),
            token_hash_hex: hash_hex,
            csr_pem: req.csr_pem,
            hostname: req.hostname,
            agent_version: req.agent_version,
            kernel_version: req.kernel_version,
            arch: req.arch,
        };
        match self.write_command(cmd).await? {
            Response::BootstrapResult {
                node,
                client_cert_pem,
                ca_cert_pem,
            } => Ok(BootstrapOutcome {
                cluster_slug: node.cluster_slug.clone().unwrap_or_default(),
                cert_not_after: node.cert_expires_at.clone().unwrap_or_default(),
                node_id: node.id,
                client_cert_pem,
                ca_cert_pem,
            }),
            Response::Error { code: 401, message } => Err(StoreError::Unauthorized(message)),
            Response::Error { code: 410, message } => Err(StoreError::Gone(message)),
            Response::Error { code: 400, message } => Err(StoreError::Validation(message)),
            Response::Error { code: 503, message } => Err(StoreError::Internal(message)),
            Response::Error { message, .. } => Err(StoreError::Internal(message)),
            _ => Err(StoreError::Internal("unexpected response".into())),
        }
    }

    async fn revoke_node_cert(
        &self,
        node_id: &str,
        reason: crate::command::RevocationReason,
    ) -> Result<()> {
        let cmd = Command::RevokeNodeCert {
            request_id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now().to_rfc3339(),
            node_id: node_id.to_string(),
            reason,
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
impl AccountStore for RaftStore {
    async fn ensure_account_from_oidc(&self, req: EnsureAccountRequest) -> Result<AccountData> {
        let cmd = Command::EnsureAccountFromOidc {
            request_id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now().to_rfc3339(),
            new_id: format!("acc_{}", uuid::Uuid::new_v4().simple()),
            iss: req.iss,
            sub: req.sub,
            email: req.email,
            display_name: req.display_name,
        };
        match self.write_command(cmd).await? {
            Response::Account(a) => Ok(a),
            Response::Error { message, .. } => Err(StoreError::Internal(message)),
            _ => Err(StoreError::Internal("unexpected response".into())),
        }
    }

    async fn create_account_by_email(
        &self,
        req: crate::store::CreateAccountByEmailRequest,
    ) -> Result<AccountData> {
        let cmd = Command::CreateAccountByEmail {
            request_id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now().to_rfc3339(),
            id: format!("acc_{}", uuid::Uuid::new_v4().simple()),
            email: req.email,
            display_name: req.display_name,
        };
        match self.write_command(cmd).await? {
            Response::Account(a) => Ok(a),
            Response::Error { code: 400, message } => Err(StoreError::Validation(message)),
            Response::Error { code: 409, message } => Err(StoreError::Conflict(message)),
            Response::Error { message, .. } => Err(StoreError::Internal(message)),
            _ => Err(StoreError::Internal("unexpected response".into())),
        }
    }

    async fn get_account(&self, id: &str) -> Result<Option<AccountData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.get_account(id))
    }

    async fn get_account_by_oidc(&self, iss: &str, sub: &str) -> Result<Option<AccountData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.get_account_by_oidc(iss, sub))
    }

    async fn list_accounts(&self) -> Result<Vec<AccountData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.list_accounts())
    }

    async fn create_membership(&self, req: CreateMembershipRequest) -> Result<MembershipData> {
        let cmd = Command::CreateMembership {
            request_id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now().to_rfc3339(),
            id: format!("mbr_{}", uuid::Uuid::new_v4().simple()),
            account_id: req.account_id,
            scope: req.scope,
            role: req.role,
            created_by_account: req.created_by_account,
        };
        match self.write_command(cmd).await? {
            Response::Membership(m) => Ok(m),
            Response::Error { code: 404, message } => Err(StoreError::NotFound(message)),
            Response::Error { code: 409, message } => Err(StoreError::Conflict(message)),
            Response::Error { message, .. } => Err(StoreError::Internal(message)),
            _ => Err(StoreError::Internal("unexpected response".into())),
        }
    }

    async fn delete_membership(&self, id: &str) -> Result<()> {
        let cmd = Command::DeleteMembership {
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

    async fn list_memberships_for_account(&self, account_id: &str) -> Result<Vec<MembershipData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.list_memberships_for_account(account_id))
    }

    async fn list_memberships_at_scope(
        &self,
        scope: &MembershipScope,
    ) -> Result<Vec<MembershipData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.list_memberships_at_scope(scope))
    }

    async fn bootstrap_initial_platform_admin(&self, account_id: &str) -> Result<()> {
        let cmd = Command::BootstrapInitialPlatformAdmin {
            request_id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now().to_rfc3339(),
            id: format!("mbr_{}", uuid::Uuid::new_v4().simple()),
            account_id: account_id.to_string(),
        };
        match self.write_command(cmd).await? {
            Response::Membership(_) | Response::Ack => Ok(()),
            Response::Error { code: 404, message } => Err(StoreError::NotFound(message)),
            Response::Error { message, .. } => Err(StoreError::Internal(message)),
            _ => Err(StoreError::Internal("unexpected response".into())),
        }
    }

    async fn has_platform_admin(&self) -> Result<bool> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.has_platform_admin())
    }
}

#[async_trait]
impl crate::store::ServiceAccountStore for RaftStore {
    async fn create_service_account(
        &self,
        req: crate::store::CreateServiceAccountRequest,
    ) -> Result<AccountData> {
        let cmd = Command::CreateServiceAccount {
            request_id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now().to_rfc3339(),
            account_id: format!("acc_{}", uuid::Uuid::new_v4().simple()),
            membership_id: format!("mbr_{}", uuid::Uuid::new_v4().simple()),
            project_slug: req.project_slug,
            name: req.name,
            description: req.description,
            created_by_account: req.created_by_account,
        };
        match self.write_command(cmd).await? {
            Response::Account(a) => Ok(a),
            Response::Error { code: 404, message } => Err(StoreError::NotFound(message)),
            Response::Error { code: 409, message } => Err(StoreError::Conflict(message)),
            Response::Error { message, .. } => Err(StoreError::Internal(message)),
            _ => Err(StoreError::Internal("unexpected response".into())),
        }
    }

    async fn delete_service_account(&self, account_id: &str) -> Result<()> {
        let cmd = Command::DeleteServiceAccount {
            request_id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now().to_rfc3339(),
            account_id: account_id.to_string(),
        };
        match self.write_command(cmd).await? {
            Response::Deleted { .. } => Ok(()),
            Response::Error { code: 404, message } => Err(StoreError::NotFound(message)),
            Response::Error { code: 400, message } => Err(StoreError::Validation(message)),
            Response::Error { message, .. } => Err(StoreError::Internal(message)),
            _ => Err(StoreError::Internal("unexpected response".into())),
        }
    }

    async fn list_service_accounts_in_project(
        &self,
        project_slug: &str,
    ) -> Result<Vec<AccountData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.list_service_accounts_in_project(project_slug))
    }

    async fn create_static_api_key(
        &self,
        req: crate::store::CreateStaticApiKeyRequest,
    ) -> Result<crate::command::ApiKeyData> {
        let cmd = Command::CreateStaticApiKey {
            request_id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now().to_rfc3339(),
            id: format!("key_{}", uuid::Uuid::new_v4().simple()),
            account_id: req.account_id,
            hash_hex: req.hash_hex,
            display_prefix: req.display_prefix,
            description: req.description,
            expires_at: req.expires_at,
            created_by_account: req.created_by_account,
        };
        match self.write_command(cmd).await? {
            Response::ApiKey(k) => Ok(k),
            Response::Error { code: 404, message } => Err(StoreError::NotFound(message)),
            Response::Error { code: 400, message } => Err(StoreError::Validation(message)),
            Response::Error { code: 409, message } => Err(StoreError::Conflict(message)),
            Response::Error { message, .. } => Err(StoreError::Internal(message)),
            _ => Err(StoreError::Internal("unexpected response".into())),
        }
    }

    async fn revoke_static_api_key(&self, id: &str) -> Result<crate::command::ApiKeyData> {
        let cmd = Command::RevokeStaticApiKey {
            request_id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now().to_rfc3339(),
            id: id.to_string(),
        };
        match self.write_command(cmd).await? {
            Response::ApiKey(k) => Ok(k),
            Response::Error { code: 404, message } => Err(StoreError::NotFound(message)),
            Response::Error { message, .. } => Err(StoreError::Internal(message)),
            _ => Err(StoreError::Internal("unexpected response".into())),
        }
    }

    async fn get_api_key(&self, id: &str) -> Result<Option<crate::command::ApiKeyData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.get_api_key(id))
    }

    async fn list_api_keys_for_account(
        &self,
        account_id: &str,
    ) -> Result<Vec<crate::command::ApiKeyData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.list_api_keys_for_account(account_id))
    }
}

impl DataStore for RaftStore {
    fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }
}

fn generate_bare_token() -> String {
    use base64::Engine;
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn sha256_hex(input: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(input);
    let mut s = String::with_capacity(digest.len() * 2);
    for b in digest {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

fn short_id() -> String {
    use base64::Engine;
    use rand::RngCore;
    let mut bytes = [0u8; 8];
    rand::thread_rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}
