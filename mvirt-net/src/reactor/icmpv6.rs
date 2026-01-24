//! ICMPv6 handler for vhost-user interfaces.
//!
//! This module handles:
//! - Neighbor Solicitation (NS) → Neighbor Advertisement (NA) for gateway resolution
//! - Router Solicitation (RS) → Router Advertisement (RA) for IPv6 configuration
//! - Echo Request → Echo Reply for gateway ping (fe80::1)
//!
//! The RA does NOT include a prefix for SLAAC - VMs must use DHCPv6 for addressing.

use super::{GATEWAY_IPV6_LINK_LOCAL, GATEWAY_MAC, NicConfig};
use smoltcp::wire::{
    EthernetAddress, EthernetFrame, EthernetProtocol, EthernetRepr, Icmpv6Message, Icmpv6Packet,
    IpProtocol, Ipv6Address, Ipv6Packet, Ipv6Repr,
};
use std::net::Ipv6Addr;
use tracing::debug;

/// Ethernet header size
const ETHERNET_HEADER_SIZE: usize = 14;

/// IPv6 header size
const IPV6_HEADER_SIZE: usize = 40;

/// Handle an ICMPv6 packet from a VM.
///
/// Returns a response packet for NS (Neighbor Solicitation) or RS (Router Solicitation).
pub fn handle_icmpv6_packet(
    nic_config: &NicConfig,
    virtio_hdr: &[u8],
    ethernet_frame: &[u8],
) -> Option<Vec<u8>> {
    // Parse Ethernet frame
    let eth_frame = EthernetFrame::new_checked(ethernet_frame).ok()?;

    if eth_frame.ethertype() != EthernetProtocol::Ipv6 {
        return None;
    }

    // Parse IPv6 packet
    let ipv6_packet = Ipv6Packet::new_checked(eth_frame.payload()).ok()?;

    if ipv6_packet.next_header() != IpProtocol::Icmpv6 {
        return None;
    }

    // Parse ICMPv6 packet
    let icmpv6_packet = Icmpv6Packet::new_checked(ipv6_packet.payload()).ok()?;

    let src_addr = ipv6_packet.src_addr();
    let src_mac = eth_frame.src_addr();

    let dst_addr = ipv6_packet.dst_addr();

    // For NS/NA, we need the raw ICMPv6 data to extract the target address
    // because smoltcp's payload() for NS returns only options, not the target address.
    let icmpv6_raw = ipv6_packet.payload();

    match icmpv6_packet.msg_type() {
        Icmpv6Message::NeighborSolicit => {
            handle_neighbor_solicitation(nic_config, virtio_hdr, icmpv6_raw, src_addr, src_mac)
        }
        Icmpv6Message::RouterSolicit => {
            handle_router_solicitation(nic_config, virtio_hdr, src_addr, src_mac)
        }
        Icmpv6Message::EchoRequest => handle_echo_request(
            nic_config, virtio_hdr, icmpv6_raw, src_addr, src_mac, dst_addr,
        ),
        _ => None,
    }
}

/// Handle Neighbor Solicitation - respond with Neighbor Advertisement for gateway.
fn handle_neighbor_solicitation(
    nic_config: &NicConfig,
    virtio_hdr: &[u8],
    icmpv6_raw: &[u8],
    src_addr: Ipv6Address,
    src_mac: EthernetAddress,
) -> Option<Vec<u8>> {
    // NS packet structure (raw ICMPv6 data):
    // - Type (1 byte) at offset 0
    // - Code (1 byte) at offset 1
    // - Checksum (2 bytes) at offset 2-3
    // - Reserved (4 bytes) at offset 4-7
    // - Target Address (16 bytes) at offset 8-23
    // - Options (variable) starting at offset 24

    // Minimum NS length: type(1) + code(1) + checksum(2) + reserved(4) + target(16) = 24
    if icmpv6_raw.len() < 24 {
        return None;
    }

    // Get target address at bytes 8-23
    let target_bytes: [u8; 16] = icmpv6_raw[8..24].try_into().ok()?;
    let target_addr = Ipv6Address::from_bytes(&target_bytes);

    // Convert to std Ipv6Addr for comparison
    let target_v6 = Ipv6Addr::from(target_addr.0);
    let gateway_ll = GATEWAY_IPV6_LINK_LOCAL;

    // Also check if there's a configured gateway
    let gateway_v6 = nic_config.ipv6_gateway;

    // Respond if target is our link-local gateway or configured gateway
    let should_respond =
        target_v6 == gateway_ll || gateway_v6.map(|g| target_v6 == g).unwrap_or(false);

    if !should_respond {
        debug!(
            target = %target_addr,
            gateway_ll = %gateway_ll,
            gateway_v6 = ?gateway_v6,
            "NS not for gateway, ignoring"
        );
        return None;
    }

    debug!(
        src = %src_addr,
        target = %target_addr,
        "NS for gateway, sending NA"
    );

    build_neighbor_advertisement(virtio_hdr, src_addr, src_mac, target_addr)
}

/// Build a Neighbor Advertisement response.
fn build_neighbor_advertisement(
    virtio_hdr: &[u8],
    dst_addr: Ipv6Address,
    dst_mac: EthernetAddress,
    target_addr: Ipv6Address,
) -> Option<Vec<u8>> {
    let gateway_mac = EthernetAddress(GATEWAY_MAC);
    let gateway_ll = Ipv6Address::from_bytes(&GATEWAY_IPV6_LINK_LOCAL.octets());

    // NA packet: ICMPv6 type (1) + code (1) + checksum (2) + flags (4) + target (16) + TLLAO (8)
    // Total ICMPv6 payload: 32 bytes
    let icmpv6_len = 32;
    let ip_len = IPV6_HEADER_SIZE + icmpv6_len;
    let virtio_hdr_size = virtio_hdr.len();
    let total_len = virtio_hdr_size + ETHERNET_HEADER_SIZE + ip_len;

    let mut packet = vec![0u8; total_len];

    // Virtio header (zeroed)
    packet[..virtio_hdr_size].fill(0);

    // Ethernet header
    let eth_repr = EthernetRepr {
        src_addr: gateway_mac,
        dst_addr: dst_mac,
        ethertype: EthernetProtocol::Ipv6,
    };
    let mut eth_frame = EthernetFrame::new_unchecked(&mut packet[virtio_hdr_size..]);
    eth_repr.emit(&mut eth_frame);

    // IPv6 header
    let ipv6_repr = Ipv6Repr {
        src_addr: gateway_ll,
        dst_addr,
        next_header: IpProtocol::Icmpv6,
        payload_len: icmpv6_len,
        hop_limit: 255,
    };
    let mut ipv6_packet = Ipv6Packet::new_unchecked(eth_frame.payload_mut());
    ipv6_repr.emit(&mut ipv6_packet);

    // ICMPv6 NA
    let icmpv6_start = virtio_hdr_size + ETHERNET_HEADER_SIZE + IPV6_HEADER_SIZE;
    let icmpv6_data = &mut packet[icmpv6_start..];

    // Type: Neighbor Advertisement (136)
    icmpv6_data[0] = 136;
    // Code: 0
    icmpv6_data[1] = 0;
    // Checksum: placeholder
    icmpv6_data[2..4].fill(0);
    // Flags: Solicited (0x40) | Override (0x20) = 0x60
    icmpv6_data[4] = 0x60;
    icmpv6_data[5..8].fill(0); // Reserved
    // Target address
    icmpv6_data[8..24].copy_from_slice(&target_addr.0);
    // Target Link-Layer Address Option (TLLAO)
    icmpv6_data[24] = 2; // Type: Target Link-Layer Address
    icmpv6_data[25] = 1; // Length: 1 (in 8-byte units)
    icmpv6_data[26..32].copy_from_slice(&GATEWAY_MAC);

    // Compute ICMPv6 checksum
    let checksum = compute_icmpv6_checksum(&gateway_ll, &dst_addr, &icmpv6_data[..icmpv6_len]);
    icmpv6_data[2..4].copy_from_slice(&checksum.to_be_bytes());

    debug!(
        dst = %dst_addr,
        target = %target_addr,
        "NA built"
    );

    Some(packet)
}

/// Handle Router Solicitation - respond with Router Advertisement.
fn handle_router_solicitation(
    nic_config: &NicConfig,
    virtio_hdr: &[u8],
    src_addr: Ipv6Address,
    src_mac: EthernetAddress,
) -> Option<Vec<u8>> {
    debug!(
        src = %src_addr,
        "RS received, sending RA with M flag (use DHCPv6)"
    );

    build_router_advertisement(nic_config, virtio_hdr, src_addr, src_mac)
}

/// Handle Echo Request - respond with Echo Reply if destination is gateway.
fn handle_echo_request(
    nic_config: &NicConfig,
    virtio_hdr: &[u8],
    icmpv6_raw: &[u8],
    src_addr: Ipv6Address,
    src_mac: EthernetAddress,
    dst_addr: Ipv6Address,
) -> Option<Vec<u8>> {
    // Echo Request structure (raw ICMPv6 data):
    // - Type (1 byte) at offset 0 = 128
    // - Code (1 byte) at offset 1 = 0
    // - Checksum (2 bytes) at offset 2-3
    // - Identifier (2 bytes) at offset 4-5
    // - Sequence Number (2 bytes) at offset 6-7
    // - Data (variable) starting at offset 8

    // Minimum Echo Request length: type(1) + code(1) + checksum(2) + id(2) + seq(2) = 8
    if icmpv6_raw.len() < 8 {
        return None;
    }

    // Check if destination is our gateway address
    let dst_v6 = Ipv6Addr::from(dst_addr.0);
    let gateway_ll = GATEWAY_IPV6_LINK_LOCAL;
    let gateway_v6 = nic_config.ipv6_gateway;

    let is_for_gateway = dst_v6 == gateway_ll || gateway_v6.map(|g| dst_v6 == g).unwrap_or(false);

    if !is_for_gateway {
        return None;
    }

    debug!(
        src = %src_addr,
        dst = %dst_addr,
        "Echo Request for gateway, sending Echo Reply"
    );

    build_echo_reply(virtio_hdr, icmpv6_raw, src_addr, src_mac)
}

/// Build an Echo Reply response.
fn build_echo_reply(
    virtio_hdr: &[u8],
    echo_request: &[u8],
    dst_addr: Ipv6Address,
    dst_mac: EthernetAddress,
) -> Option<Vec<u8>> {
    let gateway_mac = EthernetAddress(GATEWAY_MAC);
    let gateway_ll = Ipv6Address::from_bytes(&GATEWAY_IPV6_LINK_LOCAL.octets());

    // Echo Reply has same structure as Echo Request, just different type
    // Copy the entire request and change the type to 129 (Echo Reply)
    let icmpv6_len = echo_request.len();
    let ip_len = IPV6_HEADER_SIZE + icmpv6_len;
    let virtio_hdr_size = virtio_hdr.len();
    let total_len = virtio_hdr_size + ETHERNET_HEADER_SIZE + ip_len;

    let mut packet = vec![0u8; total_len];

    // Virtio header (zeroed)
    packet[..virtio_hdr_size].fill(0);

    // Ethernet header
    let eth_repr = EthernetRepr {
        src_addr: gateway_mac,
        dst_addr: dst_mac,
        ethertype: EthernetProtocol::Ipv6,
    };
    let mut eth_frame = EthernetFrame::new_unchecked(&mut packet[virtio_hdr_size..]);
    eth_repr.emit(&mut eth_frame);

    // IPv6 header
    let ipv6_repr = Ipv6Repr {
        src_addr: gateway_ll,
        dst_addr,
        next_header: IpProtocol::Icmpv6,
        payload_len: icmpv6_len,
        hop_limit: 64,
    };
    let mut ipv6_packet = Ipv6Packet::new_unchecked(eth_frame.payload_mut());
    ipv6_repr.emit(&mut ipv6_packet);

    // ICMPv6 Echo Reply - copy request and change type
    let icmpv6_start = virtio_hdr_size + ETHERNET_HEADER_SIZE + IPV6_HEADER_SIZE;
    let icmpv6_data = &mut packet[icmpv6_start..];

    // Copy the echo request data (id, seq, payload)
    icmpv6_data[..icmpv6_len].copy_from_slice(echo_request);

    // Type: Echo Reply (129)
    icmpv6_data[0] = 129;
    // Code: 0 (already copied, but ensure it's 0)
    icmpv6_data[1] = 0;
    // Checksum: clear and recompute
    icmpv6_data[2..4].fill(0);

    // Compute ICMPv6 checksum
    let checksum = compute_icmpv6_checksum(&gateway_ll, &dst_addr, icmpv6_data);
    icmpv6_data[2..4].copy_from_slice(&checksum.to_be_bytes());

    Some(packet)
}

/// Build a Router Advertisement response.
///
/// This RA does NOT include a prefix (no SLAAC). It sets the M flag to indicate
/// that VMs must use DHCPv6 to obtain addresses.
fn build_router_advertisement(
    _nic_config: &NicConfig,
    virtio_hdr: &[u8],
    dst_addr: Ipv6Address,
    dst_mac: EthernetAddress,
) -> Option<Vec<u8>> {
    let gateway_mac = EthernetAddress(GATEWAY_MAC);
    let gateway_ll = Ipv6Address::from_bytes(&GATEWAY_IPV6_LINK_LOCAL.octets());

    // RA packet: ICMPv6 type (1) + code (1) + checksum (2) + hop limit (1) + flags (1) +
    //            router lifetime (2) + reachable time (4) + retrans timer (4) + SLLAO (8)
    // Total ICMPv6 payload: 24 bytes
    let icmpv6_len = 24;
    let ip_len = IPV6_HEADER_SIZE + icmpv6_len;
    let virtio_hdr_size = virtio_hdr.len();
    let total_len = virtio_hdr_size + ETHERNET_HEADER_SIZE + ip_len;

    let mut packet = vec![0u8; total_len];

    // Virtio header (zeroed)
    packet[..virtio_hdr_size].fill(0);

    // Ethernet header
    let eth_repr = EthernetRepr {
        src_addr: gateway_mac,
        dst_addr: dst_mac,
        ethertype: EthernetProtocol::Ipv6,
    };
    let mut eth_frame = EthernetFrame::new_unchecked(&mut packet[virtio_hdr_size..]);
    eth_repr.emit(&mut eth_frame);

    // IPv6 header
    let ipv6_repr = Ipv6Repr {
        src_addr: gateway_ll,
        dst_addr,
        next_header: IpProtocol::Icmpv6,
        payload_len: icmpv6_len,
        hop_limit: 255,
    };
    let mut ipv6_packet = Ipv6Packet::new_unchecked(eth_frame.payload_mut());
    ipv6_repr.emit(&mut ipv6_packet);

    // ICMPv6 RA
    let icmpv6_start = virtio_hdr_size + ETHERNET_HEADER_SIZE + IPV6_HEADER_SIZE;
    let icmpv6_data = &mut packet[icmpv6_start..];

    // Type: Router Advertisement (134)
    icmpv6_data[0] = 134;
    // Code: 0
    icmpv6_data[1] = 0;
    // Checksum: placeholder
    icmpv6_data[2..4].fill(0);
    // Cur Hop Limit: 64
    icmpv6_data[4] = 64;
    // Flags: M (Managed) = 0x80
    icmpv6_data[5] = 0x80;
    // Router Lifetime: 1800 seconds (30 minutes)
    icmpv6_data[6..8].copy_from_slice(&1800u16.to_be_bytes());
    // Reachable Time: 0 (unspecified)
    icmpv6_data[8..12].fill(0);
    // Retrans Timer: 0 (unspecified)
    icmpv6_data[12..16].fill(0);
    // Source Link-Layer Address Option (SLLAO)
    icmpv6_data[16] = 1; // Type: Source Link-Layer Address
    icmpv6_data[17] = 1; // Length: 1 (in 8-byte units)
    icmpv6_data[18..24].copy_from_slice(&GATEWAY_MAC);

    // Compute ICMPv6 checksum
    let checksum = compute_icmpv6_checksum(&gateway_ll, &dst_addr, &icmpv6_data[..icmpv6_len]);
    icmpv6_data[2..4].copy_from_slice(&checksum.to_be_bytes());

    debug!(
        dst = %dst_addr,
        "RA built (M flag set, no prefix)"
    );

    Some(packet)
}

/// Compute ICMPv6 checksum.
fn compute_icmpv6_checksum(src: &Ipv6Address, dst: &Ipv6Address, icmpv6_data: &[u8]) -> u16 {
    let mut sum: u32 = 0;

    // Pseudo-header
    for chunk in src.0.chunks(2) {
        sum += u16::from_be_bytes([chunk[0], chunk[1]]) as u32;
    }
    for chunk in dst.0.chunks(2) {
        sum += u16::from_be_bytes([chunk[0], chunk[1]]) as u32;
    }
    sum += icmpv6_data.len() as u32; // ICMPv6 length
    sum += 58u32; // Next header (ICMPv6)

    // ICMPv6 data
    let mut i = 0;
    while i + 1 < icmpv6_data.len() {
        sum += u16::from_be_bytes([icmpv6_data[i], icmpv6_data[i + 1]]) as u32;
        i += 2;
    }
    if i < icmpv6_data.len() {
        sum += (icmpv6_data[i] as u32) << 8;
    }

    // Fold to 16 bits
    while sum > 0xffff {
        sum = (sum & 0xffff) + (sum >> 16);
    }

    let result = !(sum as u16);
    if result == 0 { 0xffff } else { result }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_config() -> NicConfig {
        NicConfig {
            mac: [0x52, 0x54, 0x00, 0x12, 0x34, 0x56],
            ipv4_address: None,
            ipv4_gateway: None,
            ipv4_prefix_len: 24,
            ipv6_address: Some(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 2)),
            ipv6_gateway: Some(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1)),
            ipv6_prefix_len: 128,
            dns_servers: vec![],
        }
    }

    #[test]
    fn test_na_response() {
        let virtio_hdr = [0u8; 12];

        let response = build_neighbor_advertisement(
            &virtio_hdr,
            Ipv6Address::new(0xfe80, 0, 0, 0, 0, 0, 0, 2),
            EthernetAddress([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]),
            Ipv6Address::new(0xfe80, 0, 0, 0, 0, 0, 0, 1),
        );

        assert!(response.is_some());
        let packet = response.unwrap();

        // Verify Ethernet frame
        let eth = EthernetFrame::new_checked(&packet[12..]).unwrap();
        assert_eq!(eth.ethertype(), EthernetProtocol::Ipv6);

        // Verify IPv6 header
        let ipv6 = Ipv6Packet::new_checked(eth.payload()).unwrap();
        assert_eq!(ipv6.next_header(), IpProtocol::Icmpv6);

        // Verify ICMPv6 type is NA (136)
        let icmpv6 = Icmpv6Packet::new_checked(ipv6.payload()).unwrap();
        assert_eq!(icmpv6.msg_type(), Icmpv6Message::NeighborAdvert);
    }

    #[test]
    fn test_ra_response() {
        let config = make_test_config();
        let virtio_hdr = [0u8; 12];

        let response = build_router_advertisement(
            &config,
            &virtio_hdr,
            Ipv6Address::new(0xfe80, 0, 0, 0, 0, 0, 0, 2),
            EthernetAddress([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]),
        );

        assert!(response.is_some());
        let packet = response.unwrap();

        // Verify Ethernet frame
        let eth = EthernetFrame::new_checked(&packet[12..]).unwrap();
        assert_eq!(eth.ethertype(), EthernetProtocol::Ipv6);
        assert_eq!(eth.src_addr(), EthernetAddress(GATEWAY_MAC));

        // Verify IPv6 header
        let ipv6 = Ipv6Packet::new_checked(eth.payload()).unwrap();
        assert_eq!(ipv6.next_header(), IpProtocol::Icmpv6);
        assert_eq!(ipv6.hop_limit(), 255);

        // Verify ICMPv6 type is RA (134)
        let icmpv6 = Icmpv6Packet::new_checked(ipv6.payload()).unwrap();
        assert_eq!(icmpv6.msg_type(), Icmpv6Message::RouterAdvert);
    }

    #[test]
    fn test_echo_reply() {
        let virtio_hdr = [0u8; 12];

        // Build a simple Echo Request: type(128) + code(0) + checksum(2) + id(2) + seq(2) + data
        let mut echo_request = vec![0u8; 16];
        echo_request[0] = 128; // Type: Echo Request
        echo_request[1] = 0; // Code
        // checksum[2..4] = 0 (we don't need valid checksum for this test)
        echo_request[4..6].copy_from_slice(&0x1234u16.to_be_bytes()); // Identifier
        echo_request[6..8].copy_from_slice(&0x0001u16.to_be_bytes()); // Sequence
        echo_request[8..16].copy_from_slice(b"pingdata"); // Data

        let response = build_echo_reply(
            &virtio_hdr,
            &echo_request,
            Ipv6Address::new(0xfe80, 0, 0, 0, 0, 0, 0, 2),
            EthernetAddress([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]),
        );

        assert!(response.is_some());
        let packet = response.unwrap();

        // Verify Ethernet frame
        let eth = EthernetFrame::new_checked(&packet[12..]).unwrap();
        assert_eq!(eth.ethertype(), EthernetProtocol::Ipv6);
        assert_eq!(eth.src_addr(), EthernetAddress(GATEWAY_MAC));

        // Verify IPv6 header
        let ipv6 = Ipv6Packet::new_checked(eth.payload()).unwrap();
        assert_eq!(ipv6.next_header(), IpProtocol::Icmpv6);
        assert_eq!(ipv6.hop_limit(), 64);
        assert_eq!(
            ipv6.src_addr(),
            Ipv6Address::from_bytes(&GATEWAY_IPV6_LINK_LOCAL.octets())
        );

        // Verify ICMPv6 type is Echo Reply (129)
        let icmpv6 = Icmpv6Packet::new_checked(ipv6.payload()).unwrap();
        assert_eq!(icmpv6.msg_type(), Icmpv6Message::EchoReply);

        // Verify identifier and sequence are preserved
        let icmpv6_raw = ipv6.payload();
        assert_eq!(&icmpv6_raw[4..6], &0x1234u16.to_be_bytes());
        assert_eq!(&icmpv6_raw[6..8], &0x0001u16.to_be_bytes());
        assert_eq!(&icmpv6_raw[8..16], b"pingdata");
    }
}
