//! Test utilities for vhost-user frontend simulation
//!
//! This module provides a realistic virtio-net driver implementation
//! for integration tests, based on the Linux kernel driver.

pub mod frontend_device;
pub mod packets;
pub mod virtqueue;

pub use frontend_device::{VIRTIO_NET_HDR_SIZE, VhostUserFrontendDevice};
pub use packets::{
    ArpReply, DhcpMessageType, DhcpResponse, ETHERNET_HDR_SIZE, IcmpEchoReply, create_arp_request,
    create_dhcp_discover, create_dhcp_request, create_icmp_echo_request, parse_arp_reply,
    parse_dhcp_response, parse_icmp_echo_reply,
};
pub use virtqueue::{UsedBuffer, VirtqueueDriver};
