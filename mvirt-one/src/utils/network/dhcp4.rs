//! DHCPv4 client implementation.
//! Ported from pideisn.

use super::{Dhcp4Lease, Interface, NetlinkHandle};
use crate::error::NetworkError;
use dhcproto::v4::{DhcpOption, Flags, Message, MessageType, Opcode, OptionCode};
use dhcproto::{Decodable, Encodable};
use log::{debug, info};
use socket2::{Domain, Protocol, Socket, Type};
use std::mem::MaybeUninit;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::os::unix::io::AsRawFd;
use std::time::{Duration, Instant};
use tokio::time::timeout;

const DHCP_SERVER_PORT: u16 = 67;
const DHCP_CLIENT_PORT: u16 = 68;

/// Configure an interface using DHCPv4.
pub async fn configure(iface: &Interface, nl: &NetlinkHandle) -> Result<Dhcp4Lease, NetworkError> {
    let mut client = Dhcp4Client::new(iface)?;
    let lease = client.run().await?;

    // Calculate prefix length from netmask
    let prefix_len = netmask_to_prefix_len(lease.netmask);

    // Configure the address
    nl.add_address_v4(iface.index, lease.address, prefix_len)
        .await?;

    // Add default route if we have a gateway
    if let Some(gw) = lease.gateway {
        // Check if gateway is on a different subnet (e.g., link-local gateway like 169.254.0.1)
        // If so, add an on-link route to the gateway first
        if !is_same_subnet(lease.address, gw, lease.netmask) {
            debug!(
                "Gateway {} not on same subnet as {}/{}, adding on-link route",
                gw, lease.address, prefix_len
            );
            nl.add_onlink_route_v4(gw, iface.index).await?;
        }
        nl.add_route_v4(gw, iface.index).await?;
    }

    Ok(lease)
}

/// Check if two addresses are on the same subnet
fn is_same_subnet(addr1: Ipv4Addr, addr2: Ipv4Addr, netmask: Ipv4Addr) -> bool {
    let mask = u32::from_be_bytes(netmask.octets());
    let a1 = u32::from_be_bytes(addr1.octets());
    let a2 = u32::from_be_bytes(addr2.octets());
    (a1 & mask) == (a2 & mask)
}

fn netmask_to_prefix_len(netmask: Ipv4Addr) -> u8 {
    let bits = u32::from_be_bytes(netmask.octets());
    bits.count_ones() as u8
}

struct Dhcp4Client {
    socket: Socket,
    mac: [u8; 6],
    xid: u32,
    iface_name: String,
}

impl Dhcp4Client {
    fn new(iface: &Interface) -> Result<Self, NetworkError> {
        let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;

        socket.set_reuse_address(true)?;
        socket.set_broadcast(true)?;

        // Bind to INADDR_ANY on client port
        let addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, DHCP_CLIENT_PORT);
        socket.bind(&addr.into())?;

        // Bind to interface using SO_BINDTODEVICE
        let fd = socket.as_raw_fd();
        let name = std::ffi::CString::new(iface.name.as_str()).unwrap();
        let ret = unsafe {
            libc::setsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_BINDTODEVICE,
                name.as_ptr() as *const libc::c_void,
                name.as_bytes_with_nul().len() as libc::socklen_t,
            )
        };
        if ret < 0 {
            return Err(NetworkError::SocketError(std::io::Error::last_os_error()));
        }

        // Generate random transaction ID
        let xid = generate_xid();

        Ok(Self {
            socket,
            mac: iface.mac,
            xid,
            iface_name: iface.name.clone(),
        })
    }

    async fn run(&mut self) -> Result<Dhcp4Lease, NetworkError> {
        const MAX_RETRIES: u32 = 4;
        let mut retry = 0;

        loop {
            // Send DISCOVER
            info!("DHCPv4: Sending DISCOVER on {}", self.iface_name);
            self.send_discover()?;

            // Wait for OFFER
            let offer = match self.wait_for_offer(Duration::from_secs(4)).await {
                Ok(offer) => offer,
                Err(NetworkError::Timeout) => {
                    retry += 1;
                    if retry >= MAX_RETRIES {
                        return Err(NetworkError::NoOffer);
                    }
                    debug!("DHCPv4: Timeout waiting for OFFER, retry {}", retry);
                    continue;
                }
                Err(e) => return Err(e),
            };

            info!("DHCPv4: Received OFFER for {}", offer.offered_ip);

            // Send REQUEST
            info!("DHCPv4: Sending REQUEST for {}", offer.offered_ip);
            self.send_request(&offer)?;

            // Wait for ACK
            match self.wait_for_ack(Duration::from_secs(4)).await {
                Ok(lease) => {
                    info!("DHCPv4: Received ACK, lease time {}s", lease.lease_time);
                    return Ok(lease);
                }
                Err(NetworkError::DhcpNak) => {
                    debug!("DHCPv4: Received NAK, restarting");
                    retry = 0;
                    continue;
                }
                Err(NetworkError::Timeout) => {
                    retry += 1;
                    if retry >= MAX_RETRIES {
                        return Err(NetworkError::NoOffer);
                    }
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
    }

    fn send_discover(&self) -> Result<(), NetworkError> {
        let mut msg = Message::default();
        msg.set_opcode(Opcode::BootRequest);
        msg.set_xid(self.xid);
        msg.set_flags(Flags::default().set_broadcast());
        msg.set_chaddr(&self.mac);

        msg.opts_mut()
            .insert(DhcpOption::MessageType(MessageType::Discover));

        // Request specific options
        msg.opts_mut().insert(DhcpOption::ParameterRequestList(vec![
            OptionCode::SubnetMask,
            OptionCode::Router,
            OptionCode::DomainNameServer,
            OptionCode::DomainName,
        ]));

        let bytes = msg
            .to_vec()
            .map_err(|e| NetworkError::InvalidPacket(e.to_string()))?;
        self.send_broadcast(&bytes)
    }

    fn send_request(&self, offer: &DhcpOffer) -> Result<(), NetworkError> {
        let mut msg = Message::default();
        msg.set_opcode(Opcode::BootRequest);
        msg.set_xid(self.xid);
        msg.set_flags(Flags::default().set_broadcast());
        msg.set_chaddr(&self.mac);

        msg.opts_mut()
            .insert(DhcpOption::MessageType(MessageType::Request));
        msg.opts_mut()
            .insert(DhcpOption::RequestedIpAddress(offer.offered_ip));
        msg.opts_mut()
            .insert(DhcpOption::ServerIdentifier(offer.server_id));

        let bytes = msg
            .to_vec()
            .map_err(|e| NetworkError::InvalidPacket(e.to_string()))?;
        self.send_broadcast(&bytes)
    }

    fn send_broadcast(&self, data: &[u8]) -> Result<(), NetworkError> {
        let dest = SocketAddrV4::new(Ipv4Addr::BROADCAST, DHCP_SERVER_PORT);
        self.socket
            .send_to(data, &dest.into())
            .map_err(NetworkError::SocketError)?;
        Ok(())
    }

    async fn wait_for_offer(&self, dur: Duration) -> Result<DhcpOffer, NetworkError> {
        let start = Instant::now();

        while start.elapsed() < dur {
            let remaining = dur.saturating_sub(start.elapsed());

            match timeout(remaining, self.recv_packet()).await {
                Ok(Ok(msg)) => {
                    if msg.xid() != self.xid {
                        continue;
                    }

                    if let Some(DhcpOption::MessageType(MessageType::Offer)) =
                        msg.opts().get(OptionCode::MessageType)
                    {
                        let server_id = match msg.opts().get(OptionCode::ServerIdentifier) {
                            Some(DhcpOption::ServerIdentifier(id)) => *id,
                            _ => continue,
                        };

                        return Ok(DhcpOffer {
                            offered_ip: msg.yiaddr(),
                            server_id,
                        });
                    }
                }
                Ok(Err(_)) => continue,
                Err(_) => return Err(NetworkError::Timeout),
            }
        }

        Err(NetworkError::Timeout)
    }

    async fn wait_for_ack(&self, dur: Duration) -> Result<Dhcp4Lease, NetworkError> {
        let start = Instant::now();

        while start.elapsed() < dur {
            let remaining = dur.saturating_sub(start.elapsed());

            match timeout(remaining, self.recv_packet()).await {
                Ok(Ok(msg)) => {
                    if msg.xid() != self.xid {
                        continue;
                    }

                    match msg.opts().get(OptionCode::MessageType) {
                        Some(DhcpOption::MessageType(MessageType::Ack)) => {
                            return Ok(self.parse_lease(&msg));
                        }
                        Some(DhcpOption::MessageType(MessageType::Nak)) => {
                            return Err(NetworkError::DhcpNak);
                        }
                        _ => continue,
                    }
                }
                Ok(Err(_)) => continue,
                Err(_) => return Err(NetworkError::Timeout),
            }
        }

        Err(NetworkError::Timeout)
    }

    async fn recv_packet(&self) -> Result<Message, NetworkError> {
        let socket_clone = self.socket.try_clone()?;
        let result = tokio::task::spawn_blocking(move || {
            let mut buf: [MaybeUninit<u8>; 1500] = unsafe { MaybeUninit::uninit().assume_init() };
            socket_clone.set_read_timeout(Some(Duration::from_millis(100)))?;
            match socket_clone.recv(&mut buf) {
                Ok(len) => {
                    let initialized: Vec<u8> = buf[..len]
                        .iter()
                        .map(|b| unsafe { b.assume_init() })
                        .collect();
                    Ok(initialized)
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Err(std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    "timeout",
                )),
                Err(e) => Err(e),
            }
        })
        .await
        .map_err(|_| NetworkError::Timeout)?
        .map_err(|_| NetworkError::Timeout)?;

        Message::from_bytes(&result).map_err(|e| NetworkError::InvalidPacket(e.to_string()))
    }

    fn parse_lease(&self, msg: &Message) -> Dhcp4Lease {
        let address = msg.yiaddr();

        let netmask = match msg.opts().get(OptionCode::SubnetMask) {
            Some(DhcpOption::SubnetMask(mask)) => *mask,
            _ => Ipv4Addr::new(255, 255, 255, 0),
        };

        let gateway = match msg.opts().get(OptionCode::Router) {
            Some(DhcpOption::Router(routers)) if !routers.is_empty() => Some(routers[0]),
            _ => None,
        };

        let dns_servers = match msg.opts().get(OptionCode::DomainNameServer) {
            Some(DhcpOption::DomainNameServer(servers)) => servers.clone(),
            _ => vec![],
        };

        let lease_time = match msg.opts().get(OptionCode::AddressLeaseTime) {
            Some(DhcpOption::AddressLeaseTime(time)) => *time,
            _ => 86400, // Default 24 hours
        };

        Dhcp4Lease {
            address,
            netmask,
            gateway,
            dns_servers,
            lease_time,
        }
    }
}

struct DhcpOffer {
    offered_ip: Ipv4Addr,
    server_id: Ipv4Addr,
}

fn generate_xid() -> u32 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    (now.as_nanos() as u32) ^ (std::process::id() << 16)
}
