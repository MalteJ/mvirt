//! VM Scheduler - selects nodes for VM placement.
//!
//! The scheduler considers:
//! - Node availability (online status)
//! - Resource capacity (CPU, memory, storage)
//! - Node selector constraints (if specified in VM spec)
//! - Load balancing (prefer nodes with more available resources)

use crate::command::{NodeData, NodeStatus, VmSpec};

/// Scheduler for VM placement decisions.
pub struct Scheduler;

/// Result of scheduling a VM.
#[derive(Debug, Clone)]
pub struct ScheduleResult {
    /// Selected node ID.
    pub node_id: String,
    /// Reason for selection.
    pub reason: String,
}

/// Error when scheduling fails.
#[derive(Debug, Clone)]
pub enum ScheduleError {
    /// No nodes are available.
    NoNodesAvailable,
    /// No nodes match the selector.
    NoMatchingNodes { selector: String },
    /// No nodes have sufficient resources.
    InsufficientResources {
        required_cpu: u32,
        required_memory: u64,
        required_storage: u64,
    },
}

impl std::fmt::Display for ScheduleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScheduleError::NoNodesAvailable => write!(f, "No nodes available for scheduling"),
            ScheduleError::NoMatchingNodes { selector } => {
                write!(f, "No nodes match selector: {}", selector)
            }
            ScheduleError::InsufficientResources {
                required_cpu,
                required_memory,
                required_storage,
            } => {
                write!(
                    f,
                    "No nodes have sufficient resources (need {} CPU, {}MB RAM, {}GB storage)",
                    required_cpu, required_memory, required_storage
                )
            }
        }
    }
}

impl std::error::Error for ScheduleError {}

impl Scheduler {
    /// Create a new scheduler.
    pub fn new() -> Self {
        Self
    }

    /// Select the best node for a VM.
    ///
    /// Selection criteria (in order):
    /// 1. Node must be online
    /// 2. Node must match selector (if specified)
    /// 3. Node must have sufficient resources
    /// 4. Prefer node with most available memory (load balancing)
    pub fn select_node(
        &self,
        nodes: &[NodeData],
        spec: &VmSpec,
    ) -> Result<ScheduleResult, ScheduleError> {
        // Filter to online nodes only
        let online_nodes: Vec<_> = nodes
            .iter()
            .filter(|n| n.status == NodeStatus::Online)
            .collect();

        if online_nodes.is_empty() {
            return Err(ScheduleError::NoNodesAvailable);
        }

        // Apply node selector filter if specified
        let candidates: Vec<_> = if let Some(ref selector) = spec.node_selector {
            let filtered: Vec<_> = online_nodes
                .into_iter()
                .filter(|n| self.matches_selector(n, selector))
                .collect();

            if filtered.is_empty() {
                return Err(ScheduleError::NoMatchingNodes {
                    selector: selector.clone(),
                });
            }
            filtered
        } else {
            online_nodes
        };

        // Filter by resource requirements
        let with_resources: Vec<_> = candidates
            .into_iter()
            .filter(|n| self.has_sufficient_resources(n, spec))
            .collect();

        if with_resources.is_empty() {
            return Err(ScheduleError::InsufficientResources {
                required_cpu: spec.cpu_cores,
                required_memory: spec.memory_mb,
                required_storage: spec.disk_gb,
            });
        }

        // Select node with most available memory (simple load balancing)
        let best_node = with_resources
            .into_iter()
            .max_by_key(|n| n.resources.available_memory_mb)
            .expect("with_resources is not empty");

        Ok(ScheduleResult {
            node_id: best_node.id.clone(),
            reason: format!(
                "Selected node {} with {}MB available memory",
                best_node.name, best_node.resources.available_memory_mb
            ),
        })
    }

    /// Check if a node matches the selector.
    ///
    /// The selector can be:
    /// - A node ID (exact match)
    /// - A node name (exact match)
    /// - A label selector in format "key=value"
    fn matches_selector(&self, node: &NodeData, selector: &str) -> bool {
        // Check if selector matches node ID
        if node.id == selector {
            return true;
        }

        // Check if selector matches node name
        if node.name == selector {
            return true;
        }

        // Check if selector is a label (key=value format)
        if let Some((key, value)) = selector.split_once('=')
            && let Some(label_value) = node.labels.get(key)
        {
            return label_value == value;
        }

        false
    }

    /// Check if a node has sufficient resources for the VM.
    fn has_sufficient_resources(&self, node: &NodeData, spec: &VmSpec) -> bool {
        node.resources.available_cpu_cores >= spec.cpu_cores
            && node.resources.available_memory_mb >= spec.memory_mb
            && node.resources.available_storage_gb >= spec.disk_gb
    }
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{NodeResources, VmDesiredState};
    use std::collections::HashMap;

    fn make_node(id: &str, name: &str, status: NodeStatus, available_memory: u64) -> NodeData {
        NodeData {
            id: id.to_string(),
            name: name.to_string(),
            address: format!("{}:6001", name),
            status,
            resources: NodeResources {
                cpu_cores: 8,
                memory_mb: 16384,
                storage_gb: 500,
                available_cpu_cores: 4,
                available_memory_mb: available_memory,
                available_storage_gb: 200,
            },
            labels: HashMap::new(),
            last_heartbeat: "2024-01-01T00:00:00Z".to_string(),
            created_at: "2024-01-01T00:00:00Z".to_string(),
            updated_at: "2024-01-01T00:00:00Z".to_string(),
        }
    }

    fn make_spec(cpu: u32, memory: u64, storage: u64) -> VmSpec {
        VmSpec {
            name: "test-vm".to_string(),
            node_selector: None,
            cpu_cores: cpu,
            memory_mb: memory,
            disk_gb: storage,
            network_id: "net-1".to_string(),
            nic_id: None,
            image: "ubuntu:22.04".to_string(),
            desired_state: VmDesiredState::Running,
        }
    }

    #[test]
    fn test_select_node_prefers_most_available_memory() {
        let scheduler = Scheduler::new();
        let nodes = vec![
            make_node("node-1", "host1", NodeStatus::Online, 4096),
            make_node("node-2", "host2", NodeStatus::Online, 8192),
            make_node("node-3", "host3", NodeStatus::Online, 2048),
        ];
        let spec = make_spec(1, 1024, 10);

        let result = scheduler.select_node(&nodes, &spec).unwrap();
        assert_eq!(result.node_id, "node-2"); // Most available memory
    }

    #[test]
    fn test_select_node_filters_offline() {
        let scheduler = Scheduler::new();
        let nodes = vec![
            make_node("node-1", "host1", NodeStatus::Offline, 8192),
            make_node("node-2", "host2", NodeStatus::Online, 4096),
        ];
        let spec = make_spec(1, 1024, 10);

        let result = scheduler.select_node(&nodes, &spec).unwrap();
        assert_eq!(result.node_id, "node-2"); // Only online node
    }

    #[test]
    fn test_select_node_respects_selector() {
        let scheduler = Scheduler::new();
        let nodes = vec![
            make_node("node-1", "host1", NodeStatus::Online, 8192),
            make_node("node-2", "host2", NodeStatus::Online, 4096),
        ];
        let mut spec = make_spec(1, 1024, 10);
        spec.node_selector = Some("host2".to_string());

        let result = scheduler.select_node(&nodes, &spec).unwrap();
        assert_eq!(result.node_id, "node-2"); // Matches selector
    }

    #[test]
    fn test_select_node_insufficient_resources() {
        let scheduler = Scheduler::new();
        let nodes = vec![make_node("node-1", "host1", NodeStatus::Online, 1024)];
        let spec = make_spec(1, 8192, 10); // Needs 8GB RAM

        let result = scheduler.select_node(&nodes, &spec);
        assert!(matches!(
            result,
            Err(ScheduleError::InsufficientResources { .. })
        ));
    }

    #[test]
    fn test_select_node_no_nodes() {
        let scheduler = Scheduler::new();
        let nodes: Vec<NodeData> = vec![];
        let spec = make_spec(1, 1024, 10);

        let result = scheduler.select_node(&nodes, &spec);
        assert!(matches!(result, Err(ScheduleError::NoNodesAvailable)));
    }

    #[test]
    fn test_select_node_label_selector() {
        let scheduler = Scheduler::new();
        let mut node1 = make_node("node-1", "host1", NodeStatus::Online, 8192);
        node1
            .labels
            .insert("zone".to_string(), "us-west".to_string());

        let mut node2 = make_node("node-2", "host2", NodeStatus::Online, 4096);
        node2
            .labels
            .insert("zone".to_string(), "us-east".to_string());

        let nodes = vec![node1, node2];
        let mut spec = make_spec(1, 1024, 10);
        spec.node_selector = Some("zone=us-east".to_string());

        let result = scheduler.select_node(&nodes, &spec).unwrap();
        assert_eq!(result.node_id, "node-2"); // Matches label
    }
}
