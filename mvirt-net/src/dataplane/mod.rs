//! Data plane: per-vNIC Reactor threads
//!
//! This module contains the vhost-user backend implementation, packet processing,
//! and routing between vNICs.
//!
//! Architecture:
//! - Each vNIC gets a dedicated Reactor thread (shared-nothing)
//! - Reactors handle protocol processing (ARP, DHCP, ICMP, NDP)
//! - Inter-reactor routing via crossbeam channels
//! - Zero-copy buffer pool for high-performance packet handling

pub mod arp;
pub mod backend;
pub mod buffer;
pub mod dhcpv4;
pub mod dhcpv6;
pub mod icmp;
pub mod icmpv6;
pub mod manager;
pub mod ndp;
pub mod packet;
pub mod reactor;
pub mod router;
pub mod tun;
pub mod vhost;

pub use arp::ArpResponder;
pub use backend::{ReactorBackend, RecvResult, TunBackend, VhostBackend, VhostPacketSender};
pub use buffer::{BUFFER_SIZE, BufferPool, ETH_HEADROOM, HEADROOM, MAX_PACKET, PoolBuffer};
pub use dhcpv4::Dhcpv4Server;
pub use dhcpv6::Dhcpv6Server;
pub use icmp::IcmpResponder;
pub use icmpv6::Icmpv6Responder;
pub use manager::ReactorManager;
pub use ndp::NdpResponder;
pub use packet::{GATEWAY_IPV4, GATEWAY_MAC};
pub use reactor::{
    InboundPacket, Layer2Config, Reactor, ReactorConfig, ReactorReceiver, ReactorRegistry,
    ReactorSender, reactor_channel,
};
pub use router::{
    LocalRouting, NetworkRouter, NicChannel, RouteResult, RouteUpdate, RoutedPacket, RoutingHandle,
};
pub use tun::{TunDevice, add_route, get_routes, remove_route};
pub use vhost::{
    VIRTIO_NET_HDR_F_DATA_VALID, VIRTIO_NET_HDR_F_NEEDS_CSUM, VIRTIO_NET_HDR_GSO_NONE,
    VIRTIO_NET_HDR_GSO_TCPV4, VIRTIO_NET_HDR_GSO_TCPV6, VhostNetBackend, VirtioNetHdr, parse_mac,
};
