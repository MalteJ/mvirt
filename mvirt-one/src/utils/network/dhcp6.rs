//! DHCPv6 client implementation with prefix delegation.
//! Ported from pideisn.

use super::{DelegatedPrefix, Dhcp6Lease, Interface, NetlinkHandle};
use crate::error::NetworkError;
use dhcproto::v6::{self, DhcpOption, DhcpOptions, Message, MessageType, OptionCode};
use dhcproto::{Decodable, Encodable};
use log::{debug, info};
use socket2::{Domain, Protocol, Socket, Type};
use std::mem::MaybeUninit;
use std::net::{Ipv6Addr, SocketAddrV6};
use std::os::unix::io::AsRawFd;
use std::time::{Duration, Instant};
use tokio::time::timeout;

const DHCP6_CLIENT_PORT: u16 = 546;
const DHCP6_SERVER_PORT: u16 = 547;
const ALL_DHCP_RELAY_AGENTS_AND_SERVERS: Ipv6Addr = Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 1, 2);

/// Configure an interface using DHCPv6.
pub async fn configure(
    iface: &Interface,
    nl: &NetlinkHandle,
    request_pd: bool,
) -> Result<Dhcp6Lease, NetworkError> {
    let mut client = Dhcp6Client::new(iface, request_pd)?;
    let lease = client.run().await?;

    // Configure the address if we got one
    if let Some(addr) = lease.address {
        nl.add_address_v6(iface.index, addr, 128).await?;
    }

    Ok(lease)
}

struct Dhcp6Client {
    socket: Socket,
    mac: [u8; 6],
    duid: Vec<u8>,
    iface_name: String,
    iface_index: u32,
    request_pd: bool,
}

impl Dhcp6Client {
    fn new(iface: &Interface, request_pd: bool) -> Result<Self, NetworkError> {
        let socket = Socket::new(Domain::IPV6, Type::DGRAM, Some(Protocol::UDP))?;

        socket.set_reuse_address(true)?;

        // Bind to client port on link-local
        let addr = SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, DHCP6_CLIENT_PORT, 0, iface.index);
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

        // Generate DUID-LL (Link-Layer)
        let duid = generate_duid_ll(&iface.mac);

        Ok(Self {
            socket,
            mac: iface.mac,
            duid,
            iface_name: iface.name.clone(),
            iface_index: iface.index,
            request_pd,
        })
    }

    async fn run(&mut self) -> Result<Dhcp6Lease, NetworkError> {
        const MAX_RETRIES: u32 = 4;
        let mut retry = 0;

        loop {
            // Generate new transaction ID for each attempt
            let xid = generate_xid();

            // Send SOLICIT
            info!("DHCPv6: Sending SOLICIT on {}", self.iface_name);
            self.send_solicit(xid)?;

            // Wait for ADVERTISE
            let advertise = match self.wait_for_advertise(xid, Duration::from_secs(4)).await {
                Ok(adv) => adv,
                Err(NetworkError::Timeout) => {
                    retry += 1;
                    if retry >= MAX_RETRIES {
                        return Err(NetworkError::NoAdvertise);
                    }
                    debug!("DHCPv6: Timeout waiting for ADVERTISE, retry {}", retry);
                    continue;
                }
                Err(e) => return Err(e),
            };

            info!("DHCPv6: Received ADVERTISE");

            // Send REQUEST
            info!("DHCPv6: Sending REQUEST");
            self.send_request(xid, &advertise)?;

            // Wait for REPLY
            match self.wait_for_reply(xid, Duration::from_secs(4)).await {
                Ok(lease) => {
                    info!("DHCPv6: Received REPLY");
                    return Ok(lease);
                }
                Err(NetworkError::Timeout) => {
                    retry += 1;
                    if retry >= MAX_RETRIES {
                        return Err(NetworkError::NoAdvertise);
                    }
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
    }

    fn send_solicit(&self, xid: [u8; 3]) -> Result<(), NetworkError> {
        let mut msg = Message::new(MessageType::Solicit);
        msg.set_xid(xid);

        // Client Identifier (DUID)
        msg.opts_mut()
            .insert(DhcpOption::ClientId(self.duid.clone()));

        // Request IA_NA (address)
        let iaid = generate_iaid(&self.mac);
        msg.opts_mut().insert(DhcpOption::IANA(v6::IANA {
            id: iaid,
            t1: 0,
            t2: 0,
            opts: DhcpOptions::new(),
        }));

        // Request IA_PD (prefix delegation) if enabled
        if self.request_pd {
            let pd_iaid = generate_iaid(&self.mac).wrapping_add(1);
            let ia_prefix = v6::IAPrefix {
                preferred_lifetime: 0,
                valid_lifetime: 0,
                prefix_len: 64,
                prefix_ip: Ipv6Addr::UNSPECIFIED,
                opts: DhcpOptions::new(),
            };
            let pd_opts: DhcpOptions =
                std::iter::once(v6::DhcpOption::IAPrefix(ia_prefix)).collect();
            msg.opts_mut().insert(DhcpOption::IAPD(v6::IAPD {
                id: pd_iaid,
                t1: 0,
                t2: 0,
                opts: pd_opts,
            }));
        }

        // Option Request Option
        msg.opts_mut().insert(DhcpOption::ORO(v6::ORO {
            opts: vec![OptionCode::DomainNameServers, OptionCode::DomainSearchList],
        }));

        // Elapsed Time
        msg.opts_mut().insert(DhcpOption::ElapsedTime(0));

        let bytes = msg
            .to_vec()
            .map_err(|e| NetworkError::InvalidPacket(e.to_string()))?;
        self.send_to_servers(&bytes)
    }

    fn send_request(&self, xid: [u8; 3], advertise: &Dhcp6Advertise) -> Result<(), NetworkError> {
        let mut msg = Message::new(MessageType::Request);
        msg.set_xid(xid);

        // Client Identifier
        msg.opts_mut()
            .insert(DhcpOption::ClientId(self.duid.clone()));

        // Server Identifier
        msg.opts_mut()
            .insert(DhcpOption::ServerId(advertise.server_duid.clone()));

        // IA_NA with offered address
        let iaid = generate_iaid(&self.mac);
        let ia_na_opts: DhcpOptions = if let Some(addr) = advertise.offered_address {
            std::iter::once(v6::DhcpOption::IAAddr(v6::IAAddr {
                addr,
                preferred_life: 0,
                valid_life: 0,
                opts: DhcpOptions::new(),
            }))
            .collect()
        } else {
            DhcpOptions::new()
        };
        msg.opts_mut().insert(DhcpOption::IANA(v6::IANA {
            id: iaid,
            t1: 0,
            t2: 0,
            opts: ia_na_opts,
        }));

        // IA_PD if we're requesting prefix delegation
        if self.request_pd {
            let pd_iaid = generate_iaid(&self.mac).wrapping_add(1);
            let ia_pd_opts: DhcpOptions = if let Some(ref pd) = advertise.offered_prefix {
                std::iter::once(v6::DhcpOption::IAPrefix(v6::IAPrefix {
                    preferred_lifetime: pd.preferred_lifetime,
                    valid_lifetime: pd.valid_lifetime,
                    prefix_len: pd.prefix_len,
                    prefix_ip: pd.prefix,
                    opts: DhcpOptions::new(),
                }))
                .collect()
            } else {
                DhcpOptions::new()
            };
            msg.opts_mut().insert(DhcpOption::IAPD(v6::IAPD {
                id: pd_iaid,
                t1: 0,
                t2: 0,
                opts: ia_pd_opts,
            }));
        }

        // Option Request Option
        msg.opts_mut().insert(DhcpOption::ORO(v6::ORO {
            opts: vec![OptionCode::DomainNameServers, OptionCode::DomainSearchList],
        }));

        // Elapsed Time
        msg.opts_mut().insert(DhcpOption::ElapsedTime(0));

        let bytes = msg
            .to_vec()
            .map_err(|e| NetworkError::InvalidPacket(e.to_string()))?;
        self.send_to_servers(&bytes)
    }

    fn send_to_servers(&self, data: &[u8]) -> Result<(), NetworkError> {
        let dest = SocketAddrV6::new(
            ALL_DHCP_RELAY_AGENTS_AND_SERVERS,
            DHCP6_SERVER_PORT,
            0,
            self.iface_index,
        );
        self.socket
            .send_to(data, &dest.into())
            .map_err(NetworkError::SocketError)?;
        Ok(())
    }

    async fn wait_for_advertise(
        &self,
        xid: [u8; 3],
        dur: Duration,
    ) -> Result<Dhcp6Advertise, NetworkError> {
        let start = Instant::now();

        while start.elapsed() < dur {
            let remaining = dur.saturating_sub(start.elapsed());

            match timeout(remaining, self.recv_packet()).await {
                Ok(Ok(msg)) => {
                    if msg.xid() != xid {
                        continue;
                    }

                    if msg.msg_type() != MessageType::Advertise {
                        continue;
                    }

                    return self.parse_advertise(&msg);
                }
                Ok(Err(_)) => continue,
                Err(_) => return Err(NetworkError::Timeout),
            }
        }

        Err(NetworkError::Timeout)
    }

    async fn wait_for_reply(
        &self,
        xid: [u8; 3],
        dur: Duration,
    ) -> Result<Dhcp6Lease, NetworkError> {
        let start = Instant::now();

        while start.elapsed() < dur {
            let remaining = dur.saturating_sub(start.elapsed());

            match timeout(remaining, self.recv_packet()).await {
                Ok(Ok(msg)) => {
                    if msg.xid() != xid {
                        continue;
                    }

                    if msg.msg_type() != MessageType::Reply {
                        continue;
                    }

                    return self.parse_reply(&msg);
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

    fn parse_advertise(&self, msg: &Message) -> Result<Dhcp6Advertise, NetworkError> {
        let server_duid = match msg.opts().get(OptionCode::ServerId) {
            Some(DhcpOption::ServerId(duid)) => duid.clone(),
            _ => return Err(NetworkError::InvalidPacket("No server DUID".into())),
        };

        let mut offered_address = None;
        let mut offered_prefix = None;

        // Parse IA_NA for address
        if let Some(DhcpOption::IANA(ia_na)) = msg.opts().get(OptionCode::IANA) {
            for opt in ia_na.opts.clone() {
                if let v6::DhcpOption::IAAddr(ia_addr) = opt {
                    offered_address = Some(ia_addr.addr);
                    break;
                }
            }
        }

        // Parse IA_PD for prefix
        if let Some(DhcpOption::IAPD(ia_pd)) = msg.opts().get(OptionCode::IAPD) {
            for opt in ia_pd.opts.clone() {
                if let v6::DhcpOption::IAPrefix(ia_prefix) = opt {
                    offered_prefix = Some(DelegatedPrefix {
                        prefix: ia_prefix.prefix_ip,
                        prefix_len: ia_prefix.prefix_len,
                        preferred_lifetime: ia_prefix.preferred_lifetime,
                        valid_lifetime: ia_prefix.valid_lifetime,
                    });
                    break;
                }
            }
        }

        Ok(Dhcp6Advertise {
            server_duid,
            offered_address,
            offered_prefix,
        })
    }

    fn parse_reply(&self, msg: &Message) -> Result<Dhcp6Lease, NetworkError> {
        let mut address = None;
        let mut prefix = None;
        let mut dns_servers = Vec::new();

        // Parse IA_NA for address
        if let Some(DhcpOption::IANA(ia_na)) = msg.opts().get(OptionCode::IANA) {
            for opt in ia_na.opts.clone() {
                if let v6::DhcpOption::IAAddr(ia_addr) = opt {
                    address = Some(ia_addr.addr);
                    break;
                }
            }
        }

        // Parse IA_PD for prefix
        if let Some(DhcpOption::IAPD(ia_pd)) = msg.opts().get(OptionCode::IAPD) {
            for opt in ia_pd.opts.clone() {
                if let v6::DhcpOption::IAPrefix(ia_prefix) = opt {
                    prefix = Some(DelegatedPrefix {
                        prefix: ia_prefix.prefix_ip,
                        prefix_len: ia_prefix.prefix_len,
                        preferred_lifetime: ia_prefix.preferred_lifetime,
                        valid_lifetime: ia_prefix.valid_lifetime,
                    });
                    break;
                }
            }
        }

        // Parse DNS servers
        if let Some(DhcpOption::DomainNameServers(servers)) =
            msg.opts().get(OptionCode::DomainNameServers)
        {
            dns_servers = servers.clone();
        }

        Ok(Dhcp6Lease {
            address,
            prefix,
            dns_servers,
        })
    }
}

struct Dhcp6Advertise {
    server_duid: Vec<u8>,
    offered_address: Option<Ipv6Addr>,
    offered_prefix: Option<DelegatedPrefix>,
}

fn generate_duid_ll(mac: &[u8; 6]) -> Vec<u8> {
    // DUID-LL (Link-Layer): type (2 bytes) + hw type (2 bytes) + link-layer address
    let mut duid = vec![
        0x00, 0x03, // DUID type: DUID-LL
        0x00, 0x01, // Hardware type: Ethernet
    ];
    duid.extend_from_slice(mac);
    duid
}

fn generate_xid() -> [u8; 3] {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let val = (now.as_nanos() as u32) ^ (std::process::id() << 8);
    [(val >> 16) as u8, (val >> 8) as u8, val as u8]
}

fn generate_iaid(mac: &[u8; 6]) -> u32 {
    // Use last 4 bytes of MAC as IAID
    u32::from_be_bytes([mac[2], mac[3], mac[4], mac[5]])
}
