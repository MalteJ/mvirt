//! Network configuration utilities for uos.
//! Ported from pideisn.

pub mod dhcp4;
pub mod dhcp6;
pub mod interface;
pub mod netlink;
pub mod slaac;

use log::{error, info, warn};
use std::net::Ipv4Addr;

pub use interface::Interface;
pub use netlink::NetlinkHandle;

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

    // Configure IPv6 link-local via SLAAC first
    if let Err(e) = slaac::configure(iface, &nl).await {
        warn!("SLAAC failed for {}: {}", iface.name, e);
    }

    // Try DHCPv4
    match dhcp4::configure(iface, &nl).await {
        Ok(lease) => {
            info!(
                "DHCPv4: {} netmask {} gateway {:?}",
                lease.address, lease.netmask, lease.gateway
            );
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
            }
            if let Some(pd) = lease.prefix {
                info!("DHCPv6 PD: {}/{}", pd.prefix, pd.prefix_len);
            }
        }
        Err(e) => {
            warn!("DHCPv6 failed for {}: {}", iface.name, e);
        }
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
