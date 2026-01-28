//! Input validation for gRPC requests.

use super::storage::{RuleDirection, RuleProtocol, Storage, parse_mac_address};
use ipnet::{IpNet, Ipv4Net, Ipv6Net};
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

    #[error("Invalid routed prefix: {0}")]
    InvalidRoutedPrefix(String),

    #[error("Network identifier required")]
    NetworkIdRequired,

    // Security Group validation errors
    #[error("Security group name is required")]
    SecurityGroupNameRequired,

    #[error("Security group identifier required")]
    SecurityGroupIdRequired,

    #[error("Security group has {0} attached NICs, use force=true to delete")]
    SecurityGroupHasNics(u32),

    #[error("Invalid rule direction")]
    InvalidRuleDirection,

    #[error("Invalid rule protocol")]
    InvalidRuleProtocol,

    #[error("Invalid port range: start ({0}) > end ({1})")]
    InvalidPortRange(u32, u32),

    #[error("Port out of range: {0} (must be 0-65535)")]
    PortOutOfRange(u32),

    #[error("Ports not allowed for protocol {0}")]
    PortsNotAllowedForProtocol(String),

    #[error("Invalid CIDR: {0}")]
    InvalidCidr(String),

    #[error("Rule ID is required")]
    RuleIdRequired,

    #[error("NIC identifier required")]
    NicIdRequired,
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
    let a_net = a.network();
    let b_net = b.network();
    a.contains(&b_net) || b.contains(&a_net)
}

/// Validate network creation request.
#[allow(clippy::too_many_arguments)]
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
    let mut parsed_dns = Vec::new();
    for s in dns_servers {
        if s.is_empty() {
            continue;
        }
        let addr: IpAddr = s
            .parse()
            .map_err(|_| ValidationError::InvalidDnsServer(s.clone()))?;
        parsed_dns.push(addr);
    }

    Ok((parsed_v4, parsed_v6, parsed_dns))
}

/// Validate NIC creation request.
#[allow(clippy::type_complexity)]
pub fn validate_create_nic(
    mac_address: &str,
    ipv4_address: &str,
    ipv6_address: &str,
    ipv4_subnet: Option<Ipv4Net>,
    ipv6_prefix: Option<Ipv6Net>,
) -> Result<(Option<[u8; 6]>, Option<Ipv4Addr>, Option<Ipv6Addr>)> {
    // Parse MAC if provided
    let mac = if mac_address.is_empty() {
        None
    } else {
        Some(
            parse_mac_address(mac_address)
                .ok_or_else(|| ValidationError::InvalidMacAddress(mac_address.to_string()))?,
        )
    };

    // Parse IPv4 address
    let ipv4 = if ipv4_address.is_empty() {
        None
    } else {
        let addr: Ipv4Addr = ipv4_address
            .parse()
            .map_err(|_| ValidationError::InvalidIpv4Address(ipv4_address.to_string()))?;

        // Verify it's in the subnet
        if let Some(subnet) = ipv4_subnet
            && !subnet.contains(&addr)
        {
            return Err(ValidationError::Ipv4NotInSubnet(
                ipv4_address.to_string(),
                subnet.to_string(),
            ));
        }
        Some(addr)
    };

    // Parse IPv6 address
    let ipv6 = if ipv6_address.is_empty() {
        None
    } else {
        let addr: Ipv6Addr = ipv6_address
            .parse()
            .map_err(|_| ValidationError::InvalidIpv6Address(ipv6_address.to_string()))?;

        // Verify it's in the prefix
        if let Some(prefix) = ipv6_prefix
            && !prefix.contains(&addr)
        {
            return Err(ValidationError::Ipv6NotInPrefix(
                ipv6_address.to_string(),
                prefix.to_string(),
            ));
        }
        Some(addr)
    };

    Ok((mac, ipv4, ipv6))
}

/// Allocate the next available IPv4 address in a subnet.
pub fn allocate_ipv4_address(
    subnet: Ipv4Net,
    used: &[Ipv4Addr],
    gateway: Ipv4Addr,
) -> Option<Ipv4Addr> {
    let network = u32::from(subnet.network());
    let broadcast = u32::from(subnet.broadcast());

    // Start from network + 2 (skip network and gateway)
    for i in (network + 2)..broadcast {
        let addr = Ipv4Addr::from(i);
        if addr != gateway && !used.contains(&addr) {
            return Some(addr);
        }
    }
    None
}

/// Allocate the next available IPv6 address in a prefix.
pub fn allocate_ipv6_address(
    prefix: Ipv6Net,
    used: &[Ipv6Addr],
    gateway: Ipv6Addr,
) -> Option<Ipv6Addr> {
    let network = u128::from(prefix.network());

    // Start from network + 2
    for i in 2u128..1000 {
        // Limit search to first 1000 addresses
        let addr = Ipv6Addr::from(network + i);
        if addr != gateway && !used.contains(&addr) {
            return Some(addr);
        }
    }
    None
}

/// Parse routed prefixes.
pub fn parse_routed_prefixes(
    v4_prefixes: &[String],
    v6_prefixes: &[String],
) -> Result<(Vec<Ipv4Net>, Vec<Ipv6Net>)> {
    let mut parsed_v4 = Vec::new();
    for p in v4_prefixes {
        if p.is_empty() {
            continue;
        }
        let prefix: Ipv4Net = p
            .parse()
            .map_err(|_| ValidationError::InvalidRoutedPrefix(p.clone()))?;
        parsed_v4.push(prefix);
    }

    let mut parsed_v6 = Vec::new();
    for p in v6_prefixes {
        if p.is_empty() {
            continue;
        }
        let prefix: Ipv6Net = p
            .parse()
            .map_err(|_| ValidationError::InvalidRoutedPrefix(p.clone()))?;
        parsed_v6.push(prefix);
    }

    Ok((parsed_v4, parsed_v6))
}

// ========== Security Group Validation ==========

/// Validate security group creation request.
pub fn validate_create_security_group(name: &str) -> Result<()> {
    if name.trim().is_empty() {
        return Err(ValidationError::SecurityGroupNameRequired);
    }
    Ok(())
}

/// Parsed CIDR information for eBPF rules
#[derive(Debug, Clone)]
pub struct ParsedCidr {
    /// Network address bytes (IPv4 in first 4 bytes, IPv6 uses all 16)
    pub addr: [u8; 16],
    /// Prefix length
    pub prefix_len: u8,
    /// IP version: 4 or 6
    pub ip_version: u8,
}

/// Validated security group rule fields.
pub type ValidatedRule = (
    RuleDirection,
    RuleProtocol,
    Option<u16>,
    Option<u16>,
    Option<ParsedCidr>,
);

/// Validate security group rule creation request.
pub fn validate_security_group_rule(
    direction: i32,
    protocol: i32,
    port_start: u32,
    port_end: u32,
    cidr: &str,
) -> Result<ValidatedRule> {
    // Validate direction
    let dir = RuleDirection::from(direction);
    if matches!(dir, RuleDirection::Unspecified) {
        return Err(ValidationError::InvalidRuleDirection);
    }

    // Validate protocol
    let proto = RuleProtocol::from(protocol);
    if matches!(proto, RuleProtocol::Unspecified) {
        return Err(ValidationError::InvalidRuleProtocol);
    }

    // Validate ports
    let (parsed_port_start, parsed_port_end) = if port_start > 0 || port_end > 0 {
        // Ports are only valid for TCP/UDP
        if !matches!(
            proto,
            RuleProtocol::Tcp | RuleProtocol::Udp | RuleProtocol::All
        ) {
            return Err(ValidationError::PortsNotAllowedForProtocol(
                proto.as_str().to_string(),
            ));
        }

        // Validate port range
        if port_start > 65535 {
            return Err(ValidationError::PortOutOfRange(port_start));
        }
        if port_end > 65535 {
            return Err(ValidationError::PortOutOfRange(port_end));
        }

        let start = if port_start > 0 { port_start as u16 } else { 1 };
        let end = if port_end > 0 { port_end as u16 } else { start };

        if start > end {
            return Err(ValidationError::InvalidPortRange(start as u32, end as u32));
        }

        (Some(start), Some(end))
    } else {
        (None, None)
    };

    // Validate CIDR
    let parsed_cidr = if !cidr.is_empty() {
        Some(parse_cidr(cidr)?)
    } else {
        None
    };

    Ok((dir, proto, parsed_port_start, parsed_port_end, parsed_cidr))
}

/// Parse a CIDR string into address bytes and prefix length.
pub fn parse_cidr(cidr: &str) -> Result<ParsedCidr> {
    let net: IpNet = cidr
        .parse()
        .map_err(|_| ValidationError::InvalidCidr(cidr.to_string()))?;

    match net {
        IpNet::V4(v4) => {
            let mut addr = [0u8; 16];
            addr[..4].copy_from_slice(&v4.network().octets());
            Ok(ParsedCidr {
                addr,
                prefix_len: v4.prefix_len(),
                ip_version: 4,
            })
        }
        IpNet::V6(v6) => {
            let addr = v6.network().octets();
            Ok(ParsedCidr {
                addr,
                prefix_len: v6.prefix_len(),
                ip_version: 6,
            })
        }
    }
}
