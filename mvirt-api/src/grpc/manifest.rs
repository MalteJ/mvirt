//! Manifest builder — constructs a `NodeManifest` from Raft state for a given node.

use std::sync::Arc;

use crate::command::{ImportJobState, VmDesiredState as CmdVmDesiredState};
use crate::store::DataStore;

use super::proto::{
    NetworkSpec as ProtoNetworkSpec, NicSpec as ProtoNicSpec, NodeManifest, ResourceMeta,
    SecurityGroupSpec as ProtoSecurityGroupSpec, SecurityRule as ProtoSecurityRule,
    TemplateSpec as ProtoTemplateSpec, VmDesiredState as ProtoVmDesiredState,
    VmSpec as ProtoVmSpec, VolumeSpec as ProtoVolumeSpec,
};

/// Build a full `NodeManifest` for the given node from current store state.
pub async fn build_manifest(
    store: &Arc<dyn DataStore>,
    node_id: &str,
    revision: u64,
) -> NodeManifest {
    let vms = build_vm_specs(store, node_id).await;
    let nic_ids: Vec<String> = vms
        .iter()
        .filter_map(|vm| {
            if vm.nic_id.is_empty() {
                None
            } else {
                Some(vm.nic_id.clone())
            }
        })
        .collect();
    let nics = build_nic_specs(store, &nic_ids).await;
    let volumes = build_volume_specs(store, node_id).await;
    let templates = build_template_specs(store, node_id).await;
    let networks = build_network_specs(store).await;
    let security_groups = build_security_group_specs(store).await;

    NodeManifest {
        revision,
        vms,
        networks,
        nics,
        volumes,
        templates,
        security_groups,
        routes: vec![], // Not yet implemented
    }
}

async fn build_vm_specs(store: &Arc<dyn DataStore>, node_id: &str) -> Vec<ProtoVmSpec> {
    let vms = match store.list_vms_by_node(node_id).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("Failed to list VMs for node {}: {}", node_id, e);
            return vec![];
        }
    };

    let mut specs = Vec::with_capacity(vms.len());
    for vm in &vms {
        let desired_state = match vm.spec.desired_state {
            CmdVmDesiredState::Running => ProtoVmDesiredState::Running as i32,
            CmdVmDesiredState::Stopped => ProtoVmDesiredState::Stopped as i32,
        };

        let volume_name = match store.get_volume(&vm.spec.volume_id).await {
            Ok(Some(vol)) => vol.name,
            _ => vm.spec.volume_id.clone(),
        };

        specs.push(ProtoVmSpec {
            meta: Some(ResourceMeta {
                id: vm.id.clone(),
                name: vm.spec.name.clone(),
                project_id: vm.spec.project_id.clone(),
                node_id: vm.status.node_id.clone(),
                labels: Default::default(),
            }),
            cpu_cores: vm.spec.cpu_cores,
            memory_mb: vm.spec.memory_mb,
            volume_id: vm.spec.volume_id.clone(),
            volume_name,
            nic_id: vm.spec.nic_id.clone(),
            image: vm.spec.image.clone(),
            desired_state,
        });
    }
    specs
}

async fn build_nic_specs(store: &Arc<dyn DataStore>, nic_ids: &[String]) -> Vec<ProtoNicSpec> {
    let mut specs = Vec::with_capacity(nic_ids.len());
    for nic_id in nic_ids {
        let nic = match store.get_nic(nic_id).await {
            Ok(Some(n)) => n,
            Ok(None) => {
                tracing::warn!("NIC {} not found in store", nic_id);
                continue;
            }
            Err(e) => {
                tracing::error!("Failed to get NIC {}: {}", nic_id, e);
                continue;
            }
        };

        specs.push(ProtoNicSpec {
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
        });
    }
    specs
}

async fn build_volume_specs(store: &Arc<dyn DataStore>, node_id: &str) -> Vec<ProtoVolumeSpec> {
    let volumes = match store.list_volumes(None, Some(node_id)).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("Failed to list volumes for node {}: {}", node_id, e);
            return vec![];
        }
    };

    volumes
        .iter()
        .map(|vol| ProtoVolumeSpec {
            meta: Some(ResourceMeta {
                id: vol.id.clone(),
                name: vol.name.clone(),
                project_id: vol.project_id.clone(),
                node_id: Some(vol.node_id.clone()),
                labels: Default::default(),
            }),
            size_bytes: vol.size_bytes,
            template_id: vol.template_id.clone(),
            attached_vm_id: None,
        })
        .collect()
}

async fn build_template_specs(store: &Arc<dyn DataStore>, node_id: &str) -> Vec<ProtoTemplateSpec> {
    let mut specs = Vec::new();

    // Pending/running import jobs — these should be sent to the target node
    if let Ok(jobs) = store.list_import_jobs(None).await {
        for job in &jobs {
            if job.state == ImportJobState::Completed || job.state == ImportJobState::Failed {
                continue;
            }
            // Import jobs with a specific node_id go only to that node;
            // jobs with no node_id go to all nodes (broadcast).
            if !job.node_id.is_empty() && job.node_id != node_id {
                continue;
            }
            specs.push(ProtoTemplateSpec {
                meta: Some(ResourceMeta {
                    id: job.id.clone(),
                    name: job.template_name.clone(),
                    project_id: job.project_id.clone(),
                    node_id: if job.node_id.is_empty() {
                        None
                    } else {
                        Some(job.node_id.clone())
                    },
                    labels: Default::default(),
                }),
                url: job.url.clone(),
                checksum: None,
            });
        }
    }

    specs
}

async fn build_network_specs(store: &Arc<dyn DataStore>) -> Vec<ProtoNetworkSpec> {
    let networks = match store.list_networks().await {
        Ok(n) => n,
        Err(e) => {
            tracing::error!("Failed to list networks: {}", e);
            return vec![];
        }
    };

    networks
        .iter()
        .map(|net| ProtoNetworkSpec {
            meta: Some(ResourceMeta {
                id: net.id.clone(),
                name: net.name.clone(),
                project_id: net.project_id.clone(),
                node_id: None,
                labels: Default::default(),
            }),
            ipv4_enabled: net.ipv4_enabled,
            ipv4_prefix: net.ipv4_prefix.clone().unwrap_or_default(),
            ipv6_enabled: net.ipv6_enabled,
            ipv6_prefix: net.ipv6_prefix.clone().unwrap_or_default(),
            dns_servers: net.dns_servers.clone(),
            ntp_servers: net.ntp_servers.clone(),
            is_public: net.is_public,
        })
        .collect()
}

async fn build_security_group_specs(store: &Arc<dyn DataStore>) -> Vec<ProtoSecurityGroupSpec> {
    let sgs = match store.list_security_groups(None).await {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("Failed to list security groups: {}", e);
            return vec![];
        }
    };

    sgs.iter()
        .map(|sg| {
            let rules = sg
                .rules
                .iter()
                .map(|r| {
                    use super::proto::{
                        RuleDirection as ProtoDir, RuleProtocol as ProtoProto, security_rule,
                    };
                    use crate::command::RuleDirection;

                    let direction = match r.direction {
                        RuleDirection::Inbound => ProtoDir::Ingress as i32,
                        RuleDirection::Outbound => ProtoDir::Egress as i32,
                    };

                    let protocol = match r.protocol.as_deref() {
                        Some("tcp") => ProtoProto::Tcp as i32,
                        Some("udp") => ProtoProto::Udp as i32,
                        Some("icmp") => ProtoProto::Icmp as i32,
                        Some("icmpv6") => ProtoProto::Icmpv6 as i32,
                        Some("all") => ProtoProto::All as i32,
                        _ => ProtoProto::Unspecified as i32,
                    };

                    let target = r
                        .cidr
                        .as_ref()
                        .map(|c| security_rule::Target::Cidr(c.clone()));

                    ProtoSecurityRule {
                        id: r.id.clone(),
                        direction,
                        protocol,
                        port_start: r.port_range_start.map(|p| p as u32),
                        port_end: r.port_range_end.map(|p| p as u32),
                        target,
                    }
                })
                .collect();

            ProtoSecurityGroupSpec {
                meta: Some(ResourceMeta {
                    id: sg.id.clone(),
                    name: sg.name.clone(),
                    project_id: sg.project_id.clone(),
                    node_id: None,
                    labels: Default::default(),
                }),
                rules,
            }
        })
        .collect()
}
