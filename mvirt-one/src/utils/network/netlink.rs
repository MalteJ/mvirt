//! Netlink interface for network configuration.
//! Ported from pideisn.

use crate::error::NetworkError;
use rtnetlink::Handle;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Handle for netlink operations.
pub struct NetlinkHandle {
    handle: Handle,
}

impl NetlinkHandle {
    /// Create a new netlink handle.
    pub async fn new() -> Result<Self, NetworkError> {
        let (connection, handle, _) =
            rtnetlink::new_connection().map_err(|e| NetworkError::NetlinkError(e.to_string()))?;

        tokio::spawn(connection);

        Ok(Self { handle })
    }

    /// Bring a network interface up.
    pub async fn set_link_up(&self, index: u32) -> Result<(), NetworkError> {
        self.handle
            .link()
            .set(index)
            .up()
            .execute()
            .await
            .map_err(|e| NetworkError::NetlinkError(e.to_string()))
    }

    /// Add an IPv4 address to an interface.
    pub async fn add_address_v4(
        &self,
        index: u32,
        addr: Ipv4Addr,
        prefix_len: u8,
    ) -> Result<(), NetworkError> {
        self.handle
            .address()
            .add(index, IpAddr::V4(addr), prefix_len)
            .execute()
            .await
            .map_err(|e| NetworkError::NetlinkError(e.to_string()))
    }

    /// Add an IPv6 address to an interface.
    pub async fn add_address_v6(
        &self,
        index: u32,
        addr: Ipv6Addr,
        prefix_len: u8,
    ) -> Result<(), NetworkError> {
        self.handle
            .address()
            .add(index, IpAddr::V6(addr), prefix_len)
            .execute()
            .await
            .map_err(|e| NetworkError::NetlinkError(e.to_string()))
    }

    /// Add an IPv4 default route.
    pub async fn add_route_v4(&self, gateway: Ipv4Addr, _index: u32) -> Result<(), NetworkError> {
        self.handle
            .route()
            .add()
            .v4()
            .gateway(gateway)
            .execute()
            .await
            .map_err(|e| NetworkError::NetlinkError(e.to_string()))
    }

    /// Add an IPv6 default route.
    pub async fn add_route_v6(&self, gateway: Ipv6Addr, index: u32) -> Result<(), NetworkError> {
        self.handle
            .route()
            .add()
            .v6()
            .gateway(gateway)
            .output_interface(index)
            .execute()
            .await
            .map_err(|e| NetworkError::NetlinkError(e.to_string()))
    }
}
