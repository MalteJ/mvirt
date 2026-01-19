//! Packet building utilities for tests
//!
//! Functions to construct Ethernet frames, ARP packets, and DHCP messages.

use std::net::Ipv4Addr;

use dhcproto::v4::{DhcpOption, Message, MessageType, Opcode, OptionCode};
use dhcproto::{Decodable, Encodable};
use smoltcp::wire::{
    ArpOperation, ArpPacket, ArpRepr, EthernetAddress, EthernetFrame, EthernetProtocol,
    EthernetRepr, Icmpv4Message, Icmpv4Packet, Icmpv4Repr, IpProtocol, Ipv4Address, Ipv4Packet,
    Ipv4Repr, UdpPacket, UdpRepr,
};

/// DHCP ports
const DHCP_CLIENT_PORT: u16 = 68;
const DHCP_SERVER_PORT: u16 = 67;

/// Build a raw Ethernet frame
pub fn ethernet_frame(dst: [u8; 6], src: [u8; 6], ethertype: u16, payload: &[u8]) -> Vec<u8> {
    let repr = EthernetRepr {
        src_addr: EthernetAddress::from_bytes(&src),
        dst_addr: EthernetAddress::from_bytes(&dst),
        ethertype: EthernetProtocol::from(ethertype),
    };

    let mut buffer = vec![0u8; repr.buffer_len() + payload.len()];
    let mut frame = EthernetFrame::new_unchecked(&mut buffer);
    repr.emit(&mut frame);
    frame.payload_mut().copy_from_slice(payload);
    buffer
}

/// Build an ARP request packet
pub fn arp_request(sender_mac: [u8; 6], sender_ip: [u8; 4], target_ip: [u8; 4]) -> Vec<u8> {
    let arp_repr = ArpRepr::EthernetIpv4 {
        operation: ArpOperation::Request,
        source_hardware_addr: EthernetAddress::from_bytes(&sender_mac),
        source_protocol_addr: Ipv4Address::from_octets(sender_ip),
        target_hardware_addr: EthernetAddress::from_bytes(&[0, 0, 0, 0, 0, 0]),
        target_protocol_addr: Ipv4Address::from_octets(target_ip),
    };

    let eth_repr = EthernetRepr {
        src_addr: EthernetAddress::from_bytes(&sender_mac),
        dst_addr: EthernetAddress::BROADCAST,
        ethertype: EthernetProtocol::Arp,
    };

    let mut buffer = vec![0u8; eth_repr.buffer_len() + arp_repr.buffer_len()];
    let mut frame = EthernetFrame::new_unchecked(&mut buffer);
    eth_repr.emit(&mut frame);

    let mut arp_packet = ArpPacket::new_unchecked(frame.payload_mut());
    arp_repr.emit(&mut arp_packet);

    buffer
}

/// Parsed ARP reply
#[derive(Debug, Clone)]
pub struct ArpReply {
    pub sender_mac: [u8; 6],
    pub sender_ip: [u8; 4],
    pub target_mac: [u8; 6],
    pub target_ip: [u8; 4],
}

/// Parse an ARP reply from an Ethernet frame
pub fn parse_arp_reply(frame: &[u8]) -> Option<ArpReply> {
    let eth = EthernetFrame::new_checked(frame).ok()?;
    if eth.ethertype() != EthernetProtocol::Arp {
        return None;
    }

    let arp = ArpPacket::new_checked(eth.payload()).ok()?;
    let repr = ArpRepr::parse(&arp).ok()?;

    match repr {
        ArpRepr::EthernetIpv4 {
            operation: ArpOperation::Reply,
            source_hardware_addr,
            source_protocol_addr,
            target_hardware_addr,
            target_protocol_addr,
        } => Some(ArpReply {
            sender_mac: source_hardware_addr.as_bytes().try_into().ok()?,
            sender_ip: source_protocol_addr.octets(),
            target_mac: target_hardware_addr.as_bytes().try_into().ok()?,
            target_ip: target_protocol_addr.octets(),
        }),
        _ => None,
    }
}

/// Check if frame is an ARP reply
pub fn is_arp_reply(frame: &[u8]) -> bool {
    parse_arp_reply(frame).is_some()
}

/// Build a DHCP Discover packet wrapped in UDP/IP/Ethernet
pub fn dhcp_discover(client_mac: [u8; 6], xid: u32) -> Vec<u8> {
    use dhcproto::v4::{Flags, HType};

    let mut msg = Message::default();
    msg.set_opcode(Opcode::BootRequest);
    msg.set_htype(HType::Eth);
    msg.set_xid(xid);
    msg.set_flags(Flags::default().set_broadcast());

    // Set client hardware address (padded to 16 bytes)
    let mut chaddr = [0u8; 16];
    chaddr[..6].copy_from_slice(&client_mac);
    msg.set_chaddr(&chaddr);

    msg.opts_mut()
        .insert(DhcpOption::MessageType(MessageType::Discover));

    let dhcp_data = msg.to_vec().expect("DHCP encode failed");
    wrap_dhcp_in_udp_ip_eth(dhcp_data, client_mac)
}

/// Build a DHCP Request packet
pub fn dhcp_request(
    client_mac: [u8; 6],
    xid: u32,
    requested_ip: [u8; 4],
    server_ip: [u8; 4],
) -> Vec<u8> {
    use dhcproto::v4::{Flags, HType};

    let mut msg = Message::default();
    msg.set_opcode(Opcode::BootRequest);
    msg.set_htype(HType::Eth);
    msg.set_xid(xid);
    msg.set_flags(Flags::default().set_broadcast());

    let mut chaddr = [0u8; 16];
    chaddr[..6].copy_from_slice(&client_mac);
    msg.set_chaddr(&chaddr);

    msg.opts_mut()
        .insert(DhcpOption::MessageType(MessageType::Request));
    msg.opts_mut()
        .insert(DhcpOption::RequestedIpAddress(Ipv4Addr::from(requested_ip)));
    msg.opts_mut()
        .insert(DhcpOption::ServerIdentifier(Ipv4Addr::from(server_ip)));

    let dhcp_data = msg.to_vec().expect("DHCP encode failed");
    wrap_dhcp_in_udp_ip_eth(dhcp_data, client_mac)
}

/// Wrap DHCP message in UDP/IP/Ethernet
fn wrap_dhcp_in_udp_ip_eth(dhcp_data: Vec<u8>, client_mac: [u8; 6]) -> Vec<u8> {
    let client_eth = EthernetAddress::from_bytes(&client_mac);

    let udp_repr = UdpRepr {
        src_port: DHCP_CLIENT_PORT,
        dst_port: DHCP_SERVER_PORT,
    };

    let ipv4_repr = Ipv4Repr {
        src_addr: Ipv4Address::UNSPECIFIED,
        dst_addr: Ipv4Address::BROADCAST,
        next_header: IpProtocol::Udp,
        payload_len: udp_repr.header_len() + dhcp_data.len(),
        hop_limit: 64,
    };

    let eth_repr = EthernetRepr {
        src_addr: client_eth,
        dst_addr: EthernetAddress::BROADCAST,
        ethertype: EthernetProtocol::Ipv4,
    };

    let total_len =
        eth_repr.buffer_len() + ipv4_repr.buffer_len() + udp_repr.header_len() + dhcp_data.len();

    let mut buffer = vec![0u8; total_len];

    // Emit Ethernet
    let mut eth_frame = EthernetFrame::new_unchecked(&mut buffer);
    eth_repr.emit(&mut eth_frame);

    // Emit IPv4
    let mut ipv4_packet = Ipv4Packet::new_unchecked(eth_frame.payload_mut());
    ipv4_repr.emit(
        &mut ipv4_packet,
        &smoltcp::phy::ChecksumCapabilities::default(),
    );

    // Emit UDP
    let mut udp_packet = UdpPacket::new_unchecked(ipv4_packet.payload_mut());
    udp_repr.emit(
        &mut udp_packet,
        &ipv4_repr.src_addr.into(),
        &ipv4_repr.dst_addr.into(),
        dhcp_data.len(),
        |buf| buf.copy_from_slice(&dhcp_data),
        &smoltcp::phy::ChecksumCapabilities::default(),
    );

    buffer
}

/// DHCP message types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DhcpMessageType {
    Discover,
    Offer,
    Request,
    Decline,
    Ack,
    Nak,
    Release,
    Inform,
    Unknown(u8),
}

impl From<MessageType> for DhcpMessageType {
    fn from(t: MessageType) -> Self {
        match t {
            MessageType::Discover => DhcpMessageType::Discover,
            MessageType::Offer => DhcpMessageType::Offer,
            MessageType::Request => DhcpMessageType::Request,
            MessageType::Decline => DhcpMessageType::Decline,
            MessageType::Ack => DhcpMessageType::Ack,
            MessageType::Nak => DhcpMessageType::Nak,
            MessageType::Release => DhcpMessageType::Release,
            MessageType::Inform => DhcpMessageType::Inform,
            _ => DhcpMessageType::Unknown(0),
        }
    }
}

/// Parsed DHCP response
#[derive(Debug, Clone)]
pub struct DhcpResponse {
    pub message_type: DhcpMessageType,
    pub xid: u32,
    pub your_ip: [u8; 4],
    pub server_ip: [u8; 4],
    pub subnet_mask: Option<[u8; 4]>,
    pub router: Option<[u8; 4]>,
    pub dns_servers: Vec<[u8; 4]>,
    pub lease_time: Option<u32>,
}

/// Parse a DHCP response from an Ethernet frame
pub fn parse_dhcp_response(frame: &[u8]) -> Option<DhcpResponse> {
    let eth = EthernetFrame::new_checked(frame).ok()?;
    if eth.ethertype() != EthernetProtocol::Ipv4 {
        return None;
    }

    let ipv4 = Ipv4Packet::new_checked(eth.payload()).ok()?;
    if ipv4.next_header() != IpProtocol::Udp {
        return None;
    }

    let udp = UdpPacket::new_checked(ipv4.payload()).ok()?;
    if udp.src_port() != DHCP_SERVER_PORT || udp.dst_port() != DHCP_CLIENT_PORT {
        return None;
    }

    let mut decoder = dhcproto::decoder::Decoder::new(udp.payload());
    let msg = Message::decode(&mut decoder).ok()?;

    if msg.opcode() != Opcode::BootReply {
        return None;
    }

    let msg_type = msg.opts().get(OptionCode::MessageType).and_then(|opt| {
        if let DhcpOption::MessageType(t) = opt {
            Some(DhcpMessageType::from(*t))
        } else {
            None
        }
    })?;

    let server_ip = msg
        .opts()
        .get(OptionCode::ServerIdentifier)
        .and_then(|opt| {
            if let DhcpOption::ServerIdentifier(ip) = opt {
                Some(ip.octets())
            } else {
                None
            }
        })
        .unwrap_or([0; 4]);

    let subnet_mask = msg.opts().get(OptionCode::SubnetMask).and_then(|opt| {
        if let DhcpOption::SubnetMask(ip) = opt {
            Some(ip.octets())
        } else {
            None
        }
    });

    let router = msg.opts().get(OptionCode::Router).and_then(|opt| {
        if let DhcpOption::Router(ips) = opt {
            ips.first().map(|ip| ip.octets())
        } else {
            None
        }
    });

    let dns_servers = msg
        .opts()
        .get(OptionCode::DomainNameServer)
        .map(|opt| {
            if let DhcpOption::DomainNameServer(ips) = opt {
                ips.iter().map(|ip| ip.octets()).collect()
            } else {
                Vec::new()
            }
        })
        .unwrap_or_default();

    let lease_time = msg
        .opts()
        .get(OptionCode::AddressLeaseTime)
        .and_then(|opt| {
            if let DhcpOption::AddressLeaseTime(t) = opt {
                Some(*t)
            } else {
                None
            }
        });

    Some(DhcpResponse {
        message_type: msg_type,
        xid: msg.xid(),
        your_ip: msg.yiaddr().octets(),
        server_ip,
        subnet_mask,
        router,
        dns_servers,
        lease_time,
    })
}

/// Build an ICMP echo request (ping) packet
pub fn icmp_echo_request(
    src_mac: [u8; 6],
    dst_mac: [u8; 6],
    src_ip: [u8; 4],
    dst_ip: [u8; 4],
    ident: u16,
    seq_no: u16,
    data: &[u8],
) -> Vec<u8> {
    let icmp_repr = Icmpv4Repr::EchoRequest {
        ident,
        seq_no,
        data,
    };

    let ipv4_repr = Ipv4Repr {
        src_addr: Ipv4Address::from_octets(src_ip),
        dst_addr: Ipv4Address::from_octets(dst_ip),
        next_header: IpProtocol::Icmp,
        payload_len: icmp_repr.buffer_len(),
        hop_limit: 64,
    };

    let eth_repr = EthernetRepr {
        src_addr: EthernetAddress::from_bytes(&src_mac),
        dst_addr: EthernetAddress::from_bytes(&dst_mac),
        ethertype: EthernetProtocol::Ipv4,
    };

    let total_len = eth_repr.buffer_len() + ipv4_repr.buffer_len() + icmp_repr.buffer_len();
    let mut buffer = vec![0u8; total_len];

    let mut frame = EthernetFrame::new_unchecked(&mut buffer);
    eth_repr.emit(&mut frame);

    let mut ipv4_packet = Ipv4Packet::new_unchecked(frame.payload_mut());
    ipv4_repr.emit(
        &mut ipv4_packet,
        &smoltcp::phy::ChecksumCapabilities::default(),
    );

    let mut icmp_packet = Icmpv4Packet::new_unchecked(ipv4_packet.payload_mut());
    icmp_repr.emit(
        &mut icmp_packet,
        &smoltcp::phy::ChecksumCapabilities::default(),
    );

    buffer
}

/// Parsed ICMP echo reply
#[derive(Debug, Clone)]
pub struct IcmpEchoReply {
    pub src_ip: [u8; 4],
    pub dst_ip: [u8; 4],
    pub ident: u16,
    pub seq_no: u16,
    pub data: Vec<u8>,
}

/// Parse an ICMP echo reply from an Ethernet frame
pub fn parse_icmp_echo_reply(frame: &[u8]) -> Option<IcmpEchoReply> {
    let eth = EthernetFrame::new_checked(frame).ok()?;
    if eth.ethertype() != EthernetProtocol::Ipv4 {
        return None;
    }

    let ipv4 = Ipv4Packet::new_checked(eth.payload()).ok()?;
    if ipv4.next_header() != IpProtocol::Icmp {
        return None;
    }

    let icmp = Icmpv4Packet::new_checked(ipv4.payload()).ok()?;
    if icmp.msg_type() != Icmpv4Message::EchoReply {
        return None;
    }

    let repr = Icmpv4Repr::parse(&icmp, &smoltcp::phy::ChecksumCapabilities::default()).ok()?;

    if let Icmpv4Repr::EchoReply {
        ident,
        seq_no,
        data,
    } = repr
    {
        Some(IcmpEchoReply {
            src_ip: ipv4.src_addr().octets().try_into().ok()?,
            dst_ip: ipv4.dst_addr().octets().try_into().ok()?,
            ident,
            seq_no,
            data: data.to_vec(),
        })
    } else {
        None
    }
}

/// Check if frame is an ICMP echo reply
pub fn is_icmp_echo_reply(frame: &[u8]) -> bool {
    parse_icmp_echo_reply(frame).is_some()
}

/// Parsed ICMP echo request
#[derive(Debug, Clone)]
pub struct IcmpEchoRequest {
    pub src_mac: [u8; 6],
    pub dst_mac: [u8; 6],
    pub src_ip: [u8; 4],
    pub dst_ip: [u8; 4],
    pub ident: u16,
    pub seq_no: u16,
    pub data: Vec<u8>,
}

/// Parse an ICMP echo request from an Ethernet frame
pub fn parse_icmp_echo_request(frame: &[u8]) -> Option<IcmpEchoRequest> {
    let eth = EthernetFrame::new_checked(frame).ok()?;
    if eth.ethertype() != EthernetProtocol::Ipv4 {
        return None;
    }

    let ipv4 = Ipv4Packet::new_checked(eth.payload()).ok()?;
    if ipv4.next_header() != IpProtocol::Icmp {
        return None;
    }

    let icmp = Icmpv4Packet::new_checked(ipv4.payload()).ok()?;
    if icmp.msg_type() != Icmpv4Message::EchoRequest {
        return None;
    }

    let repr = Icmpv4Repr::parse(&icmp, &smoltcp::phy::ChecksumCapabilities::default()).ok()?;

    if let Icmpv4Repr::EchoRequest {
        ident,
        seq_no,
        data,
    } = repr
    {
        Some(IcmpEchoRequest {
            src_mac: eth.src_addr().as_bytes().try_into().ok()?,
            dst_mac: eth.dst_addr().as_bytes().try_into().ok()?,
            src_ip: ipv4.src_addr().octets().try_into().ok()?,
            dst_ip: ipv4.dst_addr().octets().try_into().ok()?,
            ident,
            seq_no,
            data: data.to_vec(),
        })
    } else {
        None
    }
}

/// Check if frame is an ICMP echo request
pub fn is_icmp_echo_request(frame: &[u8]) -> bool {
    parse_icmp_echo_request(frame).is_some()
}

// ============================================================================
// IPv6 NDP Packets
// ============================================================================

use smoltcp::wire::{
    Icmpv6Message, Icmpv6Packet, Icmpv6Repr, IpAddress, Ipv6Packet, Ipv6Repr, NdiscNeighborFlags,
    NdiscRepr, NdiscRouterFlags, RawHardwareAddress,
};

/// Gateway IPv6 link-local address
pub const GATEWAY_IPV6: [u8; 16] = [0xfe, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x01];

/// Build a Neighbor Solicitation packet for the gateway
pub fn neighbor_solicitation(src_mac: [u8; 6], src_ip: [u8; 16], target_ip: [u8; 16]) -> Vec<u8> {
    let src_addr = smoltcp::wire::Ipv6Address::from_octets(src_ip);
    let target_addr = smoltcp::wire::Ipv6Address::from_octets(target_ip);

    // Solicited-node multicast address for target
    let dst_addr = smoltcp::wire::Ipv6Address::new(
        0xff02,
        0,
        0,
        0,
        0,
        1,
        0xff00 | (target_ip[13] as u16),
        ((target_ip[14] as u16) << 8) | (target_ip[15] as u16),
    );

    let lladdr = RawHardwareAddress::from_bytes(&src_mac);
    let icmp_repr = Icmpv6Repr::Ndisc(NdiscRepr::NeighborSolicit {
        target_addr,
        lladdr: Some(lladdr),
    });

    let ipv6_repr = Ipv6Repr {
        src_addr,
        dst_addr,
        next_header: IpProtocol::Icmpv6,
        payload_len: icmp_repr.buffer_len(),
        hop_limit: 255,
    };

    // Solicited-node multicast MAC: 33:33:ff:XX:XX:XX
    let dst_mac = [
        0x33,
        0x33,
        0xff,
        target_ip[13],
        target_ip[14],
        target_ip[15],
    ];

    let eth_repr = EthernetRepr {
        src_addr: EthernetAddress::from_bytes(&src_mac),
        dst_addr: EthernetAddress::from_bytes(&dst_mac),
        ethertype: EthernetProtocol::Ipv6,
    };

    let total_len = eth_repr.buffer_len() + ipv6_repr.buffer_len() + icmp_repr.buffer_len();
    let mut buffer = vec![0u8; total_len];

    let mut frame = EthernetFrame::new_unchecked(&mut buffer);
    eth_repr.emit(&mut frame);

    let mut ipv6_packet = Ipv6Packet::new_unchecked(frame.payload_mut());
    ipv6_repr.emit(&mut ipv6_packet);

    let mut icmp_packet = Icmpv6Packet::new_unchecked(ipv6_packet.payload_mut());
    icmp_repr.emit(
        &src_addr,
        &dst_addr,
        &mut icmp_packet,
        &smoltcp::phy::ChecksumCapabilities::default(),
    );

    buffer
}

/// Parsed Neighbor Advertisement
#[derive(Debug, Clone)]
pub struct NeighborAdvertisement {
    pub src_ip: [u8; 16],
    pub target_ip: [u8; 16],
    pub target_mac: [u8; 6],
    pub router: bool,
    pub solicited: bool,
}

/// Parse a Neighbor Advertisement from an Ethernet frame
pub fn parse_neighbor_advertisement(frame: &[u8]) -> Option<NeighborAdvertisement> {
    let eth = EthernetFrame::new_checked(frame).ok()?;
    if eth.ethertype() != EthernetProtocol::Ipv6 {
        return None;
    }

    let ipv6 = Ipv6Packet::new_checked(eth.payload()).ok()?;
    if ipv6.next_header() != IpProtocol::Icmpv6 {
        return None;
    }

    let icmp = Icmpv6Packet::new_checked(ipv6.payload()).ok()?;
    if icmp.msg_type() != Icmpv6Message::NeighborAdvert {
        return None;
    }

    let repr = Icmpv6Repr::parse(
        &ipv6.src_addr(),
        &ipv6.dst_addr(),
        &icmp,
        &smoltcp::phy::ChecksumCapabilities::default(),
    )
    .ok()?;

    if let Icmpv6Repr::Ndisc(NdiscRepr::NeighborAdvert {
        flags,
        target_addr,
        lladdr,
    }) = repr
    {
        let target_mac = lladdr.map(|l| {
            let bytes = l.as_bytes();
            [bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5]]
        })?;

        Some(NeighborAdvertisement {
            src_ip: ipv6.src_addr().octets().try_into().ok()?,
            target_ip: target_addr.octets(),
            target_mac,
            router: flags.contains(NdiscNeighborFlags::ROUTER),
            solicited: flags.contains(NdiscNeighborFlags::SOLICITED),
        })
    } else {
        None
    }
}

/// Build a Router Solicitation packet
pub fn router_solicitation(src_mac: [u8; 6], src_ip: [u8; 16]) -> Vec<u8> {
    let src_addr = smoltcp::wire::Ipv6Address::from_octets(src_ip);
    let dst_addr = smoltcp::wire::Ipv6Address::new(0xff02, 0, 0, 0, 0, 0, 0, 2); // all-routers

    let lladdr = RawHardwareAddress::from_bytes(&src_mac);
    let icmp_repr = Icmpv6Repr::Ndisc(NdiscRepr::RouterSolicit {
        lladdr: Some(lladdr),
    });

    let ipv6_repr = Ipv6Repr {
        src_addr,
        dst_addr,
        next_header: IpProtocol::Icmpv6,
        payload_len: icmp_repr.buffer_len(),
        hop_limit: 255,
    };

    // All-routers multicast MAC: 33:33:00:00:00:02
    let dst_mac = [0x33, 0x33, 0x00, 0x00, 0x00, 0x02];

    let eth_repr = EthernetRepr {
        src_addr: EthernetAddress::from_bytes(&src_mac),
        dst_addr: EthernetAddress::from_bytes(&dst_mac),
        ethertype: EthernetProtocol::Ipv6,
    };

    let total_len = eth_repr.buffer_len() + ipv6_repr.buffer_len() + icmp_repr.buffer_len();
    let mut buffer = vec![0u8; total_len];

    let mut frame = EthernetFrame::new_unchecked(&mut buffer);
    eth_repr.emit(&mut frame);

    let mut ipv6_packet = Ipv6Packet::new_unchecked(frame.payload_mut());
    ipv6_repr.emit(&mut ipv6_packet);

    let mut icmp_packet = Icmpv6Packet::new_unchecked(ipv6_packet.payload_mut());
    icmp_repr.emit(
        &src_addr,
        &dst_addr,
        &mut icmp_packet,
        &smoltcp::phy::ChecksumCapabilities::default(),
    );

    buffer
}

/// Parsed Router Advertisement
#[derive(Debug, Clone)]
pub struct RouterAdvertisement {
    pub src_ip: [u8; 16],
    pub src_mac: [u8; 6],
    pub hop_limit: u8,
    pub managed: bool,
    pub other_config: bool,
    pub router_lifetime: u16,
    pub prefix: Option<([u8; 16], u8)>,
    pub mtu: Option<u32>,
}

/// Parse a Router Advertisement from an Ethernet frame
pub fn parse_router_advertisement(frame: &[u8]) -> Option<RouterAdvertisement> {
    let eth = EthernetFrame::new_checked(frame).ok()?;
    if eth.ethertype() != EthernetProtocol::Ipv6 {
        return None;
    }

    let ipv6 = Ipv6Packet::new_checked(eth.payload()).ok()?;
    if ipv6.next_header() != IpProtocol::Icmpv6 {
        return None;
    }

    let icmp = Icmpv6Packet::new_checked(ipv6.payload()).ok()?;
    if icmp.msg_type() != Icmpv6Message::RouterAdvert {
        return None;
    }

    let repr = Icmpv6Repr::parse(
        &ipv6.src_addr(),
        &ipv6.dst_addr(),
        &icmp,
        &smoltcp::phy::ChecksumCapabilities::default(),
    )
    .ok()?;

    if let Icmpv6Repr::Ndisc(NdiscRepr::RouterAdvert {
        hop_limit,
        flags,
        router_lifetime,
        lladdr,
        mtu,
        prefix_info,
        ..
    }) = repr
    {
        let src_mac = lladdr.map(|l| {
            let bytes = l.as_bytes();
            [bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5]]
        })?;

        let prefix = prefix_info.map(|p| (p.prefix.octets(), p.prefix_len));

        Some(RouterAdvertisement {
            src_ip: ipv6.src_addr().octets().try_into().ok()?,
            src_mac,
            hop_limit,
            managed: flags.contains(NdiscRouterFlags::MANAGED),
            other_config: flags.contains(NdiscRouterFlags::OTHER),
            router_lifetime: (router_lifetime.total_millis() / 1000) as u16,
            prefix,
            mtu,
        })
    } else {
        None
    }
}

// ============================================================================
// DHCPv6 Packets
// ============================================================================

use dhcproto::v6::{
    DhcpOption as Dhcpv6Option, IANA, Message as Dhcpv6Message, MessageType as Dhcpv6MessageType,
    OptionCode as Dhcpv6OptionCode,
};

/// DHCPv6 ports
const DHCPV6_CLIENT_PORT: u16 = 546;
const DHCPV6_SERVER_PORT: u16 = 547;

/// All DHCPv6 servers and relay agents multicast address
const DHCPV6_ALL_SERVERS: [u8; 16] = [0xff, 0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 2];

/// Build a DHCPv6 SOLICIT packet
pub fn dhcpv6_solicit(client_mac: [u8; 6], src_ip: [u8; 16], xid: [u8; 3], iaid: u32) -> Vec<u8> {
    // Build client DUID (DUID-LL: type 3, hardware type 1)
    let mut client_duid = vec![0, 3, 0, 1];
    client_duid.extend_from_slice(&client_mac);

    let mut msg = Dhcpv6Message::new(Dhcpv6MessageType::Solicit);
    msg.set_xid(xid);
    msg.opts_mut().insert(Dhcpv6Option::ClientId(client_duid));

    // Request IA_NA (non-temporary address)
    let iana = IANA {
        id: iaid,
        t1: 0,
        t2: 0,
        opts: Default::default(),
    };
    msg.opts_mut().insert(Dhcpv6Option::IANA(iana));

    let dhcp_data = msg.to_vec().expect("DHCPv6 encode failed");
    wrap_dhcpv6_in_udp_ip_eth(dhcp_data, client_mac, src_ip)
}

/// Build a DHCPv6 REQUEST packet
pub fn dhcpv6_request(
    client_mac: [u8; 6],
    src_ip: [u8; 16],
    xid: [u8; 3],
    server_duid: Vec<u8>,
    iaid: u32,
) -> Vec<u8> {
    // Build client DUID
    let mut client_duid = vec![0, 3, 0, 1];
    client_duid.extend_from_slice(&client_mac);

    let mut msg = Dhcpv6Message::new(Dhcpv6MessageType::Request);
    msg.set_xid(xid);
    msg.opts_mut().insert(Dhcpv6Option::ClientId(client_duid));
    msg.opts_mut().insert(Dhcpv6Option::ServerId(server_duid));

    // Request IA_NA
    let iana = IANA {
        id: iaid,
        t1: 0,
        t2: 0,
        opts: Default::default(),
    };
    msg.opts_mut().insert(Dhcpv6Option::IANA(iana));

    let dhcp_data = msg.to_vec().expect("DHCPv6 encode failed");
    wrap_dhcpv6_in_udp_ip_eth(dhcp_data, client_mac, src_ip)
}

/// Wrap DHCPv6 message in UDP/IPv6/Ethernet
fn wrap_dhcpv6_in_udp_ip_eth(dhcp_data: Vec<u8>, client_mac: [u8; 6], src_ip: [u8; 16]) -> Vec<u8> {
    let src_addr = smoltcp::wire::Ipv6Address::from_octets(src_ip);
    let dst_addr = smoltcp::wire::Ipv6Address::from_octets(DHCPV6_ALL_SERVERS);

    let udp_repr = UdpRepr {
        src_port: DHCPV6_CLIENT_PORT,
        dst_port: DHCPV6_SERVER_PORT,
    };

    let ipv6_repr = Ipv6Repr {
        src_addr,
        dst_addr,
        next_header: IpProtocol::Udp,
        payload_len: udp_repr.header_len() + dhcp_data.len(),
        hop_limit: 64,
    };

    // All DHCPv6 servers multicast MAC: 33:33:00:01:00:02
    let dst_mac = [0x33, 0x33, 0x00, 0x01, 0x00, 0x02];

    let eth_repr = EthernetRepr {
        src_addr: EthernetAddress::from_bytes(&client_mac),
        dst_addr: EthernetAddress::from_bytes(&dst_mac),
        ethertype: EthernetProtocol::Ipv6,
    };

    let total_len =
        eth_repr.buffer_len() + ipv6_repr.buffer_len() + udp_repr.header_len() + dhcp_data.len();
    let mut buffer = vec![0u8; total_len];

    let mut frame = EthernetFrame::new_unchecked(&mut buffer);
    eth_repr.emit(&mut frame);

    let mut ipv6_packet = Ipv6Packet::new_unchecked(frame.payload_mut());
    ipv6_repr.emit(&mut ipv6_packet);

    let mut udp_packet = UdpPacket::new_unchecked(ipv6_packet.payload_mut());
    udp_repr.emit(
        &mut udp_packet,
        &IpAddress::Ipv6(src_addr),
        &IpAddress::Ipv6(dst_addr),
        dhcp_data.len(),
        |buf| buf.copy_from_slice(&dhcp_data),
        &smoltcp::phy::ChecksumCapabilities::default(),
    );

    buffer
}

/// DHCPv6 message types for test assertions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dhcpv6MsgType {
    Solicit,
    Advertise,
    Request,
    Reply,
    Other(u8),
}

impl From<Dhcpv6MessageType> for Dhcpv6MsgType {
    fn from(t: Dhcpv6MessageType) -> Self {
        match t {
            Dhcpv6MessageType::Solicit => Dhcpv6MsgType::Solicit,
            Dhcpv6MessageType::Advertise => Dhcpv6MsgType::Advertise,
            Dhcpv6MessageType::Request => Dhcpv6MsgType::Request,
            Dhcpv6MessageType::Reply => Dhcpv6MsgType::Reply,
            _ => Dhcpv6MsgType::Other(0),
        }
    }
}

/// Parsed DHCPv6 response
#[derive(Debug, Clone)]
pub struct Dhcpv6Response {
    pub message_type: Dhcpv6MsgType,
    pub xid: [u8; 3],
    pub server_duid: Vec<u8>,
    pub iaid: Option<u32>,
    pub assigned_ip: Option<[u8; 16]>,
    pub preferred_lifetime: Option<u32>,
    pub valid_lifetime: Option<u32>,
    pub dns_servers: Vec<[u8; 16]>,
}

/// Parse a DHCPv6 response from an Ethernet frame
pub fn parse_dhcpv6_response(frame: &[u8]) -> Option<Dhcpv6Response> {
    let eth = EthernetFrame::new_checked(frame).ok()?;
    if eth.ethertype() != EthernetProtocol::Ipv6 {
        return None;
    }

    let ipv6 = Ipv6Packet::new_checked(eth.payload()).ok()?;
    if ipv6.next_header() != IpProtocol::Udp {
        return None;
    }

    let udp = UdpPacket::new_checked(ipv6.payload()).ok()?;
    if udp.src_port() != DHCPV6_SERVER_PORT || udp.dst_port() != DHCPV6_CLIENT_PORT {
        return None;
    }

    let mut decoder = dhcproto::decoder::Decoder::new(udp.payload());
    let msg = Dhcpv6Message::decode(&mut decoder).ok()?;

    let server_duid = msg
        .opts()
        .get(Dhcpv6OptionCode::ServerId)
        .and_then(|opt| {
            if let Dhcpv6Option::ServerId(duid) = opt {
                Some(duid.clone())
            } else {
                None
            }
        })
        .unwrap_or_default();

    let (iaid, assigned_ip, preferred_lifetime, valid_lifetime) = msg
        .opts()
        .get(Dhcpv6OptionCode::IANA)
        .and_then(|opt| {
            if let Dhcpv6Option::IANA(iana) = opt {
                let iaid = Some(iana.id);
                let addr_info = iana
                    .opts
                    .get(Dhcpv6OptionCode::IAAddr)
                    .and_then(|addr_opt| {
                        if let Dhcpv6Option::IAAddr(ia_addr) = addr_opt {
                            Some((
                                Some(ia_addr.addr.octets()),
                                Some(ia_addr.preferred_life),
                                Some(ia_addr.valid_life),
                            ))
                        } else {
                            None
                        }
                    });
                match addr_info {
                    Some((ip, pref, valid)) => Some((iaid, ip, pref, valid)),
                    None => Some((iaid, None, None, None)),
                }
            } else {
                None
            }
        })
        .unwrap_or((None, None, None, None));

    let dns_servers = msg
        .opts()
        .get(Dhcpv6OptionCode::DomainNameServers)
        .map(|opt| {
            if let Dhcpv6Option::DomainNameServers(servers) = opt {
                servers.iter().map(|ip| ip.octets()).collect()
            } else {
                Vec::new()
            }
        })
        .unwrap_or_default();

    Some(Dhcpv6Response {
        message_type: Dhcpv6MsgType::from(msg.msg_type()),
        xid: msg.xid(),
        server_duid,
        iaid,
        assigned_ip,
        preferred_lifetime,
        valid_lifetime,
        dns_servers,
    })
}

// ============================================================================
// ICMPv6 Echo (Ping) Packets
// ============================================================================

/// Build an ICMPv6 echo request (ping) packet
pub fn icmpv6_echo_request(
    src_mac: [u8; 6],
    dst_mac: [u8; 6],
    src_ip: [u8; 16],
    dst_ip: [u8; 16],
    ident: u16,
    seq_no: u16,
    data: &[u8],
) -> Vec<u8> {
    use smoltcp::wire::Icmpv6Repr;

    let src_addr = smoltcp::wire::Ipv6Address::from_octets(src_ip);
    let dst_addr = smoltcp::wire::Ipv6Address::from_octets(dst_ip);

    let icmp_repr = Icmpv6Repr::EchoRequest {
        ident,
        seq_no,
        data,
    };

    let ipv6_repr = Ipv6Repr {
        src_addr,
        dst_addr,
        next_header: IpProtocol::Icmpv6,
        payload_len: icmp_repr.buffer_len(),
        hop_limit: 64,
    };

    let eth_repr = EthernetRepr {
        src_addr: EthernetAddress::from_bytes(&src_mac),
        dst_addr: EthernetAddress::from_bytes(&dst_mac),
        ethertype: EthernetProtocol::Ipv6,
    };

    let total_len = eth_repr.buffer_len() + ipv6_repr.buffer_len() + icmp_repr.buffer_len();
    let mut buffer = vec![0u8; total_len];

    let mut frame = EthernetFrame::new_unchecked(&mut buffer);
    eth_repr.emit(&mut frame);

    let mut ipv6_packet = Ipv6Packet::new_unchecked(frame.payload_mut());
    ipv6_repr.emit(&mut ipv6_packet);

    let mut icmp_packet = Icmpv6Packet::new_unchecked(ipv6_packet.payload_mut());
    icmp_repr.emit(
        &src_addr,
        &dst_addr,
        &mut icmp_packet,
        &smoltcp::phy::ChecksumCapabilities::default(),
    );

    buffer
}

/// Parsed ICMPv6 echo reply
#[derive(Debug, Clone)]
pub struct Icmpv6EchoReply {
    pub src_ip: [u8; 16],
    pub dst_ip: [u8; 16],
    pub ident: u16,
    pub seq_no: u16,
    pub data: Vec<u8>,
}

/// Parse an ICMPv6 echo reply from an Ethernet frame
pub fn parse_icmpv6_echo_reply(frame: &[u8]) -> Option<Icmpv6EchoReply> {
    let eth = EthernetFrame::new_checked(frame).ok()?;
    if eth.ethertype() != EthernetProtocol::Ipv6 {
        return None;
    }

    let ipv6 = Ipv6Packet::new_checked(eth.payload()).ok()?;
    if ipv6.next_header() != IpProtocol::Icmpv6 {
        return None;
    }

    let icmp = Icmpv6Packet::new_checked(ipv6.payload()).ok()?;
    if icmp.msg_type() != Icmpv6Message::EchoReply {
        return None;
    }

    let repr = Icmpv6Repr::parse(
        &ipv6.src_addr(),
        &ipv6.dst_addr(),
        &icmp,
        &smoltcp::phy::ChecksumCapabilities::default(),
    )
    .ok()?;

    if let Icmpv6Repr::EchoReply {
        ident,
        seq_no,
        data,
    } = repr
    {
        Some(Icmpv6EchoReply {
            src_ip: ipv6.src_addr().octets().try_into().ok()?,
            dst_ip: ipv6.dst_addr().octets().try_into().ok()?,
            ident,
            seq_no,
            data: data.to_vec(),
        })
    } else {
        None
    }
}

/// Parsed ICMPv6 echo request
#[derive(Debug, Clone)]
pub struct Icmpv6EchoRequest {
    pub src_mac: [u8; 6],
    pub dst_mac: [u8; 6],
    pub src_ip: [u8; 16],
    pub dst_ip: [u8; 16],
    pub hop_limit: u8,
    pub ident: u16,
    pub seq_no: u16,
    pub data: Vec<u8>,
}

/// Parse an ICMPv6 echo request from an Ethernet frame
pub fn parse_icmpv6_echo_request(frame: &[u8]) -> Option<Icmpv6EchoRequest> {
    let eth = EthernetFrame::new_checked(frame).ok()?;
    if eth.ethertype() != EthernetProtocol::Ipv6 {
        return None;
    }

    let ipv6 = Ipv6Packet::new_checked(eth.payload()).ok()?;
    if ipv6.next_header() != IpProtocol::Icmpv6 {
        return None;
    }

    let icmp = Icmpv6Packet::new_checked(ipv6.payload()).ok()?;
    if icmp.msg_type() != Icmpv6Message::EchoRequest {
        return None;
    }

    let repr = Icmpv6Repr::parse(
        &ipv6.src_addr(),
        &ipv6.dst_addr(),
        &icmp,
        &smoltcp::phy::ChecksumCapabilities::default(),
    )
    .ok()?;

    if let Icmpv6Repr::EchoRequest {
        ident,
        seq_no,
        data,
    } = repr
    {
        Some(Icmpv6EchoRequest {
            src_mac: eth.src_addr().as_bytes().try_into().ok()?,
            dst_mac: eth.dst_addr().as_bytes().try_into().ok()?,
            src_ip: ipv6.src_addr().octets().try_into().ok()?,
            dst_ip: ipv6.dst_addr().octets().try_into().ok()?,
            hop_limit: ipv6.hop_limit(),
            ident,
            seq_no,
            data: data.to_vec(),
        })
    } else {
        None
    }
}

/// Check if frame is an ICMPv6 echo request
pub fn is_icmpv6_echo_request(frame: &[u8]) -> bool {
    parse_icmpv6_echo_request(frame).is_some()
}
