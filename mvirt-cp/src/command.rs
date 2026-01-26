use serde::{Deserialize, Serialize};

/// Commands that can be replicated through Raft
///
/// IMPORTANT: All timestamps must be set BEFORE the command is submitted to Raft.
/// Using `Utc::now()` inside the state machine's `apply()` breaks Raft's determinism
/// guarantee - different nodes would compute different timestamps, causing state divergence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Command {
    // Network operations
    CreateNetwork {
        request_id: String,
        /// Pre-generated ID for the network (deterministic across nodes)
        id: String,
        /// Timestamp when command was created (set before Raft replication)
        timestamp: String,
        name: String,
        ipv4_enabled: bool,
        ipv4_subnet: Option<String>,
        ipv6_enabled: bool,
        ipv6_prefix: Option<String>,
        dns_servers: Vec<String>,
        ntp_servers: Vec<String>,
        is_public: bool,
    },
    UpdateNetwork {
        request_id: String,
        id: String,
        /// Timestamp when command was created (set before Raft replication)
        timestamp: String,
        dns_servers: Vec<String>,
        ntp_servers: Vec<String>,
    },
    DeleteNetwork {
        request_id: String,
        id: String,
        force: bool,
    },

    // NIC operations
    CreateNic {
        request_id: String,
        /// Pre-generated ID for the NIC (deterministic across nodes)
        id: String,
        /// Timestamp when command was created (set before Raft replication)
        timestamp: String,
        network_id: String,
        name: Option<String>,
        mac_address: Option<String>,
        ipv4_address: Option<String>,
        ipv6_address: Option<String>,
        routed_ipv4_prefixes: Vec<String>,
        routed_ipv6_prefixes: Vec<String>,
    },
    UpdateNic {
        request_id: String,
        id: String,
        /// Timestamp when command was created (set before Raft replication)
        timestamp: String,
        routed_ipv4_prefixes: Vec<String>,
        routed_ipv6_prefixes: Vec<String>,
    },
    DeleteNic {
        request_id: String,
        id: String,
    },
}

impl Command {
    pub fn request_id(&self) -> &str {
        match self {
            Command::CreateNetwork { request_id, .. } => request_id,
            Command::UpdateNetwork { request_id, .. } => request_id,
            Command::DeleteNetwork { request_id, .. } => request_id,
            Command::CreateNic { request_id, .. } => request_id,
            Command::UpdateNic { request_id, .. } => request_id,
            Command::DeleteNic { request_id, .. } => request_id,
        }
    }
}

/// Network data stored in the state machine
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkData {
    pub id: String,
    pub name: String,
    pub ipv4_enabled: bool,
    pub ipv4_subnet: Option<String>,
    pub ipv6_enabled: bool,
    pub ipv6_prefix: Option<String>,
    pub dns_servers: Vec<String>,
    pub ntp_servers: Vec<String>,
    pub is_public: bool,
    pub nic_count: u32,
    pub created_at: String,
    pub updated_at: String,
}

/// NIC data stored in the state machine
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NicData {
    pub id: String,
    pub name: Option<String>,
    pub network_id: String,
    pub mac_address: String,
    pub ipv4_address: Option<String>,
    pub ipv6_address: Option<String>,
    pub routed_ipv4_prefixes: Vec<String>,
    pub routed_ipv6_prefixes: Vec<String>,
    pub socket_path: String,
    pub state: NicStateData,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum NicStateData {
    Created,
    Active,
    Error,
}

/// Response from applying a command
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Response {
    Network(NetworkData),
    Nic(NicData),
    Deleted { id: String },
    DeletedWithCount { id: String, nics_deleted: u32 },
    Error { code: u32, message: String },
}

impl Default for Response {
    fn default() -> Self {
        Response::Error {
            code: 0,
            message: "No response".to_string(),
        }
    }
}
