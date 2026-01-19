//! SQLite storage layer for networks and NICs.

use chrono::{DateTime, Utc};
use ipnet::{Ipv4Net, Ipv6Net};
use refinery::embed_migrations;
use rusqlite::{Connection, OptionalExtension, Row, params};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::Path;
use std::sync::Mutex;
use thiserror::Error;
use uuid::Uuid;

embed_migrations!("migrations");

/// Storage errors.
#[derive(Debug, Error)]
pub enum StorageError {
    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("Migration error: {0}")]
    Migration(#[from] refinery::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Network not found: {0}")]
    NetworkNotFound(String),

    #[error("NIC not found: {0}")]
    NicNotFound(String),

    #[error("Network name already exists: {0}")]
    NetworkNameExists(String),

    #[error("IP address already in use: {0}")]
    IpAddressInUse(String),
}

pub type Result<T> = std::result::Result<T, StorageError>;

/// NIC state enum matching proto definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum NicState {
    Unspecified = 0,
    Created = 1,
    Active = 2,
    Error = 3,
}

impl From<i32> for NicState {
    fn from(v: i32) -> Self {
        match v {
            0 => NicState::Unspecified,
            1 => NicState::Created,
            2 => NicState::Active,
            3 => NicState::Error,
            _ => NicState::Unspecified,
        }
    }
}

impl From<NicState> for i32 {
    fn from(s: NicState) -> i32 {
        s as i32
    }
}

/// Network data stored in the database.
#[derive(Debug, Clone)]
pub struct NetworkData {
    pub id: Uuid,
    pub name: String,
    pub ipv4_enabled: bool,
    pub ipv4_subnet: Option<Ipv4Net>,
    pub ipv6_enabled: bool,
    pub ipv6_prefix: Option<Ipv6Net>,
    pub dns_servers: Vec<IpAddr>,
    pub ntp_servers: Vec<IpAddr>,
    pub is_public: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl NetworkData {
    /// Get IPv4 gateway address (first usable address in subnet).
    pub fn ipv4_gateway(&self) -> Option<Ipv4Addr> {
        self.ipv4_subnet.map(|net| {
            let network = u32::from(net.network());
            Ipv4Addr::from(network + 1)
        })
    }

    /// Get IPv6 gateway address (::1 in prefix).
    pub fn ipv6_gateway(&self) -> Option<Ipv6Addr> {
        self.ipv6_prefix.map(|net| {
            let network = u128::from(net.network());
            Ipv6Addr::from(network + 1)
        })
    }
}

/// NIC data stored in the database.
#[derive(Debug, Clone)]
pub struct NicData {
    pub id: Uuid,
    pub name: Option<String>,
    pub network_id: Uuid,
    pub mac_address: [u8; 6],
    pub ipv4_address: Option<Ipv4Addr>,
    pub ipv6_address: Option<Ipv6Addr>,
    pub routed_ipv4_prefixes: Vec<Ipv4Net>,
    pub routed_ipv6_prefixes: Vec<Ipv6Net>,
    pub socket_path: String,
    pub state: NicState,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl NicData {
    /// Format MAC address as string.
    pub fn mac_string(&self) -> String {
        format!(
            "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            self.mac_address[0],
            self.mac_address[1],
            self.mac_address[2],
            self.mac_address[3],
            self.mac_address[4],
            self.mac_address[5]
        )
    }
}

/// SQLite storage for networks and NICs.
pub struct Storage {
    conn: Mutex<Connection>,
}

impl Storage {
    /// Create a new storage instance with the given database path.
    pub fn new(path: &Path) -> Result<Self> {
        let mut conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;

        // Run migrations
        migrations::runner().run(&mut conn)?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Create an in-memory storage instance (for testing).
    pub fn in_memory() -> Result<Self> {
        let mut conn = Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;

        // Run migrations
        migrations::runner().run(&mut conn)?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    // ========== Network Operations ==========

    /// Create a new network.
    pub fn create_network(&self, network: &NetworkData) -> Result<()> {
        let conn = self.conn.lock().unwrap();

        let dns_json = serde_json::to_string(&network.dns_servers)?;
        let ntp_json = serde_json::to_string(&network.ntp_servers)?;

        conn.execute(
            "INSERT INTO networks (id, name, ipv4_enabled, ipv4_subnet, ipv6_enabled, ipv6_prefix, dns_servers, ntp_servers, is_public, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                network.id.to_string(),
                network.name,
                network.ipv4_enabled,
                network.ipv4_subnet.map(|n| n.to_string()),
                network.ipv6_enabled,
                network.ipv6_prefix.map(|n| n.to_string()),
                dns_json,
                ntp_json,
                network.is_public,
                network.created_at.to_rfc3339(),
                network.updated_at.to_rfc3339(),
            ],
        ).map_err(|e| {
            if let rusqlite::Error::SqliteFailure(ref err, _) = e
                && err.code == rusqlite::ErrorCode::ConstraintViolation {
                    return StorageError::NetworkNameExists(network.name.clone());
                }
            StorageError::Database(e)
        })?;

        Ok(())
    }

    /// Get a network by ID.
    pub fn get_network_by_id(&self, id: &Uuid) -> Result<Option<NetworkData>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, name, ipv4_enabled, ipv4_subnet, ipv6_enabled, ipv6_prefix, dns_servers, ntp_servers, is_public, created_at, updated_at
             FROM networks WHERE id = ?1",
            params![id.to_string()],
            |row| Ok(Self::row_to_network(row)),
        )
        .optional()?
        .transpose()
    }

    /// Get a network by name.
    pub fn get_network_by_name(&self, name: &str) -> Result<Option<NetworkData>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, name, ipv4_enabled, ipv4_subnet, ipv6_enabled, ipv6_prefix, dns_servers, ntp_servers, is_public, created_at, updated_at
             FROM networks WHERE name = ?1",
            params![name],
            |row| Ok(Self::row_to_network(row)),
        )
        .optional()?
        .transpose()
    }

    /// List all networks.
    pub fn list_networks(&self) -> Result<Vec<NetworkData>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, ipv4_enabled, ipv4_subnet, ipv6_enabled, ipv6_prefix, dns_servers, ntp_servers, is_public, created_at, updated_at
             FROM networks ORDER BY created_at",
        )?;

        let networks = stmt
            .query_map([], |row| Ok(Self::row_to_network(row)))?
            .collect::<std::result::Result<Vec<_>, _>>()?
            .into_iter()
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(networks)
    }

    /// List all public networks.
    pub fn list_public_networks(&self) -> Result<Vec<NetworkData>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, ipv4_enabled, ipv4_subnet, ipv6_enabled, ipv6_prefix, dns_servers, ntp_servers, is_public, created_at, updated_at
             FROM networks WHERE is_public = 1 ORDER BY created_at",
        )?;

        let networks = stmt
            .query_map([], |row| Ok(Self::row_to_network(row)))?
            .collect::<std::result::Result<Vec<_>, _>>()?
            .into_iter()
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(networks)
    }

    /// Update network DNS and NTP servers.
    pub fn update_network(
        &self,
        id: &Uuid,
        dns_servers: &[IpAddr],
        ntp_servers: &[IpAddr],
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let dns_json = serde_json::to_string(dns_servers)?;
        let ntp_json = serde_json::to_string(ntp_servers)?;
        let now = Utc::now().to_rfc3339();

        let rows = conn.execute(
            "UPDATE networks SET dns_servers = ?1, ntp_servers = ?2, updated_at = ?3 WHERE id = ?4",
            params![dns_json, ntp_json, now, id.to_string()],
        )?;

        if rows == 0 {
            return Err(StorageError::NetworkNotFound(id.to_string()));
        }
        Ok(())
    }

    /// Delete a network by ID.
    pub fn delete_network(&self, id: &Uuid) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute(
            "DELETE FROM networks WHERE id = ?1",
            params![id.to_string()],
        )?;
        Ok(rows > 0)
    }

    /// Count NICs in a network.
    pub fn count_nics_in_network(&self, network_id: &Uuid) -> Result<u32> {
        let conn = self.conn.lock().unwrap();
        let count: u32 = conn.query_row(
            "SELECT COUNT(*) FROM nics WHERE network_id = ?1",
            params![network_id.to_string()],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    fn row_to_network(row: &Row) -> Result<NetworkData> {
        let id_str: String = row.get(0)?;
        let name: String = row.get(1)?;
        let ipv4_enabled: bool = row.get(2)?;
        let ipv4_subnet_str: Option<String> = row.get(3)?;
        let ipv6_enabled: bool = row.get(4)?;
        let ipv6_prefix_str: Option<String> = row.get(5)?;
        let dns_json: String = row.get(6)?;
        let ntp_json: String = row.get(7)?;
        let is_public: bool = row.get(8)?;
        let created_at_str: String = row.get(9)?;
        let updated_at_str: String = row.get(10)?;

        Ok(NetworkData {
            id: Uuid::parse_str(&id_str).unwrap(),
            name,
            ipv4_enabled,
            ipv4_subnet: ipv4_subnet_str.map(|s| s.parse().unwrap()),
            ipv6_enabled,
            ipv6_prefix: ipv6_prefix_str.map(|s| s.parse().unwrap()),
            dns_servers: serde_json::from_str(&dns_json)?,
            ntp_servers: serde_json::from_str(&ntp_json)?,
            is_public,
            created_at: DateTime::parse_from_rfc3339(&created_at_str)
                .unwrap()
                .with_timezone(&Utc),
            updated_at: DateTime::parse_from_rfc3339(&updated_at_str)
                .unwrap()
                .with_timezone(&Utc),
        })
    }

    // ========== NIC Operations ==========

    /// Create a new NIC.
    pub fn create_nic(&self, nic: &NicData) -> Result<()> {
        let conn = self.conn.lock().unwrap();

        let routed_v4_json = serde_json::to_string(
            &nic.routed_ipv4_prefixes
                .iter()
                .map(|p| p.to_string())
                .collect::<Vec<_>>(),
        )?;
        let routed_v6_json = serde_json::to_string(
            &nic.routed_ipv6_prefixes
                .iter()
                .map(|p| p.to_string())
                .collect::<Vec<_>>(),
        )?;

        conn.execute(
            "INSERT INTO nics (id, name, network_id, mac_address, ipv4_address, ipv6_address, routed_ipv4_prefixes, routed_ipv6_prefixes, socket_path, state, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                nic.id.to_string(),
                nic.name,
                nic.network_id.to_string(),
                nic.mac_string(),
                nic.ipv4_address.map(|a| a.to_string()),
                nic.ipv6_address.map(|a| a.to_string()),
                routed_v4_json,
                routed_v6_json,
                nic.socket_path,
                i32::from(nic.state),
                nic.created_at.to_rfc3339(),
                nic.updated_at.to_rfc3339(),
            ],
        )?;

        Ok(())
    }

    /// Get a NIC by ID.
    pub fn get_nic_by_id(&self, id: &Uuid) -> Result<Option<NicData>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, name, network_id, mac_address, ipv4_address, ipv6_address, routed_ipv4_prefixes, routed_ipv6_prefixes, socket_path, state, created_at, updated_at
             FROM nics WHERE id = ?1",
            params![id.to_string()],
            |row| Ok(Self::row_to_nic(row)),
        )
        .optional()?
        .transpose()
    }

    /// Get a NIC by name.
    pub fn get_nic_by_name(&self, name: &str) -> Result<Option<NicData>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, name, network_id, mac_address, ipv4_address, ipv6_address, routed_ipv4_prefixes, routed_ipv6_prefixes, socket_path, state, created_at, updated_at
             FROM nics WHERE name = ?1",
            params![name],
            |row| Ok(Self::row_to_nic(row)),
        )
        .optional()?
        .transpose()
    }

    /// List all NICs.
    pub fn list_nics(&self) -> Result<Vec<NicData>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, network_id, mac_address, ipv4_address, ipv6_address, routed_ipv4_prefixes, routed_ipv6_prefixes, socket_path, state, created_at, updated_at
             FROM nics ORDER BY created_at",
        )?;

        let nics = stmt
            .query_map([], |row| Ok(Self::row_to_nic(row)))?
            .collect::<std::result::Result<Vec<_>, _>>()?
            .into_iter()
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(nics)
    }

    /// List NICs in a network.
    pub fn list_nics_in_network(&self, network_id: &Uuid) -> Result<Vec<NicData>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, network_id, mac_address, ipv4_address, ipv6_address, routed_ipv4_prefixes, routed_ipv6_prefixes, socket_path, state, created_at, updated_at
             FROM nics WHERE network_id = ?1 ORDER BY created_at",
        )?;

        let nics = stmt
            .query_map(params![network_id.to_string()], |row| {
                Ok(Self::row_to_nic(row))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?
            .into_iter()
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(nics)
    }

    /// Update NIC routed prefixes.
    pub fn update_nic_routed_prefixes(
        &self,
        id: &Uuid,
        routed_ipv4: &[Ipv4Net],
        routed_ipv6: &[Ipv6Net],
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let v4_json = serde_json::to_string(
            &routed_ipv4
                .iter()
                .map(|p| p.to_string())
                .collect::<Vec<_>>(),
        )?;
        let v6_json = serde_json::to_string(
            &routed_ipv6
                .iter()
                .map(|p| p.to_string())
                .collect::<Vec<_>>(),
        )?;
        let now = Utc::now().to_rfc3339();

        let rows = conn.execute(
            "UPDATE nics SET routed_ipv4_prefixes = ?1, routed_ipv6_prefixes = ?2, updated_at = ?3 WHERE id = ?4",
            params![v4_json, v6_json, now, id.to_string()],
        )?;

        if rows == 0 {
            return Err(StorageError::NicNotFound(id.to_string()));
        }
        Ok(())
    }

    /// Update NIC state.
    pub fn update_nic_state(&self, id: &Uuid, state: NicState) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = Utc::now().to_rfc3339();

        let rows = conn.execute(
            "UPDATE nics SET state = ?1, updated_at = ?2 WHERE id = ?3",
            params![i32::from(state), now, id.to_string()],
        )?;

        if rows == 0 {
            return Err(StorageError::NicNotFound(id.to_string()));
        }
        Ok(())
    }

    /// Delete a NIC by ID.
    pub fn delete_nic(&self, id: &Uuid) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute("DELETE FROM nics WHERE id = ?1", params![id.to_string()])?;
        Ok(rows > 0)
    }

    /// Check if an IPv4 address is already in use in a network.
    pub fn is_ipv4_in_use(&self, network_id: &Uuid, addr: Ipv4Addr) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let count: u32 = conn.query_row(
            "SELECT COUNT(*) FROM nics WHERE network_id = ?1 AND ipv4_address = ?2",
            params![network_id.to_string(), addr.to_string()],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// Check if an IPv6 address is already in use in a network.
    pub fn is_ipv6_in_use(&self, network_id: &Uuid, addr: Ipv6Addr) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let count: u32 = conn.query_row(
            "SELECT COUNT(*) FROM nics WHERE network_id = ?1 AND ipv6_address = ?2",
            params![network_id.to_string(), addr.to_string()],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// Get all used IPv4 addresses in a network.
    pub fn get_used_ipv4_addresses(&self, network_id: &Uuid) -> Result<Vec<Ipv4Addr>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT ipv4_address FROM nics WHERE network_id = ?1 AND ipv4_address IS NOT NULL",
        )?;

        let addrs = stmt
            .query_map(params![network_id.to_string()], |row| {
                let addr_str: String = row.get(0)?;
                Ok(addr_str.parse::<Ipv4Addr>().unwrap())
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(addrs)
    }

    /// Get all used IPv6 addresses in a network.
    pub fn get_used_ipv6_addresses(&self, network_id: &Uuid) -> Result<Vec<Ipv6Addr>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT ipv6_address FROM nics WHERE network_id = ?1 AND ipv6_address IS NOT NULL",
        )?;

        let addrs = stmt
            .query_map(params![network_id.to_string()], |row| {
                let addr_str: String = row.get(0)?;
                Ok(addr_str.parse::<Ipv6Addr>().unwrap())
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(addrs)
    }

    fn row_to_nic(row: &Row) -> Result<NicData> {
        let id_str: String = row.get(0)?;
        let name: Option<String> = row.get(1)?;
        let network_id_str: String = row.get(2)?;
        let mac_str: String = row.get(3)?;
        let ipv4_str: Option<String> = row.get(4)?;
        let ipv6_str: Option<String> = row.get(5)?;
        let routed_v4_json: String = row.get(6)?;
        let routed_v6_json: String = row.get(7)?;
        let socket_path: String = row.get(8)?;
        let state_int: i32 = row.get(9)?;
        let created_at_str: String = row.get(10)?;
        let updated_at_str: String = row.get(11)?;

        // Parse MAC address
        let mac_parts: Vec<&str> = mac_str.split(':').collect();
        let mut mac = [0u8; 6];
        for (i, part) in mac_parts.iter().enumerate().take(6) {
            mac[i] = u8::from_str_radix(part, 16).unwrap_or(0);
        }

        // Parse routed prefixes
        let routed_v4_strs: Vec<String> = serde_json::from_str(&routed_v4_json)?;
        let routed_v6_strs: Vec<String> = serde_json::from_str(&routed_v6_json)?;

        Ok(NicData {
            id: Uuid::parse_str(&id_str).unwrap(),
            name,
            network_id: Uuid::parse_str(&network_id_str).unwrap(),
            mac_address: mac,
            ipv4_address: ipv4_str.map(|s| s.parse().unwrap()),
            ipv6_address: ipv6_str.map(|s| s.parse().unwrap()),
            routed_ipv4_prefixes: routed_v4_strs
                .iter()
                .map(|s: &String| s.parse().unwrap())
                .collect(),
            routed_ipv6_prefixes: routed_v6_strs
                .iter()
                .map(|s: &String| s.parse().unwrap())
                .collect(),
            socket_path,
            state: NicState::from(state_int),
            created_at: DateTime::parse_from_rfc3339(&created_at_str)
                .unwrap()
                .with_timezone(&Utc),
            updated_at: DateTime::parse_from_rfc3339(&updated_at_str)
                .unwrap()
                .with_timezone(&Utc),
        })
    }
}

/// Parse MAC address string to bytes.
pub fn parse_mac_address(s: &str) -> Option<[u8; 6]> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 6 {
        return None;
    }

    let mut mac = [0u8; 6];
    for (i, part) in parts.iter().enumerate() {
        mac[i] = u8::from_str_radix(part, 16).ok()?;
    }
    Some(mac)
}

/// Generate a random MAC address with local admin bit set.
pub fn generate_mac_address() -> [u8; 6] {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let mut mac = [0u8; 6];
    rng.fill(&mut mac);
    // Set locally administered and unicast bits
    mac[0] = (mac[0] & 0xfe) | 0x02;
    mac
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_storage_create_network() {
        let storage = Storage::in_memory().unwrap();

        let network = NetworkData {
            id: Uuid::new_v4(),
            name: "test-network".to_string(),
            ipv4_enabled: true,
            ipv4_subnet: Some("10.0.0.0/24".parse().unwrap()),
            ipv6_enabled: false,
            ipv6_prefix: None,
            dns_servers: vec!["8.8.8.8".parse().unwrap()],
            ntp_servers: vec![],
            is_public: true,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        storage.create_network(&network).unwrap();

        let fetched = storage.get_network_by_id(&network.id).unwrap().unwrap();
        assert_eq!(fetched.name, "test-network");
        assert!(fetched.ipv4_enabled);
        assert_eq!(fetched.ipv4_subnet, Some("10.0.0.0/24".parse().unwrap()));
    }

    #[test]
    fn test_storage_create_nic() {
        let storage = Storage::in_memory().unwrap();

        let network = NetworkData {
            id: Uuid::new_v4(),
            name: "test-network".to_string(),
            ipv4_enabled: true,
            ipv4_subnet: Some("10.0.0.0/24".parse().unwrap()),
            ipv6_enabled: false,
            ipv6_prefix: None,
            dns_servers: vec![],
            ntp_servers: vec![],
            is_public: false,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        storage.create_network(&network).unwrap();

        let nic = NicData {
            id: Uuid::new_v4(),
            name: Some("test-nic".to_string()),
            network_id: network.id,
            mac_address: [0x02, 0x00, 0x00, 0x00, 0x00, 0x01],
            ipv4_address: Some("10.0.0.5".parse().unwrap()),
            ipv6_address: None,
            routed_ipv4_prefixes: vec![],
            routed_ipv6_prefixes: vec![],
            socket_path: "/run/mvirt/net/nic-test.sock".to_string(),
            state: NicState::Created,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        storage.create_nic(&nic).unwrap();

        let fetched = storage.get_nic_by_id(&nic.id).unwrap().unwrap();
        assert_eq!(fetched.name, Some("test-nic".to_string()));
        assert_eq!(fetched.ipv4_address, Some("10.0.0.5".parse().unwrap()));
    }

    #[test]
    fn test_parse_mac_address() {
        let mac = parse_mac_address("02:00:00:00:00:01").unwrap();
        assert_eq!(mac, [0x02, 0x00, 0x00, 0x00, 0x00, 0x01]);

        assert!(parse_mac_address("invalid").is_none());
        assert!(parse_mac_address("02:00:00:00:00").is_none());
    }

    #[test]
    fn test_generate_mac_address() {
        let mac = generate_mac_address();
        // Check locally administered bit
        assert_eq!(mac[0] & 0x02, 0x02);
        // Check unicast bit
        assert_eq!(mac[0] & 0x01, 0x00);
    }
}
