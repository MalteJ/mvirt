//! Test utilities for vhost-user frontend simulation
//!
//! This module provides a realistic virtio-net driver implementation
//! for integration tests, based on the Linux kernel driver.

pub mod frontend_device;
pub mod packets;
pub mod virtqueue;

pub use frontend_device::{VIRTIO_NET_HDR_SIZE, VhostUserFrontendDevice};
pub use packets::{
    ArpReply, DhcpMessageType, DhcpResponse, Dhcpv6MessageType, Dhcpv6Response, ETHERNET_HDR_SIZE,
    IcmpEchoReply, Icmpv6EchoReply, NaResponse, RaResponse, create_arp_request,
    create_dhcp_discover, create_dhcp_request, create_dhcpv6_request, create_dhcpv6_solicit,
    create_icmp_echo_request, create_icmpv6_echo_request, create_neighbor_solicitation,
    create_router_solicitation, generate_duid_ll, parse_arp_reply, parse_dhcp_response,
    parse_dhcpv6_response, parse_icmp_echo_reply, parse_icmpv6_echo_reply,
    parse_neighbor_advertisement, parse_router_advertisement,
};
pub use virtqueue::{UsedBuffer, VirtqueueDriver};
