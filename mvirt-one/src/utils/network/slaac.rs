//! SLAAC (Stateless Address Autoconfiguration) implementation.
//! Ported from pideisn.

use super::interface::mac_to_eui64;
use super::{Interface, NetlinkHandle};
use crate::error::NetworkError;
use log::{debug, info};
use std::net::Ipv6Addr;
use std::time::Duration;
use tokio::time::timeout;

// ICMPv6 types
const ICMPV6_ROUTER_SOLICITATION: u8 = 133;
const ICMPV6_ROUTER_ADVERTISEMENT: u8 = 134;

// IPv6 multicast addresses
const ALL_ROUTERS_MULTICAST: Ipv6Addr = Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 0, 2);

/// SLAAC configuration result.
#[derive(Debug, Clone, Default)]
pub struct SlaacInfo {
    pub gateway: Option<Ipv6Addr>,
}

/// Configure an interface using SLAAC.
pub async fn configure(iface: &Interface, nl: &NetlinkHandle) -> Result<SlaacInfo, NetworkError> {
    // Generate and configure link-local address
    let link_local = generate_link_local(&iface.mac);
    info!("SLAAC: Configuring link-local {}", link_local);

    nl.add_address_v6(iface.index, link_local, 64).await?;

    // Wait a moment for DAD (Duplicate Address Detection)
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Send Router Solicitation and process Router Advertisements
    let socket = create_icmpv6_socket(iface)?;

    for attempt in 0..3 {
        debug!(
            "SLAAC: Sending Router Solicitation (attempt {})",
            attempt + 1
        );
        send_router_solicitation(&socket, iface.index)?;

        match timeout(Duration::from_secs(2), receive_ra(&socket)).await {
            Ok(Ok(ra)) => {
                let gateway = ra.router_addr;
                process_router_advertisement(&ra, iface, nl).await?;
                return Ok(SlaacInfo {
                    gateway: Some(gateway),
                });
            }
            Ok(Err(e)) => {
                debug!("SLAAC: Error receiving RA: {}", e);
            }
            Err(_) => {
                debug!("SLAAC: Timeout waiting for RA");
            }
        }
    }

    // No router found, but link-local is configured - that's okay
    info!("SLAAC: No router found, using link-local only");
    Ok(SlaacInfo::default())
}

fn generate_link_local(mac: &[u8; 6]) -> Ipv6Addr {
    let eui64 = mac_to_eui64(mac);

    Ipv6Addr::new(
        0xfe80,
        0,
        0,
        0,
        u16::from_be_bytes([eui64[0], eui64[1]]),
        u16::from_be_bytes([eui64[2], eui64[3]]),
        u16::from_be_bytes([eui64[4], eui64[5]]),
        u16::from_be_bytes([eui64[6], eui64[7]]),
    )
}

struct RawSocket {
    fd: i32,
}

impl RawSocket {
    fn try_clone(&self) -> Result<Self, std::io::Error> {
        let new_fd = unsafe { libc::dup(self.fd) };
        if new_fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(RawSocket { fd: new_fd })
    }

    fn set_read_timeout(&self, dur: Option<Duration>) -> Result<(), std::io::Error> {
        let tv = match dur {
            Some(d) => libc::timeval {
                tv_sec: d.as_secs() as i64,
                tv_usec: d.subsec_micros() as i64,
            },
            None => libc::timeval {
                tv_sec: 0,
                tv_usec: 0,
            },
        };
        let ret = unsafe {
            libc::setsockopt(
                self.fd,
                libc::SOL_SOCKET,
                libc::SO_RCVTIMEO,
                &tv as *const _ as *const libc::c_void,
                std::mem::size_of::<libc::timeval>() as libc::socklen_t,
            )
        };
        if ret < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }

    fn recv_from(&self, buf: &mut [u8]) -> Result<(usize, libc::sockaddr_in6), std::io::Error> {
        let mut addr: libc::sockaddr_in6 = unsafe { std::mem::zeroed() };
        let mut addrlen = std::mem::size_of::<libc::sockaddr_in6>() as libc::socklen_t;

        let len = unsafe {
            libc::recvfrom(
                self.fd,
                buf.as_mut_ptr() as *mut libc::c_void,
                buf.len(),
                0,
                &mut addr as *mut _ as *mut libc::sockaddr,
                &mut addrlen,
            )
        };
        if len < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok((len as usize, addr))
    }

    fn send_to(&self, data: &[u8], dest: &libc::sockaddr_in6) -> Result<usize, std::io::Error> {
        let len = unsafe {
            libc::sendto(
                self.fd,
                data.as_ptr() as *const libc::c_void,
                data.len(),
                0,
                dest as *const _ as *const libc::sockaddr,
                std::mem::size_of::<libc::sockaddr_in6>() as libc::socklen_t,
            )
        };
        if len < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(len as usize)
    }
}

impl Drop for RawSocket {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.fd);
        }
    }
}

fn create_icmpv6_socket(iface: &Interface) -> Result<RawSocket, NetworkError> {
    // Create raw ICMPv6 socket
    let fd = unsafe { libc::socket(libc::AF_INET6, libc::SOCK_RAW, libc::IPPROTO_ICMPV6) };
    if fd < 0 {
        return Err(NetworkError::SocketError(std::io::Error::last_os_error()));
    }

    let socket = RawSocket { fd };

    // Set hop limit to 255 (required for NDP)
    let hops: libc::c_int = 255;
    unsafe {
        libc::setsockopt(
            fd,
            libc::IPPROTO_IPV6,
            libc::IPV6_MULTICAST_HOPS,
            &hops as *const _ as *const libc::c_void,
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        );
        libc::setsockopt(
            fd,
            libc::IPPROTO_IPV6,
            libc::IPV6_UNICAST_HOPS,
            &hops as *const _ as *const libc::c_void,
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        );
    }

    // Bind to interface
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

    // Join all-nodes multicast group for receiving RAs
    let mreq = libc::ipv6_mreq {
        ipv6mr_multiaddr: libc::in6_addr {
            s6_addr: [0xff, 0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1],
        },
        ipv6mr_interface: iface.index,
    };
    unsafe {
        libc::setsockopt(
            fd,
            libc::IPPROTO_IPV6,
            libc::IPV6_ADD_MEMBERSHIP,
            &mreq as *const _ as *const libc::c_void,
            std::mem::size_of::<libc::ipv6_mreq>() as libc::socklen_t,
        );
    }

    Ok(socket)
}

fn send_router_solicitation(socket: &RawSocket, iface_index: u32) -> Result<(), NetworkError> {
    // Build Router Solicitation packet
    // ICMPv6 header: type (1) + code (1) + checksum (2) + reserved (4)
    let packet = vec![
        ICMPV6_ROUTER_SOLICITATION, // Type
        0,                          // Code
        0,
        0, // Checksum (kernel calculates for raw sockets)
        0,
        0,
        0,
        0, // Reserved
    ];

    // Build destination address
    let dest = libc::sockaddr_in6 {
        sin6_family: libc::AF_INET6 as libc::sa_family_t,
        sin6_port: 0,
        sin6_flowinfo: 0,
        sin6_addr: libc::in6_addr {
            s6_addr: ALL_ROUTERS_MULTICAST.octets(),
        },
        sin6_scope_id: iface_index,
    };

    socket
        .send_to(&packet, &dest)
        .map_err(NetworkError::SocketError)?;

    Ok(())
}

async fn receive_ra(socket: &RawSocket) -> Result<RouterAdvertisement, NetworkError> {
    let socket_clone = socket.try_clone().map_err(NetworkError::SocketError)?;
    let result = tokio::task::spawn_blocking(move || {
        let mut buf = [0u8; 1500];
        socket_clone.set_read_timeout(Some(Duration::from_millis(500)))?;
        let (len, addr) = socket_clone.recv_from(&mut buf)?;
        Ok::<_, std::io::Error>((len, addr, buf))
    })
    .await
    .map_err(|_| NetworkError::Timeout)?
    .map_err(NetworkError::SocketError)?;

    let (len, addr, buf) = result;

    // Parse ICMPv6 packet
    if len < 16 {
        return Err(NetworkError::InvalidPacket("RA too short".into()));
    }

    if buf[0] != ICMPV6_ROUTER_ADVERTISEMENT {
        return Err(NetworkError::InvalidPacket(
            "Not a Router Advertisement".into(),
        ));
    }

    // Extract router address from source
    let router_addr = Ipv6Addr::from(addr.sin6_addr.s6_addr);

    parse_router_advertisement(&buf[..len], router_addr)
}

fn parse_router_advertisement(
    data: &[u8],
    router_addr: Ipv6Addr,
) -> Result<RouterAdvertisement, NetworkError> {
    if data.len() < 16 {
        return Err(NetworkError::InvalidPacket("RA too short".into()));
    }

    let router_lifetime = u16::from_be_bytes([data[6], data[7]]);

    let mut prefixes = Vec::new();
    let mut pos = 16; // Skip ICMPv6 header

    // Parse options
    while pos + 2 <= data.len() {
        let opt_type = data[pos];
        let opt_len = data[pos + 1] as usize * 8;

        if opt_len == 0 || pos + opt_len > data.len() {
            break;
        }

        // Prefix Information option (type 3)
        if opt_type == 3 && opt_len >= 32 {
            let prefix_len = data[pos + 2];
            let flags = data[pos + 3];
            let autonomous = (flags & 0x40) != 0;
            let valid_lifetime =
                u32::from_be_bytes([data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7]]);
            let preferred_lifetime =
                u32::from_be_bytes([data[pos + 8], data[pos + 9], data[pos + 10], data[pos + 11]]);

            let mut prefix_bytes = [0u8; 16];
            prefix_bytes.copy_from_slice(&data[pos + 16..pos + 32]);
            let prefix = Ipv6Addr::from(prefix_bytes);

            prefixes.push(PrefixInfo {
                prefix,
                prefix_len,
                autonomous,
                valid_lifetime,
                preferred_lifetime,
            });
        }

        pos += opt_len;
    }

    Ok(RouterAdvertisement {
        router_addr,
        router_lifetime,
        prefixes,
    })
}

async fn process_router_advertisement(
    ra: &RouterAdvertisement,
    iface: &Interface,
    nl: &NetlinkHandle,
) -> Result<(), NetworkError> {
    info!("SLAAC: Received RA from {}", ra.router_addr);

    // Configure addresses from prefixes with autonomous flag
    for prefix_info in &ra.prefixes {
        if !prefix_info.autonomous {
            continue;
        }

        // Generate address from prefix + EUI-64
        let addr =
            generate_address_from_prefix(&prefix_info.prefix, prefix_info.prefix_len, &iface.mac);

        info!(
            "SLAAC: Configuring {} (prefix {}/{})",
            addr, prefix_info.prefix, prefix_info.prefix_len
        );

        if let Err(e) = nl
            .add_address_v6(iface.index, addr, prefix_info.prefix_len)
            .await
        {
            debug!("SLAAC: Failed to add address: {}", e);
        }
    }

    // Add default route if router lifetime > 0
    if ra.router_lifetime > 0 {
        info!("SLAAC: Adding default route via {}", ra.router_addr);
        if let Err(e) = nl.add_route_v6(ra.router_addr, iface.index).await {
            debug!("SLAAC: Failed to add route: {}", e);
        }
    }

    Ok(())
}

fn generate_address_from_prefix(prefix: &Ipv6Addr, _prefix_len: u8, mac: &[u8; 6]) -> Ipv6Addr {
    let eui64 = mac_to_eui64(mac);
    let prefix_segments = prefix.segments();

    // For /64 prefix, replace the last 64 bits with EUI-64
    Ipv6Addr::new(
        prefix_segments[0],
        prefix_segments[1],
        prefix_segments[2],
        prefix_segments[3],
        u16::from_be_bytes([eui64[0], eui64[1]]),
        u16::from_be_bytes([eui64[2], eui64[3]]),
        u16::from_be_bytes([eui64[4], eui64[5]]),
        u16::from_be_bytes([eui64[6], eui64[7]]),
    )
}

#[derive(Debug)]
struct RouterAdvertisement {
    router_addr: Ipv6Addr,
    router_lifetime: u16,
    prefixes: Vec<PrefixInfo>,
}

#[derive(Debug)]
struct PrefixInfo {
    prefix: Ipv6Addr,
    prefix_len: u8,
    autonomous: bool,
    #[allow(dead_code)]
    valid_lifetime: u32,
    #[allow(dead_code)]
    preferred_lifetime: u32,
}
