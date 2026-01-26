//! Control Plane state machine.

use lru::LruCache;
use mraft::StateMachine;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::num::NonZeroUsize;

use crate::command::{Command, NetworkData, NicData, NicStateData, Response};

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
    fn apply(&mut self, cmd: Command) -> Response {
        self.ensure_cache();

        // Check idempotency cache
        if let Some(cache) = &self.applied_requests
            && let Some(response) = cache.peek(cmd.request_id())
        {
            return response.clone();
        }

        let response = match cmd.clone() {
            Command::CreateNetwork {
                id,
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
                    return Response::Error {
                        code: 409,
                        message: format!("Network with name '{}' already exists", name),
                    };
                }

                // Check for duplicate ID (idempotency)
                if self.networks.contains_key(&id) {
                    return Response::Network(self.networks.get(&id).unwrap().clone());
                }

                let now = chrono::Utc::now().to_rfc3339();

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
                    created_at: now.clone(),
                    updated_at: now,
                };

                self.networks.insert(id, network.clone());
                Response::Network(network)
            }

            Command::UpdateNetwork {
                id,
                dns_servers,
                ntp_servers,
                ..
            } => match self.networks.get_mut(&id) {
                Some(network) => {
                    network.dns_servers = dns_servers;
                    network.ntp_servers = ntp_servers;
                    network.updated_at = chrono::Utc::now().to_rfc3339();
                    Response::Network(network.clone())
                }
                None => Response::Error {
                    code: 404,
                    message: format!("Network '{}' not found", id),
                },
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
                    return Response::Error {
                        code: 409,
                        message: format!(
                            "Network has {} NICs, use force=true to delete",
                            nics_in_network.len()
                        ),
                    };
                }

                // Delete NICs if force
                let nics_deleted = nics_in_network.len() as u32;
                for nic_id in nics_in_network {
                    self.nics.remove(&nic_id);
                }

                match self.networks.remove(&id) {
                    Some(_) => Response::DeletedWithCount { id, nics_deleted },
                    None => Response::Error {
                        code: 404,
                        message: format!("Network '{}' not found", id),
                    },
                }
            }

            Command::CreateNic {
                id,
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
                    return Response::Error {
                        code: 404,
                        message: format!("Network '{}' not found", network_id),
                    };
                }

                // Check for duplicate ID (idempotency)
                if self.nics.contains_key(&id) {
                    return Response::Nic(self.nics.get(&id).unwrap().clone());
                }

                let now = chrono::Utc::now().to_rfc3339();

                // Generate MAC if not provided - use id as seed for determinism
                let mac = mac_address.unwrap_or_else(|| generate_mac_from_id(&id));

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
                    created_at: now.clone(),
                    updated_at: now,
                };

                self.nics.insert(id, nic.clone());

                // Update network NIC count
                if let Some(network) = self.networks.get_mut(&network_id) {
                    network.nic_count += 1;
                }

                Response::Nic(nic)
            }

            Command::UpdateNic {
                id,
                routed_ipv4_prefixes,
                routed_ipv6_prefixes,
                ..
            } => match self.nics.get_mut(&id) {
                Some(nic) => {
                    nic.routed_ipv4_prefixes = routed_ipv4_prefixes;
                    nic.routed_ipv6_prefixes = routed_ipv6_prefixes;
                    nic.updated_at = chrono::Utc::now().to_rfc3339();
                    Response::Nic(nic.clone())
                }
                None => Response::Error {
                    code: 404,
                    message: format!("NIC '{}' not found", id),
                },
            },

            Command::DeleteNic { id, .. } => match self.nics.remove(&id) {
                Some(nic) => {
                    // Update network NIC count
                    if let Some(network) = self.networks.get_mut(&nic.network_id) {
                        network.nic_count = network.nic_count.saturating_sub(1);
                    }
                    Response::Deleted { id }
                }
                None => Response::Error {
                    code: 404,
                    message: format!("NIC '{}' not found", id),
                },
            },
        };

        // Cache the response
        if let Some(cache) = &mut self.applied_requests {
            cache.put(cmd.request_id().to_string(), response.clone());
        }

        response
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
