//! Network configuration utilities for uos.
//! Ported from pideisn.

pub mod dhcp4;
pub mod dhcp6;
pub mod interface;
pub mod netlink;
pub mod slaac;

use log::{error, info, warn};
use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::OnceLock;
use tokio::sync::RwLock;

pub use interface::Interface;
pub use netlink::NetlinkHandle;

/// Global network state, populated after DHCP/NDP configuration.
static NETWORK_STATE: OnceLock<RwLock<NetworkState>> = OnceLock::new();

/// Complete network state for all interfaces.
#[derive(Debug, Clone, Default)]
pub struct NetworkState {
    pub interfaces: Vec<InterfaceState>,
}

/// Network state for a single interface.
#[derive(Debug, Clone, Default)]
pub struct InterfaceState {
    pub name: String,
    pub mac_address: String,

    // IPv4 (from DHCPv4)
    pub ipv4_address: Option<Ipv4Addr>,
    pub ipv4_netmask: Option<Ipv4Addr>,
    pub ipv4_gateway: Option<Ipv4Addr>,
    pub ipv4_dns: Vec<Ipv4Addr>,

    // IPv6 (from SLAAC/DHCPv6)
    pub ipv6_address: Option<Ipv6Addr>,
    pub ipv6_gateway: Option<Ipv6Addr>,
    pub ipv6_dns: Vec<Ipv6Addr>,

    // DHCPv6 Prefix Delegation
    pub delegated_prefix: Option<String>,
}

/// Get the current network state.
pub async fn get_network_state() -> NetworkState {
    match NETWORK_STATE.get() {
        Some(lock) => lock.read().await.clone(),
        None => NetworkState::default(),
    }
}

fn get_or_init_state() -> &'static RwLock<NetworkState> {
    NETWORK_STATE.get_or_init(|| RwLock::new(NetworkState::default()))
}

/// Configure all network interfaces.
/// Should only be called when running as PID 1.
pub async fn configure_all() {
    let interfaces = match interface::discover_interfaces() {
        Ok(ifaces) => ifaces,
        Err(e) => {
            error!("Failed to discover interfaces: {}", e);
            return;
        }
    };

    if interfaces.is_empty() {
        warn!("No network interfaces found");
        return;
    }

    for iface in interfaces {
        info!("Configuring interface: {}", iface.name);
        configure_interface(&iface).await;
    }
}

async fn configure_interface(iface: &Interface) {
    let nl = match NetlinkHandle::new().await {
        Ok(nl) => nl,
        Err(e) => {
            error!("Failed to create netlink handle: {}", e);
            return;
        }
    };

    // Bring interface up
    if let Err(e) = nl.set_link_up(iface.index).await {
        error!("Failed to bring up {}: {}", iface.name, e);
        return;
    }
    info!("Interface {} is up", iface.name);

    // Initialize interface state
    let mut iface_state = InterfaceState {
        name: iface.name.clone(),
        mac_address: format!(
            "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            iface.mac[0], iface.mac[1], iface.mac[2], iface.mac[3], iface.mac[4], iface.mac[5]
        ),
        ..Default::default()
    };

    // Configure IPv6 link-local via SLAAC first
    match slaac::configure(iface, &nl).await {
        Ok(slaac_info) => {
            iface_state.ipv6_gateway = slaac_info.gateway;
        }
        Err(e) => {
            warn!("SLAAC failed for {}: {}", iface.name, e);
        }
    }

    // Try DHCPv4
    match dhcp4::configure(iface, &nl).await {
        Ok(lease) => {
            info!(
                "DHCPv4: {} netmask {} gateway {:?}",
                lease.address, lease.netmask, lease.gateway
            );
            iface_state.ipv4_address = Some(lease.address);
            iface_state.ipv4_netmask = Some(lease.netmask);
            iface_state.ipv4_gateway = lease.gateway;
            iface_state.ipv4_dns = lease.dns_servers;
        }
        Err(e) => {
            warn!("DHCPv4 failed for {}: {}", iface.name, e);
        }
    }

    // Try DHCPv6 with prefix delegation
    match dhcp6::configure(iface, &nl, true).await {
        Ok(lease) => {
            if let Some(addr) = lease.address {
                info!("DHCPv6: {}", addr);
                iface_state.ipv6_address = Some(addr);
            }
            if let Some(pd) = &lease.prefix {
                info!("DHCPv6 PD: {}/{}", pd.prefix, pd.prefix_len);
                iface_state.delegated_prefix = Some(format!("{}/{}", pd.prefix, pd.prefix_len));
            }
            iface_state.ipv6_dns = lease.dns_servers;
        }
        Err(e) => {
            warn!("DHCPv6 failed for {}: {}", iface.name, e);
        }
    }

    // Store interface state
    {
        let state = get_or_init_state();
        let mut guard = state.write().await;
        guard.interfaces.push(iface_state);
    }
}

/// DHCPv4 lease information.
#[derive(Debug, Clone)]
pub struct Dhcp4Lease {
    pub address: Ipv4Addr,
    pub netmask: Ipv4Addr,
    pub gateway: Option<Ipv4Addr>,
    pub dns_servers: Vec<Ipv4Addr>,
    pub lease_time: u32,
}

/// DHCPv6 lease information.
#[derive(Debug, Clone)]
pub struct Dhcp6Lease {
    pub address: Option<std::net::Ipv6Addr>,
    pub prefix: Option<DelegatedPrefix>,
    pub dns_servers: Vec<std::net::Ipv6Addr>,
}

/// IPv6 delegated prefix information.
#[derive(Debug, Clone)]
pub struct DelegatedPrefix {
    pub prefix: std::net::Ipv6Addr,
    pub prefix_len: u8,
    pub preferred_lifetime: u32,
    pub valid_lifetime: u32,
}
