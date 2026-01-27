//! Protocol handlers for DHCP, ARP, and NDP via AF_PACKET raw sockets.

use crate::grpc::storage::{NetworkData, NicData};
use dhcproto::v4::{
    Decodable, DhcpOption, Encodable, Message as DhcpMessage, MessageType as DhcpMessageType,
    Opcode,
};
use ipnet::Ipv4Net;
use smoltcp::phy::ChecksumCapabilities;
use smoltcp::wire::{
    ArpOperation, ArpPacket, ArpRepr, EthernetAddress, EthernetFrame, EthernetProtocol,
    EthernetRepr, Icmpv4Message, Icmpv4Packet, Icmpv4Repr, Icmpv6Message, Icmpv6Packet, Icmpv6Repr,
    IpAddress, IpProtocol, Ipv4Address, Ipv4Packet, Ipv4Repr, Ipv6Address, Ipv6Packet, Ipv6Repr,
    NdiscNeighborFlags, NdiscRepr, UdpPacket, UdpRepr,
};
use socket2::{Domain, Protocol, Socket, Type};
use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::Arc;
use thiserror::Error;
use tokio::io::unix::AsyncFd;
use tokio::sync::RwLock;
use tracing::{debug, error, info};

/// Fixed gateway MAC address (same as mvirt-net for consistency).
/// Uses locally administered, unicast format.
pub const GATEWAY_MAC: [u8; 6] = [0x02, 0x00, 0x00, 0x00, 0x00, 0x01];

/// Link-local gateway IPv4 address (like AWS/GCP VPCs).
/// VMs route all traffic via this gateway using classless static routes.
pub const GATEWAY_IPV4_LINK_LOCAL: Ipv4Addr = Ipv4Addr::new(169, 254, 0, 1);

/// Link-local gateway IPv6 address.
pub const GATEWAY_IPV6_LINK_LOCAL: Ipv6Addr = Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1);

/// Protocol handler errors.
#[derive(Debug, Error)]
pub enum ProtoError {
    #[error("Socket error: {0}")]
    Socket(#[from] io::Error),

    #[error("Parse error: {0}")]
    Parse(String),

    #[error("No configuration for interface {0}")]
    NoConfig(u32),
}

pub type Result<T> = std::result::Result<T, ProtoError>;

/// Configuration for a NIC's protocol handling.
#[derive(Debug, Clone)]
pub struct NicConfig {
    pub nic: NicData,
    pub network: NetworkData,
}

/// Protocol handler that processes DHCP/ARP/NDP on raw sockets.
pub struct ProtocolHandler {
    /// Configurations indexed by TAP interface index
    configs: Arc<RwLock<HashMap<u32, NicConfig>>>,
}

impl ProtocolHandler {
    pub fn new() -> Self {
        Self {
            configs: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register a NIC for protocol handling.
    pub async fn register_nic(&self, if_index: u32, nic: NicData, network: NetworkData) {
        let mut configs = self.configs.write().await;
        configs.insert(if_index, NicConfig { nic, network });
        info!(if_index, "NIC registered for protocol handling");
    }

    /// Unregister a NIC.
    pub async fn unregister_nic(&self, if_index: u32) {
        let mut configs = self.configs.write().await;
        configs.remove(&if_index);
        info!(if_index, "NIC unregistered from protocol handling");
    }

    /// Spawn a handler task for a TAP interface.
    ///
    /// Note: This uses AF_PACKET sockets which work for bridged interfaces
    /// but not for direct TAP access. Use `spawn_handler_with_fd` for TAP devices.
    pub fn spawn_handler(&self, tap_name: String, if_index: u32) -> tokio::task::JoinHandle<()> {
        let configs = Arc::clone(&self.configs);

        tokio::spawn(async move {
            if let Err(e) = run_handler_af_packet(tap_name.clone(), if_index, configs).await {
                error!(tap_name, if_index, error = %e, "Protocol handler failed");
            }
        })
    }

    /// Spawn a handler task that reads directly from a TAP file descriptor.
    ///
    /// This is the preferred method for testing or when you have direct access
    /// to the TAP device fd.
    pub fn spawn_handler_with_fd(
        &self,
        tap_name: String,
        if_index: u32,
        tap_fd: OwnedFd,
    ) -> tokio::task::JoinHandle<()> {
        let configs = Arc::clone(&self.configs);

        tokio::spawn(async move {
            if let Err(e) = run_handler_tap_fd(tap_name.clone(), if_index, tap_fd, configs).await {
                error!(tap_name, if_index, error = %e, "Protocol handler failed");
            }
        })
    }
}

impl Default for ProtocolHandler {
    fn default() -> Self {
        Self::new()
    }
}

/// Synchronously process a packet and return the response (if any).
///
/// This function is useful for testing or for integration with existing
/// packet processing pipelines where you have your own read/write loop.
pub fn process_packet_sync(nic: &NicData, network: &NetworkData, packet: &[u8]) -> Option<Vec<u8>> {
    let config = NicConfig {
        nic: nic.clone(),
        network: network.clone(),
    };
    process_packet(&config, packet)
}

/// Run the protocol handler using a TAP file descriptor directly.
///
/// This reads packets from the TAP fd (VM → host direction) and writes
/// responses back to the TAP fd (host → VM direction).
async fn run_handler_tap_fd(
    tap_name: String,
    if_index: u32,
    tap_fd: OwnedFd,
    configs: Arc<RwLock<HashMap<u32, NicConfig>>>,
) -> Result<()> {
    // Set non-blocking
    let flags = unsafe { libc::fcntl(tap_fd.as_raw_fd(), libc::F_GETFL) };
    if flags < 0 {
        return Err(ProtoError::Socket(io::Error::last_os_error()));
    }
    let ret = unsafe { libc::fcntl(tap_fd.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) };
    if ret < 0 {
        return Err(ProtoError::Socket(io::Error::last_os_error()));
    }

    let async_fd = AsyncFd::new(tap_fd)?;
    let mut buf = vec![0u8; 2048];

    info!(tap_name, if_index, "Protocol handler started (TAP fd mode)");

    loop {
        // Wait for readable
        let mut guard = async_fd.readable().await?;

        // Read packet from TAP
        let n = match guard.try_io(|fd| -> io::Result<usize> {
            let ret = unsafe {
                libc::read(
                    fd.as_raw_fd(),
                    buf.as_mut_ptr() as *mut libc::c_void,
                    buf.len(),
                )
            };
            if ret < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(ret as usize)
            }
        }) {
            Ok(Ok(n)) => n,
            Ok(Err(e)) if e.kind() == io::ErrorKind::WouldBlock => continue,
            Ok(Err(e)) => return Err(ProtoError::Socket(e)),
            Err(_) => continue, // Would block
        };

        if n == 0 {
            continue;
        }

        let packet = &buf[..n];
        debug!(n, "Received packet from TAP fd");

        // Get config for this interface
        let configs_guard = configs.read().await;
        let config = match configs_guard.get(&if_index) {
            Some(c) => c.clone(),
            None => continue,
        };
        drop(configs_guard);

        // Process packet
        if let Some(response) = process_packet(&config, packet) {
            debug!(len = response.len(), "Sending response to TAP fd");
            // Write response back to TAP
            let mut write_guard = async_fd.writable().await?;
            let _ = write_guard.try_io(|fd| -> io::Result<usize> {
                let ret = unsafe {
                    libc::write(
                        fd.as_raw_fd(),
                        response.as_ptr() as *const libc::c_void,
                        response.len(),
                    )
                };
                if ret < 0 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(ret as usize)
                }
            });
        }
    }
}

/// Run the protocol handler for a TAP interface using AF_PACKET.
///
/// Note: This works for bridged interfaces where packets traverse the interface,
/// but NOT for direct TAP access where packets are written to the TAP fd.
async fn run_handler_af_packet(
    tap_name: String,
    if_index: u32,
    configs: Arc<RwLock<HashMap<u32, NicConfig>>>,
) -> Result<()> {
    // Create raw socket bound to the TAP interface
    let socket = Socket::new(
        Domain::PACKET,
        Type::RAW,
        Some(Protocol::from(libc::ETH_P_ALL)),
    )?;
    socket.set_nonblocking(true)?;

    // Bind to interface using sockaddr_ll
    let mut addr: libc::sockaddr_ll = unsafe { std::mem::zeroed() };
    addr.sll_family = libc::AF_PACKET as u16;
    addr.sll_protocol = (libc::ETH_P_ALL as u16).to_be();
    addr.sll_ifindex = if_index as i32;

    let ret = unsafe {
        libc::bind(
            socket.as_raw_fd(),
            &addr as *const libc::sockaddr_ll as *const libc::sockaddr,
            std::mem::size_of::<libc::sockaddr_ll>() as libc::socklen_t,
        )
    };
    if ret < 0 {
        return Err(ProtoError::Socket(io::Error::last_os_error()));
    }
    debug!(tap_name, if_index, "Bound AF_PACKET socket to interface");

    // Extract raw fd from socket
    let raw_fd = socket.as_raw_fd();
    std::mem::forget(socket); // Prevent socket from closing the fd
    let fd = unsafe { OwnedFd::from_raw_fd(raw_fd) };
    let async_fd = AsyncFd::new(fd)?;

    let mut buf = vec![0u8; 2048];

    info!(tap_name, if_index, "Protocol handler started");

    debug!("Entering packet receive loop");
    loop {
        // Wait for readable
        debug!("Waiting for socket to be readable...");
        let mut guard = async_fd.readable().await?;
        debug!("Socket is readable, trying to receive");

        // Read packet
        let n = match guard.try_io(|fd| -> io::Result<usize> {
            let ret = unsafe {
                libc::recv(
                    fd.as_raw_fd(),
                    buf.as_mut_ptr() as *mut libc::c_void,
                    buf.len(),
                    0,
                )
            };
            debug!(ret, "recv() returned");
            if ret < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(ret as usize)
            }
        }) {
            Ok(Ok(n)) => n,
            Ok(Err(e)) if e.kind() == io::ErrorKind::WouldBlock => {
                debug!("recv() would block");
                continue;
            }
            Ok(Err(e)) => return Err(ProtoError::Socket(e)),
            Err(_) => {
                debug!("try_io returned Err (would block)");
                continue; // Would block
            }
        };

        if n == 0 {
            debug!("recv() returned 0 bytes");
            continue;
        }

        let packet = &buf[..n];
        debug!(n, "Received packet on AF_PACKET socket");

        // Get config for this interface
        let configs_guard = configs.read().await;
        let config = match configs_guard.get(&if_index) {
            Some(c) => c.clone(),
            None => continue,
        };
        drop(configs_guard);

        // Process packet
        if let Some(response) = process_packet(&config, packet) {
            // Send response
            let mut write_guard = async_fd.writable().await?;
            let _ = write_guard.try_io(|fd| -> io::Result<usize> {
                let ret = unsafe {
                    libc::send(
                        fd.as_raw_fd(),
                        response.as_ptr() as *const libc::c_void,
                        response.len(),
                        0,
                    )
                };
                if ret < 0 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(ret as usize)
                }
            });
        }
    }
}

/// Process an incoming packet and return an optional response.
fn process_packet(config: &NicConfig, packet: &[u8]) -> Option<Vec<u8>> {
    let eth_frame = EthernetFrame::new_checked(packet).ok()?;

    debug!(ethertype = ?eth_frame.ethertype(), "Processing packet");

    match eth_frame.ethertype() {
        EthernetProtocol::Arp => process_arp(config, &eth_frame),
        EthernetProtocol::Ipv4 => process_ipv4(config, &eth_frame),
        EthernetProtocol::Ipv6 => process_ipv6(config, &eth_frame),
        _ => None,
    }
}

/// Process ARP request.
fn process_arp(_config: &NicConfig, eth_frame: &EthernetFrame<&[u8]>) -> Option<Vec<u8>> {
    let arp_packet = ArpPacket::new_checked(eth_frame.payload()).ok()?;
    let arp_repr = ArpRepr::parse(&arp_packet).ok()?;

    // Only handle IPv4 ARP requests
    if let ArpRepr::EthernetIpv4 {
        operation: ArpOperation::Request,
        source_hardware_addr,
        source_protocol_addr,
        target_protocol_addr,
        ..
    } = arp_repr
    {
        // Check if they're asking for the link-local gateway (169.254.0.1)
        let gateway_addr = Ipv4Address::from_bytes(&GATEWAY_IPV4_LINK_LOCAL.octets());
        if target_protocol_addr != gateway_addr {
            return None;
        }

        // Build ARP reply with fixed gateway MAC
        let arp_reply = ArpRepr::EthernetIpv4 {
            operation: ArpOperation::Reply,
            source_hardware_addr: EthernetAddress(GATEWAY_MAC),
            source_protocol_addr: target_protocol_addr,
            target_hardware_addr: source_hardware_addr,
            target_protocol_addr: source_protocol_addr,
        };

        let eth_reply = EthernetRepr {
            src_addr: EthernetAddress(GATEWAY_MAC),
            dst_addr: source_hardware_addr,
            ethertype: EthernetProtocol::Arp,
        };

        // Serialize
        let total_len = eth_reply.buffer_len() + arp_reply.buffer_len();
        let mut buf = vec![0u8; total_len];
        let mut eth_frame = EthernetFrame::new_unchecked(&mut buf);
        eth_reply.emit(&mut eth_frame);
        let mut arp_packet = ArpPacket::new_unchecked(eth_frame.payload_mut());
        arp_reply.emit(&mut arp_packet);

        debug!(
            gateway = %GATEWAY_IPV4_LINK_LOCAL,
            requester = %source_protocol_addr,
            "ARP reply sent"
        );
        return Some(buf);
    }

    None
}

/// Process IPv4 packet (looking for DHCP and ICMP).
fn process_ipv4(config: &NicConfig, eth_frame: &EthernetFrame<&[u8]>) -> Option<Vec<u8>> {
    let ipv4_packet = Ipv4Packet::new_checked(eth_frame.payload()).ok()?;

    match ipv4_packet.next_header() {
        IpProtocol::Udp => {
            let udp_packet = UdpPacket::new_checked(ipv4_packet.payload()).ok()?;
            let src_port = udp_packet.src_port();
            let dst_port = udp_packet.dst_port();

            // DHCP: client port 68, server port 67
            if src_port == 68 && dst_port == 67 {
                return process_dhcp(config, eth_frame, udp_packet.payload());
            }
        }
        IpProtocol::Icmp => {
            // ICMP echo request handling
            return process_icmp(eth_frame, &ipv4_packet);
        }
        _ => {}
    }

    None
}

/// Process ICMP echo requests to the gateway.
fn process_icmp(
    eth_frame: &EthernetFrame<&[u8]>,
    ipv4_packet: &Ipv4Packet<&[u8]>,
) -> Option<Vec<u8>> {
    let icmp_packet = Icmpv4Packet::new_checked(ipv4_packet.payload()).ok()?;

    // Only respond to echo requests
    if icmp_packet.msg_type() != Icmpv4Message::EchoRequest {
        return None;
    }

    // Only respond if destined for the gateway
    let dst_ip = ipv4_packet.dst_addr();
    let gateway_addr = Ipv4Address::from_bytes(&GATEWAY_IPV4_LINK_LOCAL.octets());
    if dst_ip != gateway_addr {
        return None;
    }

    // Build echo reply
    let icmp_repr = Icmpv4Repr::EchoReply {
        ident: icmp_packet.echo_ident(),
        seq_no: icmp_packet.echo_seq_no(),
        data: icmp_packet.data(),
    };

    // Build IP header (swap src/dst)
    let ipv4_repr = Ipv4Repr {
        src_addr: gateway_addr,
        dst_addr: ipv4_packet.src_addr(),
        next_header: IpProtocol::Icmp,
        payload_len: icmp_repr.buffer_len(),
        hop_limit: 64,
    };

    // Build Ethernet header
    let eth_repr = EthernetRepr {
        src_addr: EthernetAddress(GATEWAY_MAC),
        dst_addr: eth_frame.src_addr(),
        ethertype: EthernetProtocol::Ipv4,
    };

    // Serialize
    let total_len = eth_repr.buffer_len() + ipv4_repr.buffer_len() + icmp_repr.buffer_len();
    let mut buffer = vec![0u8; total_len];

    let mut eth_out = EthernetFrame::new_unchecked(&mut buffer);
    eth_repr.emit(&mut eth_out);

    let mut ipv4_out = Ipv4Packet::new_unchecked(eth_out.payload_mut());
    ipv4_repr.emit(&mut ipv4_out, &ChecksumCapabilities::default());

    let mut icmp_out = Icmpv4Packet::new_unchecked(ipv4_out.payload_mut());
    icmp_repr.emit(&mut icmp_out, &ChecksumCapabilities::default());

    debug!(
        src = %ipv4_packet.src_addr(),
        "ICMP echo reply to gateway"
    );

    Some(buffer)
}

/// Process DHCP message.
fn process_dhcp(
    config: &NicConfig,
    _eth_frame: &EthernetFrame<&[u8]>,
    payload: &[u8],
) -> Option<Vec<u8>> {
    let dhcp_msg = DhcpMessage::decode(&mut dhcproto::decoder::Decoder::new(payload)).ok()?;

    // Only handle requests from client
    if dhcp_msg.opcode() != Opcode::BootRequest {
        return None;
    }

    let msg_type = dhcp_msg.opts().get(dhcproto::v4::OptionCode::MessageType)?;
    let msg_type = match msg_type {
        DhcpOption::MessageType(t) => *t,
        _ => return None,
    };

    let client_mac = dhcp_msg.chaddr();

    // Verify this is from our NIC's MAC
    if client_mac[..6] != config.nic.mac_address {
        return None;
    }

    let assigned_ip = config.nic.ipv4_address?;
    let subnet = config.network.ipv4_subnet?;
    let subnet_mask = Ipv4Addr::from(
        0xFFFFFFFFu32
            .checked_shl(32 - subnet.prefix_len() as u32)
            .unwrap_or(0),
    );

    let response_type = match msg_type {
        DhcpMessageType::Discover => DhcpMessageType::Offer,
        DhcpMessageType::Request => DhcpMessageType::Ack,
        _ => return None,
    };

    // Build DHCP response
    // Use link-local gateway (169.254.0.1) like AWS/GCP VPCs
    let mut reply = DhcpMessage::default();
    reply.set_opcode(Opcode::BootReply);
    reply.set_xid(dhcp_msg.xid());
    reply.set_yiaddr(assigned_ip);
    reply.set_siaddr(GATEWAY_IPV4_LINK_LOCAL);
    reply.set_chaddr(client_mac);
    reply.set_flags(dhcp_msg.flags());

    reply
        .opts_mut()
        .insert(DhcpOption::MessageType(response_type));
    reply
        .opts_mut()
        .insert(DhcpOption::ServerIdentifier(GATEWAY_IPV4_LINK_LOCAL));
    reply.opts_mut().insert(DhcpOption::SubnetMask(subnet_mask));
    reply
        .opts_mut()
        .insert(DhcpOption::Router(vec![GATEWAY_IPV4_LINK_LOCAL]));
    reply.opts_mut().insert(DhcpOption::AddressLeaseTime(86400)); // 24 hours

    // Classless Static Routes (Option 121) - RFC 3442
    // This is required for the link-local gateway to work properly:
    // 1. Gateway route: 169.254.0.1/32 via on-link (0.0.0.0)
    // 2. Default route: 0.0.0.0/0 via 169.254.0.1
    let gateway_net = Ipv4Net::new(GATEWAY_IPV4_LINK_LOCAL, 32).unwrap();
    let default_net = Ipv4Net::new(Ipv4Addr::UNSPECIFIED, 0).unwrap();
    let routes = vec![
        (gateway_net, Ipv4Addr::UNSPECIFIED),   // 169.254.0.1/32 on-link
        (default_net, GATEWAY_IPV4_LINK_LOCAL), // 0.0.0.0/0 via gateway
    ];
    reply
        .opts_mut()
        .insert(DhcpOption::ClasslessStaticRoute(routes));

    // Add DNS servers
    let dns_v4: Vec<Ipv4Addr> = config
        .network
        .dns_servers
        .iter()
        .filter_map(|ip| match ip {
            IpAddr::V4(v4) => Some(*v4),
            _ => None,
        })
        .collect();
    if !dns_v4.is_empty() {
        reply
            .opts_mut()
            .insert(DhcpOption::DomainNameServer(dns_v4));
    }

    // Encode DHCP message
    let mut dhcp_buf = Vec::new();
    let mut encoder = dhcproto::encoder::Encoder::new(&mut dhcp_buf);
    reply.encode(&mut encoder).ok()?;

    // Build UDP
    let udp_repr = UdpRepr {
        src_port: 67,
        dst_port: 68,
    };

    // Destination: broadcast or unicast based on flags
    let broadcast_flag = dhcp_msg.flags().broadcast();
    let (dst_ip, dst_mac) = if broadcast_flag {
        // Broadcast flag set
        (Ipv4Address::BROADCAST, EthernetAddress::BROADCAST)
    } else {
        (
            Ipv4Address::from_bytes(&assigned_ip.octets()),
            EthernetAddress(config.nic.mac_address),
        )
    };

    let src_ip = Ipv4Address::from_bytes(&GATEWAY_IPV4_LINK_LOCAL.octets());

    let ipv4_repr = Ipv4Repr {
        src_addr: src_ip,
        dst_addr: dst_ip,
        next_header: IpProtocol::Udp,
        payload_len: udp_repr.header_len() + dhcp_buf.len(),
        hop_limit: 64,
    };

    let eth_repr = EthernetRepr {
        src_addr: EthernetAddress(GATEWAY_MAC),
        dst_addr: dst_mac,
        ethertype: EthernetProtocol::Ipv4,
    };

    // Serialize
    let total_len =
        eth_repr.buffer_len() + ipv4_repr.buffer_len() + udp_repr.header_len() + dhcp_buf.len();
    let mut buf = vec![0u8; total_len];

    let mut eth_frame = EthernetFrame::new_unchecked(&mut buf);
    eth_repr.emit(&mut eth_frame);

    let mut ipv4_packet = Ipv4Packet::new_unchecked(eth_frame.payload_mut());
    ipv4_repr.emit(&mut ipv4_packet, &ChecksumCapabilities::default());

    let mut udp_packet = UdpPacket::new_unchecked(ipv4_packet.payload_mut());
    udp_repr.emit(
        &mut udp_packet,
        &IpAddress::Ipv4(ipv4_repr.src_addr),
        &IpAddress::Ipv4(ipv4_repr.dst_addr),
        dhcp_buf.len(),
        |buf| buf.copy_from_slice(&dhcp_buf),
        &ChecksumCapabilities::default(),
    );

    info!(
        response_type = ?response_type,
        assigned_ip = %assigned_ip,
        client_mac = %EthernetAddress(config.nic.mac_address),
        "DHCP response sent"
    );

    Some(buf)
}

/// Process IPv6 packet (looking for NDP).
fn process_ipv6(config: &NicConfig, eth_frame: &EthernetFrame<&[u8]>) -> Option<Vec<u8>> {
    let ipv6_packet = Ipv6Packet::new_checked(eth_frame.payload()).ok()?;

    if ipv6_packet.next_header() != IpProtocol::Icmpv6 {
        return None;
    }

    let icmpv6_packet = Icmpv6Packet::new_checked(ipv6_packet.payload()).ok()?;

    // Handle Neighbor Solicitation
    if icmpv6_packet.msg_type() == Icmpv6Message::NeighborSolicit {
        return process_neighbor_solicitation(config, eth_frame, &ipv6_packet, &icmpv6_packet);
    }

    // Handle Router Solicitation
    if icmpv6_packet.msg_type() == Icmpv6Message::RouterSolicit {
        return process_router_solicitation(config);
    }

    None
}

/// Process NDP Neighbor Solicitation.
fn process_neighbor_solicitation(
    _config: &NicConfig,
    eth_frame: &EthernetFrame<&[u8]>,
    ipv6_packet: &Ipv6Packet<&[u8]>,
    icmpv6_packet: &Icmpv6Packet<&[u8]>,
) -> Option<Vec<u8>> {
    let ndisc_repr = NdiscRepr::parse(icmpv6_packet).ok()?;

    if let NdiscRepr::NeighborSolicit {
        target_addr,
        lladdr: _,
    } = ndisc_repr
    {
        // Check if they're asking for the link-local gateway (fe80::1)
        let gateway_addr = Ipv6Address::from_bytes(&GATEWAY_IPV6_LINK_LOCAL.octets());
        if target_addr != gateway_addr {
            return None;
        }

        let src_addr = ipv6_packet.src_addr();

        // Build Neighbor Advertisement with fixed gateway MAC
        let ndisc_reply = NdiscRepr::NeighborAdvert {
            flags: NdiscNeighborFlags::ROUTER
                | NdiscNeighborFlags::SOLICITED
                | NdiscNeighborFlags::OVERRIDE,
            target_addr,
            lladdr: Some(EthernetAddress(GATEWAY_MAC).into()),
        };

        let icmpv6_repr = Icmpv6Repr::Ndisc(ndisc_reply);

        let ipv6_repr = Ipv6Repr {
            src_addr: target_addr,
            dst_addr: src_addr,
            next_header: IpProtocol::Icmpv6,
            payload_len: icmpv6_repr.buffer_len(),
            hop_limit: 255,
        };

        let eth_repr = EthernetRepr {
            src_addr: EthernetAddress(GATEWAY_MAC),
            dst_addr: eth_frame.src_addr(),
            ethertype: EthernetProtocol::Ipv6,
        };

        // Serialize
        let total_len = eth_repr.buffer_len() + ipv6_repr.buffer_len() + icmpv6_repr.buffer_len();
        let mut buf = vec![0u8; total_len];

        let mut eth_out = EthernetFrame::new_unchecked(&mut buf);
        eth_repr.emit(&mut eth_out);

        let mut ipv6_out = Ipv6Packet::new_unchecked(eth_out.payload_mut());
        ipv6_repr.emit(&mut ipv6_out);

        let mut icmpv6_out = Icmpv6Packet::new_unchecked(ipv6_out.payload_mut());
        icmpv6_repr.emit(
            &IpAddress::Ipv6(ipv6_repr.src_addr),
            &IpAddress::Ipv6(ipv6_repr.dst_addr),
            &mut icmpv6_out,
            &ChecksumCapabilities::default(),
        );

        debug!(
            gateway = %GATEWAY_IPV6_LINK_LOCAL,
            requester = %src_addr,
            "NDP Neighbor Advertisement sent"
        );
        return Some(buf);
    }

    None
}

/// Process Router Solicitation (simplified - just sends RA with prefix info).
fn process_router_solicitation(config: &NicConfig) -> Option<Vec<u8>> {
    let prefix = config.network.ipv6_prefix?;

    // For now, we don't send full RA - the VM should get IPv6 via SLAAC
    // based on the prefix. This is a simplified implementation.
    debug!(
        prefix = %prefix,
        "Router Solicitation received (RA not fully implemented)"
    );

    None
}
