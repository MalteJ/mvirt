//! Control Plane state machine.

use lru::LruCache;
use mraft::StateMachine;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::num::NonZeroUsize;

use crate::command::{Command, NetworkData, NicData, NicStateData, Response};
use crate::store::Event;

/// Control Plane state - replicated across all nodes via Raft.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CpState {
    pub networks: HashMap<String, NetworkData>,
    pub nics: HashMap<String, NicData>,
    /// Idempotency cache for request deduplication
    #[serde(skip)]
    applied_requests: Option<LruCache<String, Response>>,
}

impl Default for CpState {
    fn default() -> Self {
        Self {
            networks: HashMap::new(),
            nics: HashMap::new(),
            applied_requests: Some(LruCache::new(NonZeroUsize::new(1000).unwrap())),
        }
    }
}

impl CpState {
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

    /// Ensure the idempotency cache is initialized (after deserialization)
    fn ensure_cache(&mut self) {
        if self.applied_requests.is_none() {
            self.applied_requests = Some(LruCache::new(NonZeroUsize::new(1000).unwrap()));
        }
    }
}

impl StateMachine<Command, Response> for CpState {
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
            Command::CreateNetwork {
                id,
                timestamp,
                name,
                ipv4_enabled,
                ipv4_subnet,
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
                    name,
                    ipv4_enabled,
                    ipv4_subnet,
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
                network_id,
                name,
                mac_address,
                ipv4_address,
                ipv6_address,
                routed_ipv4_prefixes,
                routed_ipv6_prefixes,
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

                // Check for duplicate ID (idempotency)
                if self.nics.contains_key(&id) {
                    return (Response::Nic(self.nics.get(&id).unwrap().clone()), vec![]);
                }

                // Generate MAC if not provided - use id as seed for determinism
                let mac = mac_address.unwrap_or_else(|| generate_mac_from_id(&id));

                // Use timestamp from command for determinism
                let nic = NicData {
                    id: id.clone(),
                    name,
                    network_id: network_id.clone(),
                    mac_address: mac,
                    ipv4_address,
                    ipv6_address,
                    routed_ipv4_prefixes,
                    routed_ipv6_prefixes,
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
    fn apply(state: &mut CpState, cmd: Command) -> Response {
        let (response, _events) = state.apply(cmd);
        response
    }

    fn create_network_cmd(request_id: &str, id: &str, name: &str) -> Command {
        Command::CreateNetwork {
            request_id: request_id.to_string(),
            id: id.to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(), // Fixed timestamp for deterministic tests
            name: name.to_string(),
            ipv4_enabled: true,
            ipv4_subnet: Some("10.0.0.0/24".to_string()),
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
            timestamp: "2024-01-01T00:00:00Z".to_string(), // Fixed timestamp for deterministic tests
            network_id: network_id.to_string(),
            name: name.map(|s| s.to_string()),
            mac_address: None,
            ipv4_address: None,
            ipv6_address: None,
            routed_ipv4_prefixes: vec![],
            routed_ipv6_prefixes: vec![],
        }
    }

    #[test]
    fn test_create_network() {
        let mut state = CpState::default();
        let cmd = create_network_cmd("req-1", "net-1", "test-network");
        let response = apply(&mut state, cmd);

        match response {
            Response::Network(data) => {
                assert_eq!(data.id, "net-1");
                assert_eq!(data.name, "test-network");
                assert!(data.ipv4_enabled);
                assert_eq!(data.ipv4_subnet, Some("10.0.0.0/24".to_string()));
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
        let mut state = CpState::default();

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
        let mut state = CpState::default();

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
        let mut state = CpState::default();

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
        let mut state = CpState::default();

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
        let mut state = CpState::default();

        // Create network
        apply(&mut state, create_network_cmd("req-1", "net-1", "test-net"));

        // Create NIC in network
        apply(&mut state, create_nic_cmd("req-2", "nic-1", "net-1", Some("my-nic")));

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
        let mut state = CpState::default();

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
        let mut state = CpState::default();

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
        let mut state = CpState::default();

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
        let mut state = CpState::default();

        apply(&mut state, create_network_cmd("req-1", "net-uuid-123", "my-network"));

        // Find by name
        let network = state.get_network_by_name("my-network");
        assert!(network.is_some());
        assert_eq!(network.unwrap().id, "net-uuid-123");

        // Non-existent name
        assert!(state.get_network_by_name("unknown").is_none());
    }

    #[test]
    fn test_get_nic_by_name() {
        let mut state = CpState::default();

        apply(&mut state, create_network_cmd("req-1", "net-1", "test-net"));
        apply(&mut state, create_nic_cmd(
            "req-2",
            "nic-uuid-456",
            "net-1",
            Some("my-nic"),
        ));

        // Find by name
        let nic = state.get_nic_by_name("my-nic");
        assert!(nic.is_some());
        assert_eq!(nic.unwrap().id, "nic-uuid-456");

        // Non-existent name
        assert!(state.get_nic_by_name("unknown").is_none());
    }

    #[test]
    fn test_create_nic_network_not_found() {
        let mut state = CpState::default();

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
        let mut state = CpState::default();

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
        let mut state = CpState::default();

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
        let mut state = CpState::default();

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
        let mut state = CpState::default();

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
        let mut state = CpState::default();

        // Create two networks
        apply(&mut state, create_network_cmd("req-1", "net-1", "network-1"));
        apply(&mut state, create_network_cmd("req-2", "net-2", "network-2"));

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
        let mut state = CpState::default();

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
        let mut state = CpState::default();

        let delete_cmd = Command::DeleteNic {
            request_id: "req-1".to_string(),
            id: "non-existent".to_string(),
        };
        let response = apply(&mut state, delete_cmd);

        assert!(matches!(response, Response::Error { code: 404, .. }));
    }
}
