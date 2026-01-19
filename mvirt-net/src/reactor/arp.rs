//! ARP (Address Resolution Protocol) handler for vhost-user interfaces.
//!
//! This module handles ARP requests from VMs and responds with the gateway MAC address.
//! The virtual gateway presents itself with a fixed MAC address for all gateway IPs.

use super::{GATEWAY_MAC, NicConfig};
use smoltcp::wire::{
    ArpOperation, ArpPacket, ArpRepr, EthernetAddress, EthernetFrame, EthernetProtocol,
    EthernetRepr, Ipv4Address,
};
use std::net::Ipv4Addr;
use tracing::debug;

/// Ethernet header size
const ETHERNET_HEADER_SIZE: usize = 14;

/// ARP packet size (for Ethernet + IPv4)
const ARP_PACKET_SIZE: usize = 28;

/// Handle an ARP packet from a VM.
///
/// Returns a response packet if the ARP request is for an address we should respond to
/// (gateway IP or any IP we're proxying for).
pub fn handle_arp_packet(
    nic_config: &NicConfig,
    virtio_hdr: &[u8],
    ethernet_frame: &[u8],
) -> Option<Vec<u8>> {
    // Parse the Ethernet frame
    let eth_frame = EthernetFrame::new_checked(ethernet_frame).ok()?;

    // Verify it's an ARP packet
    if eth_frame.ethertype() != EthernetProtocol::Arp {
        return None;
    }

    // Parse the ARP packet
    let arp_packet = ArpPacket::new_checked(eth_frame.payload()).ok()?;
    let arp_repr = ArpRepr::parse(&arp_packet).ok()?;

    match arp_repr {
        ArpRepr::EthernetIpv4 {
            operation: ArpOperation::Request,
            source_hardware_addr,
            source_protocol_addr,
            target_protocol_addr,
            ..
        } => {
            // Check if the request is for our gateway IP
            let gateway_ip = nic_config.ipv4_gateway?;
            let target_ip = Ipv4Addr::from(target_protocol_addr.0);

            if target_ip == gateway_ip {
                debug!(
                    src_mac = ?source_hardware_addr,
                    src_ip = %source_protocol_addr,
                    target_ip = %target_protocol_addr,
                    "ARP request for gateway, sending reply"
                );

                return Some(build_arp_reply(
                    virtio_hdr,
                    source_hardware_addr,
                    source_protocol_addr,
                    target_protocol_addr,
                ));
            }

            // For proxy ARP, we could respond for other IPs in the network
            // For now, we only respond for the gateway
            debug!(
                src_ip = %source_protocol_addr,
                target_ip = %target_protocol_addr,
                gateway = %gateway_ip,
                "ARP request not for gateway, ignoring"
            );
            None
        }
        _ => None,
    }
}

/// Build an ARP reply packet.
fn build_arp_reply(
    virtio_hdr: &[u8],
    target_hardware_addr: EthernetAddress,
    target_protocol_addr: Ipv4Address,
    source_protocol_addr: Ipv4Address,
) -> Vec<u8> {
    let virtio_hdr_size = virtio_hdr.len();
    let total_size = virtio_hdr_size + ETHERNET_HEADER_SIZE + ARP_PACKET_SIZE;
    let mut reply = vec![0u8; total_size];

    // Copy virtio header (zeroed is fine for TX)
    reply[..virtio_hdr_size].copy_from_slice(virtio_hdr);

    // Build Ethernet frame
    let gateway_mac = EthernetAddress(GATEWAY_MAC);
    let eth_repr = EthernetRepr {
        src_addr: gateway_mac,
        dst_addr: target_hardware_addr,
        ethertype: EthernetProtocol::Arp,
    };

    let mut eth_frame = EthernetFrame::new_unchecked(&mut reply[virtio_hdr_size..]);
    eth_repr.emit(&mut eth_frame);

    // Build ARP reply
    let arp_repr = ArpRepr::EthernetIpv4 {
        operation: ArpOperation::Reply,
        source_hardware_addr: gateway_mac,
        source_protocol_addr,
        target_hardware_addr,
        target_protocol_addr,
    };

    let mut arp_packet = ArpPacket::new_unchecked(eth_frame.payload_mut());
    arp_repr.emit(&mut arp_packet);

    debug!(
        dst_mac = ?target_hardware_addr,
        gateway_mac = ?gateway_mac,
        gateway_ip = %source_protocol_addr,
        "ARP reply built"
    );

    reply
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_config() -> NicConfig {
        NicConfig {
            mac: [0x52, 0x54, 0x00, 0x12, 0x34, 0x56],
            ipv4_address: Some(Ipv4Addr::new(10, 0, 0, 2)),
            ipv4_gateway: Some(Ipv4Addr::new(10, 0, 0, 1)),
            ipv4_prefix_len: 24,
            ipv6_address: None,
            ipv6_gateway: None,
            ipv6_prefix_len: 64,
            dns_servers: vec![],
        }
    }

    #[test]
    fn test_arp_request_for_gateway() {
        let config = make_test_config();
        let virtio_hdr = [0u8; 12];

        // Build an ARP request from the VM asking for the gateway
        let mut packet = vec![0u8; ETHERNET_HEADER_SIZE + ARP_PACKET_SIZE];

        // Ethernet header
        let vm_mac = EthernetAddress([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);
        let broadcast = EthernetAddress([0xff, 0xff, 0xff, 0xff, 0xff, 0xff]);
        let eth_repr = EthernetRepr {
            src_addr: vm_mac,
            dst_addr: broadcast,
            ethertype: EthernetProtocol::Arp,
        };
        let mut eth_frame = EthernetFrame::new_unchecked(&mut packet);
        eth_repr.emit(&mut eth_frame);

        // ARP request
        let arp_repr = ArpRepr::EthernetIpv4 {
            operation: ArpOperation::Request,
            source_hardware_addr: vm_mac,
            source_protocol_addr: Ipv4Address::new(10, 0, 0, 2),
            target_hardware_addr: EthernetAddress([0, 0, 0, 0, 0, 0]),
            target_protocol_addr: Ipv4Address::new(10, 0, 0, 1), // Gateway IP
        };
        let mut arp_packet = ArpPacket::new_unchecked(eth_frame.payload_mut());
        arp_repr.emit(&mut arp_packet);

        // Handle the request
        let reply = handle_arp_packet(&config, &virtio_hdr, &packet);
        assert!(reply.is_some(), "Should respond to ARP request for gateway");

        // Verify the reply
        let reply = reply.unwrap();
        let reply_eth = EthernetFrame::new_checked(&reply[12..]).unwrap();
        assert_eq!(reply_eth.ethertype(), EthernetProtocol::Arp);
        assert_eq!(reply_eth.dst_addr(), vm_mac);
        assert_eq!(reply_eth.src_addr(), EthernetAddress(GATEWAY_MAC));

        let reply_arp = ArpPacket::new_checked(reply_eth.payload()).unwrap();
        let reply_repr = ArpRepr::parse(&reply_arp).unwrap();
        match reply_repr {
            ArpRepr::EthernetIpv4 {
                operation,
                source_hardware_addr,
                source_protocol_addr,
                target_hardware_addr,
                target_protocol_addr,
            } => {
                assert_eq!(operation, ArpOperation::Reply);
                assert_eq!(source_hardware_addr, EthernetAddress(GATEWAY_MAC));
                assert_eq!(source_protocol_addr, Ipv4Address::new(10, 0, 0, 1));
                assert_eq!(target_hardware_addr, vm_mac);
                assert_eq!(target_protocol_addr, Ipv4Address::new(10, 0, 0, 2));
            }
            _ => panic!("Expected EthernetIpv4 ARP reply"),
        }
    }

    #[test]
    fn test_arp_request_for_other_ip() {
        let config = make_test_config();
        let virtio_hdr = [0u8; 12];

        // Build an ARP request for a non-gateway IP
        let mut packet = vec![0u8; ETHERNET_HEADER_SIZE + ARP_PACKET_SIZE];

        let vm_mac = EthernetAddress([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);
        let broadcast = EthernetAddress([0xff, 0xff, 0xff, 0xff, 0xff, 0xff]);
        let eth_repr = EthernetRepr {
            src_addr: vm_mac,
            dst_addr: broadcast,
            ethertype: EthernetProtocol::Arp,
        };
        let mut eth_frame = EthernetFrame::new_unchecked(&mut packet);
        eth_repr.emit(&mut eth_frame);

        let arp_repr = ArpRepr::EthernetIpv4 {
            operation: ArpOperation::Request,
            source_hardware_addr: vm_mac,
            source_protocol_addr: Ipv4Address::new(10, 0, 0, 2),
            target_hardware_addr: EthernetAddress([0, 0, 0, 0, 0, 0]),
            target_protocol_addr: Ipv4Address::new(10, 0, 0, 100), // Not gateway
        };
        let mut arp_packet = ArpPacket::new_unchecked(eth_frame.payload_mut());
        arp_repr.emit(&mut arp_packet);

        let reply = handle_arp_packet(&config, &virtio_hdr, &packet);
        assert!(
            reply.is_none(),
            "Should not respond to ARP for non-gateway IP"
        );
    }
}
