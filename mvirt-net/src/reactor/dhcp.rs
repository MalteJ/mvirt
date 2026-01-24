//! DHCPv4 server for vhost-user interfaces.
//!
//! This module implements a minimal DHCP server that responds to DISCOVER and REQUEST
//! messages from VMs with the configured IP address, gateway, and DNS servers.

use super::{GATEWAY_IPV4_LINK_LOCAL, GATEWAY_MAC, NicConfig};
use dhcproto::v4::{DhcpOption, Flags, Message, MessageType, Opcode, OptionCode};
use dhcproto::{Decodable, Decoder, Encodable, Encoder};
use ipnet::Ipv4Net;
use smoltcp::wire::{
    EthernetAddress, EthernetFrame, EthernetProtocol, EthernetRepr, IpProtocol, Ipv4Address,
    Ipv4Packet, Ipv4Repr, UdpPacket, UdpRepr,
};
use std::net::Ipv4Addr;
use tracing::debug;

/// Ethernet header size
const ETHERNET_HEADER_SIZE: usize = 14;

/// Minimum IPv4 header size
const IPV4_HEADER_SIZE: usize = 20;

/// UDP header size
const UDP_HEADER_SIZE: usize = 8;

/// DHCP server port
const DHCP_SERVER_PORT: u16 = 67;

/// DHCP client port
const DHCP_CLIENT_PORT: u16 = 68;

/// Default lease time in seconds (24 hours)
const DEFAULT_LEASE_TIME: u32 = 86400;

/// Handle a DHCP packet from a VM.
///
/// Returns a response packet if this is a DHCP request we should respond to.
pub fn handle_dhcp_packet(
    nic_config: &NicConfig,
    virtio_hdr: &[u8],
    ethernet_frame: &[u8],
) -> Option<Vec<u8>> {
    // Parse Ethernet frame
    let eth_frame = match EthernetFrame::new_checked(ethernet_frame) {
        Ok(f) => f,
        Err(e) => {
            debug!(error = ?e, "DHCP: failed to parse Ethernet frame");
            return None;
        }
    };

    if eth_frame.ethertype() != EthernetProtocol::Ipv4 {
        return None;
    }

    // Parse IPv4 packet
    let ipv4_packet = match Ipv4Packet::new_checked(eth_frame.payload()) {
        Ok(p) => p,
        Err(e) => {
            debug!(error = ?e, payload_len = eth_frame.payload().len(), "DHCP: failed to parse IPv4 packet");
            return None;
        }
    };

    if ipv4_packet.next_header() != IpProtocol::Udp {
        debug!(proto = ?ipv4_packet.next_header(), "DHCP: not UDP");
        return None;
    }

    // Parse UDP packet
    let udp_packet = match UdpPacket::new_checked(ipv4_packet.payload()) {
        Ok(p) => p,
        Err(e) => {
            debug!(error = ?e, "DHCP: failed to parse UDP packet");
            return None;
        }
    };

    // Check if it's a DHCP packet (client → server)
    if udp_packet.dst_port() != DHCP_SERVER_PORT {
        debug!(dst_port = udp_packet.dst_port(), "DHCP: wrong port");
        return None;
    }

    // Parse DHCP message
    let dhcp_payload = udp_packet.payload();
    let mut decoder = Decoder::new(dhcp_payload);
    let dhcp_msg = Message::decode(&mut decoder).ok()?;

    // Check if it's a request from a client
    if dhcp_msg.opcode() != Opcode::BootRequest {
        return None;
    }

    // Get message type
    let msg_type = get_dhcp_message_type(&dhcp_msg)?;

    debug!(
        msg_type = ?msg_type,
        client_mac = ?dhcp_msg.chaddr(),
        xid = dhcp_msg.xid(),
        "DHCP message received"
    );

    match msg_type {
        MessageType::Discover => handle_discover(nic_config, virtio_hdr, &eth_frame, &dhcp_msg),
        MessageType::Request => handle_request(nic_config, virtio_hdr, &eth_frame, &dhcp_msg),
        _ => {
            debug!(msg_type = ?msg_type, "Ignoring DHCP message type");
            None
        }
    }
}

/// Get the DHCP message type from options.
fn get_dhcp_message_type(msg: &Message) -> Option<MessageType> {
    msg.opts().get(OptionCode::MessageType).and_then(|opt| {
        if let DhcpOption::MessageType(mt) = opt {
            Some(*mt)
        } else {
            None
        }
    })
}

/// Handle DHCP DISCOVER - respond with OFFER.
fn handle_discover(
    nic_config: &NicConfig,
    virtio_hdr: &[u8],
    eth_frame: &EthernetFrame<&[u8]>,
    discover: &Message,
) -> Option<Vec<u8>> {
    let ipv4_address = nic_config.ipv4_address?;

    debug!(
        offered_ip = %ipv4_address,
        gateway = %GATEWAY_IPV4_LINK_LOCAL,
        xid = discover.xid(),
        "Sending DHCP OFFER"
    );

    build_dhcp_response(
        nic_config,
        virtio_hdr,
        eth_frame,
        discover,
        MessageType::Offer,
        ipv4_address,
    )
}

/// Handle DHCP REQUEST - respond with ACK.
fn handle_request(
    nic_config: &NicConfig,
    virtio_hdr: &[u8],
    eth_frame: &EthernetFrame<&[u8]>,
    request: &Message,
) -> Option<Vec<u8>> {
    let ipv4_address = nic_config.ipv4_address?;

    // Check if the requested IP matches what we configured
    // Get the requested IP from options
    let requested_ip = request
        .opts()
        .get(OptionCode::RequestedIpAddress)
        .and_then(|opt| {
            if let DhcpOption::RequestedIpAddress(ip) = opt {
                Some(*ip)
            } else {
                None
            }
        });

    // If client requests a specific IP that's not what we configured, send NAK
    if let Some(req_ip) = requested_ip
        && req_ip != ipv4_address
    {
        debug!(
            requested = %req_ip,
            configured = %ipv4_address,
            "Client requested wrong IP, sending NAK"
        );
        return build_dhcp_nak(nic_config, virtio_hdr, eth_frame, request);
    }

    debug!(
        assigned_ip = %ipv4_address,
        xid = request.xid(),
        "Sending DHCP ACK"
    );

    build_dhcp_response(
        nic_config,
        virtio_hdr,
        eth_frame,
        request,
        MessageType::Ack,
        ipv4_address,
    )
}

/// Build a DHCP response (OFFER or ACK).
fn build_dhcp_response(
    nic_config: &NicConfig,
    virtio_hdr: &[u8],
    _eth_frame: &EthernetFrame<&[u8]>,
    request: &Message,
    msg_type: MessageType,
    assigned_ip: Ipv4Addr,
) -> Option<Vec<u8>> {
    // Build DHCP response message
    let mut response = Message::default();
    response.set_opcode(Opcode::BootReply);
    response.set_htype(request.htype());
    response.set_xid(request.xid());
    response.set_flags(request.flags());
    response.set_yiaddr(assigned_ip);
    response.set_siaddr(GATEWAY_IPV4_LINK_LOCAL);
    response.set_chaddr(request.chaddr());

    // Set broadcast flag if client requested it
    if request.flags().broadcast() {
        response.set_flags(Flags::default().set_broadcast());
    }

    // Add DHCP options
    let opts = response.opts_mut();

    // Message type
    opts.insert(DhcpOption::MessageType(msg_type));

    // Server identifier (link-local gateway)
    opts.insert(DhcpOption::ServerIdentifier(GATEWAY_IPV4_LINK_LOCAL));

    // Lease time
    opts.insert(DhcpOption::AddressLeaseTime(DEFAULT_LEASE_TIME));

    // Subnet mask - use /32 since we use link-local gateway with static routes
    opts.insert(DhcpOption::SubnetMask(Ipv4Addr::new(255, 255, 255, 255)));

    // Router (link-local gateway) - for legacy clients without Option 121 support
    opts.insert(DhcpOption::Router(vec![GATEWAY_IPV4_LINK_LOCAL]));

    // Classless Static Routes (Option 121) - takes precedence over Router
    // Route format: Vec<(destination_network, next_hop)>
    // - 169.254.0.1/32 via 0.0.0.0 (on-link route to gateway)
    // - 0.0.0.0/0 via 169.254.0.1 (default route via gateway)
    let gateway_net = Ipv4Net::new(GATEWAY_IPV4_LINK_LOCAL, 32).unwrap();
    let default_net = Ipv4Net::new(Ipv4Addr::UNSPECIFIED, 0).unwrap();
    opts.insert(DhcpOption::ClasslessStaticRoute(vec![
        (gateway_net, Ipv4Addr::UNSPECIFIED),   // 169.254.0.1/32 on-link
        (default_net, GATEWAY_IPV4_LINK_LOCAL), // 0.0.0.0/0 via gateway
    ]));

    // DNS servers
    let dns_v4: Vec<Ipv4Addr> = nic_config
        .dns_servers
        .iter()
        .filter_map(|ip| match ip {
            std::net::IpAddr::V4(v4) => Some(*v4),
            _ => None,
        })
        .collect();
    if !dns_v4.is_empty() {
        opts.insert(DhcpOption::DomainNameServer(dns_v4));
    }

    // Encode the DHCP message
    let mut dhcp_bytes = Vec::new();
    let mut encoder = Encoder::new(&mut dhcp_bytes);
    response.encode(&mut encoder).ok()?;

    build_dhcp_packet(nic_config, virtio_hdr, request, &dhcp_bytes, assigned_ip)
}

/// Build a DHCP NAK response.
fn build_dhcp_nak(
    nic_config: &NicConfig,
    virtio_hdr: &[u8],
    _eth_frame: &EthernetFrame<&[u8]>,
    request: &Message,
) -> Option<Vec<u8>> {
    let mut response = Message::default();
    response.set_opcode(Opcode::BootReply);
    response.set_htype(request.htype());
    response.set_xid(request.xid());
    response.set_chaddr(request.chaddr());

    let opts = response.opts_mut();
    opts.insert(DhcpOption::MessageType(MessageType::Nak));
    opts.insert(DhcpOption::ServerIdentifier(GATEWAY_IPV4_LINK_LOCAL));

    let mut dhcp_bytes = Vec::new();
    let mut encoder = Encoder::new(&mut dhcp_bytes);
    response.encode(&mut encoder).ok()?;

    // For NAK, we always broadcast
    build_dhcp_packet_broadcast(nic_config, virtio_hdr, &dhcp_bytes)
}

/// Build the complete DHCP response packet with Ethernet/IP/UDP headers.
fn build_dhcp_packet(
    _nic_config: &NicConfig,
    virtio_hdr: &[u8],
    request: &Message,
    dhcp_bytes: &[u8],
    assigned_ip: Ipv4Addr,
) -> Option<Vec<u8>> {
    let virtio_hdr_size = virtio_hdr.len();

    // Determine destination based on broadcast flag and client address
    let (dst_ip, dst_mac) =
        if request.flags().broadcast() || request.ciaddr() == Ipv4Addr::UNSPECIFIED {
            // Broadcast response
            (
                Ipv4Address::BROADCAST,
                EthernetAddress([0xff, 0xff, 0xff, 0xff, 0xff, 0xff]),
            )
        } else {
            // Unicast to the client's IP using assigned address
            let client_mac: [u8; 6] = request.chaddr()[..6].try_into().ok()?;
            (
                Ipv4Address::from_bytes(&assigned_ip.octets()),
                EthernetAddress(client_mac),
            )
        };

    let udp_len = UDP_HEADER_SIZE + dhcp_bytes.len();
    let ip_len = IPV4_HEADER_SIZE + udp_len;
    let total_len = virtio_hdr_size + ETHERNET_HEADER_SIZE + ip_len;

    let mut packet = vec![0u8; total_len];

    // Virtio header (zeroed)
    packet[..virtio_hdr_size].fill(0);

    // Ethernet header
    let gateway_mac = EthernetAddress(GATEWAY_MAC);
    let eth_repr = EthernetRepr {
        src_addr: gateway_mac,
        dst_addr: dst_mac,
        ethertype: EthernetProtocol::Ipv4,
    };
    let mut eth_frame = EthernetFrame::new_unchecked(&mut packet[virtio_hdr_size..]);
    eth_repr.emit(&mut eth_frame);

    // IPv4 header - source is the link-local gateway
    let ip_repr = Ipv4Repr {
        src_addr: Ipv4Address::from_bytes(&GATEWAY_IPV4_LINK_LOCAL.octets()),
        dst_addr: dst_ip,
        next_header: IpProtocol::Udp,
        payload_len: udp_len,
        hop_limit: 64,
    };
    let mut ip_packet = Ipv4Packet::new_unchecked(eth_frame.payload_mut());
    ip_repr.emit(
        &mut ip_packet,
        &smoltcp::phy::ChecksumCapabilities::default(),
    );

    // UDP header
    let udp_repr = UdpRepr {
        src_port: DHCP_SERVER_PORT,
        dst_port: DHCP_CLIENT_PORT,
    };
    let mut udp_packet = UdpPacket::new_unchecked(ip_packet.payload_mut());
    udp_repr.emit(
        &mut udp_packet,
        &ip_repr.src_addr.into(),
        &ip_repr.dst_addr.into(),
        dhcp_bytes.len(),
        |buf| buf.copy_from_slice(dhcp_bytes),
        &smoltcp::phy::ChecksumCapabilities::default(),
    );

    debug!(
        dst_mac = ?dst_mac,
        dst_ip = %dst_ip,
        len = total_len,
        "DHCP response built"
    );

    Some(packet)
}

/// Build a broadcast DHCP packet (for NAK).
fn build_dhcp_packet_broadcast(
    _nic_config: &NicConfig,
    virtio_hdr: &[u8],
    dhcp_bytes: &[u8],
) -> Option<Vec<u8>> {
    let virtio_hdr_size = virtio_hdr.len();

    let udp_len = UDP_HEADER_SIZE + dhcp_bytes.len();
    let ip_len = IPV4_HEADER_SIZE + udp_len;
    let total_len = virtio_hdr_size + ETHERNET_HEADER_SIZE + ip_len;

    let mut packet = vec![0u8; total_len];

    // Virtio header (zeroed)
    packet[..virtio_hdr_size].fill(0);

    // Ethernet header
    let gateway_mac = EthernetAddress(GATEWAY_MAC);
    let broadcast_mac = EthernetAddress([0xff, 0xff, 0xff, 0xff, 0xff, 0xff]);
    let eth_repr = EthernetRepr {
        src_addr: gateway_mac,
        dst_addr: broadcast_mac,
        ethertype: EthernetProtocol::Ipv4,
    };
    let mut eth_frame = EthernetFrame::new_unchecked(&mut packet[virtio_hdr_size..]);
    eth_repr.emit(&mut eth_frame);

    // IPv4 header - source is the link-local gateway
    let ip_repr = Ipv4Repr {
        src_addr: Ipv4Address::from_bytes(&GATEWAY_IPV4_LINK_LOCAL.octets()),
        dst_addr: Ipv4Address::BROADCAST,
        next_header: IpProtocol::Udp,
        payload_len: udp_len,
        hop_limit: 64,
    };
    let mut ip_packet = Ipv4Packet::new_unchecked(eth_frame.payload_mut());
    ip_repr.emit(
        &mut ip_packet,
        &smoltcp::phy::ChecksumCapabilities::default(),
    );

    // UDP header
    let udp_repr = UdpRepr {
        src_port: DHCP_SERVER_PORT,
        dst_port: DHCP_CLIENT_PORT,
    };
    let mut udp_packet = UdpPacket::new_unchecked(ip_packet.payload_mut());
    udp_repr.emit(
        &mut udp_packet,
        &ip_repr.src_addr.into(),
        &ip_repr.dst_addr.into(),
        dhcp_bytes.len(),
        |buf| buf.copy_from_slice(dhcp_bytes),
        &smoltcp::phy::ChecksumCapabilities::default(),
    );

    Some(packet)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_link_local_gateway_constant() {
        // Verify the link-local gateway address
        assert_eq!(GATEWAY_IPV4_LINK_LOCAL, Ipv4Addr::new(169, 254, 0, 1));
    }
}
