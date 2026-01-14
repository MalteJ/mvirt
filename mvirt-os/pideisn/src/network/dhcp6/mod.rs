mod client;

use crate::error::NetworkError;
use crate::network::{Dhcp6Lease, Interface, NetlinkHandle};

pub async fn configure(
    iface: &Interface,
    nl: &NetlinkHandle,
    request_pd: bool,
) -> Result<Dhcp6Lease, NetworkError> {
    let mut client = client::Dhcp6Client::new(iface, request_pd)?;
    let lease = client.run().await?;

    // Configure the address if we got one
    if let Some(addr) = lease.address {
        nl.add_address_v6(iface.index, addr, 128).await?;
    }

    Ok(lease)
}
