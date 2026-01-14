mod client;

use crate::error::NetworkError;
use crate::network::{Dhcp4Lease, Interface, NetlinkHandle};

pub async fn configure(iface: &Interface, nl: &NetlinkHandle) -> Result<Dhcp4Lease, NetworkError> {
    let mut client = client::Dhcp4Client::new(iface)?;
    let lease = client.run().await?;

    // Calculate prefix length from netmask
    let prefix_len = netmask_to_prefix_len(lease.netmask);

    // Configure the address
    nl.add_address_v4(iface.index, lease.address, prefix_len)
        .await?;

    // Add default route if we have a gateway
    if let Some(gw) = lease.gateway {
        nl.add_route_v4(gw, iface.index).await?;
    }

    Ok(lease)
}

fn netmask_to_prefix_len(netmask: std::net::Ipv4Addr) -> u8 {
    let bits = u32::from_be_bytes(netmask.octets());
    bits.count_ones() as u8
}
