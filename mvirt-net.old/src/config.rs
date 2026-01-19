use chrono::Utc;
use uuid::Uuid;

/// Network entry stored in SQLite
#[derive(Debug, Clone)]
pub struct NetworkEntry {
    pub id: String,
    pub name: String,
    pub ipv4_enabled: bool,
    pub ipv4_subnet: Option<String>,
    pub ipv6_enabled: bool,
    pub ipv6_prefix: Option<String>,
    pub dns_servers: Vec<String>,
    pub ntp_servers: Vec<String>,
    pub is_public: bool,
    pub created_at: String,
    pub updated_at: String,
}

impl NetworkEntry {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        name: String,
        ipv4_enabled: bool,
        ipv4_subnet: Option<String>,
        ipv6_enabled: bool,
        ipv6_prefix: Option<String>,
        dns_servers: Vec<String>,
        ntp_servers: Vec<String>,
        is_public: bool,
    ) -> Self {
        let now = Utc::now().to_rfc3339();
        Self {
            id: Uuid::new_v4().to_string(),
            name,
            ipv4_enabled,
            ipv4_subnet,
            ipv6_enabled,
            ipv6_prefix,
            dns_servers,
            ntp_servers,
            is_public,
            created_at: now.clone(),
            updated_at: now,
        }
    }
}

/// NIC entry stored in SQLite
#[derive(Debug, Clone)]
pub struct NicEntry {
    pub id: String,
    pub name: Option<String>,
    pub network_id: String,
    pub mac_address: String,
    pub ipv4_address: Option<String>,
    pub ipv6_address: Option<String>,
    pub routed_ipv4_prefixes: Vec<String>,
    pub routed_ipv6_prefixes: Vec<String>,
    pub socket_path: String,
    pub state: NicState,
    pub created_at: String,
    pub updated_at: String,
}

/// Builder for creating NicEntry
pub struct NicEntryBuilder {
    name: Option<String>,
    network_id: String,
    mac_address: String,
    ipv4_address: Option<String>,
    ipv6_address: Option<String>,
    routed_ipv4_prefixes: Vec<String>,
    routed_ipv6_prefixes: Vec<String>,
    socket_path: String,
}

impl NicEntryBuilder {
    pub fn new(network_id: String, mac_address: String, socket_path: String) -> Self {
        Self {
            name: None,
            network_id,
            mac_address,
            ipv4_address: None,
            ipv6_address: None,
            routed_ipv4_prefixes: Vec::new(),
            routed_ipv6_prefixes: Vec::new(),
            socket_path,
        }
    }

    pub fn name(mut self, name: Option<String>) -> Self {
        self.name = name;
        self
    }

    pub fn ipv4_address(mut self, addr: Option<String>) -> Self {
        self.ipv4_address = addr;
        self
    }

    pub fn ipv6_address(mut self, addr: Option<String>) -> Self {
        self.ipv6_address = addr;
        self
    }

    pub fn routed_ipv4_prefixes(mut self, prefixes: Vec<String>) -> Self {
        self.routed_ipv4_prefixes = prefixes;
        self
    }

    pub fn routed_ipv6_prefixes(mut self, prefixes: Vec<String>) -> Self {
        self.routed_ipv6_prefixes = prefixes;
        self
    }

    pub fn build(self) -> NicEntry {
        let now = Utc::now().to_rfc3339();
        NicEntry {
            id: Uuid::new_v4().to_string(),
            name: self.name,
            network_id: self.network_id,
            mac_address: self.mac_address,
            ipv4_address: self.ipv4_address,
            ipv6_address: self.ipv6_address,
            routed_ipv4_prefixes: self.routed_ipv4_prefixes,
            routed_ipv6_prefixes: self.routed_ipv6_prefixes,
            socket_path: self.socket_path,
            state: NicState::Created,
            created_at: now.clone(),
            updated_at: now,
        }
    }
}

/// NIC state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NicState {
    Created,
    Active,
    Error,
}

impl NicState {
    pub fn as_str(&self) -> &'static str {
        match self {
            NicState::Created => "created",
            NicState::Active => "active",
            NicState::Error => "error",
        }
    }
}

impl std::str::FromStr for NicState {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "created" => Ok(NicState::Created),
            "active" => Ok(NicState::Active),
            "error" => Ok(NicState::Error),
            _ => Err(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nic_state_roundtrip() {
        for state in [NicState::Created, NicState::Active, NicState::Error] {
            let s = state.as_str();
            let parsed: NicState = s.parse().unwrap();
            assert_eq!(state, parsed);
        }
    }

    #[test]
    fn test_nic_state_invalid() {
        assert!("invalid".parse::<NicState>().is_err());
    }

    #[test]
    fn test_network_entry_new() {
        let entry = NetworkEntry::new(
            "test-net".to_string(),
            true,
            Some("10.0.0.0/24".to_string()),
            false,
            None,
            vec!["1.1.1.1".to_string()],
            vec![],
            false,
        );

        assert_eq!(entry.name, "test-net");
        assert!(entry.ipv4_enabled);
        assert_eq!(entry.ipv4_subnet, Some("10.0.0.0/24".to_string()));
        assert!(!entry.ipv6_enabled);
        assert!(!entry.is_public);
        assert!(!entry.id.is_empty());
    }

    #[test]
    fn test_nic_entry_builder() {
        let entry = NicEntryBuilder::new(
            "net-123".to_string(),
            "52:54:00:12:34:56".to_string(),
            "/tmp/test.sock".to_string(),
        )
        .name(Some("eth0".to_string()))
        .ipv4_address(Some("10.0.0.5".to_string()))
        .routed_ipv4_prefixes(vec!["10.0.1.0/24".to_string()])
        .build();

        assert_eq!(entry.network_id, "net-123");
        assert_eq!(entry.mac_address, "52:54:00:12:34:56");
        assert_eq!(entry.name, Some("eth0".to_string()));
        assert_eq!(entry.ipv4_address, Some("10.0.0.5".to_string()));
        assert_eq!(entry.routed_ipv4_prefixes, vec!["10.0.1.0/24".to_string()]);
        assert_eq!(entry.state, NicState::Created);
        assert!(!entry.id.is_empty());
    }
}
