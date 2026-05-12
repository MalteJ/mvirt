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

use crate::ca::{InternalCa, new_serial, sign_node_leaf};
use crate::command::{
    AccountData, AccountKind, ApiKeyData, ClusterData, Command, MembershipData, MembershipScope,
    NetworkData, NicData, NicSpec, NicStatus, NodeData, NodeStatus, OnboardingTokenData, OrgData,
    ProjectData, Response, RevocationReason, RevokedCertData, Role, SecurityGroupData,
    SecurityGroupRuleData, ServerCertData, SnapshotData, TemplateData, TemplatePhase, TemplateSpec,
    TemplateStatus, VmData, VmPhase, VmStatus, VolumeData, VolumeSpec, VolumeStatus,
};
#[cfg(test)]
use crate::command::{OrgContact, VolumePhase};
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
const CLUSTERS: TableDefinition<&str, &[u8]> = TableDefinition::new("clusters");
const VOLUMES: TableDefinition<&str, &[u8]> = TableDefinition::new("volumes");
const TEMPLATES: TableDefinition<&str, &[u8]> = TableDefinition::new("templates");
const SECURITY_GROUPS: TableDefinition<&str, &[u8]> = TableDefinition::new("security_groups");
// Node-onboarding state (ADR-0006).
const ONBOARDING_TOKENS: TableDefinition<&str, &[u8]> = TableDefinition::new("onboarding_tokens");
const REVOKED_CERTS: TableDefinition<&str, &[u8]> = TableDefinition::new("revoked_certs");
const PKI: TableDefinition<&str, &[u8]> = TableDefinition::new("pki");
const ACCOUNTS: TableDefinition<&str, &[u8]> = TableDefinition::new("accounts");
const MEMBERSHIPS: TableDefinition<&str, &[u8]> = TableDefinition::new("memberships");
/// Static API keys for ServiceAccounts (ADR-0004). Keyed by key id.
const API_KEYS: TableDefinition<&str, &[u8]> = TableDefinition::new("api_keys");

/// Singleton key inside PKI for the CA material.
const PKI_KEY_CA: &str = "internal_ca";
/// Singleton key inside PKI for the cplane tunnel server cert.
const PKI_KEY_SERVER: &str = "server_cert";

const ALL_TABLES: &[TableDefinition<&str, &[u8]>] = &[
    NODES,
    NETWORKS,
    NICS,
    VMS,
    ORGS,
    PROJECTS,
    CLUSTERS,
    VOLUMES,
    TEMPLATES,
    SECURITY_GROUPS,
    ONBOARDING_TOKENS,
    REVOKED_CERTS,
    ACCOUNTS,
    MEMBERSHIPS,
    API_KEYS,
    PKI,
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

    pub fn list_networks_by_project(&self, project_slug: &str) -> Vec<NetworkData> {
        read_list::<NetworkData>(&self.read_txn(), NETWORKS)
            .into_iter()
            .filter(|n| n.project_slug == project_slug)
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

    pub fn list_nics_by_project(&self, project_slug: &str) -> Vec<NicData> {
        read_list::<NicData>(&self.read_txn(), NICS)
            .into_iter()
            .filter(|n| n.spec.project_slug == project_slug)
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

    pub fn list_vms_by_project(&self, project_slug: &str) -> Vec<VmData> {
        read_list::<VmData>(&self.read_txn(), VMS)
            .into_iter()
            .filter(|v| v.spec.project_slug == project_slug)
            .collect()
    }

    pub fn vm_count(&self) -> usize {
        read_count(&self.read_txn(), VMS)
    }

    pub fn vm_ids(&self) -> Vec<String> {
        read_keys(&self.read_txn(), VMS)
    }

    // =========================================================================
    // Org queries — Org is keyed by slug (no separate UUID).
    // =========================================================================

    pub fn get_org(&self, slug: &str) -> Option<OrgData> {
        read_get(&self.read_txn(), ORGS, slug)
    }

    pub fn list_orgs(&self) -> Vec<OrgData> {
        read_list(&self.read_txn(), ORGS)
    }

    pub fn org_count(&self) -> usize {
        read_count(&self.read_txn(), ORGS)
    }

    pub fn org_slugs(&self) -> Vec<String> {
        read_keys(&self.read_txn(), ORGS)
    }

    // =========================================================================
    // Project queries — Project is keyed by slug.
    // =========================================================================

    pub fn get_project(&self, slug: &str) -> Option<ProjectData> {
        read_get(&self.read_txn(), PROJECTS, slug)
    }

    pub fn list_projects(&self) -> Vec<ProjectData> {
        read_list(&self.read_txn(), PROJECTS)
    }

    pub fn list_projects_by_org(&self, org_slug: &str) -> Vec<ProjectData> {
        read_list::<ProjectData>(&self.read_txn(), PROJECTS)
            .into_iter()
            .filter(|p| p.org_slug == org_slug)
            .collect()
    }

    // =========================================================================
    // Cluster queries — Cluster is keyed by slug. See ADR-0005.
    // =========================================================================

    pub fn get_cluster(&self, slug: &str) -> Option<ClusterData> {
        read_get(&self.read_txn(), CLUSTERS, slug)
    }

    pub fn list_clusters(&self) -> Vec<ClusterData> {
        read_list(&self.read_txn(), CLUSTERS)
    }

    pub fn list_clusters_by_org(&self, org_slug: &str) -> Vec<ClusterData> {
        read_list::<ClusterData>(&self.read_txn(), CLUSTERS)
            .into_iter()
            .filter(|c| c.org_slug == org_slug)
            .collect()
    }

    /// All Clusters that include this `node_id` in their `node_ids` list.
    /// A Node may legitimately be in more than one Cluster (ADR-0005).
    pub fn list_clusters_with_node(&self, node_id: &str) -> Vec<ClusterData> {
        read_list::<ClusterData>(&self.read_txn(), CLUSTERS)
            .into_iter()
            .filter(|c| c.node_ids.iter().any(|n| n == node_id))
            .collect()
    }

    // =========================================================================
    // Node-onboarding queries (ADR-0006).
    // =========================================================================

    pub fn get_internal_ca(&self) -> Option<InternalCa> {
        read_get(&self.read_txn(), PKI, PKI_KEY_CA)
    }

    pub fn get_server_cert(&self) -> Option<ServerCertData> {
        read_get(&self.read_txn(), PKI, PKI_KEY_SERVER)
    }

    pub fn list_onboarding_tokens(&self) -> Vec<OnboardingTokenData> {
        read_list(&self.read_txn(), ONBOARDING_TOKENS)
    }

    pub fn list_onboarding_tokens_by_cluster(
        &self,
        cluster_slug: &str,
    ) -> Vec<OnboardingTokenData> {
        read_list::<OnboardingTokenData>(&self.read_txn(), ONBOARDING_TOKENS)
            .into_iter()
            .filter(|t| t.cluster_slug == cluster_slug)
            .collect()
    }

    pub fn get_onboarding_token(&self, id: &str) -> Option<OnboardingTokenData> {
        read_get(&self.read_txn(), ONBOARDING_TOKENS, id)
    }

    pub fn list_revoked_certs(&self) -> Vec<RevokedCertData> {
        read_list(&self.read_txn(), REVOKED_CERTS)
    }

    pub fn is_cert_revoked(&self, serial_hex: &str) -> bool {
        self.read_txn()
            .open_table(REVOKED_CERTS)
            .map(|t| t.get(serial_hex).map(|g| g.is_some()).unwrap_or(false))
            .unwrap_or(false)
    }

    // =========================================================================
    // Account + Membership queries (ADR-0004).
    // =========================================================================

    pub fn get_account(&self, id: &str) -> Option<AccountData> {
        read_get(&self.read_txn(), ACCOUNTS, id)
    }

    /// Look up the User Account whose `(iss, sub)` matches. Linear scan over
    /// ACCOUNTS — fine at expected sizes; if it ever gets hot we add a
    /// secondary index.
    pub fn get_account_by_oidc(&self, iss: &str, sub: &str) -> Option<AccountData> {
        read_list::<AccountData>(&self.read_txn(), ACCOUNTS)
            .into_iter()
            .find(|a| {
                a.external_iss.as_deref() == Some(iss) && a.external_sub.as_deref() == Some(sub)
            })
    }

    pub fn list_accounts(&self) -> Vec<AccountData> {
        read_list(&self.read_txn(), ACCOUNTS)
    }

    pub fn get_membership(&self, id: &str) -> Option<MembershipData> {
        read_get(&self.read_txn(), MEMBERSHIPS, id)
    }

    pub fn list_memberships(&self) -> Vec<MembershipData> {
        read_list(&self.read_txn(), MEMBERSHIPS)
    }

    pub fn list_memberships_for_account(&self, account_id: &str) -> Vec<MembershipData> {
        read_list::<MembershipData>(&self.read_txn(), MEMBERSHIPS)
            .into_iter()
            .filter(|m| m.account_id == account_id)
            .collect()
    }

    pub fn list_memberships_at_scope(&self, scope: &MembershipScope) -> Vec<MembershipData> {
        read_list::<MembershipData>(&self.read_txn(), MEMBERSHIPS)
            .into_iter()
            .filter(|m| &m.scope == scope)
            .collect()
    }

    /// True if any platform-admin membership exists. Used by the initial
    /// admin bootstrap to detect first-login state without races.
    pub fn has_platform_admin(&self) -> bool {
        read_list::<MembershipData>(&self.read_txn(), MEMBERSHIPS)
            .into_iter()
            .any(|m| m.scope == MembershipScope::Platform && m.role == Role::PlatformAdmin)
    }

    // =========================================================================
    // ServiceAccount + API key queries (ADR-0004)
    // =========================================================================

    /// All ServiceAccounts whose home project is `project_slug`.
    pub fn list_service_accounts_in_project(&self, project_slug: &str) -> Vec<AccountData> {
        read_list::<AccountData>(&self.read_txn(), ACCOUNTS)
            .into_iter()
            .filter(|a| {
                a.kind == AccountKind::ServiceAccount
                    && a.project_slug.as_deref() == Some(project_slug)
            })
            .collect()
    }

    pub fn get_api_key(&self, id: &str) -> Option<ApiKeyData> {
        read_get(&self.read_txn(), API_KEYS, id)
    }

    pub fn list_api_keys_for_account(&self, account_id: &str) -> Vec<ApiKeyData> {
        read_list::<ApiKeyData>(&self.read_txn(), API_KEYS)
            .into_iter()
            .filter(|k| k.account_id == account_id)
            .collect()
    }

    // =========================================================================
    // Volume queries
    // =========================================================================

    pub fn get_volume(&self, id: &str) -> Option<VolumeData> {
        read_get(&self.read_txn(), VOLUMES, id)
    }

    pub fn get_volume_by_name(&self, project_slug: &str, name: &str) -> Option<VolumeData> {
        read_list::<VolumeData>(&self.read_txn(), VOLUMES)
            .into_iter()
            .find(|v| v.spec.project_slug == project_slug && v.spec.name == name)
    }

    pub fn list_volumes(
        &self,
        project_slug: Option<&str>,
        node_id: Option<&str>,
    ) -> Vec<VolumeData> {
        read_list::<VolumeData>(&self.read_txn(), VOLUMES)
            .into_iter()
            .filter(|v| {
                project_slug.is_none_or(|pid| v.spec.project_slug == pid)
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

    pub fn list_templates_by_project(&self, project_slug: &str) -> Vec<TemplateData> {
        read_list::<TemplateData>(&self.read_txn(), TEMPLATES)
            .into_iter()
            .filter(|t| t.spec.project_slug == project_slug)
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

    pub fn list_security_groups_by_project(&self, project_slug: &str) -> Vec<SecurityGroupData> {
        read_list::<SecurityGroupData>(&self.read_txn(), SECURITY_GROUPS)
            .into_iter()
            .filter(|sg| sg.project_slug == project_slug)
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
                    cluster_slug: None,
                    cert_serial_hex: None,
                    cert_expires_at: None,
                    hostname: None,
                    agent_version: None,
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
                project_slug,
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
                    .any(|n| n.project_slug == project_slug && n.name == name)
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
                    project_slug,
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
                project_slug,
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
                        project_slug,
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
                    .any(|v| v.spec.project_slug == spec.project_slug && v.spec.name == spec.name)
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
            // Org Commands — keyed by slug.
            // =================================================================
            Command::CreateOrg {
                timestamp,
                slug,
                name,
                contact,
                ..
            } => {
                let txn = self.db.begin_write().expect("begin");

                if txn_has(&txn, ORGS, &slug) {
                    return (
                        Response::Error {
                            code: 409,
                            message: format!("Org with slug '{}' already exists", slug),
                        },
                        vec![],
                    );
                }

                let org = OrgData {
                    slug: slug.clone(),
                    name,
                    contact,
                    created_at: timestamp.clone(),
                    updated_at: timestamp,
                };

                txn_put(&txn, ORGS, &slug, &org);
                txn.commit().expect("commit");
                (Response::Org(org), vec![])
            }

            Command::UpdateOrg {
                slug,
                timestamp,
                name,
                contact,
                ..
            } => {
                let txn = self.db.begin_write().expect("begin");
                let Some(mut org) = txn_get::<OrgData>(&txn, ORGS, &slug) else {
                    return (
                        Response::Error {
                            code: 404,
                            message: format!("Org '{}' not found", slug),
                        },
                        vec![],
                    );
                };
                if let Some(n) = name {
                    org.name = n;
                }
                if let Some(c) = contact {
                    // Whole-record replace. The REST handler echoes the
                    // current contact and overwrites only the fields the
                    // user changed in the form, so a partial PATCH on the
                    // wire still ends up as a full record here.
                    org.contact = c;
                }
                org.updated_at = timestamp;
                txn_put(&txn, ORGS, &slug, &org);
                txn.commit().expect("commit");
                (Response::Org(org), vec![])
            }

            Command::DeleteOrg { slug, .. } => {
                let txn = self.db.begin_write().expect("begin");
                if !txn_has(&txn, ORGS, &slug) {
                    return (
                        Response::Error {
                            code: 404,
                            message: format!("Org '{}' not found", slug),
                        },
                        vec![],
                    );
                }
                let project_count = txn_list::<ProjectData>(&txn, PROJECTS)
                    .iter()
                    .filter(|p| p.org_slug == slug)
                    .count();
                if project_count > 0 {
                    return (
                        Response::Error {
                            code: 409,
                            message: format!(
                                "Org '{}' has {} project(s); delete them first",
                                slug, project_count
                            ),
                        },
                        vec![],
                    );
                }
                txn_delete(&txn, ORGS, &slug);
                txn.commit().expect("commit");
                (Response::Deleted { id: slug }, vec![])
            }

            // =================================================================
            // Project Commands — keyed by slug.
            // =================================================================
            Command::CreateProject {
                timestamp,
                org_slug,
                slug,
                name,
                description,
                ..
            } => {
                let txn = self.db.begin_write().expect("begin");

                if txn_has(&txn, PROJECTS, &slug) {
                    return (
                        Response::Error {
                            code: 409,
                            message: format!("Project with slug '{}' already exists", slug),
                        },
                        vec![],
                    );
                }

                if !txn_has(&txn, ORGS, &org_slug) {
                    return (
                        Response::Error {
                            code: 404,
                            message: format!("Org '{}' not found", org_slug),
                        },
                        vec![],
                    );
                }

                let project = ProjectData {
                    slug: slug.clone(),
                    org_slug,
                    name,
                    description,
                    created_at: timestamp.clone(),
                    updated_at: timestamp,
                };

                txn_put(&txn, PROJECTS, &slug, &project);
                txn.commit().expect("commit");
                (Response::Project(project), vec![])
            }

            Command::DeleteProject { slug, .. } => {
                let txn = self.db.begin_write().expect("begin");
                if !txn_has(&txn, PROJECTS, &slug) {
                    return (
                        Response::Error {
                            code: 404,
                            message: format!("Project '{}' not found", slug),
                        },
                        vec![],
                    );
                }
                txn_delete(&txn, PROJECTS, &slug);
                txn.commit().expect("commit");
                (Response::Deleted { id: slug }, vec![])
            }

            // =================================================================
            // Cluster Commands — keyed by slug. See ADR-0005.
            // =================================================================
            Command::CreateCluster {
                timestamp,
                org_slug,
                slug,
                name,
                description,
                location,
                ..
            } => {
                let txn = self.db.begin_write().expect("begin");

                if txn_has(&txn, CLUSTERS, &slug) {
                    return (
                        Response::Error {
                            code: 409,
                            message: format!("Cluster with slug '{}' already exists", slug),
                        },
                        vec![],
                    );
                }

                if !txn_has(&txn, ORGS, &org_slug) {
                    return (
                        Response::Error {
                            code: 404,
                            message: format!("Org '{}' not found", org_slug),
                        },
                        vec![],
                    );
                }

                let cluster = ClusterData {
                    slug: slug.clone(),
                    org_slug,
                    name,
                    description,
                    location,
                    node_ids: Vec::new(),
                    created_at: timestamp.clone(),
                    updated_at: timestamp,
                };

                txn_put(&txn, CLUSTERS, &slug, &cluster);
                txn.commit().expect("commit");
                (Response::Cluster(cluster), vec![])
            }

            Command::UpdateCluster {
                slug,
                timestamp,
                name,
                description,
                location,
                ..
            } => {
                let txn = self.db.begin_write().expect("begin");
                let Some(mut cluster) = txn_get::<ClusterData>(&txn, CLUSTERS, &slug) else {
                    return (
                        Response::Error {
                            code: 404,
                            message: format!("Cluster '{}' not found", slug),
                        },
                        vec![],
                    );
                };
                if let Some(n) = name {
                    cluster.name = n;
                }
                if let Some(d) = description {
                    cluster.description = d;
                }
                if let Some(l) = location {
                    cluster.location = l;
                }
                cluster.updated_at = timestamp;
                txn_put(&txn, CLUSTERS, &slug, &cluster);
                txn.commit().expect("commit");
                (Response::Cluster(cluster), vec![])
            }

            Command::DeleteCluster { slug, .. } => {
                let txn = self.db.begin_write().expect("begin");
                if !txn_has(&txn, CLUSTERS, &slug) {
                    return (
                        Response::Error {
                            code: 404,
                            message: format!("Cluster '{}' not found", slug),
                        },
                        vec![],
                    );
                }
                // ADR-0005: reject if any resource references this Cluster.
                // Today no resource carries `cluster_slug` yet; the gate becomes
                // load-bearing once VM/Volume specs gain that field. Until then,
                // the Cluster's `node_ids` is the only reference and clearing it
                // is the operator's job (DELETE membership first).
                txn_delete(&txn, CLUSTERS, &slug);
                txn.commit().expect("commit");
                (Response::Deleted { id: slug }, vec![])
            }

            Command::AddNodeToCluster {
                timestamp,
                cluster_slug,
                node_id,
                ..
            } => {
                let txn = self.db.begin_write().expect("begin");
                let Some(mut cluster) = txn_get::<ClusterData>(&txn, CLUSTERS, &cluster_slug)
                else {
                    return (
                        Response::Error {
                            code: 404,
                            message: format!("Cluster '{}' not found", cluster_slug),
                        },
                        vec![],
                    );
                };
                if !txn_has(&txn, NODES, &node_id) {
                    return (
                        Response::Error {
                            code: 404,
                            message: format!("Node '{}' not found", node_id),
                        },
                        vec![],
                    );
                }
                if !cluster.node_ids.iter().any(|n| n == &node_id) {
                    cluster.node_ids.push(node_id);
                    cluster.updated_at = timestamp;
                    txn_put(&txn, CLUSTERS, &cluster_slug, &cluster);
                }
                txn.commit().expect("commit");
                (Response::Cluster(cluster), vec![])
            }

            Command::RemoveNodeFromCluster {
                timestamp,
                cluster_slug,
                node_id,
                ..
            } => {
                let txn = self.db.begin_write().expect("begin");
                let Some(mut cluster) = txn_get::<ClusterData>(&txn, CLUSTERS, &cluster_slug)
                else {
                    return (
                        Response::Error {
                            code: 404,
                            message: format!("Cluster '{}' not found", cluster_slug),
                        },
                        vec![],
                    );
                };
                let before = cluster.node_ids.len();
                cluster.node_ids.retain(|n| n != &node_id);
                if cluster.node_ids.len() != before {
                    cluster.updated_at = timestamp;
                    txn_put(&txn, CLUSTERS, &cluster_slug, &cluster);
                }
                txn.commit().expect("commit");
                (Response::Cluster(cluster), vec![])
            }

            // =================================================================
            // Node onboarding (ADR-0006).
            // =================================================================
            Command::EnsureInternalCa { ca, .. } => {
                let txn = self.db.begin_write().expect("begin");
                if txn_get::<InternalCa>(&txn, PKI, PKI_KEY_CA).is_none() {
                    txn_put(&txn, PKI, PKI_KEY_CA, &ca);
                }
                txn.commit().expect("commit");
                (Response::Ack, vec![])
            }

            Command::UpdateServerCert {
                timestamp,
                cert_pem,
                key_pem,
                serial_hex,
                not_after,
                ..
            } => {
                let txn = self.db.begin_write().expect("begin");
                let data = ServerCertData {
                    cert_pem,
                    key_pem,
                    serial_hex,
                    not_after,
                    created_at: timestamp,
                };
                txn_put(&txn, PKI, PKI_KEY_SERVER, &data);
                txn.commit().expect("commit");
                (Response::Ack, vec![])
            }

            Command::CreateOnboardingToken {
                timestamp,
                id,
                token_hash_hex,
                cluster_slug,
                node_id,
                hostname,
                description,
                expires_at,
                created_by_account,
                ..
            } => {
                let txn = self.db.begin_write().expect("begin");
                let Some(mut cluster) = txn_get::<ClusterData>(&txn, CLUSTERS, &cluster_slug)
                else {
                    return (
                        Response::Error {
                            code: 404,
                            message: format!("Cluster '{}' not found", cluster_slug),
                        },
                        vec![],
                    );
                };
                if txn_has(&txn, ONBOARDING_TOKENS, &id) {
                    return (
                        Response::Error {
                            code: 409,
                            message: format!("Onboarding token '{}' already exists", id),
                        },
                        vec![],
                    );
                }
                if txn_has(&txn, NODES, &node_id) {
                    return (
                        Response::Error {
                            code: 409,
                            message: format!("Node '{}' already exists", node_id),
                        },
                        vec![],
                    );
                }
                // Per-cluster hostname uniqueness — operator can't issue two
                // tokens for the same hostname in the same cluster (they'd
                // be indistinguishable in the UI).
                let dup_hostname = txn_list::<NodeData>(&txn, NODES).into_iter().any(|n| {
                    n.cluster_slug.as_deref() == Some(cluster_slug.as_str()) && n.name == hostname
                });
                if dup_hostname {
                    return (
                        Response::Error {
                            code: 409,
                            message: format!(
                                "A node with hostname '{}' already exists in cluster '{}'",
                                hostname, cluster_slug
                            ),
                        },
                        vec![],
                    );
                }

                let token = OnboardingTokenData {
                    id: id.clone(),
                    token_hash_hex,
                    cluster_slug: cluster_slug.clone(),
                    node_id: node_id.clone(),
                    hostname: hostname.clone(),
                    description,
                    expires_at,
                    used_at: None,
                    used_by_node_id: None,
                    created_by_account,
                    created_at: timestamp.clone(),
                };
                txn_put(&txn, ONBOARDING_TOKENS, &id, &token);

                // Placeholder Node row with status: Onboarding so the
                // operator sees the host in the cluster list immediately.
                let node = NodeData {
                    id: node_id.clone(),
                    name: hostname,
                    address: String::new(),
                    status: NodeStatus::Onboarding,
                    resources: Default::default(),
                    labels: Default::default(),
                    last_heartbeat: timestamp.clone(),
                    created_at: timestamp.clone(),
                    updated_at: timestamp.clone(),
                    cluster_slug: Some(cluster_slug.clone()),
                    cert_serial_hex: None,
                    cert_expires_at: None,
                    hostname: None,
                    agent_version: None,
                };
                txn_put(&txn, NODES, &node_id, &node);

                if !cluster.node_ids.iter().any(|n| n == &node_id) {
                    cluster.node_ids.push(node_id);
                    cluster.updated_at = timestamp;
                    txn_put(&txn, CLUSTERS, &cluster_slug, &cluster);
                }

                txn.commit().expect("commit");
                (Response::OnboardingToken(token), vec![])
            }

            Command::DeleteOnboardingToken {
                cluster_slug, id, ..
            } => {
                let txn = self.db.begin_write().expect("begin");
                let Some(t) = txn_get::<OnboardingTokenData>(&txn, ONBOARDING_TOKENS, &id) else {
                    return (
                        Response::Error {
                            code: 404,
                            message: format!("Onboarding token '{}' not found", id),
                        },
                        vec![],
                    );
                };
                if t.cluster_slug != cluster_slug {
                    return (
                        Response::Error {
                            code: 404,
                            message: format!(
                                "Token '{}' does not belong to cluster '{}'",
                                id, cluster_slug
                            ),
                        },
                        vec![],
                    );
                }
                // If the token never got redeemed, also tear down the
                // placeholder Node row and remove it from the cluster — the
                // operator wants the entry gone, not a half-state.
                if t.used_at.is_none() && txn_has(&txn, NODES, &t.node_id) {
                    txn_delete(&txn, NODES, &t.node_id);
                    if let Some(mut cluster) =
                        txn_get::<ClusterData>(&txn, CLUSTERS, &t.cluster_slug)
                    {
                        let before = cluster.node_ids.len();
                        cluster.node_ids.retain(|n| n != &t.node_id);
                        if cluster.node_ids.len() != before {
                            let slug = cluster.slug.clone();
                            txn_put(&txn, CLUSTERS, &slug, &cluster);
                        }
                    }
                }
                txn_delete(&txn, ONBOARDING_TOKENS, &id);
                txn.commit().expect("commit");
                (Response::Deleted { id }, vec![])
            }

            Command::RedeemOnboardingToken {
                timestamp,
                token_hash_hex,
                csr_pem,
                hostname,
                agent_version,
                kernel_version,
                arch,
                ..
            } => {
                let txn = self.db.begin_write().expect("begin");

                // 1. Find token by hash. With low token volumes (<1000) the
                //    scan is cheap; redeem-path is rare anyway.
                let mut token: Option<OnboardingTokenData> = None;
                for t in txn_list::<OnboardingTokenData>(&txn, ONBOARDING_TOKENS) {
                    if t.token_hash_hex == token_hash_hex {
                        token = Some(t);
                        break;
                    }
                }
                let Some(mut token) = token else {
                    return (
                        Response::Error {
                            code: 401,
                            message: "invalid onboarding token".into(),
                        },
                        vec![],
                    );
                };

                if token.used_at.is_some() {
                    return (
                        Response::Error {
                            code: 401,
                            message: "invalid onboarding token".into(),
                        },
                        vec![],
                    );
                }
                if timestamp.as_str() > token.expires_at.as_str() {
                    return (
                        Response::Error {
                            code: 401,
                            message: "invalid onboarding token".into(),
                        },
                        vec![],
                    );
                }

                if !txn_has(&txn, CLUSTERS, &token.cluster_slug) {
                    return (
                        Response::Error {
                            code: 410,
                            message: format!(
                                "cluster '{}' gone since token issuance",
                                token.cluster_slug
                            ),
                        },
                        vec![],
                    );
                }

                // The placeholder Node row was created with the token; if the
                // operator deleted it post-issue, refuse — the token alone
                // can't reconstitute the row safely.
                let Some(mut node) = txn_get::<NodeData>(&txn, NODES, &token.node_id) else {
                    return (
                        Response::Error {
                            code: 410,
                            message: format!(
                                "placeholder node '{}' was deleted; re-issue a fresh token",
                                token.node_id
                            ),
                        },
                        vec![],
                    );
                };

                let Some(ca) = txn_get::<InternalCa>(&txn, PKI, PKI_KEY_CA) else {
                    return (
                        Response::Error {
                            code: 503,
                            message: "internal CA not initialised".into(),
                        },
                        vec![],
                    );
                };

                let serial = new_serial();
                let signed = match sign_node_leaf(
                    &ca,
                    &csr_pem,
                    &token.node_id,
                    &token.cluster_slug,
                    serial,
                ) {
                    Ok(s) => s,
                    Err(e) => {
                        return (
                            Response::Error {
                                code: 400,
                                message: format!("CSR invalid: {e}"),
                            },
                            vec![],
                        );
                    }
                };

                token.used_at = Some(timestamp.clone());
                token.used_by_node_id = Some(token.node_id.clone());
                let token_id = token.id.clone();
                let resolved_node_id = token.node_id.clone();
                txn_put(&txn, ONBOARDING_TOKENS, &token_id, &token);

                // Flip the placeholder to Online + fill in cert metadata.
                // We keep the operator-supplied display `name`; the agent's
                // reported hostname/kernel/arch go into the diagnostic
                // fields so the UI can show them if desired.
                node.status = NodeStatus::Online;
                node.last_heartbeat = timestamp.clone();
                node.updated_at = timestamp.clone();
                node.cert_serial_hex = Some(signed.serial_hex.clone());
                node.cert_expires_at = Some(signed.not_after.clone());
                node.hostname = Some(hostname);
                node.agent_version =
                    Some(format!("{} ({} {})", agent_version, kernel_version, arch));
                txn_put(&txn, NODES, &resolved_node_id, &node);

                txn.commit().expect("commit");
                (
                    Response::BootstrapResult {
                        node,
                        client_cert_pem: signed.cert_pem,
                        ca_cert_pem: ca.ca_cert_pem,
                    },
                    vec![],
                )
            }

            Command::RevokeNodeCert {
                timestamp,
                node_id,
                reason,
                ..
            } => {
                let txn = self.db.begin_write().expect("begin");
                let Some(mut node) = txn_get::<NodeData>(&txn, NODES, &node_id) else {
                    return (
                        Response::Error {
                            code: 404,
                            message: format!("Node '{}' not found", node_id),
                        },
                        vec![],
                    );
                };

                if let Some(serial) = node.cert_serial_hex.clone() {
                    let revoked = RevokedCertData {
                        serial_hex: serial.clone(),
                        node_id: node_id.clone(),
                        revoked_at: timestamp.clone(),
                        reason,
                    };
                    txn_put(&txn, REVOKED_CERTS, &serial, &revoked);
                }

                match reason {
                    RevocationReason::Decommission => {
                        // Drop unredeemed tokens that were pre-allocated for
                        // this node — otherwise they'd dangle with a
                        // missing-node reference and would 410 on redeem.
                        for t in txn_list::<OnboardingTokenData>(&txn, ONBOARDING_TOKENS) {
                            if t.node_id == node_id && t.used_at.is_none() {
                                txn_delete(&txn, ONBOARDING_TOKENS, &t.id);
                            }
                        }
                        for mut cluster in txn_list::<ClusterData>(&txn, CLUSTERS) {
                            let before = cluster.node_ids.len();
                            cluster.node_ids.retain(|n| n != &node_id);
                            if cluster.node_ids.len() != before {
                                cluster.updated_at = timestamp.clone();
                                let slug = cluster.slug.clone();
                                txn_put(&txn, CLUSTERS, &slug, &cluster);
                            }
                        }
                        txn_delete(&txn, NODES, &node_id);
                    }
                    RevocationReason::Compromise | RevocationReason::Other => {
                        node.status = NodeStatus::Revoked;
                        node.updated_at = timestamp;
                        txn_put(&txn, NODES, &node_id, &node);
                    }
                }
                txn.commit().expect("commit");
                (Response::Deleted { id: node_id }, vec![])
            }

            // =================================================================
            // Account + Membership (ADR-0004).
            // =================================================================
            Command::CreateAccountByEmail {
                timestamp,
                id,
                email,
                display_name,
                ..
            } => {
                let txn = self.db.begin_write().expect("begin");
                let normalized = email.trim().to_lowercase();
                if normalized.is_empty() {
                    return (
                        Response::Error {
                            code: 400,
                            message: "email is required".into(),
                        },
                        vec![],
                    );
                }
                let dup = txn_list::<AccountData>(&txn, ACCOUNTS)
                    .into_iter()
                    .any(|a| {
                        a.email.as_deref().map(|e| e.to_lowercase()) == Some(normalized.clone())
                    });
                if dup {
                    return (
                        Response::Error {
                            code: 409,
                            message: format!("Account with email '{}' already exists", normalized),
                        },
                        vec![],
                    );
                }
                let acc = AccountData {
                    id: id.clone(),
                    kind: AccountKind::User,
                    external_iss: None,
                    external_sub: None,
                    email: Some(normalized),
                    display_name,
                    project_slug: None,
                    description: None,
                    created_at: timestamp.clone(),
                    updated_at: timestamp,
                };
                txn_put(&txn, ACCOUNTS, &id, &acc);
                txn.commit().expect("commit");
                (Response::Account(acc), vec![])
            }

            Command::EnsureAccountFromOidc {
                timestamp,
                new_id,
                iss,
                sub,
                email,
                display_name,
                ..
            } => {
                let txn = self.db.begin_write().expect("begin");
                let all = txn_list::<AccountData>(&txn, ACCOUNTS);
                // 1. Match on (iss, sub) → already linked.
                let existing = all.iter().find(|a| {
                    a.external_iss.as_deref() == Some(iss.as_str())
                        && a.external_sub.as_deref() == Some(sub.as_str())
                });
                let account = if let Some(acc) = existing {
                    let mut acc = acc.clone();
                    let mut changed = false;
                    if email.is_some() && acc.email != email {
                        acc.email = email;
                        changed = true;
                    }
                    if display_name.is_some() && acc.display_name != display_name {
                        acc.display_name = display_name;
                        changed = true;
                    }
                    if changed {
                        acc.updated_at = timestamp;
                        let id = acc.id.clone();
                        txn_put(&txn, ACCOUNTS, &id, &acc);
                    }
                    acc
                } else if let Some(invited) = email.as_ref().and_then(|e| {
                    let lc = e.to_lowercase();
                    // 2. Email-match on a pre-invite Account (no iss/sub yet)
                    //    → link the OIDC identity to that row. Next time the
                    //    user logs in, path #1 wins.
                    all.iter().find(|a| {
                        a.external_iss.is_none()
                            && a.external_sub.is_none()
                            && a.email.as_deref().map(|s| s.to_lowercase()) == Some(lc.clone())
                    })
                }) {
                    let mut acc = invited.clone();
                    acc.external_iss = Some(iss);
                    acc.external_sub = Some(sub);
                    acc.email = email;
                    if display_name.is_some() {
                        acc.display_name = display_name;
                    }
                    acc.updated_at = timestamp;
                    let id = acc.id.clone();
                    txn_put(&txn, ACCOUNTS, &id, &acc);
                    acc
                } else {
                    // 3. Brand-new Account — first time we've seen this user.
                    let acc = AccountData {
                        id: new_id.clone(),
                        kind: AccountKind::User,
                        external_iss: Some(iss),
                        external_sub: Some(sub),
                        email,
                        display_name,
                        project_slug: None,
                        description: None,
                        created_at: timestamp.clone(),
                        updated_at: timestamp,
                    };
                    txn_put(&txn, ACCOUNTS, &new_id, &acc);
                    acc
                };
                txn.commit().expect("commit");
                (Response::Account(account), vec![])
            }

            Command::CreateMembership {
                timestamp,
                id,
                account_id,
                scope,
                role,
                created_by_account,
                ..
            } => {
                let txn = self.db.begin_write().expect("begin");
                if txn_has(&txn, MEMBERSHIPS, &id) {
                    return (
                        Response::Error {
                            code: 409,
                            message: format!("Membership '{}' already exists", id),
                        },
                        vec![],
                    );
                }
                if !txn_has(&txn, ACCOUNTS, &account_id) {
                    return (
                        Response::Error {
                            code: 404,
                            message: format!("Account '{}' not found", account_id),
                        },
                        vec![],
                    );
                }
                // Scope-target existence checks — fail fast if the operator
                // tries to grant a membership at a scope that doesn't exist.
                match &scope {
                    MembershipScope::Platform => {}
                    MembershipScope::Org { org_slug } => {
                        if !txn_has(&txn, ORGS, org_slug) {
                            return (
                                Response::Error {
                                    code: 404,
                                    message: format!("Org '{}' not found", org_slug),
                                },
                                vec![],
                            );
                        }
                    }
                    MembershipScope::Project { project_slug } => {
                        if !txn_has(&txn, PROJECTS, project_slug) {
                            return (
                                Response::Error {
                                    code: 404,
                                    message: format!("Project '{}' not found", project_slug),
                                },
                                vec![],
                            );
                        }
                    }
                }
                // Reject duplicate (account, scope, role).
                let dup = txn_list::<MembershipData>(&txn, MEMBERSHIPS)
                    .into_iter()
                    .any(|m| m.account_id == account_id && m.scope == scope && m.role == role);
                if dup {
                    return (
                        Response::Error {
                            code: 409,
                            message: "membership already granted".into(),
                        },
                        vec![],
                    );
                }
                let m = MembershipData {
                    id: id.clone(),
                    account_id,
                    scope,
                    role,
                    created_by_account,
                    created_at: timestamp,
                };
                txn_put(&txn, MEMBERSHIPS, &id, &m);
                txn.commit().expect("commit");
                (Response::Membership(m), vec![])
            }

            Command::DeleteMembership { id, .. } => {
                let txn = self.db.begin_write().expect("begin");
                if !txn_has(&txn, MEMBERSHIPS, &id) {
                    return (
                        Response::Error {
                            code: 404,
                            message: format!("Membership '{}' not found", id),
                        },
                        vec![],
                    );
                }
                txn_delete(&txn, MEMBERSHIPS, &id);
                txn.commit().expect("commit");
                (Response::Deleted { id }, vec![])
            }

            Command::BootstrapInitialPlatformAdmin {
                timestamp,
                id,
                account_id,
                ..
            } => {
                let txn = self.db.begin_write().expect("begin");
                // Race-safe: noop if any platform-admin already exists.
                let exists = txn_list::<MembershipData>(&txn, MEMBERSHIPS)
                    .into_iter()
                    .any(|m| m.scope == MembershipScope::Platform && m.role == Role::PlatformAdmin);
                if exists {
                    txn.commit().expect("commit");
                    return (Response::Ack, vec![]);
                }
                if !txn_has(&txn, ACCOUNTS, &account_id) {
                    return (
                        Response::Error {
                            code: 404,
                            message: format!("Account '{}' not found", account_id),
                        },
                        vec![],
                    );
                }
                let m = MembershipData {
                    id: id.clone(),
                    account_id: account_id.clone(),
                    scope: MembershipScope::Platform,
                    role: Role::PlatformAdmin,
                    created_by_account: account_id,
                    created_at: timestamp,
                };
                txn_put(&txn, MEMBERSHIPS, &id, &m);
                txn.commit().expect("commit");
                (Response::Membership(m), vec![])
            }

            // =================================================================
            // ServiceAccount + StaticApiKey Commands (ADR-0004)
            // =================================================================
            Command::CreateServiceAccount {
                timestamp,
                account_id,
                membership_id,
                project_slug,
                name,
                description,
                created_by_account,
                ..
            } => {
                let txn = self.db.begin_write().expect("begin");
                if !txn_has(&txn, PROJECTS, &project_slug) {
                    return (
                        Response::Error {
                            code: 404,
                            message: format!("Project '{}' not found", project_slug),
                        },
                        vec![],
                    );
                }
                // ADR-0004 §"Account model": ServiceAccount per
                // (project_slug, name) must be unique. Scan accounts within
                // the project; SA count per project is bounded so the linear
                // scan is fine.
                let dup = txn_list::<AccountData>(&txn, ACCOUNTS)
                    .into_iter()
                    .any(|a| {
                        a.kind == AccountKind::ServiceAccount
                            && a.project_slug.as_deref() == Some(project_slug.as_str())
                            && a.display_name.as_deref() == Some(name.as_str())
                    });
                if dup {
                    return (
                        Response::Error {
                            code: 409,
                            message: format!(
                                "ServiceAccount '{}' already exists in project '{}'",
                                name, project_slug
                            ),
                        },
                        vec![],
                    );
                }
                let acc = AccountData {
                    id: account_id.clone(),
                    kind: AccountKind::ServiceAccount,
                    external_iss: None,
                    external_sub: None,
                    email: None,
                    display_name: Some(name),
                    project_slug: Some(project_slug.clone()),
                    description,
                    created_at: timestamp.clone(),
                    updated_at: timestamp.clone(),
                };
                txn_put(&txn, ACCOUNTS, &account_id, &acc);
                // Auto-grant the SA project-admin in its home project. SAs
                // need explicit memberships just like users — the Account
                // row alone carries no privileges.
                let m = MembershipData {
                    id: membership_id.clone(),
                    account_id: account_id.clone(),
                    scope: MembershipScope::Project {
                        project_slug: project_slug.clone(),
                    },
                    role: Role::ProjectAdmin,
                    created_by_account,
                    created_at: timestamp,
                };
                txn_put(&txn, MEMBERSHIPS, &membership_id, &m);
                txn.commit().expect("commit");
                (Response::Account(acc), vec![])
            }

            Command::DeleteServiceAccount { account_id, .. } => {
                let txn = self.db.begin_write().expect("begin");
                let Some(acc) = txn_get::<AccountData>(&txn, ACCOUNTS, &account_id) else {
                    return (
                        Response::Error {
                            code: 404,
                            message: format!("Account '{}' not found", account_id),
                        },
                        vec![],
                    );
                };
                if acc.kind != AccountKind::ServiceAccount {
                    return (
                        Response::Error {
                            code: 400,
                            message: "not a ServiceAccount".into(),
                        },
                        vec![],
                    );
                }
                // Cascade: keys + memberships before the Account row, so an
                // interrupted apply can't leave orphan rows pointing at a
                // dead account.
                let key_ids: Vec<String> = txn_list::<ApiKeyData>(&txn, API_KEYS)
                    .into_iter()
                    .filter(|k| k.account_id == account_id)
                    .map(|k| k.id)
                    .collect();
                for kid in key_ids {
                    txn_delete(&txn, API_KEYS, &kid);
                }
                let mem_ids: Vec<String> = txn_list::<MembershipData>(&txn, MEMBERSHIPS)
                    .into_iter()
                    .filter(|m| m.account_id == account_id)
                    .map(|m| m.id)
                    .collect();
                for mid in mem_ids {
                    txn_delete(&txn, MEMBERSHIPS, &mid);
                }
                txn_delete(&txn, ACCOUNTS, &account_id);
                txn.commit().expect("commit");
                (Response::Deleted { id: account_id }, vec![])
            }

            Command::CreateStaticApiKey {
                timestamp,
                id,
                account_id,
                hash_hex,
                display_prefix,
                description,
                expires_at,
                created_by_account,
                ..
            } => {
                let txn = self.db.begin_write().expect("begin");
                let Some(acc) = txn_get::<AccountData>(&txn, ACCOUNTS, &account_id) else {
                    return (
                        Response::Error {
                            code: 404,
                            message: format!("Account '{}' not found", account_id),
                        },
                        vec![],
                    );
                };
                if acc.kind != AccountKind::ServiceAccount {
                    return (
                        Response::Error {
                            code: 400,
                            message: "API keys are only valid for ServiceAccounts".into(),
                        },
                        vec![],
                    );
                }
                if txn_has(&txn, API_KEYS, &id) {
                    return (
                        Response::Error {
                            code: 409,
                            message: format!("API key '{}' already exists", id),
                        },
                        vec![],
                    );
                }
                let key = ApiKeyData {
                    id: id.clone(),
                    account_id,
                    hash_hex,
                    display_prefix,
                    description,
                    expires_at,
                    last_used_at: None,
                    revoked_at: None,
                    created_at: timestamp,
                    created_by_account,
                };
                txn_put(&txn, API_KEYS, &id, &key);
                txn.commit().expect("commit");
                (Response::ApiKey(key), vec![])
            }

            Command::RevokeStaticApiKey { timestamp, id, .. } => {
                let txn = self.db.begin_write().expect("begin");
                let Some(mut key) = txn_get::<ApiKeyData>(&txn, API_KEYS, &id) else {
                    return (
                        Response::Error {
                            code: 404,
                            message: format!("API key '{}' not found", id),
                        },
                        vec![],
                    );
                };
                if key.revoked_at.is_none() {
                    key.revoked_at = Some(timestamp);
                    txn_put(&txn, API_KEYS, &id, &key);
                }
                txn.commit().expect("commit");
                (Response::ApiKey(key), vec![])
            }

            // =================================================================
            // Volume Commands
            // =================================================================
            Command::CreateVolume {
                id,
                timestamp,
                project_slug,
                node_id,
                name,
                size_bytes,
                template_id,
                ..
            } => {
                let txn = self.db.begin_write().expect("begin");

                if !txn_has(&txn, PROJECTS, &project_slug) {
                    return (
                        Response::Error {
                            code: 404,
                            message: format!("Project '{}' not found", project_slug),
                        },
                        vec![],
                    );
                }

                if let Some(existing) = txn_get::<VolumeData>(&txn, VOLUMES, &id) {
                    return (Response::Volume(existing), vec![]);
                }

                if txn_list::<VolumeData>(&txn, VOLUMES)
                    .iter()
                    .any(|v| v.spec.project_slug == project_slug && v.spec.name == name)
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
                        project_slug,
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
                let old = vol.clone();
                vol.status.phase = phase;
                if let Some(p) = path {
                    vol.status.path = p;
                }
                vol.status.used_bytes = used_bytes;
                vol.status.error = error;
                vol.updated_at = timestamp;
                txn_put(&txn, VOLUMES, &id, &vol);
                txn.commit().expect("commit");
                (
                    Response::Volume(vol.clone()),
                    vec![Event::VolumeStatusUpdated { id, old, new: vol }],
                )
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
                project_slug,
                node_id,
                name,
                size_bytes,
                source_url,
                total_bytes,
                ..
            } => {
                let txn = self.db.begin_write().expect("begin");
                if txn_list::<TemplateData>(&txn, TEMPLATES).iter().any(|t| {
                    t.spec.project_slug == project_slug
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
                        project_slug,
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
                project_slug,
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
                    .any(|sg| sg.project_slug == project_slug && sg.name == name)
                {
                    return (
                        Response::Error {
                            code: 409,
                            message: format!("Security group '{}' already exists in project", name),
                        },
                        vec![],
                    );
                }

                if !txn_has(&txn, PROJECTS, &project_slug) {
                    return (
                        Response::Error {
                            code: 404,
                            message: format!("Project '{}' not found", project_slug),
                        },
                        vec![],
                    );
                }

                let sg = SecurityGroupData {
                    id: id.clone(),
                    project_slug,
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

            Command::UpdateSecurityGroup {
                timestamp,
                id,
                name,
                description,
                ..
            } => {
                let txn = self.db.begin_write().expect("begin");
                let Some(mut sg) = txn_get::<SecurityGroupData>(&txn, SECURITY_GROUPS, &id) else {
                    return (
                        Response::Error {
                            code: 404,
                            message: format!("Security group '{}' not found", id),
                        },
                        vec![],
                    );
                };
                if let Some(n) = name {
                    sg.name = n;
                }
                if let Some(d) = description {
                    sg.description = d;
                }
                sg.updated_at = timestamp;
                txn_put(&txn, SECURITY_GROUPS, &id, &sg);
                txn.commit().expect("commit");
                (Response::SecurityGroup(sg), vec![])
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

            Command::UpdateSecurityGroupRule {
                timestamp,
                security_group_id,
                rule_id,
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
                let Some(rule) = sg.rules.iter_mut().find(|r| r.id == rule_id) else {
                    return (
                        Response::Error {
                            code: 404,
                            message: format!("Rule '{}' not found", rule_id),
                        },
                        vec![],
                    );
                };
                if let Some(d) = description {
                    rule.description = d;
                }
                sg.updated_at = timestamp;
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
    /// As a convenience for legacy tests, this auto-creates `TEST_ORG_SLUG` on
    /// the first `CreateProject` against the shared org slug — keeps tests that
    /// predate the Org introduction working without per-test setup boilerplate.
    /// Tests that exercise Org behavior directly (`test_create_project_org_not_found`,
    /// `test_delete_org_with_projects_rejects`) bypass this by either not
    /// referencing TEST_ORG_SLUG or by setting up Orgs explicitly.
    fn apply(state: &mut ApiState, cmd: Command) -> Response {
        if let Command::CreateProject { org_slug, .. } = &cmd
            && org_slug == TEST_ORG_SLUG
            && state.get_org(TEST_ORG_SLUG).is_none()
        {
            let setup = create_org_cmd("test-org-setup", TEST_ORG_SLUG);
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
            project_slug: "test-project".to_string(),
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
            project_slug: "test-project".to_string(),
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

    /// Default Org slug used by test helpers that don't care which Org a Project lives in.
    const TEST_ORG_SLUG: &str = "org-test";

    fn create_org_cmd(request_id: &str, slug: &str) -> Command {
        Command::CreateOrg {
            request_id: request_id.to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            slug: slug.to_string(),
            name: format!("Org {}", slug),
            contact: OrgContact::default(),
        }
    }

    /// Ensure the shared TEST_ORG_SLUG exists; idempotent.
    fn ensure_test_org(state: &mut ApiState) {
        if state.get_org(TEST_ORG_SLUG).is_none() {
            apply(state, create_org_cmd("test-org-req", TEST_ORG_SLUG));
        }
    }

    /// Create a Project in TEST_ORG. `slug` is the project slug.
    fn create_project_cmd(request_id: &str, slug: &str, name: &str) -> Command {
        Command::CreateProject {
            request_id: request_id.to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            org_slug: TEST_ORG_SLUG.to_string(),
            slug: slug.to_string(),
            name: name.to_string(),
            description: Some("Test project".to_string()),
        }
    }

    #[test]
    fn test_create_org() {
        let mut state = ApiState::default();
        let response = apply(&mut state, create_org_cmd("req-1", "acme"));
        match response {
            Response::Org(data) => {
                assert_eq!(data.slug, "acme");
                assert_eq!(data.name, "Org acme");
            }
            other => panic!("unexpected: {:?}", other),
        }
        assert_eq!(state.list_orgs().len(), 1);
        assert!(state.get_org("acme").is_some());
    }

    #[test]
    fn test_create_org_duplicate_slug_rejects() {
        // Same slug from a different request_id is a name collision, not a
        // retry — return 409 so the user sees the conflict. Same-request_id
        // retries are caught earlier by the applied_requests cache.
        let mut state = ApiState::default();
        apply(&mut state, create_org_cmd("req-1", "acme"));
        let response = apply(&mut state, create_org_cmd("req-2", "acme"));
        assert!(matches!(response, Response::Error { code: 409, .. }));
        assert_eq!(state.list_orgs().len(), 1);
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
                slug: TEST_ORG_SLUG.to_string(),
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
                assert_eq!(data.slug, "proj-1");
                assert_eq!(data.org_slug, TEST_ORG_SLUG);
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
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            org_slug: "no-such-org".to_string(),
            slug: "proj-1".to_string(),
            name: "x".to_string(),
            description: None,
        };
        let response = apply(&mut state, cmd);
        assert!(matches!(response, Response::Error { code: 404, .. }));
    }

    #[test]
    fn test_create_project_duplicate_slug_rejects() {
        // Project slugs are platform-wide unique. A second create with the
        // same slug — even from a different request_id — is a collision,
        // not a retry; reply 409. Same-request_id retries are caught
        // earlier by the applied_requests cache.
        let mut state = ApiState::default();
        ensure_test_org(&mut state);

        apply(
            &mut state,
            create_project_cmd("req-1", "proj-1", "dup-name"),
        );

        let response = apply(&mut state, create_project_cmd("req-2", "proj-1", "second"));
        assert!(matches!(response, Response::Error { code: 409, .. }));
        assert_eq!(state.list_projects().len(), 1);
    }

    #[test]
    fn test_create_project_duplicate_slug_across_orgs_rejects() {
        // Even when the second create targets a different Org, the slug is
        // still globally unique — collision, 409.
        let mut state = ApiState::default();
        ensure_test_org(&mut state);
        apply(&mut state, create_org_cmd("req-org2", "other-org"));
        apply(
            &mut state,
            create_project_cmd("req-1", "shared-slug", "first"),
        );

        let cmd = Command::CreateProject {
            request_id: "req-2".to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            org_slug: "other-org".to_string(),
            slug: "shared-slug".to_string(),
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
            slug: "proj-1".to_string(),
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
            slug: "non-existent".to_string(),
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

        let project = state.get_project("proj-uuid");
        assert!(project.is_some());
        assert_eq!(project.unwrap().slug, "proj-uuid");

        assert!(state.get_project("unknown").is_none());
    }

    // =========================================================================
    // Volume Tests
    // =========================================================================

    fn create_volume_cmd(
        request_id: &str,
        id: &str,
        project_slug: &str,
        node_id: &str,
        name: &str,
        size: u64,
    ) -> Command {
        Command::CreateVolume {
            request_id: request_id.to_string(),
            id: id.to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            project_slug: project_slug.to_string(),
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
                assert_eq!(data.spec.project_slug, "proj-1");
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
            project_slug: "test-project".to_string(),
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
            project_slug: "proj-1".to_string(),
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
                project_slug: "proj-1".to_string(),
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
            project_slug: "proj-1".to_string(),
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
            project_slug: "test-project".to_string(),
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
                project_slug: "test-project".to_string(),
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
                project_slug: "test-project".to_string(),
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
                project_slug: "test-project".to_string(),
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
