//! L3 router for inter-vNIC packet forwarding
//!
//! Maintains per-network routing tables with efficient LPM (Longest Prefix Match)
//! using prefix tries. Handles packet forwarding between vNICs in the same network.
//!
//! For public networks, packets without a local destination are returned to the caller
//! for forwarding to the TUN device (internet access).

use std::collections::HashMap;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::{Arc, RwLock};

use crossbeam_channel::Sender;
use ipnet::{Ipv4Net, Ipv6Net};
use nix::sys::eventfd::EventFd;
use prefix_trie::PrefixMap;
use smoltcp::wire::{EthernetProtocol, Ipv4Packet, Ipv6Packet};
use tracing::debug;

use super::buffer::{BufferPool, PoolBuffer};
use super::packet::{GATEWAY_MAC, parse_ethernet};
use super::worker::RoutedPacket;

/// Result of routing a packet
pub enum RouteResult {
    /// Packet was dropped (TTL expired, parse error, no route in non-public network)
    Dropped,
    /// Packet was successfully routed to a local NIC
    Routed,
    /// Packet should be sent to internet via TUN (public network, no local route)
    /// Contains the IP packet without Ethernet header in a PoolBuffer
    ToInternet(PoolBuffer),
}

/// Route entry in the routing table
#[derive(Clone, Debug)]
pub struct RouteEntry {
    /// Target NIC ID
    pub nic_id: String,
    /// Whether this is a directly connected route (vNIC's own address)
    pub direct: bool,
}

/// Channel and wakeup signal for a NIC
pub struct NicChannel {
    /// Channel sender for routed packets
    pub sender: Sender<RoutedPacket>,
    /// EventFd to wake up the worker's RX injection thread
    pub wakeup: Arc<EventFd>,
    /// NIC's MAC address for Ethernet header rewriting
    pub mac: [u8; 6],
}

/// Per-network router with efficient LPM using prefix tries
#[derive(Clone)]
pub struct NetworkRouter {
    inner: Arc<RwLock<NetworkRouterInner>>,
    #[allow(dead_code)]
    network_id: String,
    /// Shared buffer pool for zero-copy packet allocation
    pool: Arc<BufferPool>,
}

struct NetworkRouterInner {
    /// IPv4 routes with O(log n) LPM
    ipv4_routes: PrefixMap<Ipv4Net, RouteEntry>,
    /// IPv6 routes with O(log n) LPM
    ipv6_routes: PrefixMap<Ipv6Net, RouteEntry>,
    /// Channel senders per NIC for packet forwarding
    nic_channels: HashMap<String, NicChannel>,
    /// Whether this is a public network (allows internet access via TUN)
    is_public: bool,
}

impl NetworkRouter {
    /// Create a new router for a specific network
    ///
    /// # Arguments
    /// * `network_id` - Unique network identifier
    /// * `is_public` - If true, packets without local destination go to TUN (internet).
    ///   If false, such packets are dropped (network isolation).
    /// * `pool` - Shared buffer pool for zero-copy packet allocation
    pub fn new(network_id: String, is_public: bool, pool: Arc<BufferPool>) -> Self {
        Self {
            inner: Arc::new(RwLock::new(NetworkRouterInner {
                ipv4_routes: PrefixMap::new(),
                ipv6_routes: PrefixMap::new(),
                nic_channels: HashMap::new(),
                is_public,
            })),
            network_id,
            pool,
        }
    }

    /// Check if this router is for a public network
    pub fn is_public(&self) -> bool {
        self.inner.read().unwrap().is_public
    }

    /// Register a NIC's channel for receiving routed packets
    pub fn register_nic(&self, nic_id: String, channel: NicChannel) {
        let mut inner = self.inner.write().unwrap();
        inner.nic_channels.insert(nic_id, channel);
    }

    /// Unregister a NIC and remove its routes
    pub fn unregister_nic(&self, nic_id: &str) {
        let mut inner = self.inner.write().unwrap();
        inner.nic_channels.remove(nic_id);
        // Remove routes for this NIC
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

    /// Look up an IPv4 address using LPM - O(log n)
    pub fn lookup_ipv4(&self, addr: Ipv4Addr) -> Option<RouteEntry> {
        let inner = self.inner.read().unwrap();
        // Create a /32 prefix for the lookup address
        let key = Ipv4Net::new(addr, 32).ok()?;
        inner
            .ipv4_routes
            .get_lpm(&key)
            .map(|(_, entry)| entry.clone())
    }

    /// Look up an IPv6 address using LPM - O(log n)
    pub fn lookup_ipv6(&self, addr: Ipv6Addr) -> Option<RouteEntry> {
        let inner = self.inner.read().unwrap();
        // Create a /128 prefix for the lookup address
        let key = Ipv6Net::new(addr, 128).ok()?;
        inner
            .ipv6_routes
            .get_lpm(&key)
            .map(|(_, entry)| entry.clone())
    }

    /// Route a packet to its destination (zero-copy path)
    ///
    /// Returns:
    /// - `RouteResult::Routed` if packet was sent to a local NIC
    /// - `RouteResult::ToInternet(buffer)` if packet should go to TUN (public networks only)
    /// - `RouteResult::Dropped` if packet was dropped (TTL expired, parse error, or no route in non-public network)
    pub fn route_packet(&self, source_nic_id: &str, packet: &[u8]) -> RouteResult {
        // Parse the packet to determine destination
        let Some(frame) = parse_ethernet(packet) else {
            return RouteResult::Dropped;
        };

        // Ethernet header is 14 bytes
        const ETH_HEADER_LEN: usize = 14;

        // Allocate a PoolBuffer for the packet (single copy)
        let Some(mut buffer) = self.pool.alloc() else {
            debug!("Buffer pool exhausted, dropping packet");
            return RouteResult::Dropped;
        };

        // Copy packet into buffer (this is the ONLY copy in the routing path)
        let write_area = buffer.write_area();
        if packet.len() > write_area.len() {
            debug!(len = packet.len(), "Packet too large, dropping");
            return RouteResult::Dropped;
        }
        write_area[..packet.len()].copy_from_slice(packet);
        buffer.len = packet.len();

        let target_nic_id = match frame.ethertype() {
            EthernetProtocol::Ipv4 => {
                let Ok(ipv4) = Ipv4Packet::new_checked(frame.payload()) else {
                    return RouteResult::Dropped;
                };

                // Check TTL - drop if expired
                let ttl = ipv4.hop_limit();
                if ttl <= 1 {
                    debug!(ttl, "Dropping packet: TTL expired");
                    return RouteResult::Dropped;
                }

                let dst = ipv4.dst_addr();
                let target = self.lookup_ipv4(dst).map(|e| e.nic_id);

                // Decrement TTL IN-PLACE
                decrement_ipv4_ttl(&mut buffer.data_mut()[ETH_HEADER_LEN..]);

                target
            }
            EthernetProtocol::Ipv6 => {
                let Ok(ipv6) = Ipv6Packet::new_checked(frame.payload()) else {
                    return RouteResult::Dropped;
                };

                // Check Hop Limit - drop if expired
                let hop_limit = ipv6.hop_limit();
                if hop_limit <= 1 {
                    debug!(hop_limit, "Dropping packet: Hop Limit expired");
                    return RouteResult::Dropped;
                }

                let dst = ipv6.dst_addr();

                // Skip link-local addresses (handled locally)
                if dst.segments()[0] == 0xfe80 {
                    return RouteResult::Dropped;
                }

                let target = self.lookup_ipv6(dst).map(|e| e.nic_id);

                // Decrement Hop Limit IN-PLACE
                decrement_ipv6_hop_limit(&mut buffer.data_mut()[ETH_HEADER_LEN..]);

                target
            }
            _ => return RouteResult::Dropped,
        };

        // No local target found
        let Some(target) = target_nic_id else {
            // Check if this is a public network - if so, forward to internet via TUN
            let inner = self.inner.read().unwrap();
            if inner.is_public {
                // Strip Ethernet header IN-PLACE for TUN (no copy!)
                buffer.strip_eth_header();
                debug!(
                    source = %source_nic_id,
                    len = buffer.len,
                    "Forwarding to internet (TUN)"
                );
                return RouteResult::ToInternet(buffer);
            }
            // Non-public network: DROP - complete network isolation
            debug!(
                source = %source_nic_id,
                "Dropping packet: no local route (non-public network)"
            );
            return RouteResult::Dropped;
        };

        // Don't route back to same NIC
        if target == source_nic_id {
            return RouteResult::Dropped;
        }

        // Send to target NIC
        let inner = self.inner.read().unwrap();
        if let Some(channel) = inner.nic_channels.get(&target) {
            // Rewrite Ethernet header IN-PLACE for L3 routing:
            // - Dst MAC = target NIC's MAC
            // - Src MAC = gateway MAC (we are the router)
            rewrite_ethernet_header(buffer.data_mut(), channel.mac, GATEWAY_MAC);

            let routed = RoutedPacket {
                target_nic_id: target.clone(),
                buffer,
            };

            if channel.sender.send(routed).is_ok() {
                // Signal the target worker to wake up and process RX
                let _ = channel.wakeup.write(1);

                debug!(
                    source = %source_nic_id,
                    target = %target,
                    len = packet.len(),
                    "Routed packet"
                );
                return RouteResult::Routed;
            }
        }

        RouteResult::Dropped
    }
}

/// Rewrite Ethernet header with new MAC addresses
/// Ethernet header: dst_mac[6] + src_mac[6] + ethertype[2]
fn rewrite_ethernet_header(packet: &mut [u8], dst_mac: [u8; 6], src_mac: [u8; 6]) {
    if packet.len() < 14 {
        return;
    }
    packet[0..6].copy_from_slice(&dst_mac);
    packet[6..12].copy_from_slice(&src_mac);
}

/// Decrement IPv4 TTL and update header checksum
/// IPv4 header: TTL is at offset 8, checksum is at offset 10-11
fn decrement_ipv4_ttl(ip_packet: &mut [u8]) {
    if ip_packet.len() < 20 {
        return;
    }

    // Decrement TTL
    ip_packet[8] = ip_packet[8].saturating_sub(1);

    // Update header checksum incrementally
    // Since we decremented TTL by 1, we need to add 0x0100 to the checksum
    // (TTL is in the high byte of its 16-bit word in the checksum calculation)
    let old_check = u16::from_be_bytes([ip_packet[10], ip_packet[11]]);
    let mut new_check = old_check as u32 + 0x0100;

    // Handle one's complement overflow
    if new_check > 0xFFFF {
        new_check = (new_check & 0xFFFF) + 1;
    }

    ip_packet[10..12].copy_from_slice(&(new_check as u16).to_be_bytes());
}

/// Decrement IPv6 Hop Limit
/// IPv6 header: Hop Limit is at offset 7
fn decrement_ipv6_hop_limit(ip_packet: &mut [u8]) {
    if ip_packet.len() < 40 {
        return;
    }

    // Decrement Hop Limit (no checksum in IPv6 header)
    ip_packet[7] = ip_packet[7].saturating_sub(1);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_pool() -> Arc<BufferPool> {
        Arc::new(BufferPool::new().expect("Failed to create pool"))
    }

    #[test]
    fn test_ipv4_lpm() {
        let router = NetworkRouter::new("test-net".to_string(), false, test_pool());

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
    fn test_ipv6_lpm() {
        let router = NetworkRouter::new("test-net".to_string(), false, test_pool());

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
        let router = NetworkRouter::new("test-net".to_string(), false, test_pool());

        router.add_ipv4_route("10.0.0.0/24".parse().unwrap(), "nic-1".to_string(), false);
        router.add_ipv4_route("10.0.1.0/24".parse().unwrap(), "nic-2".to_string(), false);

        // Unregister nic-1
        router.unregister_nic("nic-1");

        // nic-1's routes should be gone
        assert!(router.lookup_ipv4(Ipv4Addr::new(10, 0, 0, 1)).is_none());

        // nic-2's routes should remain
        assert!(router.lookup_ipv4(Ipv4Addr::new(10, 0, 1, 1)).is_some());
    }

    #[test]
    fn test_network_isolation() {
        // Two separate routers for different networks
        let router_a = NetworkRouter::new("net-a".to_string(), false, test_pool());
        let router_b = NetworkRouter::new("net-b".to_string(), false, test_pool());

        // Same prefix in both networks
        router_a.add_ipv4_route("10.0.0.0/24".parse().unwrap(), "nic-a".to_string(), false);
        router_b.add_ipv4_route("10.0.0.0/24".parse().unwrap(), "nic-b".to_string(), false);

        // Each router returns its own NIC
        assert_eq!(
            router_a
                .lookup_ipv4(Ipv4Addr::new(10, 0, 0, 5))
                .unwrap()
                .nic_id,
            "nic-a"
        );
        assert_eq!(
            router_b
                .lookup_ipv4(Ipv4Addr::new(10, 0, 0, 5))
                .unwrap()
                .nic_id,
            "nic-b"
        );
    }

    #[test]
    fn test_decrement_ipv4_ttl() {
        // Minimal IPv4 header (20 bytes)
        // Version/IHL=0x45, TOS=0, Length=20, ID=0, Flags/Frag=0,
        // TTL=64, Protocol=0, Checksum, Src, Dst
        let mut packet = [
            0x45, 0x00, 0x00, 0x14, // Version, IHL, TOS, Total Length
            0x00, 0x00, 0x00, 0x00, // ID, Flags, Fragment Offset
            0x40, 0x00, 0x00, 0x00, // TTL=64, Protocol, Checksum (placeholder)
            0x0a, 0x00, 0x00, 0x01, // Src: 10.0.0.1
            0x0a, 0x00, 0x00, 0x02, // Dst: 10.0.0.2
        ];

        // Calculate correct checksum first
        let checksum = compute_ipv4_checksum(&packet);
        packet[10..12].copy_from_slice(&checksum.to_be_bytes());

        let original_ttl = packet[8];
        let original_checksum = u16::from_be_bytes([packet[10], packet[11]]);

        decrement_ipv4_ttl(&mut packet);

        // TTL should be decremented
        assert_eq!(packet[8], original_ttl - 1);

        // Verify checksum is still valid
        let new_checksum = compute_ipv4_checksum(&packet);
        assert_eq!(
            new_checksum, 0,
            "Checksum should be valid (0 when computed over header with checksum)"
        );

        // Checksum should have increased by 0x0100
        let updated_checksum = u16::from_be_bytes([packet[10], packet[11]]);
        assert!(updated_checksum > original_checksum);
    }

    #[test]
    fn test_decrement_ipv4_ttl_overflow() {
        // Test checksum overflow (when adding 0x0100 causes carry)
        let mut packet = [
            0x45, 0x00, 0x00, 0x14, 0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0xFF,
            0x00, // Checksum = 0xFF00 (will overflow when adding 0x0100)
            0x0a, 0x00, 0x00, 0x01, 0x0a, 0x00, 0x00, 0x02,
        ];

        decrement_ipv4_ttl(&mut packet);

        // TTL should be decremented
        assert_eq!(packet[8], 63);

        // Checksum should wrap around correctly
        let checksum = u16::from_be_bytes([packet[10], packet[11]]);
        assert_eq!(checksum, 0x0001); // 0xFF00 + 0x0100 = 0x10000 -> 0x0001
    }

    #[test]
    fn test_decrement_ipv6_hop_limit() {
        // Minimal IPv6 header (40 bytes)
        let mut packet = [
            0x60, 0x00, 0x00, 0x00, // Version, Traffic Class, Flow Label
            0x00, 0x00, 0x00, 0x40, // Payload Length, Next Header, Hop Limit=64
            // Source address (16 bytes)
            0xfd, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x01, // Destination address (16 bytes)
            0xfd, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x02,
        ];

        let original_hop_limit = packet[7];

        decrement_ipv6_hop_limit(&mut packet);

        // Hop Limit should be decremented
        assert_eq!(packet[7], original_hop_limit - 1);
    }

    /// Helper to compute IPv4 header checksum
    fn compute_ipv4_checksum(header: &[u8]) -> u16 {
        let mut sum: u32 = 0;
        for i in (0..20).step_by(2) {
            if i == 10 {
                continue; // Skip checksum field
            }
            sum += u16::from_be_bytes([header[i], header[i + 1]]) as u32;
        }
        // Add checksum field
        sum += u16::from_be_bytes([header[10], header[11]]) as u32;

        // Fold 32-bit sum to 16 bits
        while sum >> 16 != 0 {
            sum = (sum & 0xFFFF) + (sum >> 16);
        }

        !(sum as u16)
    }

    #[test]
    fn test_rewrite_ethernet_header() {
        // Ethernet frame: dst[6] + src[6] + ethertype[2] + payload
        let mut packet = vec![
            // Dst MAC: 11:11:11:11:11:11
            0x11, 0x11, 0x11, 0x11, 0x11, 0x11, // Src MAC: 22:22:22:22:22:22
            0x22, 0x22, 0x22, 0x22, 0x22, 0x22, // EtherType: IPv4 (0x0800)
            0x08, 0x00, // Payload
            0xAA, 0xBB, 0xCC,
        ];

        let new_dst = [0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x01];
        let new_src = [0xCA, 0xFE, 0xBA, 0xBE, 0x00, 0x02];

        rewrite_ethernet_header(&mut packet, new_dst, new_src);

        // Check dst MAC was rewritten
        assert_eq!(&packet[0..6], &new_dst);
        // Check src MAC was rewritten
        assert_eq!(&packet[6..12], &new_src);
        // EtherType should be unchanged
        assert_eq!(&packet[12..14], &[0x08, 0x00]);
        // Payload should be unchanged
        assert_eq!(&packet[14..], &[0xAA, 0xBB, 0xCC]);
    }
}
