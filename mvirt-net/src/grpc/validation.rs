//! Input validation for gRPC requests.

use super::storage::{NetworkData, Storage};
use ipnet::{Ipv4Net, Ipv6Net};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use thiserror::Error;

/// Validation errors.
#[derive(Debug, Error)]
pub enum ValidationError {
    #[error("Network name is required")]
    NetworkNameRequired,

    #[error("At least one IP version (IPv4 or IPv6) must be enabled")]
    NoIpVersionEnabled,

    #[error("IPv4 subnet is required when IPv4 is enabled")]
    Ipv4SubnetRequired,

    #[error("IPv6 prefix is required when IPv6 is enabled")]
    Ipv6PrefixRequired,

    #[error("Invalid IPv4 subnet: {0}")]
    InvalidIpv4Subnet(String),

    #[error("Invalid IPv6 prefix: {0}")]
    InvalidIpv6Prefix(String),

    #[error("Public network subnet {0} overlaps with existing public network '{1}' ({2})")]
    SubnetOverlap(String, String, String),

    #[error("Invalid MAC address: {0}")]
    InvalidMacAddress(String),

    #[error("Invalid IPv4 address: {0}")]
    InvalidIpv4Address(String),

    #[error("Invalid IPv6 address: {0}")]
    InvalidIpv6Address(String),

    #[error("IPv4 address {0} is not within network subnet {1}")]
    Ipv4NotInSubnet(String, String),

    #[error("IPv6 address {0} is not within network prefix {1}")]
    Ipv6NotInPrefix(String, String),

    #[error("IPv4 address {0} is already in use")]
    Ipv4AddressInUse(String),

    #[error("IPv6 address {0} is already in use")]
    Ipv6AddressInUse(String),

    #[error("Network has {0} NICs, use force=true to delete")]
    NetworkHasNics(u32),

    #[error("Invalid DNS server address: {0}")]
    InvalidDnsServer(String),

    #[error("Invalid NTP server address: {0}")]
    InvalidNtpServer(String),

    #[error("Invalid routed prefix: {0}")]
    InvalidRoutedPrefix(String),

    #[error("Network identifier required")]
    NetworkIdRequired,
}

pub type Result<T> = std::result::Result<T, ValidationError>;

/// Check if two IPv4 subnets overlap.
pub fn ipv4_subnets_overlap(a: &Ipv4Net, b: &Ipv4Net) -> bool {
    a.contains(&b.network())
        || a.contains(&b.broadcast())
        || b.contains(&a.network())
        || b.contains(&a.broadcast())
}

/// Check if two IPv6 prefixes overlap.
pub fn ipv6_prefixes_overlap(a: &Ipv6Net, b: &Ipv6Net) -> bool {
    // For IPv6, check if either prefix contains the other's network address
    let a_net = a.network();
    let b_net = b.network();

    // Check if a contains b's network or b contains a's network
    a.contains(&b_net) || b.contains(&a_net)
}

/// Validate network creation request.
pub fn validate_create_network(
    name: &str,
    ipv4_enabled: bool,
    ipv4_subnet: &str,
    ipv6_enabled: bool,
    ipv6_prefix: &str,
    is_public: bool,
    dns_servers: &[String],
    storage: &Storage,
) -> Result<(Option<Ipv4Net>, Option<Ipv6Net>, Vec<IpAddr>)> {
    // Name required
    if name.trim().is_empty() {
        return Err(ValidationError::NetworkNameRequired);
    }

    // At least one IP version
    if !ipv4_enabled && !ipv6_enabled {
        return Err(ValidationError::NoIpVersionEnabled);
    }

    // Parse IPv4 subnet
    let parsed_v4 = if ipv4_enabled {
        if ipv4_subnet.is_empty() {
            return Err(ValidationError::Ipv4SubnetRequired);
        }
        let subnet: Ipv4Net = ipv4_subnet
            .parse()
            .map_err(|_| ValidationError::InvalidIpv4Subnet(ipv4_subnet.to_string()))?;
        Some(subnet)
    } else {
        None
    };

    // Parse IPv6 prefix
    let parsed_v6 = if ipv6_enabled {
        if ipv6_prefix.is_empty() {
            return Err(ValidationError::Ipv6PrefixRequired);
        }
        let prefix: Ipv6Net = ipv6_prefix
            .parse()
            .map_err(|_| ValidationError::InvalidIpv6Prefix(ipv6_prefix.to_string()))?;
        Some(prefix)
    } else {
        None
    };

    // Check for subnet overlap with other public networks
    if is_public {
        let public_networks = storage.list_public_networks().map_err(|_| {
            ValidationError::SubnetOverlap(
                "unknown".to_string(),
                "unknown".to_string(),
                "database error".to_string(),
            )
        })?;

        for existing in &public_networks {
            // Check IPv4 overlap
            if let (Some(new_v4), Some(existing_v4)) = (&parsed_v4, &existing.ipv4_subnet)
                && ipv4_subnets_overlap(new_v4, existing_v4)
            {
                return Err(ValidationError::SubnetOverlap(
                    new_v4.to_string(),
                    existing.name.clone(),
                    existing_v4.to_string(),
                ));
            }

            // Check IPv6 overlap
            if let (Some(new_v6), Some(existing_v6)) = (&parsed_v6, &existing.ipv6_prefix)
                && ipv6_prefixes_overlap(new_v6, existing_v6)
            {
                return Err(ValidationError::SubnetOverlap(
                    new_v6.to_string(),
                    existing.name.clone(),
                    existing_v6.to_string(),
                ));
            }
        }
    }

    // Parse DNS servers
    let parsed_dns: Vec<IpAddr> = dns_servers
        .iter()
        .filter(|s| !s.is_empty())
        .map(|s| {
            s.parse::<IpAddr>()
                .map_err(|_| ValidationError::InvalidDnsServer(s.clone()))
        })
        .collect::<Result<Vec<_>>>()?;

    Ok((parsed_v4, parsed_v6, parsed_dns))
}

/// Validate NIC creation request.
pub fn validate_create_nic(
    network: &NetworkData,
    mac_address: &str,
    ipv4_address: &str,
    ipv6_address: &str,
    routed_ipv4_prefixes: &[String],
    routed_ipv6_prefixes: &[String],
    storage: &Storage,
) -> Result<(
    Option<[u8; 6]>,
    Option<Ipv4Addr>,
    Option<Ipv6Addr>,
    Vec<Ipv4Net>,
    Vec<Ipv6Net>,
)> {
    // Parse MAC address (optional)
    let parsed_mac = if mac_address.is_empty() {
        None
    } else {
        Some(parse_mac(mac_address)?)
    };

    // Parse IPv4 address (optional)
    let parsed_v4 = if ipv4_address.is_empty() {
        None
    } else {
        let addr: Ipv4Addr = ipv4_address
            .parse()
            .map_err(|_| ValidationError::InvalidIpv4Address(ipv4_address.to_string()))?;

        // Check if in network subnet
        if let Some(subnet) = &network.ipv4_subnet {
            if !subnet.contains(&addr) {
                return Err(ValidationError::Ipv4NotInSubnet(
                    addr.to_string(),
                    subnet.to_string(),
                ));
            }
        } else {
            return Err(ValidationError::Ipv4NotInSubnet(
                addr.to_string(),
                "no subnet configured".to_string(),
            ));
        }

        // Check if already in use
        if storage.is_ipv4_in_use(&network.id, addr).unwrap_or(false) {
            return Err(ValidationError::Ipv4AddressInUse(addr.to_string()));
        }

        Some(addr)
    };

    // Parse IPv6 address (optional)
    let parsed_v6 = if ipv6_address.is_empty() {
        None
    } else {
        let addr: Ipv6Addr = ipv6_address
            .parse()
            .map_err(|_| ValidationError::InvalidIpv6Address(ipv6_address.to_string()))?;

        // Check if in network prefix
        if let Some(prefix) = &network.ipv6_prefix {
            if !prefix.contains(&addr) {
                return Err(ValidationError::Ipv6NotInPrefix(
                    addr.to_string(),
                    prefix.to_string(),
                ));
            }
        } else {
            return Err(ValidationError::Ipv6NotInPrefix(
                addr.to_string(),
                "no prefix configured".to_string(),
            ));
        }

        // Check if already in use
        if storage.is_ipv6_in_use(&network.id, addr).unwrap_or(false) {
            return Err(ValidationError::Ipv6AddressInUse(addr.to_string()));
        }

        Some(addr)
    };

    // Parse routed IPv4 prefixes
    let parsed_routed_v4: Vec<Ipv4Net> = routed_ipv4_prefixes
        .iter()
        .filter(|s| !s.is_empty())
        .map(|s| {
            s.parse::<Ipv4Net>()
                .map_err(|_| ValidationError::InvalidRoutedPrefix(s.clone()))
        })
        .collect::<Result<Vec<_>>>()?;

    // Parse routed IPv6 prefixes
    let parsed_routed_v6: Vec<Ipv6Net> = routed_ipv6_prefixes
        .iter()
        .filter(|s| !s.is_empty())
        .map(|s| {
            s.parse::<Ipv6Net>()
                .map_err(|_| ValidationError::InvalidRoutedPrefix(s.clone()))
        })
        .collect::<Result<Vec<_>>>()?;

    Ok((
        parsed_mac,
        parsed_v4,
        parsed_v6,
        parsed_routed_v4,
        parsed_routed_v6,
    ))
}

/// Parse MAC address string.
fn parse_mac(s: &str) -> Result<[u8; 6]> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 6 {
        return Err(ValidationError::InvalidMacAddress(s.to_string()));
    }

    let mut mac = [0u8; 6];
    for (i, part) in parts.iter().enumerate() {
        mac[i] = u8::from_str_radix(part, 16)
            .map_err(|_| ValidationError::InvalidMacAddress(s.to_string()))?;
    }
    Ok(mac)
}

/// Allocate the next available IPv4 address in a network.
///
/// Starts at network + 1 (e.g., 10.0.0.1 for 10.0.0.0/24) since the gateway
/// uses a link-local address (169.254.0.1) instead of an address from the subnet.
pub fn allocate_ipv4_address(network: &NetworkData, storage: &Storage) -> Option<Ipv4Addr> {
    let subnet = network.ipv4_subnet?;
    let used = storage.get_used_ipv4_addresses(&network.id).ok()?;

    // Start from network + 1 (skip only network address)
    // Gateway uses link-local 169.254.0.1, not an address from this subnet
    let network_addr = u32::from(subnet.network());
    let broadcast_addr = u32::from(subnet.broadcast());

    for addr_int in (network_addr + 1)..broadcast_addr {
        let addr = Ipv4Addr::from(addr_int);
        if !used.contains(&addr) {
            return Some(addr);
        }
    }

    None
}

/// Allocate the next available IPv6 address in a network.
///
/// Starts at prefix + 1 (e.g., 2001:db8::1 for 2001:db8::/64) since the gateway
/// uses a link-local address (fe80::1) instead of an address from the prefix.
pub fn allocate_ipv6_address(network: &NetworkData, storage: &Storage) -> Option<Ipv6Addr> {
    let prefix = network.ipv6_prefix?;
    let used = storage.get_used_ipv6_addresses(&network.id).ok()?;

    // Start from prefix + 1 (skip only :: network address)
    // Gateway uses link-local fe80::1, not an address from this prefix
    let network_addr = u128::from(prefix.network());

    // Limit search to first 65536 addresses
    for offset in 1u128..65536 {
        let addr = Ipv6Addr::from(network_addr + offset);
        if !used.contains(&addr) {
            return Some(addr);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ipv4_subnets_overlap() {
        let a: Ipv4Net = "10.0.0.0/24".parse().unwrap();
        let b: Ipv4Net = "10.0.0.0/16".parse().unwrap();
        assert!(ipv4_subnets_overlap(&a, &b));

        let c: Ipv4Net = "10.0.0.0/24".parse().unwrap();
        let d: Ipv4Net = "10.0.1.0/24".parse().unwrap();
        assert!(!ipv4_subnets_overlap(&c, &d));

        let e: Ipv4Net = "192.168.0.0/24".parse().unwrap();
        let f: Ipv4Net = "10.0.0.0/8".parse().unwrap();
        assert!(!ipv4_subnets_overlap(&e, &f));
    }

    #[test]
    fn test_parse_mac() {
        let mac = parse_mac("02:00:00:00:00:01").unwrap();
        assert_eq!(mac, [0x02, 0x00, 0x00, 0x00, 0x00, 0x01]);

        assert!(parse_mac("invalid").is_err());
        assert!(parse_mac("02:00:00:00:00").is_err());
        assert!(parse_mac("02:00:00:00:00:xx").is_err());
    }

    #[test]
    fn test_allocate_ipv4_starts_at_first_usable() {
        use crate::grpc::storage::{NetworkData, Storage};
        use chrono::Utc;
        use uuid::Uuid;

        let storage = Storage::in_memory().unwrap();

        // Create network 10.0.0.0/24
        let network = NetworkData {
            id: Uuid::new_v4(),
            name: "test-v4".to_string(),
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

        // First allocated IP should be 10.0.0.1 (only skip .0 network address)
        // Gateway uses link-local 169.254.0.1, not an address from this subnet
        let first_ip = allocate_ipv4_address(&network, &storage).unwrap();
        assert_eq!(
            first_ip,
            Ipv4Addr::new(10, 0, 0, 1),
            "First IPv4 should be .1 (gateway uses link-local 169.254.0.1)"
        );
    }

    #[test]
    fn test_allocate_ipv6_starts_at_first_usable() {
        use crate::grpc::storage::{NetworkData, Storage};
        use chrono::Utc;
        use std::net::Ipv6Addr;
        use uuid::Uuid;

        let storage = Storage::in_memory().unwrap();

        // Create network 2001:db8::/64
        let network = NetworkData {
            id: Uuid::new_v4(),
            name: "test-v6".to_string(),
            ipv4_enabled: false,
            ipv4_subnet: None,
            ipv6_enabled: true,
            ipv6_prefix: Some("2001:db8::/64".parse().unwrap()),
            dns_servers: vec![],
            ntp_servers: vec![],
            is_public: false,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        storage.create_network(&network).unwrap();

        // First allocated IP should be 2001:db8::1 (only skip :: network address)
        // Gateway uses link-local fe80::1, not an address from this prefix
        let first_ip = allocate_ipv6_address(&network, &storage).unwrap();
        assert_eq!(
            first_ip,
            Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1),
            "First IPv6 should be ::1 (gateway uses link-local fe80::1)"
        );
    }
}
