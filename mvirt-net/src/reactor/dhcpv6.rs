//! DHCPv6 server for vhost-user interfaces.
//!
//! This module implements a minimal DHCPv6 server that responds to SOLICIT and REQUEST
//! messages from VMs with the configured IPv6 address (/128) and DNS servers.

use super::{GATEWAY_MAC, NicConfig};
use dhcproto::v6::{
    DhcpOption, IAAddr, IANA, Message, MessageType, OptionCode, Status, StatusCode,
};
use dhcproto::{Decodable, Decoder, Encodable, Encoder};
use smoltcp::wire::{
    EthernetAddress, EthernetFrame, EthernetProtocol, EthernetRepr, IpProtocol, Ipv6Address,
    Ipv6Packet, Ipv6Repr, UdpPacket,
};
use std::net::Ipv6Addr;
use tracing::debug;

/// Ethernet header size
const ETHERNET_HEADER_SIZE: usize = 14;

/// IPv6 header size
const IPV6_HEADER_SIZE: usize = 40;

/// UDP header size
const UDP_HEADER_SIZE: usize = 8;

/// DHCPv6 server port
const DHCP6_SERVER_PORT: u16 = 547;

/// DHCPv6 client port
const DHCP6_CLIENT_PORT: u16 = 546;

/// Default preferred lifetime in seconds (24 hours)
const PREFERRED_LIFETIME: u32 = 86400;

/// Default valid lifetime in seconds (48 hours)
const VALID_LIFETIME: u32 = 172800;

/// Handle a DHCPv6 packet from a VM.
///
/// Returns a response packet if this is a DHCPv6 request we should respond to.
pub fn handle_dhcpv6_packet(
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

    if ipv6_packet.next_header() != IpProtocol::Udp {
        return None;
    }

    // Parse UDP packet
    let udp_packet = UdpPacket::new_checked(ipv6_packet.payload()).ok()?;

    // Check if it's a DHCPv6 packet (client → server)
    if udp_packet.dst_port() != DHCP6_SERVER_PORT {
        return None;
    }

    // Parse DHCPv6 message
    let dhcp_payload = udp_packet.payload();
    let mut decoder = Decoder::new(dhcp_payload);
    let dhcp_msg = Message::decode(&mut decoder).ok()?;

    let src_addr = ipv6_packet.src_addr();
    let src_mac = eth_frame.src_addr();

    debug!(
        msg_type = ?dhcp_msg.msg_type(),
        xid = ?dhcp_msg.xid(),
        src = %src_addr,
        "DHCPv6 message received"
    );

    match dhcp_msg.msg_type() {
        MessageType::Solicit => {
            handle_solicit(nic_config, virtio_hdr, &dhcp_msg, src_addr, src_mac)
        }
        MessageType::Request => {
            handle_request(nic_config, virtio_hdr, &dhcp_msg, src_addr, src_mac)
        }
        MessageType::Confirm => {
            handle_confirm(nic_config, virtio_hdr, &dhcp_msg, src_addr, src_mac)
        }
        MessageType::Renew | MessageType::Rebind => {
            handle_request(nic_config, virtio_hdr, &dhcp_msg, src_addr, src_mac)
        }
        MessageType::Release | MessageType::Decline => {
            debug!(msg_type = ?dhcp_msg.msg_type(), "DHCPv6 release/decline received");
            None
        }
        MessageType::InformationRequest => {
            handle_information_request(nic_config, virtio_hdr, &dhcp_msg, src_addr, src_mac)
        }
        _ => {
            debug!(msg_type = ?dhcp_msg.msg_type(), "Ignoring DHCPv6 message type");
            None
        }
    }
}

/// Handle DHCPv6 SOLICIT - respond with ADVERTISE.
fn handle_solicit(
    nic_config: &NicConfig,
    virtio_hdr: &[u8],
    solicit: &Message,
    src_addr: Ipv6Address,
    src_mac: EthernetAddress,
) -> Option<Vec<u8>> {
    let ipv6_address = nic_config.ipv6_address?;

    debug!(
        offered_ip = %ipv6_address,
        xid = ?solicit.xid(),
        "Sending DHCPv6 ADVERTISE"
    );

    build_dhcpv6_response(
        nic_config,
        virtio_hdr,
        solicit,
        MessageType::Advertise,
        src_addr,
        src_mac,
        Some(ipv6_address),
    )
}

/// Handle DHCPv6 REQUEST - respond with REPLY.
fn handle_request(
    nic_config: &NicConfig,
    virtio_hdr: &[u8],
    request: &Message,
    src_addr: Ipv6Address,
    src_mac: EthernetAddress,
) -> Option<Vec<u8>> {
    let ipv6_address = nic_config.ipv6_address?;

    debug!(
        assigned_ip = %ipv6_address,
        xid = ?request.xid(),
        "Sending DHCPv6 REPLY"
    );

    build_dhcpv6_response(
        nic_config,
        virtio_hdr,
        request,
        MessageType::Reply,
        src_addr,
        src_mac,
        Some(ipv6_address),
    )
}

/// Handle DHCPv6 CONFIRM - respond with REPLY (status only).
fn handle_confirm(
    nic_config: &NicConfig,
    virtio_hdr: &[u8],
    confirm: &Message,
    src_addr: Ipv6Address,
    src_mac: EthernetAddress,
) -> Option<Vec<u8>> {
    debug!(xid = ?confirm.xid(), "Sending DHCPv6 REPLY for CONFIRM");

    build_dhcpv6_response(
        nic_config,
        virtio_hdr,
        confirm,
        MessageType::Reply,
        src_addr,
        src_mac,
        None,
    )
}

/// Handle DHCPv6 INFORMATION-REQUEST - respond with REPLY (options only).
fn handle_information_request(
    nic_config: &NicConfig,
    virtio_hdr: &[u8],
    request: &Message,
    src_addr: Ipv6Address,
    src_mac: EthernetAddress,
) -> Option<Vec<u8>> {
    debug!(xid = ?request.xid(), "Sending DHCPv6 REPLY for INFORMATION-REQUEST");

    build_dhcpv6_response(
        nic_config,
        virtio_hdr,
        request,
        MessageType::Reply,
        src_addr,
        src_mac,
        None,
    )
}

/// Build a DHCPv6 response (ADVERTISE or REPLY).
fn build_dhcpv6_response(
    nic_config: &NicConfig,
    virtio_hdr: &[u8],
    request: &Message,
    msg_type: MessageType,
    dst_addr: Ipv6Address,
    dst_mac: EthernetAddress,
    assigned_ip: Option<Ipv6Addr>,
) -> Option<Vec<u8>> {
    // Build DHCPv6 response message
    let mut response = Message::new(msg_type);
    response.set_xid(request.xid());

    // Get client DUID from request
    let client_duid = request.opts().get(OptionCode::ClientId)?;
    let client_duid_bytes = match client_duid {
        DhcpOption::ClientId(duid) => duid.clone(),
        _ => return None,
    };

    // Server DUID - use a simple DUID-LL based on gateway MAC
    // Format: type (2 bytes) + hw type (2 bytes) + hw address (6 bytes)
    let mut server_duid_bytes = Vec::with_capacity(10);
    server_duid_bytes.extend_from_slice(&[0x00, 0x03]); // DUID-LL type
    server_duid_bytes.extend_from_slice(&[0x00, 0x01]); // Ethernet hw type
    server_duid_bytes.extend_from_slice(&GATEWAY_MAC);

    // Add options
    response
        .opts_mut()
        .insert(DhcpOption::ClientId(client_duid_bytes));
    response
        .opts_mut()
        .insert(DhcpOption::ServerId(server_duid_bytes));

    // Add IA_NA with address if provided
    // IMPORTANT: We must echo back the client's IAID, not use a hardcoded value
    if let Some(addr) = assigned_ip {
        // Extract client's IAID from the request's IA_NA option
        let client_iaid = request
            .opts()
            .get(OptionCode::IANA)
            .and_then(|opt| match opt {
                DhcpOption::IANA(iana) => Some(iana.id),
                _ => None,
            })
            .unwrap_or(1); // Fallback to 1 if no IA_NA in request

        let ia_addr = IAAddr {
            addr,
            preferred_life: PREFERRED_LIFETIME,
            valid_life: VALID_LIFETIME,
            opts: Default::default(),
        };

        let ia_na = IANA {
            id: client_iaid, // Echo back client's IAID
            t1: PREFERRED_LIFETIME / 2,
            t2: (PREFERRED_LIFETIME * 4) / 5,
            opts: {
                let mut opts = dhcproto::v6::DhcpOptions::new();
                opts.insert(DhcpOption::IAAddr(ia_addr));
                opts
            },
        };

        response.opts_mut().insert(DhcpOption::IANA(ia_na));
    }

    // Add DNS servers if configured
    let dns_v6: Vec<Ipv6Addr> = nic_config
        .dns_servers
        .iter()
        .filter_map(|ip| match ip {
            std::net::IpAddr::V6(v6) => Some(*v6),
            _ => None,
        })
        .collect();
    if !dns_v6.is_empty() {
        response
            .opts_mut()
            .insert(DhcpOption::DomainNameServers(dns_v6));
    }

    // Status code: Success
    response
        .opts_mut()
        .insert(DhcpOption::StatusCode(StatusCode {
            status: Status::Success,
            msg: String::new(),
        }));

    // Encode the DHCPv6 message
    let mut dhcp_bytes = Vec::new();
    let mut encoder = Encoder::new(&mut dhcp_bytes);
    response.encode(&mut encoder).ok()?;

    build_dhcpv6_packet(virtio_hdr, &dhcp_bytes, dst_addr, dst_mac)
}

/// Build the complete DHCPv6 response packet with Ethernet/IPv6/UDP headers.
fn build_dhcpv6_packet(
    virtio_hdr: &[u8],
    dhcp_bytes: &[u8],
    dst_addr: Ipv6Address,
    dst_mac: EthernetAddress,
) -> Option<Vec<u8>> {
    let virtio_hdr_size = virtio_hdr.len();
    let gateway_ll = Ipv6Address::new(
        0xfe80,
        0,
        0,
        0,
        ((GATEWAY_MAC[0] as u16 ^ 0x02) << 8) | GATEWAY_MAC[1] as u16,
        (GATEWAY_MAC[2] as u16) << 8 | 0xff,
        0xfe00 | GATEWAY_MAC[3] as u16,
        (GATEWAY_MAC[4] as u16) << 8 | GATEWAY_MAC[5] as u16,
    );

    let udp_len = UDP_HEADER_SIZE + dhcp_bytes.len();
    let total_len = virtio_hdr_size + ETHERNET_HEADER_SIZE + IPV6_HEADER_SIZE + udp_len;

    let mut packet = vec![0u8; total_len];

    // Virtio header (zeroed)
    packet[..virtio_hdr_size].fill(0);

    // Ethernet header
    let gateway_mac = EthernetAddress(GATEWAY_MAC);
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
        next_header: IpProtocol::Udp,
        payload_len: udp_len,
        hop_limit: 64,
    };
    let mut ipv6_packet = Ipv6Packet::new_unchecked(eth_frame.payload_mut());
    ipv6_repr.emit(&mut ipv6_packet);

    // UDP header and payload
    let udp_start = virtio_hdr_size + ETHERNET_HEADER_SIZE + IPV6_HEADER_SIZE;
    let udp_slice = &mut packet[udp_start..];

    // Write UDP header manually
    udp_slice[0..2].copy_from_slice(&DHCP6_SERVER_PORT.to_be_bytes());
    udp_slice[2..4].copy_from_slice(&DHCP6_CLIENT_PORT.to_be_bytes());
    udp_slice[4..6].copy_from_slice(&(udp_len as u16).to_be_bytes());
    udp_slice[6..8].fill(0); // checksum placeholder
    udp_slice[8..8 + dhcp_bytes.len()].copy_from_slice(dhcp_bytes);

    // Compute UDP checksum over pseudo-header + UDP
    let checksum = compute_udp6_checksum(
        &ipv6_repr.src_addr,
        &ipv6_repr.dst_addr,
        &udp_slice[..udp_len],
    );
    udp_slice[6..8].copy_from_slice(&checksum.to_be_bytes());

    debug!(
        dst = %dst_addr,
        len = total_len,
        "DHCPv6 response built"
    );

    Some(packet)
}

/// Compute UDP checksum for IPv6.
fn compute_udp6_checksum(src: &Ipv6Address, dst: &Ipv6Address, udp_data: &[u8]) -> u16 {
    let mut sum: u32 = 0;

    // Pseudo-header
    for chunk in src.0.chunks(2) {
        sum += u16::from_be_bytes([chunk[0], chunk[1]]) as u32;
    }
    for chunk in dst.0.chunks(2) {
        sum += u16::from_be_bytes([chunk[0], chunk[1]]) as u32;
    }
    sum += udp_data.len() as u32; // UDP length
    sum += 17u32; // Next header (UDP)

    // UDP data
    let mut i = 0;
    while i + 1 < udp_data.len() {
        sum += u16::from_be_bytes([udp_data[i], udp_data[i + 1]]) as u32;
        i += 2;
    }
    if i < udp_data.len() {
        sum += (udp_data[i] as u32) << 8;
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

    #[test]
    fn test_build_dhcpv6_packet() {
        let virtio_hdr = [0u8; 12];
        let dhcp_bytes = vec![1, 2, 3, 4]; // Dummy DHCP payload

        let packet = build_dhcpv6_packet(
            &virtio_hdr,
            &dhcp_bytes,
            Ipv6Address::new(0xfe80, 0, 0, 0, 0, 0, 0, 2),
            EthernetAddress([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]),
        );

        assert!(packet.is_some());
        let packet = packet.unwrap();

        // Verify Ethernet header
        let eth = EthernetFrame::new_checked(&packet[12..]).unwrap();
        assert_eq!(eth.ethertype(), EthernetProtocol::Ipv6);
        assert_eq!(eth.src_addr(), EthernetAddress(GATEWAY_MAC));

        // Verify IPv6 header
        let ipv6 = Ipv6Packet::new_checked(eth.payload()).unwrap();
        assert_eq!(ipv6.next_header(), IpProtocol::Udp);
    }
}
