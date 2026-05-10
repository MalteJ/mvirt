//! API Server state machine.

use lru::LruCache;
use mraft::StateMachine;
use redb::{
    ReadTransaction, ReadableTable, ReadableTableMetadata, TableDefinition, WriteTransaction,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::Arc;

#[cfg(test)]
use crate::command::VolumePhase;
use crate::command::{
    Command, NetworkData, NicData, NicSpec, NicStatus, NodeData, NodeStatus, OrgData, ProjectData,
    Response, SecurityGroupData, SecurityGroupRuleData, SnapshotData, TemplateData, TemplatePhase,
    TemplateSpec, TemplateStatus, VmData, VmPhase, VmStatus, VolumeData, VolumeSpec, VolumeStatus,
};
use crate::store::Event;

// =============================================================================
// redb schema
// =============================================================================

const NODES: TableDefinition<&str, &[u8]> = TableDefinition::new("nodes");
const NETWORKS: TableDefinition<&str, &[u8]> = TableDefinition::new("networks");
const NICS: TableDefinition<&str, &[u8]> = TableDefinition::new("nics");
const VMS: TableDefinition<&str, &[u8]> = TableDefinition::new("vms");
const ORGS: TableDefinition<&str, &[u8]> = TableDefinition::new("orgs");
const PROJECTS: TableDefinition<&str, &[u8]> = TableDefinition::new("projects");
const VOLUMES: TableDefinition<&str, &[u8]> = TableDefinition::new("volumes");
const TEMPLATES: TableDefinition<&str, &[u8]> = TableDefinition::new("templates");
const SECURITY_GROUPS: TableDefinition<&str, &[u8]> = TableDefinition::new("security_groups");

const ALL_TABLES: &[TableDefinition<&str, &[u8]>] = &[
    NODES,
    NETWORKS,
    NICS,
    VMS,
    ORGS,
    PROJECTS,
    VOLUMES,
    TEMPLATES,
    SECURITY_GROUPS,
];

// =============================================================================
// redb helpers (bincode-encoded values)
//
// All `expect`s fire only on programmer error (table not registered, corrupt
// bincode payload). Operational errors (disk full, etc.) are rare enough that
// failing fast is preferable to silently dropping commands.
// =============================================================================

fn read_get<V: DeserializeOwned>(
    txn: &ReadTransaction,
    table: TableDefinition<&str, &[u8]>,
    key: &str,
) -> Option<V> {
    let t = txn.open_table(table).expect("open_table");
    let g = t.get(key).expect("get")?;
    Some(bincode::deserialize(g.value()).expect("decode"))
}

fn read_list<V: DeserializeOwned>(
    txn: &ReadTransaction,
    table: TableDefinition<&str, &[u8]>,
) -> Vec<V> {
    let t = txn.open_table(table).expect("open_table");
    t.iter()
        .expect("iter")
        .map(|r| {
            let (_, v) = r.expect("row");
            bincode::deserialize(v.value()).expect("decode")
        })
        .collect()
}

fn read_list_with_keys<V: DeserializeOwned>(
    txn: &ReadTransaction,
    table: TableDefinition<&str, &[u8]>,
) -> HashMap<String, V> {
    let t = txn.open_table(table).expect("open_table");
    t.iter()
        .expect("iter")
        .map(|r| {
            let (k, v) = r.expect("row");
            (
                k.value().to_string(),
                bincode::deserialize(v.value()).expect("decode"),
            )
        })
        .collect()
}

fn read_keys(txn: &ReadTransaction, table: TableDefinition<&str, &[u8]>) -> Vec<String> {
    let t = txn.open_table(table).expect("open_table");
    t.iter()
        .expect("iter")
        .map(|r| {
            let (k, _) = r.expect("row");
            k.value().to_string()
        })
        .collect()
}

fn read_count(txn: &ReadTransaction, table: TableDefinition<&str, &[u8]>) -> usize {
    let t = txn.open_table(table).expect("open_table");
    t.len().expect("len") as usize
}

fn txn_get<V: DeserializeOwned>(
    txn: &WriteTransaction,
    table: TableDefinition<&str, &[u8]>,
    key: &str,
) -> Option<V> {
    let t = txn.open_table(table).expect("open_table");
    let g = t.get(key).expect("get")?;
    Some(bincode::deserialize(g.value()).expect("decode"))
}

fn txn_has(txn: &WriteTransaction, table: TableDefinition<&str, &[u8]>, key: &str) -> bool {
    let t = txn.open_table(table).expect("open_table");
    t.get(key).expect("get").is_some()
}

fn txn_list<V: DeserializeOwned>(
    txn: &WriteTransaction,
    table: TableDefinition<&str, &[u8]>,
) -> Vec<V> {
    let t = txn.open_table(table).expect("open_table");
    t.iter()
        .expect("iter")
        .map(|r| {
            let (_, v) = r.expect("row");
            bincode::deserialize(v.value()).expect("decode")
        })
        .collect()
}

fn txn_put<V: Serialize>(
    txn: &WriteTransaction,
    table: TableDefinition<&str, &[u8]>,
    key: &str,
    val: &V,
) {
    let bytes = bincode::serialize(val).expect("encode");
    let mut t = txn.open_table(table).expect("open_table");
    t.insert(key, bytes.as_slice()).expect("insert");
}

fn txn_delete(txn: &WriteTransaction, table: TableDefinition<&str, &[u8]>, key: &str) {
    let mut t = txn.open_table(table).expect("open_table");
    t.remove(key).expect("remove");
}

/// API Server state - replicated across all nodes via Raft.
///
/// Storage: redb tables. Reads open short read transactions; writes happen
/// inside the apply path. `Default` opens an in-memory database (used by tests
/// and dev mode); `open(path)` opens a file-backed one for production.
pub struct ApiState {
    db: Arc<redb::Database>,
    applied_requests: Arc<parking_lot::Mutex<LruCache<String, Response>>>,
}

impl std::fmt::Debug for ApiState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiState").finish_non_exhaustive()
    }
}

impl Clone for ApiState {
    fn clone(&self) -> Self {
        Self {
            db: Arc::clone(&self.db),
            applied_requests: Arc::clone(&self.applied_requests),
        }
    }
}

impl Default for ApiState {
    fn default() -> Self {
        let db = redb::Builder::new()
            .create_with_backend(redb::backends::InMemoryBackend::new())
            .expect("create in-memory redb");
        Self::with_db(db)
    }
}

impl ApiState {
    /// Open a file-backed redb database for production use.
    pub fn open(path: &Path) -> Result<Self, redb::DatabaseError> {
        let db = redb::Database::create(path)?;
        Ok(Self::with_db(db))
    }

    fn with_db(db: redb::Database) -> Self {
        let s = Self {
            db: Arc::new(db),
            applied_requests: Arc::new(parking_lot::Mutex::new(LruCache::new(
                NonZeroUsize::new(1000).unwrap(),
            ))),
        };
        s.init_tables();
        s
    }

    fn init_tables(&self) {
        let txn = self.db.begin_write().expect("begin_write init");
        for table in ALL_TABLES {
            txn.open_table(*table).expect("open_table init");
        }
        txn.commit().expect("commit init");
    }

    fn read_txn(&self) -> ReadTransaction {
        self.db.begin_read().expect("begin_read")
    }

    // =========================================================================
    // Node queries
    // =========================================================================

    pub fn get_node(&self, id: &str) -> Option<NodeData> {
        read_get(&self.read_txn(), NODES, id)
    }

    pub fn get_node_by_name(&self, name: &str) -> Option<NodeData> {
        read_list::<NodeData>(&self.read_txn(), NODES)
            .into_iter()
            .find(|n| n.name == name)
    }

    pub fn list_nodes(&self) -> Vec<NodeData> {
        read_list(&self.read_txn(), NODES)
    }

    pub fn list_online_nodes(&self) -> Vec<NodeData> {
        read_list::<NodeData>(&self.read_txn(), NODES)
            .into_iter()
            .filter(|n| n.status == NodeStatus::Online)
            .collect()
    }

    pub fn node_count(&self) -> usize {
        read_count(&self.read_txn(), NODES)
    }

    pub fn node_ids(&self) -> Vec<String> {
        read_keys(&self.read_txn(), NODES)
    }

    // =========================================================================
    // Network queries
    // =========================================================================

    pub fn get_network(&self, id: &str) -> Option<NetworkData> {
        read_get(&self.read_txn(), NETWORKS, id)
    }

    pub fn get_network_by_name(&self, name: &str) -> Option<NetworkData> {
        read_list::<NetworkData>(&self.read_txn(), NETWORKS)
            .into_iter()
            .find(|n| n.name == name)
    }

    pub fn list_networks(&self) -> Vec<NetworkData> {
        read_list(&self.read_txn(), NETWORKS)
    }

    pub fn list_networks_by_project(&self, project_id: &str) -> Vec<NetworkData> {
        read_list::<NetworkData>(&self.read_txn(), NETWORKS)
            .into_iter()
            .filter(|n| n.project_id == project_id)
            .collect()
    }

    pub fn network_count(&self) -> usize {
        read_count(&self.read_txn(), NETWORKS)
    }

    pub fn network_ids(&self) -> Vec<String> {
        read_keys(&self.read_txn(), NETWORKS)
    }

    // =========================================================================
    // NIC queries
    // =========================================================================

    pub fn get_nic(&self, id: &str) -> Option<NicData> {
        read_get(&self.read_txn(), NICS, id)
    }

    pub fn get_nic_by_name(&self, name: &str) -> Option<NicData> {
        read_list::<NicData>(&self.read_txn(), NICS)
            .into_iter()
            .find(|n| n.spec.name.as_deref() == Some(name))
    }

    pub fn list_nics(&self, network_id: Option<&str>) -> Vec<NicData> {
        let all = read_list::<NicData>(&self.read_txn(), NICS);
        match network_id {
            Some(net_id) => all
                .into_iter()
                .filter(|n| n.spec.network_id == net_id)
                .collect(),
            None => all,
        }
    }

    pub fn list_nics_by_project(&self, project_id: &str) -> Vec<NicData> {
        read_list::<NicData>(&self.read_txn(), NICS)
            .into_iter()
            .filter(|n| n.spec.project_id == project_id)
            .collect()
    }

    pub fn nic_count(&self) -> usize {
        read_count(&self.read_txn(), NICS)
    }

    pub fn nic_ids(&self) -> Vec<String> {
        read_keys(&self.read_txn(), NICS)
    }

    // =========================================================================
    // VM queries
    // =========================================================================

    pub fn get_vm(&self, id: &str) -> Option<VmData> {
        read_get(&self.read_txn(), VMS, id)
    }

    pub fn get_vm_by_name(&self, name: &str) -> Option<VmData> {
        read_list::<VmData>(&self.read_txn(), VMS)
            .into_iter()
            .find(|v| v.spec.name == name)
    }

    pub fn list_vms(&self, node_id: Option<&str>) -> Vec<VmData> {
        let all = read_list::<VmData>(&self.read_txn(), VMS);
        match node_id {
            Some(nid) => all
                .into_iter()
                .filter(|v| v.status.node_id.as_deref() == Some(nid))
                .collect(),
            None => all,
        }
    }

    pub fn list_vms_by_phase(&self, phase: VmPhase) -> Vec<VmData> {
        read_list::<VmData>(&self.read_txn(), VMS)
            .into_iter()
            .filter(|v| v.status.phase == phase)
            .collect()
    }

    pub fn list_vms_by_project(&self, project_id: &str) -> Vec<VmData> {
        read_list::<VmData>(&self.read_txn(), VMS)
            .into_iter()
            .filter(|v| v.spec.project_id == project_id)
            .collect()
    }

    pub fn vm_count(&self) -> usize {
        read_count(&self.read_txn(), VMS)
    }

    pub fn vm_ids(&self) -> Vec<String> {
        read_keys(&self.read_txn(), VMS)
    }

    // =========================================================================
    // Org queries
    // =========================================================================

    pub fn get_org(&self, id: &str) -> Option<OrgData> {
        read_get(&self.read_txn(), ORGS, id)
    }

    pub fn get_org_by_slug(&self, slug: &str) -> Option<OrgData> {
        read_list::<OrgData>(&self.read_txn(), ORGS)
            .into_iter()
            .find(|o| o.slug == slug)
    }

    pub fn list_orgs(&self) -> Vec<OrgData> {
        read_list(&self.read_txn(), ORGS)
    }

    pub fn org_count(&self) -> usize {
        read_count(&self.read_txn(), ORGS)
    }

    pub fn org_ids(&self) -> Vec<String> {
        read_keys(&self.read_txn(), ORGS)
    }

    // =========================================================================
    // Project queries
    // =========================================================================

    pub fn get_project(&self, id: &str) -> Option<ProjectData> {
        read_get(&self.read_txn(), PROJECTS, id)
    }

    pub fn get_project_by_slug(&self, slug: &str) -> Option<ProjectData> {
        read_list::<ProjectData>(&self.read_txn(), PROJECTS)
            .into_iter()
            .find(|p| p.slug == slug)
    }

    pub fn list_projects(&self) -> Vec<ProjectData> {
        read_list(&self.read_txn(), PROJECTS)
    }

    pub fn list_projects_by_org(&self, org_id: &str) -> Vec<ProjectData> {
        read_list::<ProjectData>(&self.read_txn(), PROJECTS)
            .into_iter()
            .filter(|p| p.org_id == org_id)
            .collect()
    }

    // =========================================================================
    // Volume queries
    // =========================================================================

    pub fn get_volume(&self, id: &str) -> Option<VolumeData> {
        read_get(&self.read_txn(), VOLUMES, id)
    }

    pub fn get_volume_by_name(&self, project_id: &str, name: &str) -> Option<VolumeData> {
        read_list::<VolumeData>(&self.read_txn(), VOLUMES)
            .into_iter()
            .find(|v| v.spec.project_id == project_id && v.spec.name == name)
    }

    pub fn list_volumes(&self, project_id: Option<&str>, node_id: Option<&str>) -> Vec<VolumeData> {
        read_list::<VolumeData>(&self.read_txn(), VOLUMES)
            .into_iter()
            .filter(|v| {
                project_id.is_none_or(|pid| v.spec.project_id == pid)
                    && node_id.is_none_or(|nid| v.spec.node_id == nid)
            })
            .collect()
    }

    pub fn volume_count(&self) -> usize {
        read_count(&self.read_txn(), VOLUMES)
    }

    pub fn volume_ids(&self) -> Vec<String> {
        read_keys(&self.read_txn(), VOLUMES)
    }

    // =========================================================================
    // Template queries
    // =========================================================================

    pub fn get_template(&self, id: &str) -> Option<TemplateData> {
        read_get(&self.read_txn(), TEMPLATES, id)
    }

    pub fn get_template_by_name(&self, name: &str) -> Option<TemplateData> {
        read_list::<TemplateData>(&self.read_txn(), TEMPLATES)
            .into_iter()
            .find(|t| t.spec.name == name)
    }

    pub fn list_templates(&self, node_id: Option<&str>) -> Vec<TemplateData> {
        let all = read_list::<TemplateData>(&self.read_txn(), TEMPLATES);
        match node_id {
            Some(nid) => all.into_iter().filter(|t| t.spec.node_id == nid).collect(),
            None => all,
        }
    }

    pub fn list_templates_by_project(&self, project_id: &str) -> Vec<TemplateData> {
        read_list::<TemplateData>(&self.read_txn(), TEMPLATES)
            .into_iter()
            .filter(|t| t.spec.project_id == project_id)
            .collect()
    }

    pub fn template_count(&self) -> usize {
        read_count(&self.read_txn(), TEMPLATES)
    }

    pub fn template_ids(&self) -> Vec<String> {
        read_keys(&self.read_txn(), TEMPLATES)
    }

    // =========================================================================
    // Security Group queries
    // =========================================================================

    pub fn get_security_group(&self, id: &str) -> Option<SecurityGroupData> {
        read_get(&self.read_txn(), SECURITY_GROUPS, id)
    }

    pub fn list_security_groups(&self) -> Vec<SecurityGroupData> {
        read_list(&self.read_txn(), SECURITY_GROUPS)
    }

    pub fn list_security_groups_by_project(&self, project_id: &str) -> Vec<SecurityGroupData> {
        read_list::<SecurityGroupData>(&self.read_txn(), SECURITY_GROUPS)
            .into_iter()
            .filter(|sg| sg.project_id == project_id)
            .collect()
    }

    pub fn security_group_count(&self) -> usize {
        read_count(&self.read_txn(), SECURITY_GROUPS)
    }

    pub fn security_group_ids(&self) -> Vec<String> {
        read_keys(&self.read_txn(), SECURITY_GROUPS)
    }
}

impl StateMachine<Command, Response> for ApiState {
    type Event = Event;

    fn apply(&mut self, cmd: Command) -> (Response, Vec<Self::Event>) {
        // Check idempotency cache
        if let Some(response) = self.applied_requests.lock().peek(cmd.request_id()).cloned() {
            return (response, vec![]);
        }

        let (response, events) = match cmd.clone() {
            // =================================================================
            // Node Commands
            // =================================================================
            Command::RegisterNode {
                id,
                timestamp,
                name,
                address,
                resources,
                labels,
                ..
            } => {
                let txn = self.db.begin_write().expect("begin");
                if txn_list::<NodeData>(&txn, NODES)
                    .iter()
                    .any(|n| n.name == name)
                {
                    return (
                        Response::Error {
                            code: 409,
                            message: format!("Node with name '{}' already exists", name),
                        },
                        vec![],
                    );
                }

                if let Some(existing) = txn_get::<NodeData>(&txn, NODES, &id) {
                    return (Response::Node(existing), vec![]);
                }

                let node = NodeData {
                    id: id.clone(),
                    name,
                    address,
                    status: NodeStatus::Online,
                    resources,
                    labels,
                    last_heartbeat: timestamp.clone(),
                    created_at: timestamp.clone(),
                    updated_at: timestamp,
                };

                txn_put(&txn, NODES, &id, &node);
                txn.commit().expect("commit");
                (
                    Response::Node(node.clone()),
                    vec![Event::NodeRegistered(node)],
                )
            }

            Command::UpdateNodeStatus {
                node_id,
                timestamp,
                status,
                resources,
                ..
            } => {
                let txn = self.db.begin_write().expect("begin");
                let Some(old_node) = txn_get::<NodeData>(&txn, NODES, &node_id) else {
                    return (
                        Response::Error {
                            code: 404,
                            message: format!("Node '{}' not found", node_id),
                        },
                        vec![],
                    );
                };
                let mut new_node = old_node.clone();
                new_node.status = status;
                if let Some(res) = resources {
                    new_node.resources = res;
                }
                new_node.last_heartbeat = timestamp.clone();
                new_node.updated_at = timestamp;
                txn_put(&txn, NODES, &node_id, &new_node);
                txn.commit().expect("commit");
                (
                    Response::Node(new_node.clone()),
                    vec![Event::NodeUpdated {
                        id: node_id,
                        old: old_node,
                        new: new_node,
                    }],
                )
            }

            Command::DeregisterNode { node_id, .. } => {
                let txn = self.db.begin_write().expect("begin");
                if !txn_has(&txn, NODES, &node_id) {
                    return (
                        Response::Error {
                            code: 404,
                            message: format!("Node '{}' not found", node_id),
                        },
                        vec![],
                    );
                }
                txn_delete(&txn, NODES, &node_id);
                txn.commit().expect("commit");
                (
                    Response::Deleted {
                        id: node_id.clone(),
                    },
                    vec![Event::NodeDeregistered { id: node_id }],
                )
            }

            // =================================================================
            // Network Commands
            // =================================================================
            Command::CreateNetwork {
                id,
                timestamp,
                project_id,
                name,
                ipv4_enabled,
                ipv4_prefix,
                ipv6_enabled,
                ipv6_prefix,
                dns_servers,
                ntp_servers,
                is_public,
                ..
            } => {
                let txn = self.db.begin_write().expect("begin");
                if txn_list::<NetworkData>(&txn, NETWORKS)
                    .iter()
                    .any(|n| n.project_id == project_id && n.name == name)
                {
                    return (
                        Response::Error {
                            code: 409,
                            message: format!(
                                "Network with name '{}' already exists in project",
                                name
                            ),
                        },
                        vec![],
                    );
                }

                if let Some(existing) = txn_get::<NetworkData>(&txn, NETWORKS, &id) {
                    return (Response::Network(existing), vec![]);
                }

                let network = NetworkData {
                    id: id.clone(),
                    project_id,
                    name,
                    ipv4_enabled,
                    ipv4_prefix,
                    ipv6_enabled,
                    ipv6_prefix,
                    dns_servers,
                    ntp_servers,
                    is_public,
                    nic_count: 0,
                    created_at: timestamp.clone(),
                    updated_at: timestamp,
                };

                txn_put(&txn, NETWORKS, &id, &network);
                txn.commit().expect("commit");
                (
                    Response::Network(network.clone()),
                    vec![Event::NetworkCreated(network)],
                )
            }

            Command::UpdateNetwork {
                id,
                timestamp,
                dns_servers,
                ntp_servers,
                ..
            } => {
                let txn = self.db.begin_write().expect("begin");
                let Some(old_network) = txn_get::<NetworkData>(&txn, NETWORKS, &id) else {
                    return (
                        Response::Error {
                            code: 404,
                            message: format!("Network '{}' not found", id),
                        },
                        vec![],
                    );
                };
                let mut new_network = old_network.clone();
                new_network.dns_servers = dns_servers;
                new_network.ntp_servers = ntp_servers;
                new_network.updated_at = timestamp;
                txn_put(&txn, NETWORKS, &id, &new_network);
                txn.commit().expect("commit");
                (
                    Response::Network(new_network.clone()),
                    vec![Event::NetworkUpdated {
                        id,
                        old: old_network,
                        new: new_network,
                    }],
                )
            }

            Command::DeleteNetwork { id, force, .. } => {
                let txn = self.db.begin_write().expect("begin");
                let nics_in_network: Vec<NicData> = txn_list::<NicData>(&txn, NICS)
                    .into_iter()
                    .filter(|n| n.spec.network_id == id)
                    .collect();

                if !nics_in_network.is_empty() && !force {
                    return (
                        Response::Error {
                            code: 409,
                            message: format!(
                                "Network has {} NICs, use force=true to delete",
                                nics_in_network.len()
                            ),
                        },
                        vec![],
                    );
                }

                if !txn_has(&txn, NETWORKS, &id) {
                    return (
                        Response::Error {
                            code: 404,
                            message: format!("Network '{}' not found", id),
                        },
                        vec![],
                    );
                }

                let nics_deleted = nics_in_network.len() as u32;
                let mut events = Vec::new();
                for nic in nics_in_network {
                    txn_delete(&txn, NICS, &nic.id);
                    events.push(Event::NicDeleted {
                        id: nic.id,
                        network_id: nic.spec.network_id,
                    });
                }
                txn_delete(&txn, NETWORKS, &id);
                txn.commit().expect("commit");
                events.push(Event::NetworkDeleted { id: id.clone() });
                (Response::DeletedWithCount { id, nics_deleted }, events)
            }

            Command::CreateNic {
                id,
                timestamp,
                project_id,
                network_id,
                name,
                mac_address,
                ipv4_address,
                ipv6_address,
                routed_ipv4_prefixes,
                routed_ipv6_prefixes,
                security_group_id,
                ..
            } => {
                let txn = self.db.begin_write().expect("begin");

                let Some(mut network) = txn_get::<NetworkData>(&txn, NETWORKS, &network_id) else {
                    return (
                        Response::Error {
                            code: 404,
                            message: format!("Network '{}' not found", network_id),
                        },
                        vec![],
                    );
                };

                let mut sg_to_update: Option<SecurityGroupData> = None;
                if let Some(ref sg_id) = security_group_id {
                    match txn_get::<SecurityGroupData>(&txn, SECURITY_GROUPS, sg_id) {
                        Some(sg) => sg_to_update = Some(sg),
                        None => {
                            return (
                                Response::Error {
                                    code: 404,
                                    message: format!("Security group '{}' not found", sg_id),
                                },
                                vec![],
                            );
                        }
                    }
                }

                if let Some(existing) = txn_get::<NicData>(&txn, NICS, &id) {
                    return (Response::Nic(existing), vec![]);
                }

                let mac = mac_address.unwrap_or_else(|| generate_mac_from_id(&id));

                let nic = NicData {
                    id: id.clone(),
                    spec: NicSpec {
                        project_id,
                        name,
                        network_id: network_id.clone(),
                        mac_address: mac,
                        ipv4_address,
                        ipv6_address,
                        routed_ipv4_prefixes,
                        routed_ipv6_prefixes,
                        security_group_id,
                        vm_id: None,
                    },
                    status: NicStatus {
                        socket_path: format!("/run/mvirt-net/nic-{}.sock", id),
                        ..Default::default()
                    },
                    created_at: timestamp.clone(),
                    updated_at: timestamp,
                };

                txn_put(&txn, NICS, &id, &nic);

                network.nic_count += 1;
                txn_put(&txn, NETWORKS, &network_id, &network);

                if let Some(mut sg) = sg_to_update {
                    sg.nic_count += 1;
                    let sg_id = sg.id.clone();
                    txn_put(&txn, SECURITY_GROUPS, &sg_id, &sg);
                }

                txn.commit().expect("commit");
                (Response::Nic(nic.clone()), vec![Event::NicCreated(nic)])
            }

            Command::UpdateNic {
                id,
                timestamp,
                routed_ipv4_prefixes,
                routed_ipv6_prefixes,
                ..
            } => {
                let txn = self.db.begin_write().expect("begin");
                let Some(old_nic) = txn_get::<NicData>(&txn, NICS, &id) else {
                    return (
                        Response::Error {
                            code: 404,
                            message: format!("NIC '{}' not found", id),
                        },
                        vec![],
                    );
                };
                let mut new_nic = old_nic.clone();
                new_nic.spec.routed_ipv4_prefixes = routed_ipv4_prefixes;
                new_nic.spec.routed_ipv6_prefixes = routed_ipv6_prefixes;
                new_nic.updated_at = timestamp;
                txn_put(&txn, NICS, &id, &new_nic);
                txn.commit().expect("commit");
                (
                    Response::Nic(new_nic.clone()),
                    vec![Event::NicUpdated {
                        id,
                        old: old_nic,
                        new: new_nic,
                    }],
                )
            }

            Command::DeleteNic { id, .. } => {
                let txn = self.db.begin_write().expect("begin");
                let Some(nic) = txn_get::<NicData>(&txn, NICS, &id) else {
                    return (
                        Response::Error {
                            code: 404,
                            message: format!("NIC '{}' not found", id),
                        },
                        vec![],
                    );
                };
                txn_delete(&txn, NICS, &id);

                if let Some(mut network) =
                    txn_get::<NetworkData>(&txn, NETWORKS, &nic.spec.network_id)
                {
                    network.nic_count = network.nic_count.saturating_sub(1);
                    txn_put(&txn, NETWORKS, &nic.spec.network_id, &network);
                }

                if let Some(ref sg_id) = nic.spec.security_group_id
                    && let Some(mut sg) = txn_get::<SecurityGroupData>(&txn, SECURITY_GROUPS, sg_id)
                {
                    sg.nic_count = sg.nic_count.saturating_sub(1);
                    txn_put(&txn, SECURITY_GROUPS, sg_id, &sg);
                }

                txn.commit().expect("commit");
                let network_id = nic.spec.network_id.clone();
                (
                    Response::Deleted { id: id.clone() },
                    vec![Event::NicDeleted { id, network_id }],
                )
            }

            Command::AttachNic {
                id,
                timestamp,
                vm_id,
                ..
            } => {
                let txn = self.db.begin_write().expect("begin");
                let Some(old_nic) = txn_get::<NicData>(&txn, NICS, &id) else {
                    return (
                        Response::Error {
                            code: 404,
                            message: format!("NIC '{}' not found", id),
                        },
                        vec![],
                    );
                };
                if old_nic.spec.vm_id.is_some() {
                    return (
                        Response::Error {
                            code: 409,
                            message: format!("NIC '{}' is already attached to a VM", id),
                        },
                        vec![],
                    );
                }
                if !txn_has(&txn, VMS, &vm_id) {
                    return (
                        Response::Error {
                            code: 404,
                            message: format!("VM '{}' not found", vm_id),
                        },
                        vec![],
                    );
                }
                let mut new_nic = old_nic.clone();
                new_nic.spec.vm_id = Some(vm_id);
                new_nic.updated_at = timestamp;
                txn_put(&txn, NICS, &id, &new_nic);
                txn.commit().expect("commit");
                (
                    Response::Nic(new_nic.clone()),
                    vec![Event::NicUpdated {
                        id,
                        old: old_nic,
                        new: new_nic,
                    }],
                )
            }

            Command::DetachNic { id, timestamp, .. } => {
                let txn = self.db.begin_write().expect("begin");
                let Some(old_nic) = txn_get::<NicData>(&txn, NICS, &id) else {
                    return (
                        Response::Error {
                            code: 404,
                            message: format!("NIC '{}' not found", id),
                        },
                        vec![],
                    );
                };
                let mut new_nic = old_nic.clone();
                new_nic.spec.vm_id = None;
                new_nic.updated_at = timestamp;
                txn_put(&txn, NICS, &id, &new_nic);
                txn.commit().expect("commit");
                (
                    Response::Nic(new_nic.clone()),
                    vec![Event::NicUpdated {
                        id,
                        old: old_nic,
                        new: new_nic,
                    }],
                )
            }

            Command::UpdateNicStatus {
                id,
                timestamp,
                phase,
                socket_path,
                message,
                ..
            } => {
                let txn = self.db.begin_write().expect("begin");
                let Some(old_nic) = txn_get::<NicData>(&txn, NICS, &id) else {
                    return (
                        Response::Error {
                            code: 404,
                            message: format!("NIC '{}' not found", id),
                        },
                        vec![],
                    );
                };
                let mut nic = old_nic;
                nic.status.phase = phase;
                if !socket_path.is_empty() {
                    nic.status.socket_path = socket_path;
                }
                nic.status.message = message;
                nic.updated_at = timestamp;
                txn_put(&txn, NICS, &id, &nic);
                txn.commit().expect("commit");
                (Response::Nic(nic), vec![])
            }

            // =================================================================
            // VM Commands
            // =================================================================
            Command::CreateVm {
                id,
                timestamp,
                spec,
                ..
            } => {
                let txn = self.db.begin_write().expect("begin");
                if txn_list::<VmData>(&txn, VMS)
                    .iter()
                    .any(|v| v.spec.project_id == spec.project_id && v.spec.name == spec.name)
                {
                    return (
                        Response::Error {
                            code: 409,
                            message: format!(
                                "VM with name '{}' already exists in project",
                                spec.name
                            ),
                        },
                        vec![],
                    );
                }

                if let Some(existing) = txn_get::<VmData>(&txn, VMS, &id) {
                    return (Response::Vm(existing), vec![]);
                }

                if !txn_has(&txn, NICS, &spec.nic_id) {
                    return (
                        Response::Error {
                            code: 404,
                            message: format!("NIC '{}' not found", spec.nic_id),
                        },
                        vec![],
                    );
                }

                let vm = VmData {
                    id: id.clone(),
                    spec,
                    status: VmStatus {
                        phase: VmPhase::Pending,
                        ..Default::default()
                    },
                    created_at: timestamp.clone(),
                    updated_at: timestamp,
                };

                txn_put(&txn, VMS, &id, &vm);
                txn.commit().expect("commit");
                (Response::Vm(vm.clone()), vec![Event::VmCreated(vm)])
            }

            Command::UpdateVmSpec {
                id,
                timestamp,
                desired_state,
                ..
            } => {
                let txn = self.db.begin_write().expect("begin");
                let Some(old_vm) = txn_get::<VmData>(&txn, VMS, &id) else {
                    return (
                        Response::Error {
                            code: 404,
                            message: format!("VM '{}' not found", id),
                        },
                        vec![],
                    );
                };
                let mut new_vm = old_vm.clone();
                new_vm.spec.desired_state = desired_state;
                new_vm.updated_at = timestamp;
                txn_put(&txn, VMS, &id, &new_vm);
                txn.commit().expect("commit");
                (
                    Response::Vm(new_vm.clone()),
                    vec![Event::VmUpdated {
                        id,
                        old: old_vm,
                        new: new_vm,
                    }],
                )
            }

            Command::UpdateVmStatus {
                id,
                timestamp,
                status,
                ..
            } => {
                let txn = self.db.begin_write().expect("begin");
                let Some(old_vm) = txn_get::<VmData>(&txn, VMS, &id) else {
                    return (
                        Response::Error {
                            code: 404,
                            message: format!("VM '{}' not found", id),
                        },
                        vec![],
                    );
                };
                let mut new_vm = old_vm.clone();
                new_vm.status = status;
                new_vm.updated_at = timestamp;
                txn_put(&txn, VMS, &id, &new_vm);
                txn.commit().expect("commit");
                (
                    Response::Vm(new_vm.clone()),
                    vec![Event::VmStatusUpdated {
                        id,
                        old: old_vm,
                        new: new_vm,
                    }],
                )
            }

            Command::DeleteVm { id, .. } => {
                let txn = self.db.begin_write().expect("begin");
                if !txn_has(&txn, VMS, &id) {
                    return (
                        Response::Error {
                            code: 404,
                            message: format!("VM '{}' not found", id),
                        },
                        vec![],
                    );
                }
                txn_delete(&txn, VMS, &id);
                txn.commit().expect("commit");
                (
                    Response::Deleted { id: id.clone() },
                    vec![Event::VmDeleted { id }],
                )
            }

            // =================================================================
            // Org Commands
            // =================================================================
            Command::CreateOrg {
                id,
                timestamp,
                slug,
                name,
                default_static_key_ttl_days,
                disallow_static_keys,
                ..
            } => {
                let txn = self.db.begin_write().expect("begin");

                if let Some(existing) = txn_get::<OrgData>(&txn, ORGS, &id) {
                    return (Response::Org(existing), vec![]);
                }

                if txn_list::<OrgData>(&txn, ORGS)
                    .iter()
                    .any(|o| o.slug == slug)
                {
                    return (
                        Response::Error {
                            code: 409,
                            message: format!("Org with slug '{}' already exists", slug),
                        },
                        vec![],
                    );
                }

                let org = OrgData {
                    id: id.clone(),
                    slug,
                    name,
                    default_static_key_ttl_days,
                    disallow_static_keys,
                    created_at: timestamp.clone(),
                    updated_at: timestamp,
                };

                txn_put(&txn, ORGS, &id, &org);
                txn.commit().expect("commit");
                (Response::Org(org), vec![])
            }

            Command::UpdateOrg {
                id,
                timestamp,
                name,
                default_static_key_ttl_days,
                disallow_static_keys,
                ..
            } => {
                let txn = self.db.begin_write().expect("begin");
                let Some(mut org) = txn_get::<OrgData>(&txn, ORGS, &id) else {
                    return (
                        Response::Error {
                            code: 404,
                            message: format!("Org '{}' not found", id),
                        },
                        vec![],
                    );
                };
                if let Some(n) = name {
                    org.name = n;
                }
                if let Some(t) = default_static_key_ttl_days {
                    org.default_static_key_ttl_days = t;
                }
                if let Some(d) = disallow_static_keys {
                    org.disallow_static_keys = d;
                }
                org.updated_at = timestamp;
                txn_put(&txn, ORGS, &id, &org);
                txn.commit().expect("commit");
                (Response::Org(org), vec![])
            }

            Command::DeleteOrg { id, .. } => {
                let txn = self.db.begin_write().expect("begin");
                if !txn_has(&txn, ORGS, &id) {
                    return (
                        Response::Error {
                            code: 404,
                            message: format!("Org '{}' not found", id),
                        },
                        vec![],
                    );
                }
                let project_count = txn_list::<ProjectData>(&txn, PROJECTS)
                    .iter()
                    .filter(|p| p.org_id == id)
                    .count();
                if project_count > 0 {
                    return (
                        Response::Error {
                            code: 409,
                            message: format!(
                                "Org '{}' has {} project(s); delete them first",
                                id, project_count
                            ),
                        },
                        vec![],
                    );
                }
                txn_delete(&txn, ORGS, &id);
                txn.commit().expect("commit");
                (Response::Deleted { id }, vec![])
            }

            // =================================================================
            // Project Commands
            // =================================================================
            Command::CreateProject {
                id,
                timestamp,
                org_id,
                slug,
                name,
                description,
                ..
            } => {
                let txn = self.db.begin_write().expect("begin");

                if let Some(existing) = txn_get::<ProjectData>(&txn, PROJECTS, &id) {
                    return (Response::Project(existing), vec![]);
                }

                if !txn_has(&txn, ORGS, &org_id) {
                    return (
                        Response::Error {
                            code: 404,
                            message: format!("Org '{}' not found", org_id),
                        },
                        vec![],
                    );
                }

                if txn_list::<ProjectData>(&txn, PROJECTS)
                    .iter()
                    .any(|p| p.slug == slug)
                {
                    return (
                        Response::Error {
                            code: 409,
                            message: format!("Project with slug '{}' already exists", slug),
                        },
                        vec![],
                    );
                }

                let project = ProjectData {
                    id: id.clone(),
                    org_id,
                    slug,
                    name,
                    description,
                    created_at: timestamp.clone(),
                    updated_at: timestamp,
                };

                txn_put(&txn, PROJECTS, &id, &project);
                txn.commit().expect("commit");
                (Response::Project(project), vec![])
            }

            Command::DeleteProject { id, .. } => {
                let txn = self.db.begin_write().expect("begin");
                if !txn_has(&txn, PROJECTS, &id) {
                    return (
                        Response::Error {
                            code: 404,
                            message: format!("Project '{}' not found", id),
                        },
                        vec![],
                    );
                }
                txn_delete(&txn, PROJECTS, &id);
                txn.commit().expect("commit");
                (Response::Deleted { id }, vec![])
            }

            // =================================================================
            // Volume Commands
            // =================================================================
            Command::CreateVolume {
                id,
                timestamp,
                project_id,
                node_id,
                name,
                size_bytes,
                template_id,
                ..
            } => {
                let txn = self.db.begin_write().expect("begin");

                if !txn_has(&txn, PROJECTS, &project_id) {
                    return (
                        Response::Error {
                            code: 404,
                            message: format!("Project '{}' not found", project_id),
                        },
                        vec![],
                    );
                }

                if let Some(existing) = txn_get::<VolumeData>(&txn, VOLUMES, &id) {
                    return (Response::Volume(existing), vec![]);
                }

                if txn_list::<VolumeData>(&txn, VOLUMES)
                    .iter()
                    .any(|v| v.spec.project_id == project_id && v.spec.name == name)
                {
                    return (
                        Response::Error {
                            code: 409,
                            message: format!(
                                "Volume with name '{}' already exists in project",
                                name
                            ),
                        },
                        vec![],
                    );
                }

                let mut template_to_update: Option<TemplateData> = None;
                if let Some(ref tid) = template_id {
                    match txn_get::<TemplateData>(&txn, TEMPLATES, tid) {
                        Some(template) => {
                            if template.spec.node_id != node_id {
                                return (
                                    Response::Error {
                                        code: 409,
                                        message: format!(
                                            "Template {} is on node {} but volume targets node {}",
                                            tid, template.spec.node_id, node_id
                                        ),
                                    },
                                    vec![],
                                );
                            }
                            template_to_update = Some(template);
                        }
                        None => {
                            return (
                                Response::Error {
                                    code: 404,
                                    message: format!("Template '{}' not found", tid),
                                },
                                vec![],
                            );
                        }
                    }
                }

                let volume = VolumeData {
                    id: id.clone(),
                    spec: VolumeSpec {
                        project_id,
                        node_id,
                        name,
                        size_bytes,
                        template_id,
                    },
                    status: VolumeStatus {
                        compression_ratio: 1.0,
                        ..Default::default()
                    },
                    created_at: timestamp.clone(),
                    updated_at: timestamp,
                };

                if let Some(mut template) = template_to_update {
                    template.status.clone_count += 1;
                    let tid = template.id.clone();
                    txn_put(&txn, TEMPLATES, &tid, &template);
                }

                txn_put(&txn, VOLUMES, &id, &volume);
                txn.commit().expect("commit");
                (
                    Response::Volume(volume.clone()),
                    vec![Event::VolumeCreated(volume)],
                )
            }

            Command::DeleteVolume { id, .. } => {
                let txn = self.db.begin_write().expect("begin");
                let Some(vol) = txn_get::<VolumeData>(&txn, VOLUMES, &id) else {
                    return (
                        Response::Error {
                            code: 404,
                            message: format!("Volume '{}' not found", id),
                        },
                        vec![],
                    );
                };
                txn_delete(&txn, VOLUMES, &id);

                if let Some(tid) = &vol.spec.template_id
                    && let Some(mut template) = txn_get::<TemplateData>(&txn, TEMPLATES, tid)
                {
                    template.status.clone_count = template.status.clone_count.saturating_sub(1);
                    txn_put(&txn, TEMPLATES, tid, &template);
                }

                txn.commit().expect("commit");
                let node_id = vol.spec.node_id.clone();
                (
                    Response::Deleted { id: id.clone() },
                    vec![Event::VolumeDeleted { id, node_id }],
                )
            }

            Command::UpdateVolumeStatus {
                id,
                timestamp,
                phase,
                path,
                used_bytes,
                error,
                ..
            } => {
                let txn = self.db.begin_write().expect("begin");
                let Some(mut vol) = txn_get::<VolumeData>(&txn, VOLUMES, &id) else {
                    return (
                        Response::Error {
                            code: 404,
                            message: format!("Volume '{}' not found", id),
                        },
                        vec![],
                    );
                };
                vol.status.phase = phase;
                if let Some(p) = path {
                    vol.status.path = p;
                }
                vol.status.used_bytes = used_bytes;
                vol.status.error = error;
                vol.updated_at = timestamp;
                txn_put(&txn, VOLUMES, &id, &vol);
                txn.commit().expect("commit");
                (Response::Volume(vol), vec![])
            }

            Command::ResizeVolume {
                id,
                timestamp,
                size_bytes,
                ..
            } => {
                let txn = self.db.begin_write().expect("begin");
                let Some(mut vol) = txn_get::<VolumeData>(&txn, VOLUMES, &id) else {
                    return (
                        Response::Error {
                            code: 404,
                            message: format!("Volume '{}' not found", id),
                        },
                        vec![],
                    );
                };
                if size_bytes < vol.spec.size_bytes {
                    return (
                        Response::Error {
                            code: 400,
                            message: "Cannot shrink volume".to_string(),
                        },
                        vec![],
                    );
                }
                vol.spec.size_bytes = size_bytes;
                vol.updated_at = timestamp;
                txn_put(&txn, VOLUMES, &id, &vol);
                txn.commit().expect("commit");
                (Response::Volume(vol), vec![])
            }

            Command::CreateSnapshot {
                id,
                timestamp,
                volume_id,
                name,
                ..
            } => {
                let txn = self.db.begin_write().expect("begin");
                let Some(mut vol) = txn_get::<VolumeData>(&txn, VOLUMES, &volume_id) else {
                    return (
                        Response::Error {
                            code: 404,
                            message: format!("Volume '{}' not found", volume_id),
                        },
                        vec![],
                    );
                };
                if vol.status.snapshots.iter().any(|s| s.name == name) {
                    return (
                        Response::Error {
                            code: 409,
                            message: format!(
                                "Snapshot with name '{}' already exists on volume",
                                name
                            ),
                        },
                        vec![],
                    );
                }

                let snapshot = SnapshotData {
                    id,
                    name,
                    created_at: timestamp.clone(),
                    used_bytes: 0,
                };
                vol.status.snapshots.push(snapshot);
                vol.updated_at = timestamp;
                txn_put(&txn, VOLUMES, &volume_id, &vol);
                txn.commit().expect("commit");
                (Response::Volume(vol), vec![])
            }

            // =================================================================
            // Template Commands
            // =================================================================
            Command::CreateTemplate {
                id,
                timestamp,
                project_id,
                node_id,
                name,
                size_bytes,
                source_url,
                total_bytes,
                ..
            } => {
                let txn = self.db.begin_write().expect("begin");
                if txn_list::<TemplateData>(&txn, TEMPLATES).iter().any(|t| {
                    t.spec.project_id == project_id
                        && t.spec.name == name
                        && t.status.phase == TemplatePhase::Ready
                }) {
                    return (
                        Response::Error {
                            code: 409,
                            message: format!(
                                "Template with name '{}' already exists in project",
                                name
                            ),
                        },
                        vec![],
                    );
                }

                if let Some(existing) = txn_get::<TemplateData>(&txn, TEMPLATES, &id) {
                    return (Response::Template(existing), vec![]);
                }

                let phase = if source_url.is_some() {
                    TemplatePhase::Pending
                } else {
                    TemplatePhase::Ready
                };

                let template = TemplateData {
                    id: id.clone(),
                    spec: TemplateSpec {
                        project_id,
                        node_id,
                        name,
                        source_url,
                    },
                    status: TemplateStatus {
                        phase,
                        size_bytes,
                        bytes_written: 0,
                        total_bytes,
                        clone_count: 0,
                        error: None,
                    },
                    created_at: timestamp.clone(),
                    updated_at: timestamp,
                };

                txn_put(&txn, TEMPLATES, &id, &template);
                txn.commit().expect("commit");
                (
                    Response::Template(template.clone()),
                    vec![Event::TemplateCreated(template)],
                )
            }

            Command::UpdateTemplateStatus {
                id,
                timestamp,
                phase,
                bytes_written,
                size_bytes,
                error,
                ..
            } => {
                let txn = self.db.begin_write().expect("begin");
                let Some(old) = txn_get::<TemplateData>(&txn, TEMPLATES, &id) else {
                    return (
                        Response::Error {
                            code: 404,
                            message: format!("Template '{}' not found", id),
                        },
                        vec![],
                    );
                };
                let mut new = old.clone();
                new.status.phase = phase;
                new.status.bytes_written = bytes_written;
                new.status.size_bytes = size_bytes;
                new.status.error = error;
                new.updated_at = timestamp;
                txn_put(&txn, TEMPLATES, &id, &new);
                txn.commit().expect("commit");
                (
                    Response::Template(new.clone()),
                    vec![Event::TemplateUpdated { id, old, new }],
                )
            }

            // =================================================================
            // Security Group Commands
            // =================================================================
            Command::CreateSecurityGroup {
                id,
                timestamp,
                project_id,
                name,
                description,
                ..
            } => {
                let txn = self.db.begin_write().expect("begin");

                if let Some(existing) = txn_get::<SecurityGroupData>(&txn, SECURITY_GROUPS, &id) {
                    return (Response::SecurityGroup(existing), vec![]);
                }

                if txn_list::<SecurityGroupData>(&txn, SECURITY_GROUPS)
                    .iter()
                    .any(|sg| sg.project_id == project_id && sg.name == name)
                {
                    return (
                        Response::Error {
                            code: 409,
                            message: format!("Security group '{}' already exists in project", name),
                        },
                        vec![],
                    );
                }

                if !txn_has(&txn, PROJECTS, &project_id) {
                    return (
                        Response::Error {
                            code: 404,
                            message: format!("Project '{}' not found", project_id),
                        },
                        vec![],
                    );
                }

                let sg = SecurityGroupData {
                    id: id.clone(),
                    project_id,
                    name,
                    description,
                    rules: vec![],
                    nic_count: 0,
                    created_at: timestamp.clone(),
                    updated_at: timestamp,
                };

                let sg_id = id.clone();
                txn_put(&txn, SECURITY_GROUPS, &id, &sg);
                txn.commit().expect("commit");
                (
                    Response::SecurityGroup(sg),
                    vec![Event::SecurityGroupCreated { id: sg_id }],
                )
            }

            Command::DeleteSecurityGroup { id, .. } => {
                let txn = self.db.begin_write().expect("begin");
                let nic_refs = txn_list::<NicData>(&txn, NICS)
                    .iter()
                    .any(|n| n.spec.security_group_id.as_deref() == Some(&id));
                if nic_refs {
                    return (
                        Response::Error {
                            code: 409,
                            message: format!("Security group '{}' is still referenced by NICs", id),
                        },
                        vec![],
                    );
                }

                if !txn_has(&txn, SECURITY_GROUPS, &id) {
                    return (
                        Response::Error {
                            code: 404,
                            message: format!("Security group '{}' not found", id),
                        },
                        vec![],
                    );
                }
                txn_delete(&txn, SECURITY_GROUPS, &id);
                txn.commit().expect("commit");
                (
                    Response::Deleted { id: id.clone() },
                    vec![Event::SecurityGroupDeleted { id }],
                )
            }

            Command::CreateSecurityGroupRule {
                id,
                timestamp,
                security_group_id,
                direction,
                protocol,
                port_range_start,
                port_range_end,
                cidr,
                description,
                ..
            } => {
                let txn = self.db.begin_write().expect("begin");
                let Some(mut sg) =
                    txn_get::<SecurityGroupData>(&txn, SECURITY_GROUPS, &security_group_id)
                else {
                    return (
                        Response::Error {
                            code: 404,
                            message: format!("Security group '{}' not found", security_group_id),
                        },
                        vec![],
                    );
                };
                if sg.rules.iter().any(|r| r.id == id) {
                    return (Response::SecurityGroup(sg), vec![]);
                }
                let rule = SecurityGroupRuleData {
                    id,
                    direction,
                    protocol,
                    port_range_start,
                    port_range_end,
                    cidr,
                    description,
                    created_at: timestamp.clone(),
                };
                sg.rules.push(rule);
                sg.updated_at = timestamp;
                txn_put(&txn, SECURITY_GROUPS, &security_group_id, &sg);
                txn.commit().expect("commit");
                (Response::SecurityGroup(sg), vec![])
            }

            Command::DeleteSecurityGroupRule {
                security_group_id,
                rule_id,
                ..
            } => {
                let txn = self.db.begin_write().expect("begin");
                let Some(mut sg) =
                    txn_get::<SecurityGroupData>(&txn, SECURITY_GROUPS, &security_group_id)
                else {
                    return (
                        Response::Error {
                            code: 404,
                            message: format!("Security group '{}' not found", security_group_id),
                        },
                        vec![],
                    );
                };
                let before = sg.rules.len();
                sg.rules.retain(|r| r.id != rule_id);
                if sg.rules.len() == before {
                    return (
                        Response::Error {
                            code: 404,
                            message: format!("Rule '{}' not found", rule_id),
                        },
                        vec![],
                    );
                }
                txn_put(&txn, SECURITY_GROUPS, &security_group_id, &sg);
                txn.commit().expect("commit");
                (Response::SecurityGroup(sg), vec![])
            }
        };

        // Cache the response
        self.applied_requests
            .lock()
            .put(cmd.request_id().to_string(), response.clone());

        (response, events)
    }

    fn snapshot(&self) -> Result<Vec<u8>, mraft::StateMachineError> {
        let txn = self.db.begin_read()?;
        let envelope = SnapshotEnvelope {
            nodes: read_list_with_keys(&txn, NODES),
            networks: read_list_with_keys(&txn, NETWORKS),
            nics: read_list_with_keys(&txn, NICS),
            vms: read_list_with_keys(&txn, VMS),
            orgs: read_list_with_keys(&txn, ORGS),
            projects: read_list_with_keys(&txn, PROJECTS),
            volumes: read_list_with_keys(&txn, VOLUMES),
            templates: read_list_with_keys(&txn, TEMPLATES),
            security_groups: read_list_with_keys(&txn, SECURITY_GROUPS),
        };
        Ok(bincode::serialize(&envelope)?)
    }

    fn restore(&mut self, bytes: &[u8]) -> Result<(), mraft::StateMachineError> {
        let envelope: SnapshotEnvelope = bincode::deserialize(bytes)?;
        let txn = self.db.begin_write()?;

        // Clear each table by dropping and recreating it inside the same txn.
        for table in ALL_TABLES {
            txn.delete_table(*table)?;
            txn.open_table(*table)?;
        }

        for (k, v) in &envelope.nodes {
            txn_put(&txn, NODES, k, v);
        }
        for (k, v) in &envelope.networks {
            txn_put(&txn, NETWORKS, k, v);
        }
        for (k, v) in &envelope.nics {
            txn_put(&txn, NICS, k, v);
        }
        for (k, v) in &envelope.vms {
            txn_put(&txn, VMS, k, v);
        }
        for (k, v) in &envelope.orgs {
            txn_put(&txn, ORGS, k, v);
        }
        for (k, v) in &envelope.projects {
            txn_put(&txn, PROJECTS, k, v);
        }
        for (k, v) in &envelope.volumes {
            txn_put(&txn, VOLUMES, k, v);
        }
        for (k, v) in &envelope.templates {
            txn_put(&txn, TEMPLATES, k, v);
        }
        for (k, v) in &envelope.security_groups {
            txn_put(&txn, SECURITY_GROUPS, k, v);
        }

        txn.commit()?;
        // Reset the idempotency cache; a restored snapshot is from a different
        // logical timeline and we must not return cached responses for new ids.
        *self.applied_requests.lock() = LruCache::new(NonZeroUsize::new(1000).unwrap());
        Ok(())
    }

    fn requires_external_persistence(&self) -> bool {
        false
    }
}

#[derive(Serialize, Deserialize)]
struct SnapshotEnvelope {
    nodes: HashMap<String, NodeData>,
    networks: HashMap<String, NetworkData>,
    nics: HashMap<String, NicData>,
    vms: HashMap<String, VmData>,
    orgs: HashMap<String, OrgData>,
    projects: HashMap<String, ProjectData>,
    volumes: HashMap<String, VolumeData>,
    templates: HashMap<String, TemplateData>,
    security_groups: HashMap<String, SecurityGroupData>,
}

/// Generate a deterministic MAC address from an ID
fn generate_mac_from_id(id: &str) -> String {
    // Simple hash of the ID bytes
    let bytes = id.as_bytes();
    let mut hash: u64 = 0;
    for (i, &b) in bytes.iter().enumerate() {
        hash = hash.wrapping_add((b as u64).wrapping_mul(31u64.wrapping_pow(i as u32)));
    }

    // Use 52:54:00 prefix (QEMU/KVM range) with hash-based suffix
    format!(
        "52:54:00:{:02x}:{:02x}:{:02x}",
        ((hash >> 16) & 0xff) as u8,
        ((hash >> 8) & 0xff) as u8,
        (hash & 0xff) as u8
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use mraft::StateMachine;

    /// Helper to get just the response from apply (ignoring events).
    ///
    /// As a convenience for legacy tests, this auto-creates `TEST_ORG_ID` on
    /// the first `CreateProject` against the shared org id — keeps tests that
    /// predate the Org introduction working without per-test setup boilerplate.
    /// Tests that exercise Org behavior directly (`test_create_project_org_not_found`,
    /// `test_delete_org_with_projects_rejects`) bypass this by either not
    /// referencing TEST_ORG_ID or by setting up Orgs explicitly.
    fn apply(state: &mut ApiState, cmd: Command) -> Response {
        if let Command::CreateProject { org_id, .. } = &cmd
            && org_id == TEST_ORG_ID
            && state.get_org(TEST_ORG_ID).is_none()
        {
            let setup = create_org_cmd("test-org-setup", TEST_ORG_ID, TEST_ORG_SLUG);
            state.apply(setup);
        }
        let (response, _events) = state.apply(cmd);
        response
    }

    fn create_network_cmd(request_id: &str, id: &str, name: &str) -> Command {
        Command::CreateNetwork {
            request_id: request_id.to_string(),
            id: id.to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            project_id: "test-project".to_string(),
            name: name.to_string(),
            ipv4_enabled: true,
            ipv4_prefix: Some("10.0.0.0/24".to_string()),
            ipv6_enabled: false,
            ipv6_prefix: None,
            dns_servers: vec!["8.8.8.8".to_string()],
            ntp_servers: vec![],
            is_public: false,
        }
    }

    fn create_nic_cmd(request_id: &str, id: &str, network_id: &str, name: Option<&str>) -> Command {
        Command::CreateNic {
            request_id: request_id.to_string(),
            id: id.to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            project_id: "test-project".to_string(),
            network_id: network_id.to_string(),
            name: name.map(|s| s.to_string()),
            mac_address: None,
            ipv4_address: None,
            ipv6_address: None,
            routed_ipv4_prefixes: vec![],
            routed_ipv6_prefixes: vec![],
            security_group_id: None,
        }
    }

    #[test]
    fn test_create_network() {
        let mut state = ApiState::default();
        let cmd = create_network_cmd("req-1", "net-1", "test-network");
        let response = apply(&mut state, cmd);

        match response {
            Response::Network(data) => {
                assert_eq!(data.id, "net-1");
                assert_eq!(data.name, "test-network");
                assert!(data.ipv4_enabled);
                assert_eq!(data.ipv4_prefix, Some("10.0.0.0/24".to_string()));
                assert!(!data.ipv6_enabled);
                assert_eq!(data.dns_servers, vec!["8.8.8.8".to_string()]);
                assert_eq!(data.nic_count, 0);
            }
            other => panic!("Unexpected response: {:?}", other),
        }

        // Verify state
        assert!(state.get_network("net-1").is_some());
        assert_eq!(state.list_networks().len(), 1);
    }

    #[test]
    fn test_create_network_duplicate_name() {
        let mut state = ApiState::default();

        // Create first network
        let cmd1 = create_network_cmd("req-1", "net-1", "duplicate-name");
        let response1 = apply(&mut state, cmd1);
        assert!(matches!(response1, Response::Network(_)));

        // Try to create second network with same name but different ID
        let cmd2 = create_network_cmd("req-2", "net-2", "duplicate-name");
        let response2 = apply(&mut state, cmd2);

        match response2 {
            Response::Error { code, message } => {
                assert_eq!(code, 409);
                assert!(message.contains("duplicate-name"));
            }
            other => panic!("Expected error, got: {:?}", other),
        }
    }

    #[test]
    fn test_idempotency_same_request_id() {
        let mut state = ApiState::default();

        // First request
        let cmd1 = create_network_cmd("req-same", "net-1", "network-a");
        let response1 = apply(&mut state, cmd1);
        let network1 = match &response1 {
            Response::Network(data) => data.clone(),
            other => panic!("Unexpected response: {:?}", other),
        };

        // Same request_id should return cached response
        let cmd2 = create_network_cmd("req-same", "net-2", "network-b");
        let response2 = apply(&mut state, cmd2);
        let network2 = match response2 {
            Response::Network(data) => data,
            other => panic!("Unexpected response: {:?}", other),
        };

        // Should be the same (cached) network
        assert_eq!(network1.id, network2.id);
        assert_eq!(network1.name, network2.name);
    }

    #[test]
    fn test_idempotency_different_request_id() {
        let mut state = ApiState::default();

        // First request
        let cmd1 = create_network_cmd("req-1", "net-1", "network-a");
        let response1 = apply(&mut state, cmd1);
        assert!(matches!(response1, Response::Network(_)));

        // Different request_id should try to create new network (and fail due to duplicate name)
        let cmd2 = create_network_cmd("req-2", "net-2", "network-a");
        let response2 = apply(&mut state, cmd2);
        assert!(matches!(response2, Response::Error { code: 409, .. }));
    }

    #[test]
    fn test_duplicate_id_returns_existing() {
        let mut state = ApiState::default();

        // Create first network
        let cmd1 = create_network_cmd("req-1", "net-same-id", "network-a");
        let response1 = apply(&mut state, cmd1);
        assert!(matches!(response1, Response::Network(_)));

        // Same ID with different request_id should return existing (idempotent by ID)
        let cmd2 = create_network_cmd("req-2", "net-same-id", "network-b");
        let response2 = apply(&mut state, cmd2);
        match response2 {
            Response::Network(data) => {
                assert_eq!(data.id, "net-same-id");
                assert_eq!(data.name, "network-a"); // Original name, not "network-b"
            }
            other => panic!("Expected existing network, got: {:?}", other),
        }
    }

    #[test]
    fn test_delete_network_with_nics_no_force() {
        let mut state = ApiState::default();

        // Create network
        apply(&mut state, create_network_cmd("req-1", "net-1", "test-net"));

        // Create NIC in network
        apply(
            &mut state,
            create_nic_cmd("req-2", "nic-1", "net-1", Some("my-nic")),
        );

        // Try to delete without force
        let delete_cmd = Command::DeleteNetwork {
            request_id: "req-3".to_string(),
            id: "net-1".to_string(),
            force: false,
        };
        let response = apply(&mut state, delete_cmd);

        match response {
            Response::Error { code, message } => {
                assert_eq!(code, 409);
                assert!(message.contains("1 NIC"));
            }
            other => panic!("Expected error, got: {:?}", other),
        }

        // Network should still exist
        assert!(state.get_network("net-1").is_some());
    }

    #[test]
    fn test_delete_network_with_nics_force() {
        let mut state = ApiState::default();

        // Create network
        apply(&mut state, create_network_cmd("req-1", "net-1", "test-net"));

        // Create 2 NICs in network
        apply(&mut state, create_nic_cmd("req-2", "nic-1", "net-1", None));
        apply(&mut state, create_nic_cmd("req-3", "nic-2", "net-1", None));

        // Delete with force
        let delete_cmd = Command::DeleteNetwork {
            request_id: "req-4".to_string(),
            id: "net-1".to_string(),
            force: true,
        };
        let response = apply(&mut state, delete_cmd);

        match response {
            Response::DeletedWithCount { id, nics_deleted } => {
                assert_eq!(id, "net-1");
                assert_eq!(nics_deleted, 2);
            }
            other => panic!("Expected DeletedWithCount, got: {:?}", other),
        }

        // Network and NICs should be gone
        assert!(state.get_network("net-1").is_none());
        assert!(state.get_nic("nic-1").is_none());
        assert!(state.get_nic("nic-2").is_none());
    }

    #[test]
    fn test_nic_increments_network_counter() {
        let mut state = ApiState::default();

        // Create network
        apply(&mut state, create_network_cmd("req-1", "net-1", "test-net"));
        assert_eq!(state.get_network("net-1").unwrap().nic_count, 0);

        // Create NIC
        apply(&mut state, create_nic_cmd("req-2", "nic-1", "net-1", None));
        assert_eq!(state.get_network("net-1").unwrap().nic_count, 1);

        // Create another NIC
        apply(&mut state, create_nic_cmd("req-3", "nic-2", "net-1", None));
        assert_eq!(state.get_network("net-1").unwrap().nic_count, 2);
    }

    #[test]
    fn test_nic_decrements_network_counter() {
        let mut state = ApiState::default();

        // Create network and NICs
        apply(&mut state, create_network_cmd("req-1", "net-1", "test-net"));
        apply(&mut state, create_nic_cmd("req-2", "nic-1", "net-1", None));
        apply(&mut state, create_nic_cmd("req-3", "nic-2", "net-1", None));
        assert_eq!(state.get_network("net-1").unwrap().nic_count, 2);

        // Delete NIC
        let delete_cmd = Command::DeleteNic {
            request_id: "req-4".to_string(),
            id: "nic-1".to_string(),
        };
        apply(&mut state, delete_cmd);
        assert_eq!(state.get_network("net-1").unwrap().nic_count, 1);

        // Delete second NIC
        let delete_cmd2 = Command::DeleteNic {
            request_id: "req-5".to_string(),
            id: "nic-2".to_string(),
        };
        apply(&mut state, delete_cmd2);
        assert_eq!(state.get_network("net-1").unwrap().nic_count, 0);
    }

    #[test]
    fn test_get_network_by_name() {
        let mut state = ApiState::default();

        apply(
            &mut state,
            create_network_cmd("req-1", "net-uuid-123", "my-network"),
        );

        // Find by name
        let network = state.get_network_by_name("my-network");
        assert!(network.is_some());
        assert_eq!(network.unwrap().id, "net-uuid-123");

        // Non-existent name
        assert!(state.get_network_by_name("unknown").is_none());
    }

    #[test]
    fn test_get_nic_by_name() {
        let mut state = ApiState::default();

        apply(&mut state, create_network_cmd("req-1", "net-1", "test-net"));
        apply(
            &mut state,
            create_nic_cmd("req-2", "nic-uuid-456", "net-1", Some("my-nic")),
        );

        // Find by name
        let nic = state.get_nic_by_name("my-nic");
        assert!(nic.is_some());
        assert_eq!(nic.unwrap().id, "nic-uuid-456");

        // Non-existent name
        assert!(state.get_nic_by_name("unknown").is_none());
    }

    #[test]
    fn test_create_nic_network_not_found() {
        let mut state = ApiState::default();

        // Try to create NIC in non-existent network
        let cmd = create_nic_cmd("req-1", "nic-1", "non-existent-network", None);
        let response = apply(&mut state, cmd);

        match response {
            Response::Error { code, message } => {
                assert_eq!(code, 404);
                assert!(message.contains("non-existent-network"));
            }
            other => panic!("Expected error, got: {:?}", other),
        }
    }

    #[test]
    fn test_create_nic_auto_generates_mac() {
        let mut state = ApiState::default();

        apply(&mut state, create_network_cmd("req-1", "net-1", "test-net"));
        let response = apply(&mut state, create_nic_cmd("req-2", "nic-1", "net-1", None));

        match response {
            Response::Nic(data) => {
                // MAC should start with QEMU prefix
                assert!(data.spec.mac_address.starts_with("52:54:00:"));
                // Should have full MAC format
                assert_eq!(data.spec.mac_address.matches(':').count(), 5);
            }
            other => panic!("Unexpected response: {:?}", other),
        }
    }

    #[test]
    fn test_update_network() {
        let mut state = ApiState::default();

        // Create network
        apply(&mut state, create_network_cmd("req-1", "net-1", "test-net"));

        // Update DNS and NTP servers
        let update_cmd = Command::UpdateNetwork {
            request_id: "req-2".to_string(),
            id: "net-1".to_string(),
            timestamp: "2024-01-01T00:00:01Z".to_string(),
            dns_servers: vec!["1.1.1.1".to_string(), "8.8.4.4".to_string()],
            ntp_servers: vec!["pool.ntp.org".to_string()],
        };
        let response = apply(&mut state, update_cmd);

        match response {
            Response::Network(data) => {
                assert_eq!(data.dns_servers, vec!["1.1.1.1", "8.8.4.4"]);
                assert_eq!(data.ntp_servers, vec!["pool.ntp.org"]);
            }
            other => panic!("Unexpected response: {:?}", other),
        }
    }

    #[test]
    fn test_update_network_not_found() {
        let mut state = ApiState::default();

        let update_cmd = Command::UpdateNetwork {
            request_id: "req-1".to_string(),
            id: "non-existent".to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            dns_servers: vec![],
            ntp_servers: vec![],
        };
        let response = apply(&mut state, update_cmd);

        assert!(matches!(response, Response::Error { code: 404, .. }));
    }

    #[test]
    fn test_update_nic() {
        let mut state = ApiState::default();

        apply(&mut state, create_network_cmd("req-1", "net-1", "test-net"));
        apply(&mut state, create_nic_cmd("req-2", "nic-1", "net-1", None));

        let update_cmd = Command::UpdateNic {
            request_id: "req-3".to_string(),
            id: "nic-1".to_string(),
            timestamp: "2024-01-01T00:00:01Z".to_string(),
            routed_ipv4_prefixes: vec!["192.168.1.0/24".to_string()],
            routed_ipv6_prefixes: vec!["fd00::/64".to_string()],
        };
        let response = apply(&mut state, update_cmd);

        match response {
            Response::Nic(data) => {
                assert_eq!(data.spec.routed_ipv4_prefixes, vec!["192.168.1.0/24"]);
                assert_eq!(data.spec.routed_ipv6_prefixes, vec!["fd00::/64"]);
            }
            other => panic!("Unexpected response: {:?}", other),
        }
    }

    #[test]
    fn test_list_nics_filter_by_network() {
        let mut state = ApiState::default();

        // Create two networks
        apply(
            &mut state,
            create_network_cmd("req-1", "net-1", "network-1"),
        );
        apply(
            &mut state,
            create_network_cmd("req-2", "net-2", "network-2"),
        );

        // Create NICs in different networks
        apply(&mut state, create_nic_cmd("req-3", "nic-1", "net-1", None));
        apply(&mut state, create_nic_cmd("req-4", "nic-2", "net-1", None));
        apply(&mut state, create_nic_cmd("req-5", "nic-3", "net-2", None));

        // List all NICs
        let all_nics = state.list_nics(None);
        assert_eq!(all_nics.len(), 3);

        // Filter by network-1
        let net1_nics = state.list_nics(Some("net-1"));
        assert_eq!(net1_nics.len(), 2);
        assert!(net1_nics.iter().all(|n| n.spec.network_id == "net-1"));

        // Filter by network-2
        let net2_nics = state.list_nics(Some("net-2"));
        assert_eq!(net2_nics.len(), 1);
        assert_eq!(net2_nics[0].id, "nic-3");
    }

    #[test]
    fn test_delete_network_not_found() {
        let mut state = ApiState::default();

        let delete_cmd = Command::DeleteNetwork {
            request_id: "req-1".to_string(),
            id: "non-existent".to_string(),
            force: false,
        };
        let response = apply(&mut state, delete_cmd);

        assert!(matches!(response, Response::Error { code: 404, .. }));
    }

    #[test]
    fn test_delete_nic_not_found() {
        let mut state = ApiState::default();

        let delete_cmd = Command::DeleteNic {
            request_id: "req-1".to_string(),
            id: "non-existent".to_string(),
        };
        let response = apply(&mut state, delete_cmd);

        assert!(matches!(response, Response::Error { code: 404, .. }));
    }

    // =========================================================================
    // Org / Project Tests
    // =========================================================================

    /// Default Org id used by test helpers that don't care which Org a Project lives in.
    const TEST_ORG_ID: &str = "org-test";
    const TEST_ORG_SLUG: &str = "org-test";

    fn create_org_cmd(request_id: &str, id: &str, slug: &str) -> Command {
        Command::CreateOrg {
            request_id: request_id.to_string(),
            id: id.to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            slug: slug.to_string(),
            name: format!("Org {}", slug),
            default_static_key_ttl_days: 90,
            disallow_static_keys: false,
        }
    }

    /// Ensure the shared TEST_ORG_ID exists; idempotent.
    fn ensure_test_org(state: &mut ApiState) {
        if state.get_org(TEST_ORG_ID).is_none() {
            apply(
                state,
                create_org_cmd("test-org-req", TEST_ORG_ID, TEST_ORG_SLUG),
            );
        }
    }

    /// Create a Project in TEST_ORG. `id` is also used as the slug for convenience —
    /// existing tests pass project ids like "proj-1" which are already valid slugs.
    fn create_project_cmd(request_id: &str, id: &str, name: &str) -> Command {
        Command::CreateProject {
            request_id: request_id.to_string(),
            id: id.to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            org_id: TEST_ORG_ID.to_string(),
            slug: id.to_string(),
            name: name.to_string(),
            description: Some("Test project".to_string()),
        }
    }

    #[test]
    fn test_create_org() {
        let mut state = ApiState::default();
        let response = apply(&mut state, create_org_cmd("req-1", "org-1", "acme"));
        match response {
            Response::Org(data) => {
                assert_eq!(data.id, "org-1");
                assert_eq!(data.slug, "acme");
                assert_eq!(data.default_static_key_ttl_days, 90);
            }
            other => panic!("unexpected: {:?}", other),
        }
        assert_eq!(state.list_orgs().len(), 1);
        assert!(state.get_org_by_slug("acme").is_some());
    }

    #[test]
    fn test_create_org_duplicate_slug() {
        let mut state = ApiState::default();
        apply(&mut state, create_org_cmd("req-1", "org-1", "acme"));
        let response = apply(&mut state, create_org_cmd("req-2", "org-2", "acme"));
        assert!(matches!(response, Response::Error { code: 409, .. }));
    }

    #[test]
    fn test_delete_org_with_projects_rejects() {
        let mut state = ApiState::default();
        ensure_test_org(&mut state);
        apply(&mut state, create_project_cmd("req-1", "proj-1", "p"));
        let response = apply(
            &mut state,
            Command::DeleteOrg {
                request_id: "req-2".to_string(),
                id: TEST_ORG_ID.to_string(),
            },
        );
        assert!(matches!(response, Response::Error { code: 409, .. }));
    }

    #[test]
    fn test_create_project() {
        let mut state = ApiState::default();
        ensure_test_org(&mut state);
        let cmd = create_project_cmd("req-1", "proj-1", "my-project");
        let response = apply(&mut state, cmd);

        match response {
            Response::Project(data) => {
                assert_eq!(data.id, "proj-1");
                assert_eq!(data.slug, "proj-1");
                assert_eq!(data.org_id, TEST_ORG_ID);
                assert_eq!(data.name, "my-project");
                assert_eq!(data.description, Some("Test project".to_string()));
            }
            other => panic!("Unexpected response: {:?}", other),
        }

        assert!(state.get_project("proj-1").is_some());
        assert_eq!(state.list_projects().len(), 1);
    }

    #[test]
    fn test_create_project_org_not_found() {
        let mut state = ApiState::default();
        let cmd = Command::CreateProject {
            request_id: "req-1".to_string(),
            id: "proj-1".to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            org_id: "no-such-org".to_string(),
            slug: "proj-1".to_string(),
            name: "x".to_string(),
            description: None,
        };
        let response = apply(&mut state, cmd);
        assert!(matches!(response, Response::Error { code: 404, .. }));
    }

    #[test]
    fn test_create_project_duplicate_slug() {
        let mut state = ApiState::default();
        ensure_test_org(&mut state);

        apply(
            &mut state,
            create_project_cmd("req-1", "proj-1", "dup-name"),
        );

        // Different id, same slug (id is also used as slug in the helper).
        let cmd = Command::CreateProject {
            request_id: "req-2".to_string(),
            id: "proj-2".to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            org_id: TEST_ORG_ID.to_string(),
            slug: "proj-1".to_string(),
            name: "second".to_string(),
            description: None,
        };
        let response = apply(&mut state, cmd);
        assert!(matches!(response, Response::Error { code: 409, .. }));
    }

    #[test]
    fn test_delete_project() {
        let mut state = ApiState::default();
        ensure_test_org(&mut state);
        apply(
            &mut state,
            create_project_cmd("req-1", "proj-1", "to-delete"),
        );

        let delete_cmd = Command::DeleteProject {
            request_id: "req-2".to_string(),
            id: "proj-1".to_string(),
        };
        let response = apply(&mut state, delete_cmd);

        assert!(matches!(response, Response::Deleted { .. }));
        assert!(state.get_project("proj-1").is_none());
    }

    #[test]
    fn test_delete_project_not_found() {
        let mut state = ApiState::default();

        let delete_cmd = Command::DeleteProject {
            request_id: "req-1".to_string(),
            id: "non-existent".to_string(),
        };
        let response = apply(&mut state, delete_cmd);

        assert!(matches!(response, Response::Error { code: 404, .. }));
    }

    #[test]
    fn test_get_project_by_slug() {
        let mut state = ApiState::default();
        ensure_test_org(&mut state);
        apply(
            &mut state,
            create_project_cmd("req-1", "proj-uuid", "findme"),
        );

        let project = state.get_project_by_slug("proj-uuid");
        assert!(project.is_some());
        assert_eq!(project.unwrap().id, "proj-uuid");

        assert!(state.get_project_by_slug("unknown").is_none());
    }

    // =========================================================================
    // Volume Tests
    // =========================================================================

    fn create_volume_cmd(
        request_id: &str,
        id: &str,
        project_id: &str,
        node_id: &str,
        name: &str,
        size: u64,
    ) -> Command {
        Command::CreateVolume {
            request_id: request_id.to_string(),
            id: id.to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            project_id: project_id.to_string(),
            node_id: node_id.to_string(),
            name: name.to_string(),
            size_bytes: size,
            template_id: None,
        }
    }

    #[test]
    fn test_create_volume() {
        let mut state = ApiState::default();

        // Create project first
        apply(
            &mut state,
            create_project_cmd("req-1", "proj-1", "test-proj"),
        );

        // Create volume
        let cmd = create_volume_cmd(
            "req-2",
            "vol-1",
            "proj-1",
            "node-1",
            "my-volume",
            10_000_000,
        );
        let response = apply(&mut state, cmd);

        match response {
            Response::Volume(data) => {
                assert_eq!(data.id, "vol-1");
                assert_eq!(data.spec.project_id, "proj-1");
                assert_eq!(data.spec.node_id, "node-1");
                assert_eq!(data.spec.name, "my-volume");
                assert_eq!(data.spec.size_bytes, 10_000_000);
                assert_eq!(data.status.phase, VolumePhase::Pending);
                assert!(data.status.path.is_empty());
            }
            other => panic!("Unexpected response: {:?}", other),
        }

        assert!(state.get_volume("vol-1").is_some());
    }

    #[test]
    fn test_create_volume_project_not_found() {
        let mut state = ApiState::default();

        let cmd = create_volume_cmd("req-1", "vol-1", "non-existent", "node-1", "volume", 1000);
        let response = apply(&mut state, cmd);

        assert!(matches!(response, Response::Error { code: 404, .. }));
    }

    #[test]
    fn test_create_volume_duplicate_name_in_project() {
        let mut state = ApiState::default();
        apply(
            &mut state,
            create_project_cmd("req-1", "proj-1", "test-proj"),
        );
        apply(
            &mut state,
            create_volume_cmd("req-2", "vol-1", "proj-1", "node-1", "dup-name", 1000),
        );

        let response = apply(
            &mut state,
            create_volume_cmd("req-3", "vol-2", "proj-1", "node-1", "dup-name", 1000),
        );
        assert!(matches!(response, Response::Error { code: 409, .. }));
    }

    #[test]
    fn test_resize_volume() {
        let mut state = ApiState::default();
        apply(
            &mut state,
            create_project_cmd("req-1", "proj-1", "test-proj"),
        );
        apply(
            &mut state,
            create_volume_cmd("req-2", "vol-1", "proj-1", "node-1", "volume", 1000),
        );

        // Resize larger
        let resize_cmd = Command::ResizeVolume {
            request_id: "req-3".to_string(),
            id: "vol-1".to_string(),
            timestamp: "2024-01-01T00:00:01Z".to_string(),
            size_bytes: 2000,
        };
        let response = apply(&mut state, resize_cmd);

        match response {
            Response::Volume(data) => {
                assert_eq!(data.spec.size_bytes, 2000);
            }
            other => panic!("Unexpected response: {:?}", other),
        }
    }

    #[test]
    fn test_resize_volume_cannot_shrink() {
        let mut state = ApiState::default();
        apply(
            &mut state,
            create_project_cmd("req-1", "proj-1", "test-proj"),
        );
        apply(
            &mut state,
            create_volume_cmd("req-2", "vol-1", "proj-1", "node-1", "volume", 2000),
        );

        // Try to shrink
        let resize_cmd = Command::ResizeVolume {
            request_id: "req-3".to_string(),
            id: "vol-1".to_string(),
            timestamp: "2024-01-01T00:00:01Z".to_string(),
            size_bytes: 1000,
        };
        let response = apply(&mut state, resize_cmd);

        assert!(matches!(response, Response::Error { code: 400, .. }));
    }

    #[test]
    fn test_create_snapshot() {
        let mut state = ApiState::default();
        apply(
            &mut state,
            create_project_cmd("req-1", "proj-1", "test-proj"),
        );
        apply(
            &mut state,
            create_volume_cmd("req-2", "vol-1", "proj-1", "node-1", "volume", 1000),
        );

        let snap_cmd = Command::CreateSnapshot {
            request_id: "req-3".to_string(),
            id: "snap-1".to_string(),
            timestamp: "2024-01-01T00:00:01Z".to_string(),
            volume_id: "vol-1".to_string(),
            name: "my-snapshot".to_string(),
        };
        let response = apply(&mut state, snap_cmd);

        match response {
            Response::Volume(data) => {
                assert_eq!(data.status.snapshots.len(), 1);
                assert_eq!(data.status.snapshots[0].name, "my-snapshot");
            }
            other => panic!("Unexpected response: {:?}", other),
        }
    }

    #[test]
    fn test_create_snapshot_duplicate_name() {
        let mut state = ApiState::default();
        apply(
            &mut state,
            create_project_cmd("req-1", "proj-1", "test-proj"),
        );
        apply(
            &mut state,
            create_volume_cmd("req-2", "vol-1", "proj-1", "node-1", "volume", 1000),
        );

        // Create first snapshot
        apply(
            &mut state,
            Command::CreateSnapshot {
                request_id: "req-3".to_string(),
                id: "snap-1".to_string(),
                timestamp: "2024-01-01T00:00:01Z".to_string(),
                volume_id: "vol-1".to_string(),
                name: "dup-snap".to_string(),
            },
        );

        // Try to create duplicate
        let response = apply(
            &mut state,
            Command::CreateSnapshot {
                request_id: "req-4".to_string(),
                id: "snap-2".to_string(),
                timestamp: "2024-01-01T00:00:02Z".to_string(),
                volume_id: "vol-1".to_string(),
                name: "dup-snap".to_string(),
            },
        );

        assert!(matches!(response, Response::Error { code: 409, .. }));
    }

    #[test]
    fn test_delete_volume() {
        let mut state = ApiState::default();
        apply(
            &mut state,
            create_project_cmd("req-1", "proj-1", "test-proj"),
        );
        apply(
            &mut state,
            create_volume_cmd("req-2", "vol-1", "proj-1", "node-1", "volume", 1000),
        );

        let delete_cmd = Command::DeleteVolume {
            request_id: "req-3".to_string(),
            id: "vol-1".to_string(),
        };
        let response = apply(&mut state, delete_cmd);

        assert!(matches!(response, Response::Deleted { .. }));
        assert!(state.get_volume("vol-1").is_none());
    }

    #[test]
    fn test_list_volumes_filter() {
        let mut state = ApiState::default();
        apply(&mut state, create_project_cmd("req-1", "proj-1", "proj-1"));
        apply(&mut state, create_project_cmd("req-2", "proj-2", "proj-2"));

        apply(
            &mut state,
            create_volume_cmd("req-3", "vol-1", "proj-1", "node-1", "vol-1", 1000),
        );
        apply(
            &mut state,
            create_volume_cmd("req-4", "vol-2", "proj-1", "node-2", "vol-2", 1000),
        );
        apply(
            &mut state,
            create_volume_cmd("req-5", "vol-3", "proj-2", "node-1", "vol-3", 1000),
        );

        // All volumes
        assert_eq!(state.list_volumes(None, None).len(), 3);

        // Filter by project
        assert_eq!(state.list_volumes(Some("proj-1"), None).len(), 2);

        // Filter by node
        assert_eq!(state.list_volumes(None, Some("node-1")).len(), 2);

        // Filter by both
        assert_eq!(state.list_volumes(Some("proj-1"), Some("node-1")).len(), 1);
    }

    // =========================================================================
    // Template Tests
    // =========================================================================

    fn create_template_cmd(request_id: &str, id: &str, node_id: &str, name: &str) -> Command {
        Command::CreateTemplate {
            request_id: request_id.to_string(),
            id: id.to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            project_id: "test-project".to_string(),
            node_id: node_id.to_string(),
            name: name.to_string(),
            size_bytes: 1_000_000_000,
            source_url: None,
            total_bytes: 0,
        }
    }

    #[test]
    fn test_create_template() {
        let mut state = ApiState::default();

        let cmd = create_template_cmd("req-1", "tmpl-1", "node-1", "ubuntu-22.04");
        let response = apply(&mut state, cmd);

        match response {
            Response::Template(data) => {
                assert_eq!(data.id, "tmpl-1");
                assert_eq!(data.spec.node_id, "node-1");
                assert_eq!(data.spec.name, "ubuntu-22.04");
                assert_eq!(data.status.clone_count, 0);
            }
            other => panic!("Unexpected response: {:?}", other),
        }

        assert!(state.get_template("tmpl-1").is_some());
    }

    #[test]
    fn test_create_template_duplicate_name() {
        let mut state = ApiState::default();

        apply(
            &mut state,
            create_template_cmd("req-1", "tmpl-1", "node-1", "dup-name"),
        );

        let response = apply(
            &mut state,
            create_template_cmd("req-2", "tmpl-2", "node-2", "dup-name"),
        );
        assert!(matches!(response, Response::Error { code: 409, .. }));
    }

    #[test]
    fn test_volume_from_template_increments_clone_count() {
        let mut state = ApiState::default();
        apply(
            &mut state,
            create_project_cmd("req-1", "proj-1", "test-proj"),
        );
        apply(
            &mut state,
            create_template_cmd("req-2", "tmpl-1", "node-1", "ubuntu"),
        );

        // Clone from template
        let vol_cmd = Command::CreateVolume {
            request_id: "req-3".to_string(),
            id: "vol-1".to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            project_id: "proj-1".to_string(),
            node_id: "node-1".to_string(),
            name: "cloned-vol".to_string(),
            size_bytes: 10_000_000,
            template_id: Some("tmpl-1".to_string()),
        };
        apply(&mut state, vol_cmd);

        // Check clone count incremented
        let template = state.get_template("tmpl-1").unwrap();
        assert_eq!(template.status.clone_count, 1);
    }

    #[test]
    fn test_delete_volume_decrements_clone_count() {
        let mut state = ApiState::default();
        apply(
            &mut state,
            create_project_cmd("req-1", "proj-1", "test-proj"),
        );
        apply(
            &mut state,
            create_template_cmd("req-2", "tmpl-1", "node-1", "ubuntu"),
        );

        // Clone from template
        apply(
            &mut state,
            Command::CreateVolume {
                request_id: "req-3".to_string(),
                id: "vol-1".to_string(),
                timestamp: "2024-01-01T00:00:00Z".to_string(),
                project_id: "proj-1".to_string(),
                node_id: "node-1".to_string(),
                name: "cloned-vol".to_string(),
                size_bytes: 10_000_000,
                template_id: Some("tmpl-1".to_string()),
            },
        );

        assert_eq!(state.get_template("tmpl-1").unwrap().status.clone_count, 1);

        // Delete volume
        apply(
            &mut state,
            Command::DeleteVolume {
                request_id: "req-4".to_string(),
                id: "vol-1".to_string(),
            },
        );

        assert_eq!(state.get_template("tmpl-1").unwrap().status.clone_count, 0);
    }

    #[test]
    fn test_volume_from_template_wrong_node() {
        let mut state = ApiState::default();
        apply(
            &mut state,
            create_project_cmd("req-1", "proj-1", "test-proj"),
        );
        apply(
            &mut state,
            create_template_cmd("req-2", "tmpl-1", "node-1", "ubuntu"),
        );

        // Try to clone on different node
        let vol_cmd = Command::CreateVolume {
            request_id: "req-3".to_string(),
            id: "vol-1".to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            project_id: "proj-1".to_string(),
            node_id: "node-2".to_string(), // Different node!
            name: "cloned-vol".to_string(),
            size_bytes: 10_000_000,
            template_id: Some("tmpl-1".to_string()),
        };
        let response = apply(&mut state, vol_cmd);

        assert!(matches!(response, Response::Error { code: 409, .. }));
    }

    // =========================================================================
    // Template Import Tests
    // =========================================================================

    #[test]
    fn test_create_template_with_source_url() {
        let mut state = ApiState::default();

        let cmd = Command::CreateTemplate {
            request_id: "req-1".to_string(),
            id: "tmpl-1".to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            project_id: "test-project".to_string(),
            node_id: "node-1".to_string(),
            name: "ubuntu-22.04".to_string(),
            size_bytes: 0,
            source_url: Some("https://example.com/ubuntu.qcow2".to_string()),
            total_bytes: 2_000_000_000,
        };
        let response = apply(&mut state, cmd);

        match response {
            Response::Template(data) => {
                assert_eq!(data.id, "tmpl-1");
                assert_eq!(data.status.phase, TemplatePhase::Pending);
                assert_eq!(data.status.bytes_written, 0);
                assert_eq!(data.status.total_bytes, 2_000_000_000);
                assert_eq!(
                    data.spec.source_url,
                    Some("https://example.com/ubuntu.qcow2".to_string())
                );
            }
            other => panic!("Unexpected response: {:?}", other),
        }

        assert!(state.get_template("tmpl-1").is_some());
    }

    #[test]
    fn test_update_template_status_importing() {
        let mut state = ApiState::default();

        apply(
            &mut state,
            Command::CreateTemplate {
                request_id: "req-1".to_string(),
                id: "tmpl-1".to_string(),
                timestamp: "2024-01-01T00:00:00Z".to_string(),
                project_id: "test-project".to_string(),
                node_id: "node-1".to_string(),
                name: "ubuntu".to_string(),
                size_bytes: 0,
                source_url: Some("https://example.com/ubuntu.qcow2".to_string()),
                total_bytes: 1000,
            },
        );

        let update_cmd = Command::UpdateTemplateStatus {
            request_id: "req-2".to_string(),
            id: "tmpl-1".to_string(),
            timestamp: "2024-01-01T00:00:01Z".to_string(),
            phase: TemplatePhase::Importing,
            bytes_written: 500,
            size_bytes: 1000,
            error: None,
        };
        let response = apply(&mut state, update_cmd);

        match response {
            Response::Template(data) => {
                assert_eq!(data.status.phase, TemplatePhase::Importing);
                assert_eq!(data.status.bytes_written, 500);
            }
            other => panic!("Unexpected response: {:?}", other),
        }
    }

    #[test]
    fn test_update_template_status_ready() {
        let mut state = ApiState::default();

        apply(
            &mut state,
            Command::CreateTemplate {
                request_id: "req-1".to_string(),
                id: "tmpl-1".to_string(),
                timestamp: "2024-01-01T00:00:00Z".to_string(),
                project_id: "test-project".to_string(),
                node_id: "node-1".to_string(),
                name: "ubuntu".to_string(),
                size_bytes: 0,
                source_url: Some("https://example.com/ubuntu.qcow2".to_string()),
                total_bytes: 1000,
            },
        );

        let update_cmd = Command::UpdateTemplateStatus {
            request_id: "req-2".to_string(),
            id: "tmpl-1".to_string(),
            timestamp: "2024-01-01T00:00:01Z".to_string(),
            phase: TemplatePhase::Ready,
            bytes_written: 1000,
            size_bytes: 1000,
            error: None,
        };
        let response = apply(&mut state, update_cmd);

        match response {
            Response::Template(data) => {
                assert_eq!(data.status.phase, TemplatePhase::Ready);
                assert_eq!(data.status.bytes_written, 1000);
            }
            other => panic!("Unexpected response: {:?}", other),
        }
    }

    #[test]
    fn test_update_template_status_failed() {
        let mut state = ApiState::default();

        apply(
            &mut state,
            Command::CreateTemplate {
                request_id: "req-1".to_string(),
                id: "tmpl-1".to_string(),
                timestamp: "2024-01-01T00:00:00Z".to_string(),
                project_id: "test-project".to_string(),
                node_id: "node-1".to_string(),
                name: "ubuntu".to_string(),
                size_bytes: 0,
                source_url: Some("https://example.com/ubuntu.qcow2".to_string()),
                total_bytes: 1000,
            },
        );

        let update_cmd = Command::UpdateTemplateStatus {
            request_id: "req-2".to_string(),
            id: "tmpl-1".to_string(),
            timestamp: "2024-01-01T00:00:01Z".to_string(),
            phase: TemplatePhase::Failed,
            bytes_written: 100,
            size_bytes: 1000,
            error: Some("Download failed: connection reset".to_string()),
        };
        let response = apply(&mut state, update_cmd);

        match response {
            Response::Template(data) => {
                assert_eq!(data.status.phase, TemplatePhase::Failed);
                assert_eq!(
                    data.status.error,
                    Some("Download failed: connection reset".to_string())
                );
            }
            other => panic!("Unexpected response: {:?}", other),
        }
    }

    #[test]
    fn test_update_template_status_not_found() {
        let mut state = ApiState::default();

        let update_cmd = Command::UpdateTemplateStatus {
            request_id: "req-1".to_string(),
            id: "non-existent".to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            phase: TemplatePhase::Importing,
            bytes_written: 0,
            size_bytes: 0,
            error: None,
        };
        let response = apply(&mut state, update_cmd);

        assert!(matches!(response, Response::Error { code: 404, .. }));
    }
}
