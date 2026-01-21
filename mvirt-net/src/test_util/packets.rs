//! Protocol packet builders for integration tests
//!
//! Uses smoltcp for packet construction, matching the reactor's implementation.

use super::frontend_device::VIRTIO_NET_HDR_SIZE;
use smoltcp::wire::{
    ArpOperation, ArpPacket, ArpRepr, EthernetAddress, EthernetFrame, EthernetProtocol,
    EthernetRepr, Icmpv4Message, Icmpv4Packet, Icmpv4Repr, IpProtocol, Ipv4Address, Ipv4Packet,
    Ipv4Repr, UdpPacket, UdpRepr,
};

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

/// Broadcast MAC address
pub const BROADCAST_MAC: [u8; 6] = [0xff, 0xff, 0xff, 0xff, 0xff, 0xff];

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
    let udp_len = UDP_HDR_SIZE + dhcp.len();
    let ip_len = IP_HDR_SIZE + udp_len;
    let total_len = VIRTIO_NET_HDR_SIZE + ETHERNET_HDR_SIZE + ip_len;

    let mut packet = vec![0u8; total_len];

    // Ethernet frame
    let eth_repr = EthernetRepr {
        src_addr: EthernetAddress(client_mac),
        dst_addr: EthernetAddress(BROADCAST_MAC),
        ethertype: EthernetProtocol::Ipv4,
    };
    let mut eth_frame = EthernetFrame::new_unchecked(&mut packet[VIRTIO_NET_HDR_SIZE..]);
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

/// Parse a DHCP response packet
pub fn parse_dhcp_response(packet: &[u8]) -> Option<DhcpResponse> {
    let eth_frame = EthernetFrame::new_checked(&packet[VIRTIO_NET_HDR_SIZE..]).ok()?;
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
    let total_size = VIRTIO_NET_HDR_SIZE + ETHERNET_HDR_SIZE + ARP_PKT_SIZE;
    let mut packet = vec![0u8; total_size];

    // Ethernet frame
    let eth_repr = EthernetRepr {
        src_addr: EthernetAddress(sender_mac),
        dst_addr: EthernetAddress(BROADCAST_MAC),
        ethertype: EthernetProtocol::Arp,
    };
    let mut eth_frame = EthernetFrame::new_unchecked(&mut packet[VIRTIO_NET_HDR_SIZE..]);
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
    let eth_frame = EthernetFrame::new_checked(&packet[VIRTIO_NET_HDR_SIZE..]).ok()?;
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
    let total_len = VIRTIO_NET_HDR_SIZE + ETHERNET_HDR_SIZE + ip_len;

    let mut packet = vec![0u8; total_len];

    // Ethernet frame
    let eth_repr = EthernetRepr {
        src_addr: EthernetAddress(src_mac),
        dst_addr: EthernetAddress(dst_mac),
        ethertype: EthernetProtocol::Ipv4,
    };
    let mut eth_frame = EthernetFrame::new_unchecked(&mut packet[VIRTIO_NET_HDR_SIZE..]);
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
    let eth_frame = EthernetFrame::new_checked(&packet[VIRTIO_NET_HDR_SIZE..]).ok()?;
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
