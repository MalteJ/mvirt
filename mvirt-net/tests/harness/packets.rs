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
        source_protocol_addr: Ipv4Address::from_bytes(&sender_ip),
        target_hardware_addr: EthernetAddress::from_bytes(&[0, 0, 0, 0, 0, 0]),
        target_protocol_addr: Ipv4Address::from_bytes(&target_ip),
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
            sender_ip: source_protocol_addr.as_bytes().try_into().ok()?,
            target_mac: target_hardware_addr.as_bytes().try_into().ok()?,
            target_ip: target_protocol_addr.as_bytes().try_into().ok()?,
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
        src_addr: Ipv4Address::from_bytes(&src_ip),
        dst_addr: Ipv4Address::from_bytes(&dst_ip),
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
            src_ip: ipv4.src_addr().as_bytes().try_into().ok()?,
            dst_ip: ipv4.dst_addr().as_bytes().try_into().ok()?,
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
