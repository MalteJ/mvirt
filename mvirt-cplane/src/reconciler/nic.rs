//! NIC reconciler — drives mvirt-ebpf on the owning node to create the
//! TAP + vhost-user socket for each NicData, then writes the returned
//! socket_path back via `Command::UpdateNicStatus` so the VM reconciler
//! can thread it into cloud-hypervisor.
//!
//! Network is treated as a prerequisite: this reconciler ensures the
//! parent network exists on the same node (idempotent), so the dedicated
//! network reconciler can stay a no-op until we move to multi-node
//! placement.

use std::sync::Arc;

use anyhow::Result;
use chrono::Utc;
use mvirt_daemon_protos::net::{
    CreateNetworkRequest, CreateNicRequest, GetNetworkRequest, GetNicRequest, get_network_request,
    get_nic_request,
};
use tonic::Code;
use tracing::{info, warn};

use super::Ctx;
use crate::command::{Command, NetworkData, NicData, NicPhase};
use crate::state::ApiState;
use crate::tunnel::NodeHandle;

pub fn list_ids(state: &ApiState) -> Vec<String> {
    state.nic_ids()
}

pub async fn reconcile(ctx: &Ctx, id: &str) -> Result<()> {
    let state = ctx.store.snapshot().await;
    let Some(nic) = state.get_nic(id) else {
        return Ok(());
    };
    if matches!(nic.status.phase, NicPhase::Active) && !nic.status.socket_path.is_empty() {
        return Ok(());
    }

    let Some(network) = state.get_network(&nic.spec.network_id) else {
        warn!(nic = %id, net = %nic.spec.network_id, "NIC references unknown network; skipping");
        return Ok(());
    };

    // Single-node assumption for now: pick the first connected node. Future
    // work: stamp a node_id at create time (e.g. from the parent VM's
    // schedule decision) and look it up here.
    let nodes = ctx.registry.list().await;
    let Some(node) = nodes.into_iter().next() else {
        warn!(nic = %id, "no nodes connected; will retry on resync");
        return Ok(());
    };

    info!(nic = %id, node = %node.node_id, net = %network.name, "reconciling nic");

    let result = ensure_nic(&node, &network, id, &nic).await;
    let cmd = match result {
        Ok(socket_path) => Command::UpdateNicStatus {
            request_id: uuid::Uuid::new_v4().to_string(),
            id: id.to_string(),
            timestamp: Utc::now().to_rfc3339(),
            phase: NicPhase::Active,
            socket_path,
            message: None,
        },
        Err(e) => Command::UpdateNicStatus {
            request_id: uuid::Uuid::new_v4().to_string(),
            id: id.to_string(),
            timestamp: Utc::now().to_rfc3339(),
            phase: NicPhase::Failed,
            socket_path: String::new(),
            message: Some(e),
        },
    };
    ctx.store
        .submit(cmd)
        .await
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!("write nic status: {e}"))
}

async fn ensure_nic(
    node: &Arc<NodeHandle>,
    network: &NetworkData,
    id: &str,
    nic: &NicData,
) -> std::result::Result<String, String> {
    let mut net = node.net.clone();

    // 1. Ensure the network exists on the node. CreateNetwork on
    //    mvirt-ebpf is not idempotent on duplicate, so GetNetwork first.
    match net
        .get_network(GetNetworkRequest {
            identifier: Some(get_network_request::Identifier::Id(network.id.clone())),
        })
        .await
    {
        Ok(_) => {}
        Err(s) if s.code() == Code::NotFound => {
            net.create_network(CreateNetworkRequest {
                id: network.id.clone(),
                name: network.name.clone(),
                ipv4_enabled: network.ipv4_enabled,
                ipv4_subnet: network.ipv4_prefix.clone().unwrap_or_default(),
                ipv6_enabled: network.ipv6_enabled,
                ipv6_prefix: network.ipv6_prefix.clone().unwrap_or_default(),
                dns_servers: network.dns_servers.clone(),
                ntp_servers: network.ntp_servers.clone(),
                is_public: network.is_public,
            })
            .await
            .map_err(|s| format!("create_network: {}", s.message()))?;
        }
        Err(s) => return Err(format!("get_network: {}", s.message())),
    }

    // 2. Get-or-create the NIC on the node. The socket_path comes back
    //    populated either way.
    match net
        .get_nic(GetNicRequest {
            identifier: Some(get_nic_request::Identifier::Id(id.to_string())),
        })
        .await
    {
        Ok(resp) => Ok(resp.into_inner().socket_path),
        Err(s) if s.code() == Code::NotFound => net
            .create_nic(CreateNicRequest {
                id: id.to_string(),
                network_id: network.id.clone(),
                name: nic.spec.name.clone().unwrap_or_default(),
                mac_address: nic.spec.mac_address.clone(),
                ipv4_address: nic.spec.ipv4_address.clone().unwrap_or_default(),
                ipv6_address: nic.spec.ipv6_address.clone().unwrap_or_default(),
                routed_ipv4_prefixes: nic.spec.routed_ipv4_prefixes.clone(),
                routed_ipv6_prefixes: nic.spec.routed_ipv6_prefixes.clone(),
            })
            .await
            .map(|r| r.into_inner().socket_path)
            .map_err(|s| format!("create_nic: {}", s.message())),
        Err(s) => Err(format!("get_nic: {}", s.message())),
    }
}
