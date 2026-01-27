//! Protocol packet builders for integration tests.
//!
//! Adapted from mvirt-net's test utilities, but without virtio-net headers.
//! Packets start directly with the Ethernet header for TAP device usage.

use dhcproto::v6::{DhcpOption, IAAddr, IANA, Message, MessageType, ORO, OptionCode};
use dhcproto::{Decodable, Decoder, Encodable, Encoder};
use smoltcp::wire::{
    ArpOperation, ArpPacket, ArpRepr, EthernetAddress, EthernetFrame, EthernetProtocol,
    EthernetRepr, Icmpv4Message, Icmpv4Packet, Icmpv4Repr, IpProtocol, Ipv4Address, Ipv4Packet,
    Ipv4Repr, Ipv6Address, Ipv6Packet, Ipv6Repr, UdpPacket, UdpRepr,
};
use std::net::Ipv6Addr;

// ============================================================================
// Constants
// ============================================================================

/// Ethernet header size
pub const ETHERNET_HDR_SIZE: usize = 14;

/// IP header size (without options)
pub const IP_HDR_SIZE: usize = 20;

/// UDP header size
pub const UDP_HDR_SIZE: usize = 8;

/// ARP packet size (Ethernet + IPv4)
pub const ARP_PKT_SIZE: usize = 28;

/// DHCP server port
pub const DHCP_SERVER_PORT: u16 = 67;

/// DHCP client port
pub const DHCP_CLIENT_PORT: u16 = 68;

/// DHCPv6 server port
pub const DHCP6_SERVER_PORT: u16 = 547;

/// DHCPv6 client port
pub const DHCP6_CLIENT_PORT: u16 = 546;

/// IPv6 header size
pub const IPV6_HDR_SIZE: usize = 40;

/// Broadcast MAC address
pub const BROADCAST_MAC: [u8; 6] = [0xff, 0xff, 0xff, 0xff, 0xff, 0xff];

/// IPv6 All-Routers multicast address (ff02::2)
pub const ALL_ROUTERS_MULTICAST: [u8; 16] =
    [0xff, 0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x02];

/// IPv6 All-DHCP-Servers multicast address (ff02::1:2)
pub const ALL_DHCP_SERVERS_MULTICAST: [u8; 16] =
    [0xff, 0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x01, 0, 0x02];

// ============================================================================
// DHCP Packets
// ============================================================================

/// DHCP message types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DhcpMessageType {
    Discover = 1,
    Offer = 2,
    Request = 3,
    Decline = 4,
    Ack = 5,
    Nak = 6,
    Release = 7,
    Inform = 8,
}

/// Create a DHCP DISCOVER packet
pub fn create_dhcp_discover(client_mac: [u8; 6], xid: u32) -> Vec<u8> {
    create_dhcp_packet(
        client_mac,
        xid,
        DhcpMessageType::Discover,
        [0, 0, 0, 0],
        None,
        None,
    )
}

/// Create a DHCP REQUEST packet
pub fn create_dhcp_request(
    client_mac: [u8; 6],
    xid: u32,
    requested_ip: [u8; 4],
    server_id: [u8; 4],
) -> Vec<u8> {
    create_dhcp_packet(
        client_mac,
        xid,
        DhcpMessageType::Request,
        [0, 0, 0, 0],
        Some(requested_ip),
        Some(server_id),
    )
}

fn create_dhcp_packet(
    client_mac: [u8; 6],
    xid: u32,
    msg_type: DhcpMessageType,
    client_ip: [u8; 4],
    requested_ip: Option<[u8; 4]>,
    server_id: Option<[u8; 4]>,
) -> Vec<u8> {
    // Build DHCP payload (BOOTP + options)
    let mut dhcp = vec![0u8; 300];

    // BOOTP header
    dhcp[0] = 1; // op: BOOTREQUEST
    dhcp[1] = 1; // htype: Ethernet
    dhcp[2] = 6; // hlen
    dhcp[3] = 0; // hops
    dhcp[4..8].copy_from_slice(&xid.to_be_bytes());
    dhcp[8..10].copy_from_slice(&0u16.to_be_bytes()); // secs
    dhcp[10..12].copy_from_slice(&0x8000u16.to_be_bytes()); // flags: broadcast
    dhcp[12..16].copy_from_slice(&client_ip);
    dhcp[28..34].copy_from_slice(&client_mac);

    // Magic cookie
    dhcp[236..240].copy_from_slice(&[99, 130, 83, 99]);

    // Options
    let mut idx = 240;

    // Message type
    dhcp[idx] = 53;
    dhcp[idx + 1] = 1;
    dhcp[idx + 2] = msg_type as u8;
    idx += 3;

    // Requested IP
    if let Some(ip) = requested_ip {
        dhcp[idx] = 50;
        dhcp[idx + 1] = 4;
        dhcp[idx + 2..idx + 6].copy_from_slice(&ip);
        idx += 6;
    }

    // Server identifier
    if let Some(sid) = server_id {
        dhcp[idx] = 54;
        dhcp[idx + 1] = 4;
        dhcp[idx + 2..idx + 6].copy_from_slice(&sid);
        idx += 6;
    }

    // Parameter request list
    dhcp[idx] = 55;
    dhcp[idx + 1] = 4;
    dhcp[idx + 2] = 1; // Subnet Mask
    dhcp[idx + 3] = 3; // Router
    dhcp[idx + 4] = 6; // DNS
    dhcp[idx + 5] = 51; // Lease Time
    idx += 6;

    // End
    dhcp[idx] = 255;

    // Build UDP/IP/Ethernet frame using smoltcp
    // Note: No VIRTIO_NET_HDR - packets start directly with Ethernet
    let udp_len = UDP_HDR_SIZE + dhcp.len();
    let ip_len = IP_HDR_SIZE + udp_len;
    let total_len = ETHERNET_HDR_SIZE + ip_len;

    let mut packet = vec![0u8; total_len];

    // Ethernet frame
    let eth_repr = EthernetRepr {
        src_addr: EthernetAddress(client_mac),
        dst_addr: EthernetAddress(BROADCAST_MAC),
        ethertype: EthernetProtocol::Ipv4,
    };
    let mut eth_frame = EthernetFrame::new_unchecked(&mut packet[..]);
    eth_repr.emit(&mut eth_frame);

    // IPv4 packet
    let ip_repr = Ipv4Repr {
        src_addr: Ipv4Address::UNSPECIFIED,
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

    // UDP packet
    let udp_repr = UdpRepr {
        src_port: DHCP_CLIENT_PORT,
        dst_port: DHCP_SERVER_PORT,
    };
    let mut udp_packet = UdpPacket::new_unchecked(ip_packet.payload_mut());
    udp_repr.emit(
        &mut udp_packet,
        &ip_repr.src_addr.into(),
        &ip_repr.dst_addr.into(),
        dhcp.len(),
        |buf| buf.copy_from_slice(&dhcp),
        &smoltcp::phy::ChecksumCapabilities::default(),
    );

    packet
}

/// Parsed DHCP response
#[derive(Debug)]
pub struct DhcpResponse {
    pub msg_type: DhcpMessageType,
    pub xid: u32,
    pub your_ip: [u8; 4],
    pub server_ip: [u8; 4],
    pub subnet_mask: Option<[u8; 4]>,
    pub router: Option<[u8; 4]>,
    pub dns_servers: Vec<[u8; 4]>,
    pub lease_time: Option<u32>,
}

/// Parse a DHCP response packet (without virtio-net header)
pub fn parse_dhcp_response(packet: &[u8]) -> Option<DhcpResponse> {
    let eth_frame = EthernetFrame::new_checked(packet).ok()?;
    if eth_frame.ethertype() != EthernetProtocol::Ipv4 {
        return None;
    }

    let ip_packet = Ipv4Packet::new_checked(eth_frame.payload()).ok()?;
    if ip_packet.next_header() != IpProtocol::Udp {
        return None;
    }

    let udp_packet = UdpPacket::new_checked(ip_packet.payload()).ok()?;
    if udp_packet.src_port() != DHCP_SERVER_PORT {
        return None;
    }

    let dhcp = udp_packet.payload();
    if dhcp.len() < 240 || dhcp[0] != 2 {
        // BOOTREPLY
        return None;
    }

    let xid = u32::from_be_bytes([dhcp[4], dhcp[5], dhcp[6], dhcp[7]]);
    let your_ip = [dhcp[16], dhcp[17], dhcp[18], dhcp[19]];
    let server_ip = [dhcp[20], dhcp[21], dhcp[22], dhcp[23]];

    // Check magic cookie
    if dhcp[236..240] != [99, 130, 83, 99] {
        return None;
    }

    // Parse options
    let mut msg_type = None;
    let mut subnet_mask = None;
    let mut router = None;
    let mut dns_servers = Vec::new();
    let mut lease_time = None;

    let mut idx = 240;
    while idx < dhcp.len() {
        let opt = dhcp[idx];
        if opt == 255 {
            break;
        }
        if opt == 0 {
            idx += 1;
            continue;
        }
        if idx + 1 >= dhcp.len() {
            break;
        }
        let len = dhcp[idx + 1] as usize;
        if idx + 2 + len > dhcp.len() {
            break;
        }
        let data = &dhcp[idx + 2..idx + 2 + len];

        match opt {
            53 if len == 1 => {
                msg_type = match data[0] {
                    2 => Some(DhcpMessageType::Offer),
                    5 => Some(DhcpMessageType::Ack),
                    6 => Some(DhcpMessageType::Nak),
                    _ => None,
                };
            }
            1 if len == 4 => subnet_mask = Some([data[0], data[1], data[2], data[3]]),
            3 if len >= 4 => router = Some([data[0], data[1], data[2], data[3]]),
            6 => {
                for chunk in data.chunks_exact(4) {
                    dns_servers.push([chunk[0], chunk[1], chunk[2], chunk[3]]);
                }
            }
            51 if len == 4 => {
                lease_time = Some(u32::from_be_bytes([data[0], data[1], data[2], data[3]]))
            }
            _ => {}
        }
        idx += 2 + len;
    }

    Some(DhcpResponse {
        msg_type: msg_type?,
        xid,
        your_ip,
        server_ip,
        subnet_mask,
        router,
        dns_servers,
        lease_time,
    })
}

// ============================================================================
// ARP Packets
// ============================================================================

/// Create an ARP request packet
pub fn create_arp_request(sender_mac: [u8; 6], sender_ip: [u8; 4], target_ip: [u8; 4]) -> Vec<u8> {
    let total_size = ETHERNET_HDR_SIZE + ARP_PKT_SIZE;
    let mut packet = vec![0u8; total_size];

    // Ethernet frame
    let eth_repr = EthernetRepr {
        src_addr: EthernetAddress(sender_mac),
        dst_addr: EthernetAddress(BROADCAST_MAC),
        ethertype: EthernetProtocol::Arp,
    };
    let mut eth_frame = EthernetFrame::new_unchecked(&mut packet[..]);
    eth_repr.emit(&mut eth_frame);

    // ARP packet
    let arp_repr = ArpRepr::EthernetIpv4 {
        operation: ArpOperation::Request,
        source_hardware_addr: EthernetAddress(sender_mac),
        source_protocol_addr: Ipv4Address::from_bytes(&sender_ip),
        target_hardware_addr: EthernetAddress([0; 6]),
        target_protocol_addr: Ipv4Address::from_bytes(&target_ip),
    };
    let mut arp_packet = ArpPacket::new_unchecked(eth_frame.payload_mut());
    arp_repr.emit(&mut arp_packet);

    packet
}

/// Parsed ARP reply
#[derive(Debug)]
pub struct ArpReply {
    pub sender_mac: [u8; 6],
    pub sender_ip: [u8; 4],
    pub target_mac: [u8; 6],
    pub target_ip: [u8; 4],
}

/// Parse an ARP reply packet
pub fn parse_arp_reply(packet: &[u8]) -> Option<ArpReply> {
    let eth_frame = EthernetFrame::new_checked(packet).ok()?;
    if eth_frame.ethertype() != EthernetProtocol::Arp {
        return None;
    }

    let arp_packet = ArpPacket::new_checked(eth_frame.payload()).ok()?;
    let arp_repr = ArpRepr::parse(&arp_packet).ok()?;

    match arp_repr {
        ArpRepr::EthernetIpv4 {
            operation: ArpOperation::Reply,
            source_hardware_addr,
            source_protocol_addr,
            target_hardware_addr,
            target_protocol_addr,
        } => Some(ArpReply {
            sender_mac: source_hardware_addr.0,
            sender_ip: source_protocol_addr.0,
            target_mac: target_hardware_addr.0,
            target_ip: target_protocol_addr.0,
        }),
        _ => None,
    }
}

// ============================================================================
// ICMP Packets
// ============================================================================

/// Create an ICMP echo request packet
pub fn create_icmp_echo_request(
    src_mac: [u8; 6],
    dst_mac: [u8; 6],
    src_ip: [u8; 4],
    dst_ip: [u8; 4],
    id: u16,
    seq: u16,
) -> Vec<u8> {
    let data = b"ping from test!";
    let icmp_repr = Icmpv4Repr::EchoRequest {
        ident: id,
        seq_no: seq,
        data,
    };

    let icmp_len = icmp_repr.buffer_len();
    let ip_len = IP_HDR_SIZE + icmp_len;
    let total_len = ETHERNET_HDR_SIZE + ip_len;

    let mut packet = vec![0u8; total_len];

    // Ethernet frame
    let eth_repr = EthernetRepr {
        src_addr: EthernetAddress(src_mac),
        dst_addr: EthernetAddress(dst_mac),
        ethertype: EthernetProtocol::Ipv4,
    };
    let mut eth_frame = EthernetFrame::new_unchecked(&mut packet[..]);
    eth_repr.emit(&mut eth_frame);

    // IPv4 packet
    let ip_repr = Ipv4Repr {
        src_addr: Ipv4Address::from_bytes(&src_ip),
        dst_addr: Ipv4Address::from_bytes(&dst_ip),
        next_header: IpProtocol::Icmp,
        payload_len: icmp_len,
        hop_limit: 64,
    };
    let mut ip_packet = Ipv4Packet::new_unchecked(eth_frame.payload_mut());
    ip_repr.emit(
        &mut ip_packet,
        &smoltcp::phy::ChecksumCapabilities::default(),
    );

    // ICMP packet
    let mut icmp_packet = Icmpv4Packet::new_unchecked(ip_packet.payload_mut());
    icmp_repr.emit(
        &mut icmp_packet,
        &smoltcp::phy::ChecksumCapabilities::default(),
    );

    packet
}

/// Parsed ICMP echo reply
#[derive(Debug)]
pub struct IcmpEchoReply {
    pub src_ip: [u8; 4],
    pub dst_ip: [u8; 4],
    pub id: u16,
    pub seq: u16,
}

/// Parse an ICMP echo reply packet
pub fn parse_icmp_echo_reply(packet: &[u8]) -> Option<IcmpEchoReply> {
    let eth_frame = EthernetFrame::new_checked(packet).ok()?;
    if eth_frame.ethertype() != EthernetProtocol::Ipv4 {
        return None;
    }

    let ip_packet = Ipv4Packet::new_checked(eth_frame.payload()).ok()?;
    if ip_packet.next_header() != IpProtocol::Icmp {
        return None;
    }

    let icmp_packet = Icmpv4Packet::new_checked(ip_packet.payload()).ok()?;
    if icmp_packet.msg_type() != Icmpv4Message::EchoReply {
        return None;
    }

    Some(IcmpEchoReply {
        src_ip: ip_packet.src_addr().0,
        dst_ip: ip_packet.dst_addr().0,
        id: icmp_packet.echo_ident(),
        seq: icmp_packet.echo_seq_no(),
    })
}

// ============================================================================
// ICMPv6 / Router Solicitation / Router Advertisement
// ============================================================================

/// Compute link-local IPv6 address from MAC using EUI-64
pub fn mac_to_link_local(mac: [u8; 6]) -> Ipv6Address {
    Ipv6Address::new(
        0xfe80,
        0,
        0,
        0,
        ((mac[0] as u16 ^ 0x02) << 8) | mac[1] as u16,
        (mac[2] as u16) << 8 | 0xff,
        0xfe00 | mac[3] as u16,
        (mac[4] as u16) << 8 | mac[5] as u16,
    )
}

/// Compute multicast MAC address for IPv6 multicast
fn ipv6_multicast_mac(ipv6: &[u8; 16]) -> [u8; 6] {
    [0x33, 0x33, ipv6[12], ipv6[13], ipv6[14], ipv6[15]]
}

/// Compute ICMPv6 checksum
pub fn compute_icmpv6_checksum(src: &Ipv6Address, dst: &Ipv6Address, icmpv6_data: &[u8]) -> u16 {
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

/// Create a Router Solicitation packet
pub fn create_router_solicitation(src_mac: [u8; 6]) -> Vec<u8> {
    let src_ll = mac_to_link_local(src_mac);
    let dst_addr = Ipv6Address::from_bytes(&ALL_ROUTERS_MULTICAST);
    let dst_mac = ipv6_multicast_mac(&ALL_ROUTERS_MULTICAST);

    // RS packet: ICMPv6 type (1) + code (1) + checksum (2) + reserved (4) + SLLAO (8)
    let icmpv6_len = 16;
    let ip_len = IPV6_HDR_SIZE + icmpv6_len;
    let total_len = ETHERNET_HDR_SIZE + ip_len;

    let mut packet = vec![0u8; total_len];

    // Ethernet header
    let eth_repr = EthernetRepr {
        src_addr: EthernetAddress(src_mac),
        dst_addr: EthernetAddress(dst_mac),
        ethertype: EthernetProtocol::Ipv6,
    };
    let mut eth_frame = EthernetFrame::new_unchecked(&mut packet[..]);
    eth_repr.emit(&mut eth_frame);

    // IPv6 header
    let ipv6_repr = Ipv6Repr {
        src_addr: src_ll,
        dst_addr,
        next_header: IpProtocol::Icmpv6,
        payload_len: icmpv6_len,
        hop_limit: 255,
    };
    let mut ipv6_packet = Ipv6Packet::new_unchecked(eth_frame.payload_mut());
    ipv6_repr.emit(&mut ipv6_packet);

    // ICMPv6 RS
    let icmpv6_start = ETHERNET_HDR_SIZE + IPV6_HDR_SIZE;
    let icmpv6_data = &mut packet[icmpv6_start..];

    // Type: Router Solicitation (133)
    icmpv6_data[0] = 133;
    // Code: 0
    icmpv6_data[1] = 0;
    // Checksum: placeholder
    icmpv6_data[2..4].fill(0);
    // Reserved
    icmpv6_data[4..8].fill(0);
    // Source Link-Layer Address Option (SLLAO)
    icmpv6_data[8] = 1; // Type: Source Link-Layer Address
    icmpv6_data[9] = 1; // Length: 1 (in 8-byte units)
    icmpv6_data[10..16].copy_from_slice(&src_mac);

    // Compute ICMPv6 checksum
    let checksum = compute_icmpv6_checksum(&src_ll, &dst_addr, &icmpv6_data[..icmpv6_len]);
    icmpv6_data[2..4].copy_from_slice(&checksum.to_be_bytes());

    packet
}

/// Parsed Router Advertisement response
#[derive(Debug)]
pub struct RaResponse {
    /// M flag (Managed address configuration - use DHCPv6 for address)
    pub managed_flag: bool,
    /// O flag (Other configuration - use DHCPv6 for other info)
    pub other_flag: bool,
    /// Router lifetime in seconds
    pub router_lifetime: u16,
    /// Source MAC from SLLAO option
    pub router_mac: Option<[u8; 6]>,
}

/// Parse a Router Advertisement packet
pub fn parse_router_advertisement(packet: &[u8]) -> Option<RaResponse> {
    let eth_frame = EthernetFrame::new_checked(packet).ok()?;
    if eth_frame.ethertype() != EthernetProtocol::Ipv6 {
        return None;
    }

    let ipv6_packet = Ipv6Packet::new_checked(eth_frame.payload()).ok()?;
    if ipv6_packet.next_header() != IpProtocol::Icmpv6 {
        return None;
    }

    let payload = ipv6_packet.payload();
    if payload.len() < 16 || payload[0] != 134 {
        // Type: RA = 134
        return None;
    }

    let flags = payload[5];
    let managed_flag = (flags & 0x80) != 0;
    let other_flag = (flags & 0x40) != 0;
    let router_lifetime = u16::from_be_bytes([payload[6], payload[7]]);

    // Parse options for SLLAO
    let mut router_mac = None;
    let mut idx = 16; // After fixed RA header
    while idx + 2 <= payload.len() {
        let opt_type = payload[idx];
        let opt_len = payload[idx + 1] as usize * 8;
        if opt_len == 0 || idx + opt_len > payload.len() {
            break;
        }
        if opt_type == 1 && opt_len >= 8 {
            // SLLAO
            router_mac = Some([
                payload[idx + 2],
                payload[idx + 3],
                payload[idx + 4],
                payload[idx + 5],
                payload[idx + 6],
                payload[idx + 7],
            ]);
        }
        idx += opt_len;
    }

    Some(RaResponse {
        managed_flag,
        other_flag,
        router_lifetime,
        router_mac,
    })
}

// ============================================================================
// DHCPv6 Packets
// ============================================================================

/// DHCPv6 message types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dhcpv6MessageType {
    Solicit,
    Advertise,
    Request,
    Reply,
    Unknown(u8),
}

impl From<MessageType> for Dhcpv6MessageType {
    fn from(mt: MessageType) -> Self {
        match mt {
            MessageType::Solicit => Dhcpv6MessageType::Solicit,
            MessageType::Advertise => Dhcpv6MessageType::Advertise,
            MessageType::Request => Dhcpv6MessageType::Request,
            MessageType::Reply => Dhcpv6MessageType::Reply,
            _ => Dhcpv6MessageType::Unknown(0),
        }
    }
}

/// Parsed DHCPv6 response
#[derive(Debug)]
pub struct Dhcpv6Response {
    pub msg_type: Dhcpv6MessageType,
    pub xid: [u8; 3],
    pub client_duid: Vec<u8>,
    pub server_duid: Vec<u8>,
    pub addresses: Vec<Ipv6Addr>,
    pub dns_servers: Vec<Ipv6Addr>,
}

/// Compute UDP checksum for IPv6
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

/// Create a DHCPv6 SOLICIT packet
pub fn create_dhcpv6_solicit(client_mac: [u8; 6], client_duid: &[u8]) -> Vec<u8> {
    let src_ll = mac_to_link_local(client_mac);
    let dst_addr = Ipv6Address::from_bytes(&ALL_DHCP_SERVERS_MULTICAST);
    let dst_mac = ipv6_multicast_mac(&ALL_DHCP_SERVERS_MULTICAST);

    // Build DHCPv6 SOLICIT message
    let mut solicit = Message::new(MessageType::Solicit);
    solicit.set_xid([0x12, 0x34, 0x56]);

    // Add Client ID
    solicit
        .opts_mut()
        .insert(DhcpOption::ClientId(client_duid.to_vec()));

    // Add IA_NA (requesting an address)
    let ia_na = IANA {
        id: 1,
        t1: 0,
        t2: 0,
        opts: Default::default(),
    };
    solicit.opts_mut().insert(DhcpOption::IANA(ia_na));

    // Add Option Request (request DNS servers)
    solicit.opts_mut().insert(DhcpOption::ORO(ORO {
        opts: vec![OptionCode::DomainNameServers],
    }));

    // Add Elapsed Time (0)
    solicit.opts_mut().insert(DhcpOption::ElapsedTime(0));

    // Encode DHCPv6 message
    let mut dhcp_bytes = Vec::new();
    let mut encoder = Encoder::new(&mut dhcp_bytes);
    solicit.encode(&mut encoder).unwrap();

    build_dhcpv6_packet(client_mac, src_ll, dst_addr, dst_mac, &dhcp_bytes)
}

/// Create a DHCPv6 REQUEST packet
pub fn create_dhcpv6_request(
    client_mac: [u8; 6],
    client_duid: &[u8],
    server_duid: &[u8],
    addr: Ipv6Addr,
    iaid: u32,
) -> Vec<u8> {
    let src_ll = mac_to_link_local(client_mac);
    let dst_addr = Ipv6Address::from_bytes(&ALL_DHCP_SERVERS_MULTICAST);
    let dst_mac = ipv6_multicast_mac(&ALL_DHCP_SERVERS_MULTICAST);

    // Build DHCPv6 REQUEST message
    let mut request = Message::new(MessageType::Request);
    request.set_xid([0x12, 0x34, 0x56]);

    // Add Client ID
    request
        .opts_mut()
        .insert(DhcpOption::ClientId(client_duid.to_vec()));

    // Add Server ID
    request
        .opts_mut()
        .insert(DhcpOption::ServerId(server_duid.to_vec()));

    // Add IA_NA with requested address
    let ia_addr = IAAddr {
        addr,
        preferred_life: 0,
        valid_life: 0,
        opts: Default::default(),
    };
    let ia_na = IANA {
        id: iaid,
        t1: 0,
        t2: 0,
        opts: {
            let mut opts = dhcproto::v6::DhcpOptions::new();
            opts.insert(DhcpOption::IAAddr(ia_addr));
            opts
        },
    };
    request.opts_mut().insert(DhcpOption::IANA(ia_na));

    // Add Elapsed Time
    request.opts_mut().insert(DhcpOption::ElapsedTime(0));

    // Encode DHCPv6 message
    let mut dhcp_bytes = Vec::new();
    let mut encoder = Encoder::new(&mut dhcp_bytes);
    request.encode(&mut encoder).unwrap();

    build_dhcpv6_packet(client_mac, src_ll, dst_addr, dst_mac, &dhcp_bytes)
}

/// Build the complete DHCPv6 packet with Ethernet/IPv6/UDP headers
fn build_dhcpv6_packet(
    src_mac: [u8; 6],
    src_addr: Ipv6Address,
    dst_addr: Ipv6Address,
    dst_mac: [u8; 6],
    dhcp_bytes: &[u8],
) -> Vec<u8> {
    let udp_len = UDP_HDR_SIZE + dhcp_bytes.len();
    let total_len = ETHERNET_HDR_SIZE + IPV6_HDR_SIZE + udp_len;

    let mut packet = vec![0u8; total_len];

    // Ethernet header
    let eth_repr = EthernetRepr {
        src_addr: EthernetAddress(src_mac),
        dst_addr: EthernetAddress(dst_mac),
        ethertype: EthernetProtocol::Ipv6,
    };
    let mut eth_frame = EthernetFrame::new_unchecked(&mut packet[..]);
    eth_repr.emit(&mut eth_frame);

    // IPv6 header
    let ipv6_repr = Ipv6Repr {
        src_addr,
        dst_addr,
        next_header: IpProtocol::Udp,
        payload_len: udp_len,
        hop_limit: 64,
    };
    let mut ipv6_packet = Ipv6Packet::new_unchecked(eth_frame.payload_mut());
    ipv6_repr.emit(&mut ipv6_packet);

    // UDP header and payload
    let udp_start = ETHERNET_HDR_SIZE + IPV6_HDR_SIZE;
    let udp_slice = &mut packet[udp_start..];

    // Write UDP header manually
    udp_slice[0..2].copy_from_slice(&DHCP6_CLIENT_PORT.to_be_bytes());
    udp_slice[2..4].copy_from_slice(&DHCP6_SERVER_PORT.to_be_bytes());
    udp_slice[4..6].copy_from_slice(&(udp_len as u16).to_be_bytes());
    udp_slice[6..8].fill(0); // checksum placeholder
    udp_slice[8..8 + dhcp_bytes.len()].copy_from_slice(dhcp_bytes);

    // Compute UDP checksum
    let checksum = compute_udp6_checksum(&src_addr, &dst_addr, &udp_slice[..udp_len]);
    udp_slice[6..8].copy_from_slice(&checksum.to_be_bytes());

    packet
}

/// Parse a DHCPv6 response packet
pub fn parse_dhcpv6_response(packet: &[u8]) -> Option<Dhcpv6Response> {
    let eth_frame = EthernetFrame::new_checked(packet).ok()?;
    if eth_frame.ethertype() != EthernetProtocol::Ipv6 {
        return None;
    }

    let ipv6_packet = Ipv6Packet::new_checked(eth_frame.payload()).ok()?;
    if ipv6_packet.next_header() != IpProtocol::Udp {
        return None;
    }

    let udp_packet = UdpPacket::new_checked(ipv6_packet.payload()).ok()?;
    if udp_packet.src_port() != DHCP6_SERVER_PORT {
        return None;
    }

    let dhcp_payload = udp_packet.payload();
    let mut decoder = Decoder::new(dhcp_payload);
    let dhcp_msg = Message::decode(&mut decoder).ok()?;

    let msg_type = Dhcpv6MessageType::from(dhcp_msg.msg_type());
    let xid = dhcp_msg.xid();

    // Extract options
    let mut client_duid = Vec::new();
    let mut server_duid = Vec::new();
    let mut addresses = Vec::new();
    let mut dns_servers = Vec::new();

    for opt in dhcp_msg.opts().iter() {
        match opt {
            DhcpOption::ClientId(duid) => {
                client_duid = duid.clone();
            }
            DhcpOption::ServerId(duid) => {
                server_duid = duid.clone();
            }
            DhcpOption::IANA(iana) => {
                for inner_opt in iana.opts.iter() {
                    if let DhcpOption::IAAddr(ia_addr) = inner_opt {
                        addresses.push(ia_addr.addr);
                    }
                }
            }
            DhcpOption::DomainNameServers(servers) => {
                dns_servers = servers.clone();
            }
            _ => {}
        }
    }

    Some(Dhcpv6Response {
        msg_type,
        xid,
        client_duid,
        server_duid,
        addresses,
        dns_servers,
    })
}

/// Generate a DUID-LL (DUID based on link-layer address) from a MAC address
pub fn generate_duid_ll(mac: [u8; 6]) -> Vec<u8> {
    let mut duid = Vec::with_capacity(10);
    duid.extend_from_slice(&[0x00, 0x03]); // DUID-LL type
    duid.extend_from_slice(&[0x00, 0x01]); // Ethernet hardware type
    duid.extend_from_slice(&mac);
    duid
}

// ============================================================================
// Neighbor Solicitation / Neighbor Advertisement (NDP)
// ============================================================================

/// Compute solicited-node multicast address from an IPv6 address
///
/// Returns ff02::1:ffXX:XXXX where XX:XXXX are the last 24 bits of the IPv6 address
fn solicited_node_multicast(target: &Ipv6Addr) -> [u8; 16] {
    let octets = target.octets();
    [
        0xff, 0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x01, 0xff, octets[13], octets[14], octets[15],
    ]
}

/// Create a Neighbor Solicitation packet
///
/// Creates an NS packet for the given target IPv6 address using:
/// - Solicited-node multicast for destination MAC/IP
/// - Source link-layer address option (SLLAO) with the source MAC
pub fn create_neighbor_solicitation(src_mac: [u8; 6], target_ip: Ipv6Addr) -> Vec<u8> {
    let src_ll = mac_to_link_local(src_mac);
    let solicited_node = solicited_node_multicast(&target_ip);
    let dst_addr = Ipv6Address::from_bytes(&solicited_node);
    let dst_mac = ipv6_multicast_mac(&solicited_node);

    // NS packet: ICMPv6 type (1) + code (1) + checksum (2) + reserved (4) + target (16) + SLLAO (8)
    let icmpv6_len = 32;
    let ip_len = IPV6_HDR_SIZE + icmpv6_len;
    let total_len = ETHERNET_HDR_SIZE + ip_len;

    let mut packet = vec![0u8; total_len];

    // Ethernet header
    let eth_repr = EthernetRepr {
        src_addr: EthernetAddress(src_mac),
        dst_addr: EthernetAddress(dst_mac),
        ethertype: EthernetProtocol::Ipv6,
    };
    let mut eth_frame = EthernetFrame::new_unchecked(&mut packet[..]);
    eth_repr.emit(&mut eth_frame);

    // IPv6 header
    let ipv6_repr = Ipv6Repr {
        src_addr: src_ll,
        dst_addr,
        next_header: IpProtocol::Icmpv6,
        payload_len: icmpv6_len,
        hop_limit: 255,
    };
    let mut ipv6_packet = Ipv6Packet::new_unchecked(eth_frame.payload_mut());
    ipv6_repr.emit(&mut ipv6_packet);

    // ICMPv6 NS
    let icmpv6_start = ETHERNET_HDR_SIZE + IPV6_HDR_SIZE;
    let icmpv6_data = &mut packet[icmpv6_start..];

    // Type: Neighbor Solicitation (135)
    icmpv6_data[0] = 135;
    // Code: 0
    icmpv6_data[1] = 0;
    // Checksum: placeholder
    icmpv6_data[2..4].fill(0);
    // Reserved
    icmpv6_data[4..8].fill(0);
    // Target Address (16 bytes)
    icmpv6_data[8..24].copy_from_slice(&target_ip.octets());
    // Source Link-Layer Address Option (SLLAO)
    icmpv6_data[24] = 1; // Type: Source Link-Layer Address
    icmpv6_data[25] = 1; // Length: 1 (in 8-byte units)
    icmpv6_data[26..32].copy_from_slice(&src_mac);

    // Compute ICMPv6 checksum
    let checksum = compute_icmpv6_checksum(&src_ll, &dst_addr, &icmpv6_data[..icmpv6_len]);
    icmpv6_data[2..4].copy_from_slice(&checksum.to_be_bytes());

    packet
}

/// Parsed Neighbor Advertisement response
#[derive(Debug)]
pub struct NaResponse {
    /// Target IPv6 address being advertised
    pub target_addr: Ipv6Addr,
    /// Router flag (R) - sender is a router
    pub router_flag: bool,
    /// Solicited flag (S) - response to a solicitation
    pub solicited_flag: bool,
    /// Override flag (O) - should override existing cache entry
    pub override_flag: bool,
    /// Target MAC address from Target Link-Layer Address Option (TLLAO)
    pub target_mac: Option<[u8; 6]>,
}

/// Parse a Neighbor Advertisement packet
pub fn parse_neighbor_advertisement(packet: &[u8]) -> Option<NaResponse> {
    let eth_frame = EthernetFrame::new_checked(packet).ok()?;
    if eth_frame.ethertype() != EthernetProtocol::Ipv6 {
        return None;
    }

    let ipv6_packet = Ipv6Packet::new_checked(eth_frame.payload()).ok()?;
    if ipv6_packet.next_header() != IpProtocol::Icmpv6 {
        return None;
    }

    let payload = ipv6_packet.payload();
    // Minimum NA size: type(1) + code(1) + checksum(2) + flags(4) + target(16) = 24 bytes
    if payload.len() < 24 || payload[0] != 136 {
        // Type: NA = 136
        return None;
    }

    // Parse flags (first byte of the 4-byte flags/reserved field)
    let flags = payload[4];
    let router_flag = (flags & 0x80) != 0;
    let solicited_flag = (flags & 0x40) != 0;
    let override_flag = (flags & 0x20) != 0;

    // Parse target address (bytes 8-23)
    let target_addr = Ipv6Addr::from(<[u8; 16]>::try_from(&payload[8..24]).ok()?);

    // Parse options for TLLAO (Target Link-Layer Address Option)
    let mut target_mac = None;
    let mut idx = 24; // After fixed NA header
    while idx + 2 <= payload.len() {
        let opt_type = payload[idx];
        let opt_len = payload[idx + 1] as usize * 8;
        if opt_len == 0 || idx + opt_len > payload.len() {
            break;
        }
        if opt_type == 2 && opt_len >= 8 {
            // TLLAO (type = 2)
            target_mac = Some([
                payload[idx + 2],
                payload[idx + 3],
                payload[idx + 4],
                payload[idx + 5],
                payload[idx + 6],
                payload[idx + 7],
            ]);
        }
        idx += opt_len;
    }

    Some(NaResponse {
        target_addr,
        router_flag,
        solicited_flag,
        override_flag,
        target_mac,
    })
}

// ============================================================================
// ICMPv6 Echo Request / Echo Reply (ping6)
// ============================================================================

/// Create an ICMPv6 Echo Request packet (ping6)
pub fn create_icmpv6_echo_request(
    src_mac: [u8; 6],
    dst_mac: [u8; 6],
    dst_ip: Ipv6Addr,
    id: u16,
    seq: u16,
    data: &[u8],
) -> Vec<u8> {
    let src_ll = mac_to_link_local(src_mac);
    let dst_addr = Ipv6Address::from_bytes(&dst_ip.octets());

    // Echo Request: type(1) + code(1) + checksum(2) + id(2) + seq(2) + data
    let icmpv6_len = 8 + data.len();
    let ip_len = IPV6_HDR_SIZE + icmpv6_len;
    let total_len = ETHERNET_HDR_SIZE + ip_len;

    let mut packet = vec![0u8; total_len];

    // Ethernet header
    let eth_repr = EthernetRepr {
        src_addr: EthernetAddress(src_mac),
        dst_addr: EthernetAddress(dst_mac),
        ethertype: EthernetProtocol::Ipv6,
    };
    let mut eth_frame = EthernetFrame::new_unchecked(&mut packet[..]);
    eth_repr.emit(&mut eth_frame);

    // IPv6 header
    let ipv6_repr = Ipv6Repr {
        src_addr: src_ll,
        dst_addr,
        next_header: IpProtocol::Icmpv6,
        payload_len: icmpv6_len,
        hop_limit: 64,
    };
    let mut ipv6_packet = Ipv6Packet::new_unchecked(eth_frame.payload_mut());
    ipv6_repr.emit(&mut ipv6_packet);

    // ICMPv6 Echo Request
    let icmpv6_start = ETHERNET_HDR_SIZE + IPV6_HDR_SIZE;
    let icmpv6_data = &mut packet[icmpv6_start..];

    // Type: Echo Request (128)
    icmpv6_data[0] = 128;
    // Code: 0
    icmpv6_data[1] = 0;
    // Checksum: placeholder
    icmpv6_data[2..4].fill(0);
    // Identifier
    icmpv6_data[4..6].copy_from_slice(&id.to_be_bytes());
    // Sequence Number
    icmpv6_data[6..8].copy_from_slice(&seq.to_be_bytes());
    // Data
    icmpv6_data[8..8 + data.len()].copy_from_slice(data);

    // Compute ICMPv6 checksum
    let checksum = compute_icmpv6_checksum(&src_ll, &dst_addr, &icmpv6_data[..icmpv6_len]);
    icmpv6_data[2..4].copy_from_slice(&checksum.to_be_bytes());

    packet
}

/// Parsed ICMPv6 Echo Reply response
#[derive(Debug)]
pub struct Icmpv6EchoReply {
    /// Source IPv6 address
    pub src_addr: Ipv6Addr,
    /// Identifier (should match request)
    pub id: u16,
    /// Sequence number (should match request)
    pub seq: u16,
    /// Echo data (should match request)
    pub data: Vec<u8>,
}

/// Parse an ICMPv6 Echo Reply packet
pub fn parse_icmpv6_echo_reply(packet: &[u8]) -> Option<Icmpv6EchoReply> {
    let eth_frame = EthernetFrame::new_checked(packet).ok()?;
    if eth_frame.ethertype() != EthernetProtocol::Ipv6 {
        return None;
    }

    let ipv6_packet = Ipv6Packet::new_checked(eth_frame.payload()).ok()?;
    if ipv6_packet.next_header() != IpProtocol::Icmpv6 {
        return None;
    }

    let payload = ipv6_packet.payload();
    // Minimum Echo Reply: type(1) + code(1) + checksum(2) + id(2) + seq(2) = 8
    if payload.len() < 8 || payload[0] != 129 {
        // Type: Echo Reply = 129
        return None;
    }

    let src_addr = Ipv6Addr::from(ipv6_packet.src_addr().0);
    let id = u16::from_be_bytes([payload[4], payload[5]]);
    let seq = u16::from_be_bytes([payload[6], payload[7]]);
    let data = payload[8..].to_vec();

    Some(Icmpv6EchoReply {
        src_addr,
        id,
        seq,
        data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dhcp_discover_packet() {
        let mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
        let xid = 0x12345678;

        let packet = create_dhcp_discover(mac, xid);

        // Verify packet starts with Ethernet header (no virtio header)
        let eth = EthernetFrame::new_checked(&packet).expect("Ethernet parse failed");
        assert_eq!(eth.src_addr().0, mac);
        assert_eq!(eth.dst_addr().0, BROADCAST_MAC);
        assert_eq!(eth.ethertype(), EthernetProtocol::Ipv4);

        // Verify IP
        let ip = Ipv4Packet::new_checked(eth.payload()).expect("IP parse failed");
        assert_eq!(ip.src_addr(), Ipv4Address::UNSPECIFIED);
        assert_eq!(ip.dst_addr(), Ipv4Address::BROADCAST);
        assert_eq!(ip.next_header(), IpProtocol::Udp);

        // Verify UDP
        let udp = UdpPacket::new_checked(ip.payload()).expect("UDP parse failed");
        assert_eq!(udp.src_port(), DHCP_CLIENT_PORT);
        assert_eq!(udp.dst_port(), DHCP_SERVER_PORT);

        // Verify DHCP
        let dhcp = udp.payload();
        assert_eq!(dhcp[0], 1); // BOOTREQUEST
        let parsed_xid = u32::from_be_bytes([dhcp[4], dhcp[5], dhcp[6], dhcp[7]]);
        assert_eq!(parsed_xid, xid);
    }

    #[test]
    fn test_arp_request_packet() {
        let mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
        let sender_ip = [10, 0, 0, 100];
        let target_ip = [10, 0, 0, 1];

        let packet = create_arp_request(mac, sender_ip, target_ip);

        // Verify Ethernet
        let eth = EthernetFrame::new_checked(&packet).expect("Ethernet parse failed");
        assert_eq!(eth.src_addr().0, mac);
        assert_eq!(eth.dst_addr().0, BROADCAST_MAC);
        assert_eq!(eth.ethertype(), EthernetProtocol::Arp);

        // Verify ARP
        let arp = ArpPacket::new_checked(eth.payload()).expect("ARP parse failed");
        let repr = ArpRepr::parse(&arp).expect("ARP repr failed");
        match repr {
            ArpRepr::EthernetIpv4 {
                operation,
                source_hardware_addr,
                source_protocol_addr,
                target_protocol_addr,
                ..
            } => {
                assert_eq!(operation, ArpOperation::Request);
                assert_eq!(source_hardware_addr.0, mac);
                assert_eq!(source_protocol_addr.0, sender_ip);
                assert_eq!(target_protocol_addr.0, target_ip);
            }
            _ => panic!("Expected EthernetIpv4"),
        }
    }

    #[test]
    fn test_icmp_echo_request_packet() {
        let src_mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
        let dst_mac = [0x52, 0x54, 0x00, 0xab, 0xcd, 0xef];
        let src_ip = [10, 0, 0, 100];
        let dst_ip = [10, 0, 0, 1];

        let packet = create_icmp_echo_request(src_mac, dst_mac, src_ip, dst_ip, 1234, 1);

        // Verify Ethernet
        let eth = EthernetFrame::new_checked(&packet).expect("Ethernet parse failed");
        assert_eq!(eth.src_addr().0, src_mac);
        assert_eq!(eth.dst_addr().0, dst_mac);
        assert_eq!(eth.ethertype(), EthernetProtocol::Ipv4);

        // Verify IP
        let ip = Ipv4Packet::new_checked(eth.payload()).expect("IP parse failed");
        assert_eq!(ip.src_addr().0, src_ip);
        assert_eq!(ip.dst_addr().0, dst_ip);
        assert_eq!(ip.next_header(), IpProtocol::Icmp);

        // Verify ICMP
        let icmp = Icmpv4Packet::new_checked(ip.payload()).expect("ICMP parse failed");
        assert_eq!(icmp.msg_type(), Icmpv4Message::EchoRequest);
        assert_eq!(icmp.echo_ident(), 1234);
        assert_eq!(icmp.echo_seq_no(), 1);
    }

    #[test]
    fn test_ns_packet_structure() {
        let src_mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x02];
        let target_ip = Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1);

        let packet = create_neighbor_solicitation(src_mac, target_ip);

        // Verify packet length: eth(14) + ipv6(40) + icmpv6(32) = 86
        assert_eq!(packet.len(), 86);

        // Parse Ethernet
        let eth = EthernetFrame::new_checked(&packet).expect("Ethernet parse failed");
        assert_eq!(eth.ethertype(), EthernetProtocol::Ipv6);

        // Verify solicited-node multicast destination MAC
        let expected_dst_mac = [0x33, 0x33, 0xff, 0x00, 0x00, 0x01];
        assert_eq!(eth.dst_addr().0, expected_dst_mac);

        // Parse IPv6
        let ipv6 = Ipv6Packet::new_checked(eth.payload()).expect("IPv6 parse failed");
        assert_eq!(ipv6.next_header(), IpProtocol::Icmpv6);
        assert_eq!(ipv6.hop_limit(), 255);

        // Verify solicited-node multicast destination
        let expected_dst_ip = Ipv6Address::new(0xff02, 0, 0, 0, 0, 0x0001, 0xff00, 0x0001);
        assert_eq!(ipv6.dst_addr(), expected_dst_ip);

        // Parse ICMPv6
        let icmpv6_payload = ipv6.payload();
        assert!(icmpv6_payload.len() >= 24, "ICMPv6 too short");

        // Type: 135 (NS)
        assert_eq!(icmpv6_payload[0], 135);
        // Code: 0
        assert_eq!(icmpv6_payload[1], 0);

        // Target address at bytes 8-23
        let target_bytes: [u8; 16] = icmpv6_payload[8..24].try_into().unwrap();
        let parsed_target = Ipv6Addr::from(target_bytes);
        assert_eq!(parsed_target, target_ip);
    }
}
