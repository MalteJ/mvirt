//! API Server state machine.

use lru::LruCache;
use mraft::StateMachine;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::num::NonZeroUsize;

use crate::command::{
    Command, ImportJobData, ImportJobState, NetworkData, NicData, NicStateData, NodeData,
    NodeStatus, ProjectData, Response, SecurityGroupData, SecurityGroupRuleData, SnapshotData,
    TemplateData, VmData, VmPhase, VmStatus, VolumeData,
};
use crate::store::Event;

/// API Server state - replicated across all nodes via Raft.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiState {
    pub nodes: HashMap<String, NodeData>,
    pub networks: HashMap<String, NetworkData>,
    pub nics: HashMap<String, NicData>,
    pub vms: HashMap<String, VmData>,
    pub projects: HashMap<String, ProjectData>,
    pub volumes: HashMap<String, VolumeData>,
    pub templates: HashMap<String, TemplateData>,
    pub import_jobs: HashMap<String, ImportJobData>,
    pub security_groups: HashMap<String, SecurityGroupData>,
    /// Idempotency cache for request deduplication
    #[serde(skip)]
    applied_requests: Option<LruCache<String, Response>>,
}

impl Default for ApiState {
    fn default() -> Self {
        Self {
            nodes: HashMap::new(),
            networks: HashMap::new(),
            nics: HashMap::new(),
            vms: HashMap::new(),
            projects: HashMap::new(),
            volumes: HashMap::new(),
            templates: HashMap::new(),
            import_jobs: HashMap::new(),
            security_groups: HashMap::new(),
            applied_requests: Some(LruCache::new(NonZeroUsize::new(1000).unwrap())),
        }
    }
}

impl ApiState {
    // =========================================================================
    // Node queries
    // =========================================================================

    /// Get a node by ID
    pub fn get_node(&self, id: &str) -> Option<&NodeData> {
        self.nodes.get(id)
    }

    /// Get a node by name
    pub fn get_node_by_name(&self, name: &str) -> Option<&NodeData> {
        self.nodes.values().find(|n| n.name == name)
    }

    /// List all nodes
    pub fn list_nodes(&self) -> Vec<&NodeData> {
        self.nodes.values().collect()
    }

    /// List online nodes only
    pub fn list_online_nodes(&self) -> Vec<&NodeData> {
        self.nodes
            .values()
            .filter(|n| n.status == NodeStatus::Online)
            .collect()
    }

    // =========================================================================
    // Network queries
    // =========================================================================

    /// Get a network by ID
    pub fn get_network(&self, id: &str) -> Option<&NetworkData> {
        self.networks.get(id)
    }

    /// Get a network by name
    pub fn get_network_by_name(&self, name: &str) -> Option<&NetworkData> {
        self.networks.values().find(|n| n.name == name)
    }

    /// List all networks
    pub fn list_networks(&self) -> Vec<&NetworkData> {
        self.networks.values().collect()
    }

    /// List networks by project
    pub fn list_networks_by_project(&self, project_id: &str) -> Vec<&NetworkData> {
        self.networks
            .values()
            .filter(|n| n.project_id == project_id)
            .collect()
    }

    /// Get a NIC by ID
    pub fn get_nic(&self, id: &str) -> Option<&NicData> {
        self.nics.get(id)
    }

    /// Get a NIC by name
    pub fn get_nic_by_name(&self, name: &str) -> Option<&NicData> {
        self.nics.values().find(|n| n.name.as_deref() == Some(name))
    }

    /// List all NICs, optionally filtered by network
    pub fn list_nics(&self, network_id: Option<&str>) -> Vec<&NicData> {
        match network_id {
            Some(net_id) => self
                .nics
                .values()
                .filter(|n| n.network_id == net_id)
                .collect(),
            None => self.nics.values().collect(),
        }
    }

    /// List NICs by project
    pub fn list_nics_by_project(&self, project_id: &str) -> Vec<&NicData> {
        self.nics
            .values()
            .filter(|n| n.project_id == project_id)
            .collect()
    }

    // =========================================================================
    // VM queries
    // =========================================================================

    /// Get a VM by ID
    pub fn get_vm(&self, id: &str) -> Option<&VmData> {
        self.vms.get(id)
    }

    /// Get a VM by name
    pub fn get_vm_by_name(&self, name: &str) -> Option<&VmData> {
        self.vms.values().find(|v| v.spec.name == name)
    }

    /// List all VMs, optionally filtered by node
    pub fn list_vms(&self, node_id: Option<&str>) -> Vec<&VmData> {
        match node_id {
            Some(nid) => self
                .vms
                .values()
                .filter(|v| v.status.node_id.as_deref() == Some(nid))
                .collect(),
            None => self.vms.values().collect(),
        }
    }

    /// List VMs by phase
    pub fn list_vms_by_phase(&self, phase: VmPhase) -> Vec<&VmData> {
        self.vms
            .values()
            .filter(|v| v.status.phase == phase)
            .collect()
    }

    /// List VMs by project
    pub fn list_vms_by_project(&self, project_id: &str) -> Vec<&VmData> {
        self.vms
            .values()
            .filter(|v| v.spec.project_id == project_id)
            .collect()
    }

    // =========================================================================
    // Project queries
    // =========================================================================

    /// Get a project by ID
    pub fn get_project(&self, id: &str) -> Option<&ProjectData> {
        self.projects.get(id)
    }

    /// Get a project by name
    pub fn get_project_by_name(&self, name: &str) -> Option<&ProjectData> {
        self.projects.values().find(|p| p.name == name)
    }

    /// List all projects
    pub fn list_projects(&self) -> Vec<&ProjectData> {
        self.projects.values().collect()
    }

    // =========================================================================
    // Volume queries
    // =========================================================================

    /// Get a volume by ID
    pub fn get_volume(&self, id: &str) -> Option<&VolumeData> {
        self.volumes.get(id)
    }

    /// Get a volume by name within a project
    pub fn get_volume_by_name(&self, project_id: &str, name: &str) -> Option<&VolumeData> {
        self.volumes
            .values()
            .find(|v| v.project_id == project_id && v.name == name)
    }

    /// List all volumes, optionally filtered by project or node
    pub fn list_volumes(
        &self,
        project_id: Option<&str>,
        node_id: Option<&str>,
    ) -> Vec<&VolumeData> {
        self.volumes
            .values()
            .filter(|v| {
                project_id.is_none_or(|pid| v.project_id == pid)
                    && node_id.is_none_or(|nid| v.node_id == nid)
            })
            .collect()
    }

    // =========================================================================
    // Template queries
    // =========================================================================

    /// Get a template by ID
    pub fn get_template(&self, id: &str) -> Option<&TemplateData> {
        self.templates.get(id)
    }

    /// Get a template by name
    pub fn get_template_by_name(&self, name: &str) -> Option<&TemplateData> {
        self.templates.values().find(|t| t.name == name)
    }

    /// List all templates, optionally filtered by node
    pub fn list_templates(&self, node_id: Option<&str>) -> Vec<&TemplateData> {
        match node_id {
            Some(nid) => self
                .templates
                .values()
                .filter(|t| t.node_id == nid)
                .collect(),
            None => self.templates.values().collect(),
        }
    }

    /// List templates by project
    pub fn list_templates_by_project(&self, project_id: &str) -> Vec<&TemplateData> {
        self.templates
            .values()
            .filter(|t| t.project_id == project_id)
            .collect()
    }

    // =========================================================================
    // Import job queries
    // =========================================================================

    /// Get an import job by ID
    pub fn get_import_job(&self, id: &str) -> Option<&ImportJobData> {
        self.import_jobs.get(id)
    }

    /// List import jobs, optionally filtered by state
    pub fn list_import_jobs(&self, state: Option<ImportJobState>) -> Vec<&ImportJobData> {
        match state {
            Some(s) => self.import_jobs.values().filter(|j| j.state == s).collect(),
            None => self.import_jobs.values().collect(),
        }
    }

    // =========================================================================
    // Security Group queries
    // =========================================================================

    pub fn get_security_group(&self, id: &str) -> Option<&SecurityGroupData> {
        self.security_groups.get(id)
    }

    pub fn list_security_groups(&self) -> Vec<&SecurityGroupData> {
        self.security_groups.values().collect()
    }

    pub fn list_security_groups_by_project(&self, project_id: &str) -> Vec<&SecurityGroupData> {
        self.security_groups
            .values()
            .filter(|sg| sg.project_id == project_id)
            .collect()
    }

    /// Ensure the idempotency cache is initialized (after deserialization)
    fn ensure_cache(&mut self) {
        if self.applied_requests.is_none() {
            self.applied_requests = Some(LruCache::new(NonZeroUsize::new(1000).unwrap()));
        }
    }
}

impl StateMachine<Command, Response> for ApiState {
    type Event = Event;

    fn apply(&mut self, cmd: Command) -> (Response, Vec<Self::Event>) {
        self.ensure_cache();

        // Check idempotency cache
        if let Some(cache) = &self.applied_requests
            && let Some(response) = cache.peek(cmd.request_id())
        {
            return (response.clone(), vec![]);
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
                // Check for duplicate name
                if self.nodes.values().any(|n| n.name == name) {
                    return (
                        Response::Error {
                            code: 409,
                            message: format!("Node with name '{}' already exists", name),
                        },
                        vec![],
                    );
                }

                // Check for duplicate ID (idempotency)
                if self.nodes.contains_key(&id) {
                    return (Response::Node(self.nodes.get(&id).unwrap().clone()), vec![]);
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

                self.nodes.insert(id, node.clone());
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
            } => match self.nodes.get(&node_id).cloned() {
                Some(old_node) => {
                    let node = self.nodes.get_mut(&node_id).unwrap();
                    node.status = status;
                    if let Some(res) = resources {
                        node.resources = res;
                    }
                    node.last_heartbeat = timestamp.clone();
                    node.updated_at = timestamp;
                    let new_node = node.clone();
                    (
                        Response::Node(new_node.clone()),
                        vec![Event::NodeUpdated {
                            id: node_id,
                            old: old_node,
                            new: new_node,
                        }],
                    )
                }
                None => (
                    Response::Error {
                        code: 404,
                        message: format!("Node '{}' not found", node_id),
                    },
                    vec![],
                ),
            },

            Command::DeregisterNode { node_id, .. } => match self.nodes.remove(&node_id) {
                Some(_) => (
                    Response::Deleted {
                        id: node_id.clone(),
                    },
                    vec![Event::NodeDeregistered { id: node_id }],
                ),
                None => (
                    Response::Error {
                        code: 404,
                        message: format!("Node '{}' not found", node_id),
                    },
                    vec![],
                ),
            },

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
                // Check for duplicate name
                if self.networks.values().any(|n| n.name == name) {
                    return (
                        Response::Error {
                            code: 409,
                            message: format!("Network with name '{}' already exists", name),
                        },
                        vec![],
                    );
                }

                // Check for duplicate ID (idempotency)
                if self.networks.contains_key(&id) {
                    return (
                        Response::Network(self.networks.get(&id).unwrap().clone()),
                        vec![],
                    );
                }

                // Use timestamp from command (set before Raft replication for determinism)
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

                self.networks.insert(id, network.clone());
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
            } => match self.networks.get(&id).cloned() {
                Some(old_network) => {
                    let network = self.networks.get_mut(&id).unwrap();
                    network.dns_servers = dns_servers;
                    network.ntp_servers = ntp_servers;
                    network.updated_at = timestamp; // Use timestamp from command for determinism
                    let new_network = network.clone();
                    (
                        Response::Network(new_network.clone()),
                        vec![Event::NetworkUpdated {
                            id,
                            old: old_network,
                            new: new_network,
                        }],
                    )
                }
                None => (
                    Response::Error {
                        code: 404,
                        message: format!("Network '{}' not found", id),
                    },
                    vec![],
                ),
            },

            Command::DeleteNetwork { id, force, .. } => {
                // Count NICs in this network
                let nics_in_network: Vec<String> = self
                    .nics
                    .iter()
                    .filter(|(_, n)| n.network_id == id)
                    .map(|(id, _)| id.clone())
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

                // Delete NICs if force
                let nics_deleted = nics_in_network.len() as u32;
                let mut events = Vec::new();
                for nic_id in nics_in_network {
                    if let Some(nic) = self.nics.remove(&nic_id) {
                        events.push(Event::NicDeleted {
                            id: nic_id,
                            network_id: nic.network_id,
                        });
                    }
                }

                match self.networks.remove(&id) {
                    Some(_) => {
                        events.push(Event::NetworkDeleted { id: id.clone() });
                        (Response::DeletedWithCount { id, nics_deleted }, events)
                    }
                    None => (
                        Response::Error {
                            code: 404,
                            message: format!("Network '{}' not found", id),
                        },
                        vec![],
                    ),
                }
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
                // Check network exists
                if !self.networks.contains_key(&network_id) {
                    return (
                        Response::Error {
                            code: 404,
                            message: format!("Network '{}' not found", network_id),
                        },
                        vec![],
                    );
                }

                // Check security group exists (if referenced)
                if let Some(ref sg_id) = security_group_id
                    && !self.security_groups.contains_key(sg_id)
                {
                    return (
                        Response::Error {
                            code: 404,
                            message: format!("Security group '{}' not found", sg_id),
                        },
                        vec![],
                    );
                }

                // Check for duplicate ID (idempotency)
                if self.nics.contains_key(&id) {
                    return (Response::Nic(self.nics.get(&id).unwrap().clone()), vec![]);
                }

                // Generate MAC if not provided - use id as seed for determinism
                let mac = mac_address.unwrap_or_else(|| generate_mac_from_id(&id));

                // Use timestamp from command for determinism
                let nic = NicData {
                    id: id.clone(),
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
                    socket_path: format!("/run/mvirt-net/nic-{}.sock", id),
                    state: NicStateData::Created,
                    created_at: timestamp.clone(),
                    updated_at: timestamp,
                };

                self.nics.insert(id, nic.clone());

                // Update network NIC count
                if let Some(network) = self.networks.get_mut(&network_id) {
                    network.nic_count += 1;
                }

                // Update security group NIC count
                if let Some(ref sg_id) = nic.security_group_id
                    && let Some(sg) = self.security_groups.get_mut(sg_id)
                {
                    sg.nic_count += 1;
                }

                (Response::Nic(nic.clone()), vec![Event::NicCreated(nic)])
            }

            Command::UpdateNic {
                id,
                timestamp,
                routed_ipv4_prefixes,
                routed_ipv6_prefixes,
                ..
            } => match self.nics.get(&id).cloned() {
                Some(old_nic) => {
                    let nic = self.nics.get_mut(&id).unwrap();
                    nic.routed_ipv4_prefixes = routed_ipv4_prefixes;
                    nic.routed_ipv6_prefixes = routed_ipv6_prefixes;
                    nic.updated_at = timestamp; // Use timestamp from command for determinism
                    let new_nic = nic.clone();
                    (
                        Response::Nic(new_nic.clone()),
                        vec![Event::NicUpdated {
                            id,
                            old: old_nic,
                            new: new_nic,
                        }],
                    )
                }
                None => (
                    Response::Error {
                        code: 404,
                        message: format!("NIC '{}' not found", id),
                    },
                    vec![],
                ),
            },

            Command::DeleteNic { id, .. } => match self.nics.remove(&id) {
                Some(nic) => {
                    // Update network NIC count
                    if let Some(network) = self.networks.get_mut(&nic.network_id) {
                        network.nic_count = network.nic_count.saturating_sub(1);
                    }
                    // Update security group NIC count
                    if let Some(ref sg_id) = nic.security_group_id
                        && let Some(sg) = self.security_groups.get_mut(sg_id)
                    {
                        sg.nic_count = sg.nic_count.saturating_sub(1);
                    }
                    let network_id = nic.network_id.clone();
                    (
                        Response::Deleted { id: id.clone() },
                        vec![Event::NicDeleted { id, network_id }],
                    )
                }
                None => (
                    Response::Error {
                        code: 404,
                        message: format!("NIC '{}' not found", id),
                    },
                    vec![],
                ),
            },

            Command::AttachNic {
                id,
                timestamp,
                vm_id,
                ..
            } => match self.nics.get(&id).cloned() {
                Some(old_nic) => {
                    if old_nic.vm_id.is_some() {
                        return (
                            Response::Error {
                                code: 409,
                                message: format!("NIC '{}' is already attached to a VM", id),
                            },
                            vec![],
                        );
                    }
                    // Verify VM exists
                    if !self.vms.contains_key(&vm_id) {
                        return (
                            Response::Error {
                                code: 404,
                                message: format!("VM '{}' not found", vm_id),
                            },
                            vec![],
                        );
                    }
                    let nic = self.nics.get_mut(&id).unwrap();
                    nic.vm_id = Some(vm_id);
                    nic.updated_at = timestamp;
                    let new_nic = nic.clone();
                    (
                        Response::Nic(new_nic.clone()),
                        vec![Event::NicUpdated {
                            id,
                            old: old_nic,
                            new: new_nic,
                        }],
                    )
                }
                None => (
                    Response::Error {
                        code: 404,
                        message: format!("NIC '{}' not found", id),
                    },
                    vec![],
                ),
            },

            Command::DetachNic { id, timestamp, .. } => match self.nics.get(&id).cloned() {
                Some(old_nic) => {
                    let nic = self.nics.get_mut(&id).unwrap();
                    nic.vm_id = None;
                    nic.updated_at = timestamp;
                    let new_nic = nic.clone();
                    (
                        Response::Nic(new_nic.clone()),
                        vec![Event::NicUpdated {
                            id,
                            old: old_nic,
                            new: new_nic,
                        }],
                    )
                }
                None => (
                    Response::Error {
                        code: 404,
                        message: format!("NIC '{}' not found", id),
                    },
                    vec![],
                ),
            },

            // =================================================================
            // VM Commands
            // =================================================================
            Command::CreateVm {
                id,
                timestamp,
                spec,
                ..
            } => {
                // Check for duplicate name
                if self.vms.values().any(|v| v.spec.name == spec.name) {
                    return (
                        Response::Error {
                            code: 409,
                            message: format!("VM with name '{}' already exists", spec.name),
                        },
                        vec![],
                    );
                }

                // Check for duplicate ID (idempotency)
                if self.vms.contains_key(&id) {
                    return (Response::Vm(self.vms.get(&id).unwrap().clone()), vec![]);
                }

                // Check NIC exists
                if !self.nics.contains_key(&spec.nic_id) {
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

                self.vms.insert(id, vm.clone());
                (Response::Vm(vm.clone()), vec![Event::VmCreated(vm)])
            }

            Command::UpdateVmSpec {
                id,
                timestamp,
                desired_state,
                ..
            } => match self.vms.get(&id).cloned() {
                Some(old_vm) => {
                    let vm = self.vms.get_mut(&id).unwrap();
                    vm.spec.desired_state = desired_state;
                    vm.updated_at = timestamp;
                    let new_vm = vm.clone();
                    (
                        Response::Vm(new_vm.clone()),
                        vec![Event::VmUpdated {
                            id,
                            old: old_vm,
                            new: new_vm,
                        }],
                    )
                }
                None => (
                    Response::Error {
                        code: 404,
                        message: format!("VM '{}' not found", id),
                    },
                    vec![],
                ),
            },

            Command::UpdateVmStatus {
                id,
                timestamp,
                status,
                ..
            } => match self.vms.get(&id).cloned() {
                Some(old_vm) => {
                    let vm = self.vms.get_mut(&id).unwrap();
                    vm.status = status;
                    vm.updated_at = timestamp;
                    let new_vm = vm.clone();
                    (
                        Response::Vm(new_vm.clone()),
                        vec![Event::VmStatusUpdated {
                            id,
                            old: old_vm,
                            new: new_vm,
                        }],
                    )
                }
                None => (
                    Response::Error {
                        code: 404,
                        message: format!("VM '{}' not found", id),
                    },
                    vec![],
                ),
            },

            Command::DeleteVm { id, .. } => match self.vms.remove(&id) {
                Some(_) => (
                    Response::Deleted { id: id.clone() },
                    vec![Event::VmDeleted { id }],
                ),
                None => (
                    Response::Error {
                        code: 404,
                        message: format!("VM '{}' not found", id),
                    },
                    vec![],
                ),
            },

            // =================================================================
            // Project Commands
            // =================================================================
            Command::CreateProject {
                id,
                timestamp,
                name,
                description,
                ..
            } => {
                // Check for duplicate name
                if self.projects.values().any(|p| p.name == name) {
                    return (
                        Response::Error {
                            code: 409,
                            message: format!("Project with name '{}' already exists", name),
                        },
                        vec![],
                    );
                }

                // Check for duplicate ID (idempotency)
                if self.projects.contains_key(&id) {
                    return (
                        Response::Project(self.projects.get(&id).unwrap().clone()),
                        vec![],
                    );
                }

                let project = ProjectData {
                    id: id.clone(),
                    name,
                    description,
                    created_at: timestamp,
                };

                self.projects.insert(id, project.clone());
                (Response::Project(project), vec![])
            }

            Command::DeleteProject { id, .. } => match self.projects.remove(&id) {
                Some(_) => (Response::Deleted { id }, vec![]),
                None => (
                    Response::Error {
                        code: 404,
                        message: format!("Project '{}' not found", id),
                    },
                    vec![],
                ),
            },

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
                // Check project exists
                if !self.projects.contains_key(&project_id) {
                    return (
                        Response::Error {
                            code: 404,
                            message: format!("Project '{}' not found", project_id),
                        },
                        vec![],
                    );
                }

                // Check for duplicate ID (idempotency)
                if self.volumes.contains_key(&id) {
                    return (
                        Response::Volume(self.volumes.get(&id).unwrap().clone()),
                        vec![],
                    );
                }

                // Check for duplicate name within project
                if self
                    .volumes
                    .values()
                    .any(|v| v.project_id == project_id && v.name == name)
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

                // If cloning from template, verify template exists on same node
                if let Some(ref tid) = template_id {
                    match self.templates.get(tid) {
                        Some(template) => {
                            if template.node_id != node_id {
                                return (
                                    Response::Error {
                                        code: 409,
                                        message: format!(
                                            "Template {} is on node {} but volume targets node {}",
                                            tid, template.node_id, node_id
                                        ),
                                    },
                                    vec![],
                                );
                            }
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
                    project_id,
                    node_id,
                    name,
                    path: format!("/dev/zvol/pool/vol-{}", id),
                    size_bytes,
                    used_bytes: 0,
                    compression_ratio: 1.0,
                    snapshots: vec![],
                    template_id,
                    created_at: timestamp.clone(),
                    updated_at: timestamp,
                };

                // Increment template clone count if cloning
                if let Some(tid) = &volume.template_id
                    && let Some(template) = self.templates.get_mut(tid)
                {
                    template.clone_count += 1;
                }

                self.volumes.insert(id, volume.clone());
                (Response::Volume(volume), vec![])
            }

            Command::DeleteVolume { id, .. } => match self.volumes.remove(&id) {
                Some(vol) => {
                    // Decrement template clone count if was cloned
                    if let Some(tid) = &vol.template_id
                        && let Some(template) = self.templates.get_mut(tid)
                    {
                        template.clone_count = template.clone_count.saturating_sub(1);
                    }
                    (Response::Deleted { id }, vec![])
                }
                None => (
                    Response::Error {
                        code: 404,
                        message: format!("Volume '{}' not found", id),
                    },
                    vec![],
                ),
            },

            Command::ResizeVolume {
                id,
                timestamp,
                size_bytes,
                ..
            } => match self.volumes.get_mut(&id) {
                Some(vol) => {
                    // Only allow growing, not shrinking
                    if size_bytes < vol.size_bytes {
                        return (
                            Response::Error {
                                code: 400,
                                message: "Cannot shrink volume".to_string(),
                            },
                            vec![],
                        );
                    }
                    vol.size_bytes = size_bytes;
                    vol.updated_at = timestamp;
                    (Response::Volume(vol.clone()), vec![])
                }
                None => (
                    Response::Error {
                        code: 404,
                        message: format!("Volume '{}' not found", id),
                    },
                    vec![],
                ),
            },

            Command::CreateSnapshot {
                id,
                timestamp,
                volume_id,
                name,
                ..
            } => match self.volumes.get_mut(&volume_id) {
                Some(vol) => {
                    // Check for duplicate snapshot name
                    if vol.snapshots.iter().any(|s| s.name == name) {
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
                    vol.snapshots.push(snapshot);
                    vol.updated_at = timestamp;
                    (Response::Volume(vol.clone()), vec![])
                }
                None => (
                    Response::Error {
                        code: 404,
                        message: format!("Volume '{}' not found", volume_id),
                    },
                    vec![],
                ),
            },

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
                ..
            } => {
                // Check for duplicate name
                if self.templates.values().any(|t| t.name == name) {
                    return (
                        Response::Error {
                            code: 409,
                            message: format!("Template with name '{}' already exists", name),
                        },
                        vec![],
                    );
                }

                // Check for duplicate ID (idempotency)
                if self.templates.contains_key(&id) {
                    return (
                        Response::Template(self.templates.get(&id).unwrap().clone()),
                        vec![],
                    );
                }

                let template = TemplateData {
                    id: id.clone(),
                    project_id,
                    node_id,
                    name,
                    size_bytes,
                    clone_count: 0,
                    created_at: timestamp,
                };

                self.templates.insert(id, template.clone());
                (Response::Template(template), vec![])
            }

            // =================================================================
            // Import Job Commands
            // =================================================================
            Command::CreateImportJob {
                id,
                timestamp,
                project_id,
                node_id,
                template_name,
                url,
                total_bytes,
                ..
            } => {
                // Check for duplicate ID (idempotency)
                if self.import_jobs.contains_key(&id) {
                    return (
                        Response::ImportJob(self.import_jobs.get(&id).unwrap().clone()),
                        vec![],
                    );
                }

                let job = ImportJobData {
                    id: id.clone(),
                    project_id,
                    node_id,
                    template_name,
                    url,
                    state: ImportJobState::Pending,
                    bytes_written: 0,
                    total_bytes,
                    error: None,
                    created_at: timestamp.clone(),
                    updated_at: timestamp,
                };

                self.import_jobs.insert(id, job.clone());
                (Response::ImportJob(job), vec![])
            }

            Command::UpdateImportJob {
                id,
                timestamp,
                bytes_written,
                state,
                error,
                ..
            } => match self.import_jobs.get_mut(&id) {
                Some(job) => {
                    job.bytes_written = bytes_written;
                    job.state = state;
                    job.error = error;
                    job.updated_at = timestamp;
                    (Response::ImportJob(job.clone()), vec![])
                }
                None => (
                    Response::Error {
                        code: 404,
                        message: format!("Import job '{}' not found", id),
                    },
                    vec![],
                ),
            },

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
                // Idempotent: return existing if same ID
                if let Some(existing) = self.security_groups.get(&id) {
                    return (Response::SecurityGroup(existing.clone()), vec![]);
                }

                // Duplicate name check within project
                if self
                    .security_groups
                    .values()
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

                // FK: project must exist
                if !self.projects.contains_key(&project_id) {
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
                self.security_groups.insert(id, sg.clone());
                (
                    Response::SecurityGroup(sg),
                    vec![Event::SecurityGroupCreated { id: sg_id }],
                )
            }

            Command::DeleteSecurityGroup { id, .. } => {
                // Check if any NICs reference this security group
                let nic_refs = self
                    .nics
                    .values()
                    .any(|n| n.security_group_id.as_deref() == Some(&id));
                if nic_refs {
                    return (
                        Response::Error {
                            code: 409,
                            message: format!("Security group '{}' is still referenced by NICs", id),
                        },
                        vec![],
                    );
                }

                match self.security_groups.remove(&id) {
                    Some(_) => (
                        Response::Deleted { id: id.clone() },
                        vec![Event::SecurityGroupDeleted { id }],
                    ),
                    None => (
                        Response::Error {
                            code: 404,
                            message: format!("Security group '{}' not found", id),
                        },
                        vec![],
                    ),
                }
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
            } => match self.security_groups.get_mut(&security_group_id) {
                Some(sg) => {
                    // Idempotent: return if rule ID already exists
                    if sg.rules.iter().any(|r| r.id == id) {
                        return (Response::SecurityGroup(sg.clone()), vec![]);
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
                    (Response::SecurityGroup(sg.clone()), vec![])
                }
                None => (
                    Response::Error {
                        code: 404,
                        message: format!("Security group '{}' not found", security_group_id),
                    },
                    vec![],
                ),
            },

            Command::DeleteSecurityGroupRule {
                security_group_id,
                rule_id,
                ..
            } => match self.security_groups.get_mut(&security_group_id) {
                Some(sg) => {
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
                    (Response::SecurityGroup(sg.clone()), vec![])
                }
                None => (
                    Response::Error {
                        code: 404,
                        message: format!("Security group '{}' not found", security_group_id),
                    },
                    vec![],
                ),
            },
        };

        // Cache the response
        if let Some(cache) = &mut self.applied_requests {
            cache.put(cmd.request_id().to_string(), response.clone());
        }

        (response, events)
    }
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

    /// Helper to get just the response from apply (ignoring events)
    fn apply(state: &mut ApiState, cmd: Command) -> Response {
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
                assert!(data.mac_address.starts_with("52:54:00:"));
                // Should have full MAC format
                assert_eq!(data.mac_address.matches(':').count(), 5);
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
                assert_eq!(data.routed_ipv4_prefixes, vec!["192.168.1.0/24"]);
                assert_eq!(data.routed_ipv6_prefixes, vec!["fd00::/64"]);
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
        assert!(net1_nics.iter().all(|n| n.network_id == "net-1"));

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
    // Project Tests
    // =========================================================================

    fn create_project_cmd(request_id: &str, id: &str, name: &str) -> Command {
        Command::CreateProject {
            request_id: request_id.to_string(),
            id: id.to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            name: name.to_string(),
            description: Some("Test project".to_string()),
        }
    }

    #[test]
    fn test_create_project() {
        let mut state = ApiState::default();
        let cmd = create_project_cmd("req-1", "proj-1", "my-project");
        let response = apply(&mut state, cmd);

        match response {
            Response::Project(data) => {
                assert_eq!(data.id, "proj-1");
                assert_eq!(data.name, "my-project");
                assert_eq!(data.description, Some("Test project".to_string()));
            }
            other => panic!("Unexpected response: {:?}", other),
        }

        assert!(state.get_project("proj-1").is_some());
        assert_eq!(state.list_projects().len(), 1);
    }

    #[test]
    fn test_create_project_duplicate_name() {
        let mut state = ApiState::default();

        apply(
            &mut state,
            create_project_cmd("req-1", "proj-1", "dup-name"),
        );

        let response = apply(
            &mut state,
            create_project_cmd("req-2", "proj-2", "dup-name"),
        );
        assert!(matches!(response, Response::Error { code: 409, .. }));
    }

    #[test]
    fn test_delete_project() {
        let mut state = ApiState::default();
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
    fn test_get_project_by_name() {
        let mut state = ApiState::default();
        apply(
            &mut state,
            create_project_cmd("req-1", "proj-uuid", "findme"),
        );

        let project = state.get_project_by_name("findme");
        assert!(project.is_some());
        assert_eq!(project.unwrap().id, "proj-uuid");

        assert!(state.get_project_by_name("unknown").is_none());
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
                assert_eq!(data.project_id, "proj-1");
                assert_eq!(data.node_id, "node-1");
                assert_eq!(data.name, "my-volume");
                assert_eq!(data.size_bytes, 10_000_000);
                assert!(data.path.contains("vol-1"));
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
                assert_eq!(data.size_bytes, 2000);
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
                assert_eq!(data.snapshots.len(), 1);
                assert_eq!(data.snapshots[0].name, "my-snapshot");
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
                assert_eq!(data.node_id, "node-1");
                assert_eq!(data.name, "ubuntu-22.04");
                assert_eq!(data.clone_count, 0);
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
        assert_eq!(template.clone_count, 1);
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

        assert_eq!(state.get_template("tmpl-1").unwrap().clone_count, 1);

        // Delete volume
        apply(
            &mut state,
            Command::DeleteVolume {
                request_id: "req-4".to_string(),
                id: "vol-1".to_string(),
            },
        );

        assert_eq!(state.get_template("tmpl-1").unwrap().clone_count, 0);
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
    // Import Job Tests
    // =========================================================================

    #[test]
    fn test_create_import_job() {
        let mut state = ApiState::default();

        let cmd = Command::CreateImportJob {
            request_id: "req-1".to_string(),
            id: "job-1".to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            project_id: "test-project".to_string(),
            node_id: "node-1".to_string(),
            template_name: "ubuntu-22.04".to_string(),
            url: "https://example.com/ubuntu.qcow2".to_string(),
            total_bytes: 2_000_000_000,
        };
        let response = apply(&mut state, cmd);

        match response {
            Response::ImportJob(data) => {
                assert_eq!(data.id, "job-1");
                assert_eq!(data.state, ImportJobState::Pending);
                assert_eq!(data.bytes_written, 0);
                assert_eq!(data.total_bytes, 2_000_000_000);
            }
            other => panic!("Unexpected response: {:?}", other),
        }

        assert!(state.get_import_job("job-1").is_some());
    }

    #[test]
    fn test_update_import_job_progress() {
        let mut state = ApiState::default();

        // Create import job
        apply(
            &mut state,
            Command::CreateImportJob {
                request_id: "req-1".to_string(),
                id: "job-1".to_string(),
                timestamp: "2024-01-01T00:00:00Z".to_string(),
                project_id: "test-project".to_string(),
                node_id: "node-1".to_string(),
                template_name: "ubuntu".to_string(),
                url: "https://example.com/ubuntu.qcow2".to_string(),
                total_bytes: 1000,
            },
        );

        // Update to running with progress
        let update_cmd = Command::UpdateImportJob {
            request_id: "req-2".to_string(),
            id: "job-1".to_string(),
            timestamp: "2024-01-01T00:00:01Z".to_string(),
            bytes_written: 500,
            state: ImportJobState::Running,
            error: None,
        };
        let response = apply(&mut state, update_cmd);

        match response {
            Response::ImportJob(data) => {
                assert_eq!(data.state, ImportJobState::Running);
                assert_eq!(data.bytes_written, 500);
            }
            other => panic!("Unexpected response: {:?}", other),
        }
    }

    #[test]
    fn test_update_import_job_completed() {
        let mut state = ApiState::default();

        apply(
            &mut state,
            Command::CreateImportJob {
                request_id: "req-1".to_string(),
                id: "job-1".to_string(),
                timestamp: "2024-01-01T00:00:00Z".to_string(),
                project_id: "test-project".to_string(),
                node_id: "node-1".to_string(),
                template_name: "ubuntu".to_string(),
                url: "https://example.com/ubuntu.qcow2".to_string(),
                total_bytes: 1000,
            },
        );

        // Complete the job
        let update_cmd = Command::UpdateImportJob {
            request_id: "req-2".to_string(),
            id: "job-1".to_string(),
            timestamp: "2024-01-01T00:00:01Z".to_string(),
            bytes_written: 1000,
            state: ImportJobState::Completed,
            error: None,
        };
        let response = apply(&mut state, update_cmd);

        match response {
            Response::ImportJob(data) => {
                assert_eq!(data.state, ImportJobState::Completed);
                assert_eq!(data.bytes_written, 1000);
            }
            other => panic!("Unexpected response: {:?}", other),
        }
    }

    #[test]
    fn test_update_import_job_failed() {
        let mut state = ApiState::default();

        apply(
            &mut state,
            Command::CreateImportJob {
                request_id: "req-1".to_string(),
                id: "job-1".to_string(),
                timestamp: "2024-01-01T00:00:00Z".to_string(),
                project_id: "test-project".to_string(),
                node_id: "node-1".to_string(),
                template_name: "ubuntu".to_string(),
                url: "https://example.com/ubuntu.qcow2".to_string(),
                total_bytes: 1000,
            },
        );

        // Fail the job
        let update_cmd = Command::UpdateImportJob {
            request_id: "req-2".to_string(),
            id: "job-1".to_string(),
            timestamp: "2024-01-01T00:00:01Z".to_string(),
            bytes_written: 100,
            state: ImportJobState::Failed,
            error: Some("Download failed: connection reset".to_string()),
        };
        let response = apply(&mut state, update_cmd);

        match response {
            Response::ImportJob(data) => {
                assert_eq!(data.state, ImportJobState::Failed);
                assert_eq!(
                    data.error,
                    Some("Download failed: connection reset".to_string())
                );
            }
            other => panic!("Unexpected response: {:?}", other),
        }
    }

    #[test]
    fn test_update_import_job_not_found() {
        let mut state = ApiState::default();

        let update_cmd = Command::UpdateImportJob {
            request_id: "req-1".to_string(),
            id: "non-existent".to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            bytes_written: 0,
            state: ImportJobState::Running,
            error: None,
        };
        let response = apply(&mut state, update_cmd);

        assert!(matches!(response, Response::Error { code: 404, .. }));
    }
}
