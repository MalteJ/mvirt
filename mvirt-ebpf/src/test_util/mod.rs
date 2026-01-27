//! Test utilities for mvirt-ebpf integration tests.
//!
//! Provides TAP device simulation and packet builders for testing
//! the protocol handler without real VMs.

pub mod packets;
pub mod tap_device;

pub use packets::*;
pub use tap_device::TapTestDevice;

use crate::grpc::{NetworkData, NicData, NicState};
use ipnet::Ipv4Net;
use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;
use uuid::Uuid;

/// Default timeout for packet operations in tests
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(2);

/// Create a test NIC configuration
pub fn test_nic_config(mac: [u8; 6], ipv4: Ipv4Addr, network_id: Uuid) -> NicData {
    NicData {
        id: Uuid::new_v4(),
        network_id,
        name: Some(format!("test-nic-{}", Uuid::new_v4().as_simple())),
        mac_address: mac,
        ipv4_address: Some(ipv4),
        ipv6_address: None,
        routed_ipv4_prefixes: vec![],
        routed_ipv6_prefixes: vec![],
        tap_name: String::new(), // Will be set when TAP is created
        state: NicState::Active,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    }
}

/// Create a test network configuration
pub fn test_network_config(subnet: Ipv4Net, dns: IpAddr) -> NetworkData {
    NetworkData {
        id: Uuid::new_v4(),
        name: format!("test-network-{}", Uuid::new_v4().as_simple()),
        ipv4_enabled: true,
        ipv4_subnet: Some(subnet),
        ipv6_enabled: false,
        ipv6_prefix: None,
        dns_servers: vec![dns],
        ntp_servers: vec![],
        is_public: false,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    }
}

/// Parse MAC address from string to bytes
pub fn parse_mac(mac_str: &str) -> Option<[u8; 6]> {
    let parts: Vec<&str> = mac_str.split(':').collect();
    if parts.len() != 6 {
        return None;
    }
    let mut mac = [0u8; 6];
    for (i, part) in parts.iter().enumerate() {
        mac[i] = u8::from_str_radix(part, 16).ok()?;
    }
    Some(mac)
}

/// Format MAC address bytes as string
pub fn format_mac(mac: &[u8; 6]) -> String {
    format!(
        "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    )
}
