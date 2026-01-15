//! NDP responder and Router Advertisement sender
//!
//! Handles:
//! - Neighbor Solicitation for gateway (fe80::1) -> Neighbor Advertisement
//! - Router Solicitation -> Router Advertisement

use smoltcp::wire::{
    EthernetAddress, EthernetFrame, EthernetProtocol, EthernetRepr, Icmpv6Message, Icmpv6Packet,
    Icmpv6Repr, IpProtocol, Ipv6Address, Ipv6Packet, Ipv6Repr, NdiscNeighborFlags, NdiscRepr,
    NdiscRouterFlags, RawHardwareAddress,
};
use tracing::debug;

use super::packet::{GATEWAY_MAC, parse_ethernet};

/// IPv6 link-local gateway address
pub const GATEWAY_IPV6: Ipv6Address = Ipv6Address::new(0xfe80, 0, 0, 0, 0, 0, 0, 1);

/// IPv6 all-nodes multicast address
pub const ALL_NODES_MULTICAST: Ipv6Address = Ipv6Address::new(0xff02, 0, 0, 0, 0, 0, 0, 1);

/// NDP responder configuration
pub struct NdpResponder {
    /// Virtual MAC address for this vNIC
    #[allow(dead_code)]
    nic_mac: EthernetAddress,
    /// Network prefix for Router Advertisements (if any)
    prefix: Option<(Ipv6Address, u8)>,
    /// DNS servers to advertise in RA
    #[allow(dead_code)]
    dns_servers: Vec<Ipv6Address>,
}

impl NdpResponder {
    /// Create a new NDP responder
    pub fn new(nic_mac: [u8; 6]) -> Self {
        Self {
            nic_mac: EthernetAddress::from_bytes(&nic_mac),
            prefix: None,
            dns_servers: Vec::new(),
        }
    }

    /// Set the IPv6 prefix for Router Advertisements
    pub fn set_prefix(&mut self, prefix: Ipv6Address, prefix_len: u8) {
        self.prefix = Some((prefix, prefix_len));
    }

    /// Add a DNS server for Router Advertisements
    #[allow(dead_code)]
    pub fn add_dns_server(&mut self, server: Ipv6Address) {
        self.dns_servers.push(server);
    }

    /// Process an incoming packet and potentially generate an NDP response
    pub fn process(&self, packet: &[u8]) -> Option<Vec<u8>> {
        let frame = parse_ethernet(packet)?;

        // Only process IPv6 packets
        if frame.ethertype() != EthernetProtocol::Ipv6 {
            return None;
        }

        let ipv6 = Ipv6Packet::new_checked(frame.payload()).ok()?;

        // Only process ICMPv6 packets
        if ipv6.next_header() != IpProtocol::Icmpv6 {
            return None;
        }

        let icmpv6 = Icmpv6Packet::new_checked(ipv6.payload()).ok()?;
        let src_addr = ipv6.src_addr();
        let dst_addr = ipv6.dst_addr();

        match icmpv6.msg_type() {
            Icmpv6Message::NeighborSolicit => {
                self.handle_neighbor_solicitation(&icmpv6, src_addr, dst_addr, frame.src_addr())
            }
            Icmpv6Message::RouterSolicit => self.handle_router_solicitation(src_addr),
            _ => None,
        }
    }

    /// Handle Neighbor Solicitation for gateway address
    fn handle_neighbor_solicitation(
        &self,
        icmpv6: &Icmpv6Packet<&[u8]>,
        src_addr: Ipv6Address,
        dst_addr: Ipv6Address,
        src_mac: EthernetAddress,
    ) -> Option<Vec<u8>> {
        let icmp_repr = Icmpv6Repr::parse(
            &src_addr,
            &dst_addr,
            icmpv6,
            &smoltcp::phy::ChecksumCapabilities::default(),
        )
        .ok()?;

        if let Icmpv6Repr::Ndisc(NdiscRepr::NeighborSolicit { target_addr, .. }) = icmp_repr {
            // Only respond if asking for gateway
            if target_addr == GATEWAY_IPV6 {
                debug!(
                    target_ip = %target_addr,
                    source_ip = %src_addr,
                    source_mac = %src_mac,
                    "NDP Neighbor Solicitation received"
                );

                debug!(
                    target_ip = %target_addr,
                    target_mac = %EthernetAddress::from_bytes(&GATEWAY_MAC),
                    "Sending NDP Neighbor Advertisement"
                );

                return Some(self.build_neighbor_advertisement(src_addr, src_mac));
            }
        }

        None
    }

    /// Build Neighbor Advertisement response
    fn build_neighbor_advertisement(
        &self,
        dst_addr: Ipv6Address,
        dst_mac: EthernetAddress,
    ) -> Vec<u8> {
        let gateway_mac = EthernetAddress::from_bytes(&GATEWAY_MAC);
        let lladdr = RawHardwareAddress::from_bytes(&GATEWAY_MAC);

        let icmp_repr = Icmpv6Repr::Ndisc(NdiscRepr::NeighborAdvert {
            flags: NdiscNeighborFlags::ROUTER | NdiscNeighborFlags::SOLICITED,
            target_addr: GATEWAY_IPV6,
            lladdr: Some(lladdr),
        });

        let ipv6_repr = Ipv6Repr {
            src_addr: GATEWAY_IPV6,
            dst_addr,
            next_header: IpProtocol::Icmpv6,
            payload_len: icmp_repr.buffer_len(),
            hop_limit: 255,
        };

        let eth_repr = EthernetRepr {
            src_addr: gateway_mac,
            dst_addr: dst_mac,
            ethertype: EthernetProtocol::Ipv6,
        };

        let total_len = eth_repr.buffer_len() + ipv6_repr.buffer_len() + icmp_repr.buffer_len();
        let mut buffer = vec![0u8; total_len];

        // Build Ethernet frame
        let mut frame = EthernetFrame::new_unchecked(&mut buffer);
        eth_repr.emit(&mut frame);

        // Build IPv6 packet
        let mut ipv6_packet = Ipv6Packet::new_unchecked(frame.payload_mut());
        ipv6_repr.emit(&mut ipv6_packet);

        // Build ICMPv6 packet
        let mut icmp_packet = Icmpv6Packet::new_unchecked(ipv6_packet.payload_mut());
        icmp_repr.emit(
            &GATEWAY_IPV6,
            &dst_addr,
            &mut icmp_packet,
            &smoltcp::phy::ChecksumCapabilities::default(),
        );

        buffer
    }

    /// Handle Router Solicitation
    fn handle_router_solicitation(&self, src_addr: Ipv6Address) -> Option<Vec<u8>> {
        debug!(source_ip = %src_addr, "NDP Router Solicitation received");

        debug!(
            router_ip = %GATEWAY_IPV6,
            router_mac = %EthernetAddress::from_bytes(&GATEWAY_MAC),
            prefix = ?self.prefix,
            "Sending NDP Router Advertisement"
        );

        Some(self.build_router_advertisement(src_addr))
    }

    /// Build Router Advertisement
    fn build_router_advertisement(&self, dst_addr: Ipv6Address) -> Vec<u8> {
        let gateway_mac = EthernetAddress::from_bytes(&GATEWAY_MAC);
        let lladdr = RawHardwareAddress::from_bytes(&GATEWAY_MAC);

        // Build RA with M=1, O=1 (managed config via DHCPv6)
        let icmp_repr = Icmpv6Repr::Ndisc(NdiscRepr::RouterAdvert {
            hop_limit: 64,
            flags: NdiscRouterFlags::MANAGED | NdiscRouterFlags::OTHER,
            router_lifetime: smoltcp::time::Duration::from_secs(1800),
            reachable_time: smoltcp::time::Duration::from_secs(0),
            retrans_time: smoltcp::time::Duration::from_secs(0),
            lladdr: Some(lladdr),
            mtu: Some(1500),
            prefix_info: self.prefix.map(|(prefix, prefix_len)| {
                smoltcp::wire::NdiscPrefixInformation {
                    prefix_len,
                    flags: smoltcp::wire::NdiscPrefixInfoFlags::ON_LINK
                        | smoltcp::wire::NdiscPrefixInfoFlags::ADDRCONF,
                    valid_lifetime: smoltcp::time::Duration::from_secs(86400),
                    preferred_lifetime: smoltcp::time::Duration::from_secs(14400),
                    prefix,
                }
            }),
        });

        let ipv6_repr = Ipv6Repr {
            src_addr: GATEWAY_IPV6,
            dst_addr,
            next_header: IpProtocol::Icmpv6,
            payload_len: icmp_repr.buffer_len(),
            hop_limit: 255,
        };

        // For RA, use all-nodes multicast MAC if dst is multicast
        let dst_mac = if dst_addr.is_multicast() {
            // Convert IPv6 multicast to Ethernet multicast
            let octets = dst_addr.octets();
            EthernetAddress::from_bytes(&[
                0x33, 0x33, octets[12], octets[13], octets[14], octets[15],
            ])
        } else {
            // Unicast - we need the actual MAC, but for simplicity broadcast
            EthernetAddress::BROADCAST
        };

        let eth_repr = EthernetRepr {
            src_addr: gateway_mac,
            dst_addr: dst_mac,
            ethertype: EthernetProtocol::Ipv6,
        };

        let total_len = eth_repr.buffer_len() + ipv6_repr.buffer_len() + icmp_repr.buffer_len();
        let mut buffer = vec![0u8; total_len];

        // Build Ethernet frame
        let mut frame = EthernetFrame::new_unchecked(&mut buffer);
        eth_repr.emit(&mut frame);

        // Build IPv6 packet
        let mut ipv6_packet = Ipv6Packet::new_unchecked(frame.payload_mut());
        ipv6_repr.emit(&mut ipv6_packet);

        // Build ICMPv6 packet
        let mut icmp_packet = Icmpv6Packet::new_unchecked(ipv6_packet.payload_mut());
        icmp_repr.emit(
            &GATEWAY_IPV6,
            &dst_addr,
            &mut icmp_packet,
            &smoltcp::phy::ChecksumCapabilities::default(),
        );

        buffer
    }

    /// Build an unsolicited Router Advertisement (for periodic sending)
    pub fn build_unsolicited_ra(&self) -> Vec<u8> {
        self.build_router_advertisement(ALL_NODES_MULTICAST)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gateway_ipv6() {
        assert!(GATEWAY_IPV6.is_unicast_link_local());
    }

    #[test]
    fn test_build_unsolicited_ra() {
        // No prefix - all addresses via DHCPv6, forces routing through gateway
        let responder = NdpResponder::new([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);

        let ra = responder.build_unsolicited_ra();
        assert!(!ra.is_empty());

        // Parse and verify
        let frame = parse_ethernet(&ra).unwrap();
        assert_eq!(frame.ethertype(), EthernetProtocol::Ipv6);
    }
}
