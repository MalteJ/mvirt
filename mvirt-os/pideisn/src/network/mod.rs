pub mod dhcp4;
pub mod dhcp6;
pub mod interface;
pub mod netlink;
pub mod pd;
pub mod slaac;

use crate::{log_error, log_info, log_warn};
use std::net::Ipv4Addr;

pub use interface::Interface;
pub use netlink::NetlinkHandle;

pub async fn configure_all() {
    let interfaces = match interface::discover_interfaces() {
        Ok(ifaces) => ifaces,
        Err(e) => {
            log_error!("Failed to discover interfaces: {}", e);
            return;
        }
    };

    if interfaces.is_empty() {
        log_warn!("No network interfaces found");
        return;
    }

    for iface in interfaces {
        log_info!("Configuring interface: {}", iface.name);
        configure_interface(&iface).await;
    }
}

async fn configure_interface(iface: &Interface) {
    let nl = match NetlinkHandle::new().await {
        Ok(nl) => nl,
        Err(e) => {
            log_error!("Failed to create netlink handle: {}", e);
            return;
        }
    };

    // Bring interface up
    if let Err(e) = nl.set_link_up(iface.index).await {
        log_error!("Failed to bring up {}: {}", iface.name, e);
        return;
    }
    log_info!("Interface {} is up", iface.name);

    // Configure IPv6 link-local via SLAAC first
    if let Err(e) = slaac::configure(iface, &nl).await {
        log_warn!("SLAAC failed for {}: {}", iface.name, e);
    }

    // Try DHCPv4
    match dhcp4::configure(iface, &nl).await {
        Ok(lease) => {
            log_info!(
                "DHCPv4: {} netmask {} gateway {:?}",
                lease.address,
                lease.netmask,
                lease.gateway
            );
        }
        Err(e) => {
            log_warn!("DHCPv4 failed for {}: {}", iface.name, e);
        }
    }

    // Try DHCPv6 with prefix delegation
    match dhcp6::configure(iface, &nl, true).await {
        Ok(lease) => {
            if let Some(addr) = lease.address {
                log_info!("DHCPv6: {}", addr);
            }
            if let Some(pd) = lease.prefix {
                log_info!("DHCPv6 PD: {}/{}", pd.prefix, pd.prefix_len);
                // Store prefix for nested VMs
                pd::store_delegated_prefix(pd);
            }
        }
        Err(e) => {
            log_warn!("DHCPv6 failed for {}: {}", iface.name, e);
        }
    }
}

#[derive(Debug, Clone)]
pub struct Dhcp4Lease {
    pub address: Ipv4Addr,
    pub netmask: Ipv4Addr,
    pub gateway: Option<Ipv4Addr>,
    #[allow(dead_code)]
    pub dns_servers: Vec<Ipv4Addr>,
    pub lease_time: u32,
}

#[derive(Debug, Clone)]
pub struct Dhcp6Lease {
    pub address: Option<std::net::Ipv6Addr>,
    pub prefix: Option<DelegatedPrefix>,
    #[allow(dead_code)]
    pub dns_servers: Vec<std::net::Ipv6Addr>,
}

#[derive(Debug, Clone)]
pub struct DelegatedPrefix {
    pub prefix: std::net::Ipv6Addr,
    pub prefix_len: u8,
    pub preferred_lifetime: u32,
    pub valid_lifetime: u32,
}
