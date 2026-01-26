//! RaftStore implementation - bridges DataStore traits to RaftNode.

use std::sync::Arc;

use async_trait::async_trait;
use mraft::RaftNode;
use tokio::sync::{RwLock, broadcast};

use crate::command::{Command, NetworkData, NicData, Response};
use crate::state::CpState;

use super::error::{Result, StoreError};
use super::event::Event;
use super::traits::*;

/// RaftStore wraps a RaftNode and implements the DataStore trait.
///
/// This provides a clean abstraction over Raft operations, hiding the
/// Command/Response types from handlers.
pub struct RaftStore {
    node: Arc<RwLock<RaftNode<Command, Response, CpState>>>,
    events: broadcast::Sender<Event>,
    node_id: u64,
}

impl RaftStore {
    /// Create a new RaftStore wrapping the given RaftNode.
    pub fn new(
        node: Arc<RwLock<RaftNode<Command, Response, CpState>>>,
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
impl NetworkStore for RaftStore {
    async fn list_networks(&self) -> Result<Vec<NetworkData>> {
        let node = self.node.read().await;
        let state = node.get_state().await;
        Ok(state.list_networks().into_iter().cloned().collect())
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
            name: req.name,
            ipv4_enabled: req.ipv4_enabled,
            ipv4_subnet: req.ipv4_subnet,
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
            network_id: req.network_id,
            name: req.name,
            mac_address: req.mac_address,
            ipv4_address: req.ipv4_address,
            ipv6_address: req.ipv6_address,
            routed_ipv4_prefixes: req.routed_ipv4_prefixes,
            routed_ipv6_prefixes: req.routed_ipv6_prefixes,
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
}

#[async_trait]
impl ClusterStore for RaftStore {
    async fn get_cluster_info(&self) -> Result<ClusterInfo> {
        let node = self.node.read().await;
        let metrics = node.metrics();

        Ok(ClusterInfo {
            cluster_id: "mvirt-cluster".to_string(),
            leader_id: metrics.current_leader,
            current_term: metrics.current_term,
            commit_index: metrics.last_applied.map(|l| l.index).unwrap_or(0),
            node_id: self.node_id,
            is_leader: metrics.current_leader == Some(self.node_id),
        })
    }

    async fn get_membership(&self) -> Result<Membership> {
        let node = self.node.read().await;
        let membership = node.get_membership();

        let nodes: Vec<MembershipNode> = membership
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
                MembershipNode {
                    id: *id,
                    address: addr.clone(),
                    role: role.to_string(),
                }
            })
            .collect();

        Ok(Membership {
            voters: membership.voters.into_iter().collect(),
            learners: membership.learners.into_iter().collect(),
            nodes,
        })
    }

    async fn create_join_token(&self, node_id: u64, valid_for_secs: u64) -> Result<String> {
        let node = self.node.read().await;
        node.create_join_token(node_id, valid_for_secs)
            .await
            .map_err(|e| match e {
                mraft::JoinError::NotLeader => StoreError::NotLeader { leader_id: None },
                mraft::JoinError::NoClusterSecret => {
                    StoreError::Internal("Cluster secret not configured".into())
                }
                _ => StoreError::Internal(e.to_string()),
            })
    }

    async fn remove_node(&self, node_id: u64) -> Result<()> {
        let node = self.node.read().await;
        node.remove_node(node_id).await.map_err(|e| match e {
            mraft::JoinError::NotLeader => StoreError::NotLeader { leader_id: None },
            mraft::JoinError::NotMember(_) => {
                StoreError::NotFound(format!("Node {} not found", node_id))
            }
            _ => StoreError::Internal(e.to_string()),
        })
    }
}

impl DataStore for RaftStore {
    fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }
}
