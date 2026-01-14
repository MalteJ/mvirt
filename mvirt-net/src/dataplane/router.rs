//! L3 router for inter-vNIC packet forwarding
//!
//! Maintains a routing table and handles packet forwarding between vNICs.

use std::collections::HashMap;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::{Arc, RwLock};

use crossbeam_channel::Sender;
use ipnet::{Ipv4Net, Ipv6Net};
use smoltcp::wire::{EthernetProtocol, Ipv4Packet, Ipv6Packet};
use tracing::debug;

use super::packet::parse_ethernet;
use super::worker::RoutedPacket;

/// Route entry in the routing table
#[derive(Clone, Debug)]
pub struct RouteEntry {
    /// Target NIC ID
    pub nic_id: String,
    /// Whether this is a directly connected route (vNIC's own address)
    pub direct: bool,
}

/// Thread-safe routing table
#[derive(Clone)]
pub struct Router {
    inner: Arc<RwLock<RouterInner>>,
}

struct RouterInner {
    /// IPv4 routes: prefix -> (nic_id, direct)
    ipv4_routes: HashMap<Ipv4Net, RouteEntry>,
    /// IPv6 routes: prefix -> (nic_id, direct)
    ipv6_routes: HashMap<Ipv6Net, RouteEntry>,
    /// Channel senders per NIC for packet forwarding
    nic_channels: HashMap<String, Sender<RoutedPacket>>,
}

impl Router {
    /// Create a new router
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(RouterInner {
                ipv4_routes: HashMap::new(),
                ipv6_routes: HashMap::new(),
                nic_channels: HashMap::new(),
            })),
        }
    }

    /// Register a NIC's channel for receiving routed packets
    pub fn register_nic(&self, nic_id: String, sender: Sender<RoutedPacket>) {
        let mut inner = self.inner.write().unwrap();
        inner.nic_channels.insert(nic_id, sender);
    }

    /// Unregister a NIC
    pub fn unregister_nic(&self, nic_id: &str) {
        let mut inner = self.inner.write().unwrap();
        inner.nic_channels.remove(nic_id);
        // Also remove routes for this NIC
        inner.ipv4_routes.retain(|_, v| v.nic_id != nic_id);
        inner.ipv6_routes.retain(|_, v| v.nic_id != nic_id);
    }

    /// Add an IPv4 route
    pub fn add_ipv4_route(&self, prefix: Ipv4Net, nic_id: String, direct: bool) {
        let mut inner = self.inner.write().unwrap();
        inner
            .ipv4_routes
            .insert(prefix, RouteEntry { nic_id, direct });
    }

    /// Add an IPv6 route
    pub fn add_ipv6_route(&self, prefix: Ipv6Net, nic_id: String, direct: bool) {
        let mut inner = self.inner.write().unwrap();
        inner
            .ipv6_routes
            .insert(prefix, RouteEntry { nic_id, direct });
    }

    /// Remove an IPv4 route
    pub fn remove_ipv4_route(&self, prefix: &Ipv4Net) {
        let mut inner = self.inner.write().unwrap();
        inner.ipv4_routes.remove(prefix);
    }

    /// Remove an IPv6 route
    pub fn remove_ipv6_route(&self, prefix: &Ipv6Net) {
        let mut inner = self.inner.write().unwrap();
        inner.ipv6_routes.remove(prefix);
    }

    /// Look up an IPv4 address and return the target NIC ID
    pub fn lookup_ipv4(&self, addr: Ipv4Addr) -> Option<RouteEntry> {
        let inner = self.inner.read().unwrap();
        // Find the most specific (longest prefix) match
        let mut best_match: Option<(u8, RouteEntry)> = None;

        for (prefix, entry) in &inner.ipv4_routes {
            if prefix.contains(&addr) {
                let prefix_len = prefix.prefix_len();
                if best_match.is_none() || prefix_len > best_match.as_ref().unwrap().0 {
                    best_match = Some((prefix_len, entry.clone()));
                }
            }
        }

        best_match.map(|(_, entry)| entry)
    }

    /// Look up an IPv6 address and return the target NIC ID
    pub fn lookup_ipv6(&self, addr: Ipv6Addr) -> Option<RouteEntry> {
        let inner = self.inner.read().unwrap();
        // Find the most specific (longest prefix) match
        let mut best_match: Option<(u8, RouteEntry)> = None;

        for (prefix, entry) in &inner.ipv6_routes {
            if prefix.contains(&addr) {
                let prefix_len = prefix.prefix_len();
                if best_match.is_none() || prefix_len > best_match.as_ref().unwrap().0 {
                    best_match = Some((prefix_len, entry.clone()));
                }
            }
        }

        best_match.map(|(_, entry)| entry)
    }

    /// Route a packet to its destination
    /// Returns true if the packet was routed, false if no route found
    pub fn route_packet(&self, source_nic_id: &str, packet: &[u8]) -> bool {
        // Parse the packet to determine destination
        let Some(frame) = parse_ethernet(packet) else {
            return false;
        };

        let target_nic_id = match frame.ethertype() {
            EthernetProtocol::Ipv4 => {
                let Ok(ipv4) = Ipv4Packet::new_checked(frame.payload()) else {
                    return false;
                };
                let dst = Ipv4Addr::from(ipv4.dst_addr().0);
                self.lookup_ipv4(dst).map(|e| e.nic_id)
            }
            EthernetProtocol::Ipv6 => {
                let Ok(ipv6) = Ipv6Packet::new_checked(frame.payload()) else {
                    return false;
                };
                let dst = Ipv6Addr::from(ipv6.dst_addr().0);

                // Skip link-local addresses (handled locally)
                if dst.segments()[0] == 0xfe80 {
                    return false;
                }

                self.lookup_ipv6(dst).map(|e| e.nic_id)
            }
            _ => None,
        };

        let Some(target) = target_nic_id else {
            return false;
        };

        // Don't route back to same NIC
        if target == source_nic_id {
            return false;
        }

        // Send to target NIC
        let inner = self.inner.read().unwrap();
        if let Some(sender) = inner.nic_channels.get(&target) {
            let routed = RoutedPacket {
                target_nic_id: target.clone(),
                data: packet.to_vec(),
            };

            if sender.send(routed).is_ok() {
                debug!(
                    source = %source_nic_id,
                    target = %target,
                    len = packet.len(),
                    "Routed packet"
                );
                return true;
            }
        }

        false
    }
}

impl Default for Router {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ipv4_routing() {
        let router = Router::new();

        // Add a /24 network route
        router.add_ipv4_route("10.0.0.0/24".parse().unwrap(), "nic-1".to_string(), false);

        // Add a more specific /32 route
        router.add_ipv4_route("10.0.0.5/32".parse().unwrap(), "nic-2".to_string(), true);

        // Test that specific route wins
        let entry = router.lookup_ipv4(Ipv4Addr::new(10, 0, 0, 5)).unwrap();
        assert_eq!(entry.nic_id, "nic-2");
        assert!(entry.direct);

        // Test that /24 route is used for other addresses
        let entry = router.lookup_ipv4(Ipv4Addr::new(10, 0, 0, 10)).unwrap();
        assert_eq!(entry.nic_id, "nic-1");

        // Test no route found
        assert!(router.lookup_ipv4(Ipv4Addr::new(192, 168, 0, 1)).is_none());
    }

    #[test]
    fn test_ipv6_routing() {
        let router = Router::new();

        // Add a /64 network route
        router.add_ipv6_route("fd00::/64".parse().unwrap(), "nic-1".to_string(), false);

        // Add a more specific /128 route
        router.add_ipv6_route("fd00::5/128".parse().unwrap(), "nic-2".to_string(), true);

        // Test that specific route wins
        let entry = router
            .lookup_ipv6(Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 5))
            .unwrap();
        assert_eq!(entry.nic_id, "nic-2");

        // Test that /64 route is used for other addresses
        let entry = router
            .lookup_ipv6(Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 10))
            .unwrap();
        assert_eq!(entry.nic_id, "nic-1");
    }

    #[test]
    fn test_unregister_removes_routes() {
        let router = Router::new();

        router.add_ipv4_route("10.0.0.0/24".parse().unwrap(), "nic-1".to_string(), false);
        router.add_ipv4_route("10.0.1.0/24".parse().unwrap(), "nic-2".to_string(), false);

        // Unregister nic-1
        router.unregister_nic("nic-1");

        // nic-1's routes should be gone
        assert!(router.lookup_ipv4(Ipv4Addr::new(10, 0, 0, 1)).is_none());

        // nic-2's routes should remain
        assert!(router.lookup_ipv4(Ipv4Addr::new(10, 0, 1, 1)).is_some());
    }
}
