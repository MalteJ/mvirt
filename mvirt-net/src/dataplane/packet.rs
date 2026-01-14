//! Packet parsing and building using smoltcp
//!
//! This module provides utilities for parsing and building Ethernet frames,
//! ARP packets, and IP packets.

use smoltcp::wire::{
    ArpOperation, ArpPacket, ArpRepr, EthernetAddress, EthernetFrame, EthernetProtocol,
    EthernetRepr, Ipv4Address,
};

/// Virtual gateway MAC address (used for ARP responses)
pub const GATEWAY_MAC: [u8; 6] = [0x02, 0x00, 0x00, 0x00, 0x00, 0x01];

/// IPv4 link-local gateway address
pub const GATEWAY_IPV4: Ipv4Address = Ipv4Address::new(169, 254, 0, 1);

/// Parse an Ethernet frame
pub fn parse_ethernet(data: &[u8]) -> Option<EthernetFrame<&[u8]>> {
    EthernetFrame::new_checked(data).ok()
}

/// Build an Ethernet frame with the given payload
pub fn build_ethernet_frame(
    dst_mac: EthernetAddress,
    src_mac: EthernetAddress,
    ethertype: EthernetProtocol,
    payload: &[u8],
) -> Vec<u8> {
    let repr = EthernetRepr {
        src_addr: src_mac,
        dst_addr: dst_mac,
        ethertype,
    };

    let mut buffer = vec![0u8; repr.buffer_len() + payload.len()];
    let mut frame = EthernetFrame::new_unchecked(&mut buffer);
    repr.emit(&mut frame);
    frame.payload_mut().copy_from_slice(payload);
    buffer
}

/// Parse an ARP packet from Ethernet payload
pub fn parse_arp(data: &[u8]) -> Option<ArpRepr> {
    let packet = ArpPacket::new_checked(data).ok()?;
    ArpRepr::parse(&packet).ok()
}

/// Build an ARP reply packet
pub fn build_arp_reply(
    sender_mac: EthernetAddress,
    sender_ip: Ipv4Address,
    target_mac: EthernetAddress,
    target_ip: Ipv4Address,
) -> Vec<u8> {
    let repr = ArpRepr::EthernetIpv4 {
        operation: ArpOperation::Reply,
        source_hardware_addr: sender_mac,
        source_protocol_addr: sender_ip,
        target_hardware_addr: target_mac,
        target_protocol_addr: target_ip,
    };

    let mut buffer = vec![0u8; repr.buffer_len()];
    let mut packet = ArpPacket::new_unchecked(&mut buffer);
    repr.emit(&mut packet);
    buffer
}

/// Build a complete ARP reply Ethernet frame
pub fn build_arp_reply_frame(
    dst_mac: EthernetAddress,
    src_mac: EthernetAddress,
    sender_ip: Ipv4Address,
    target_mac: EthernetAddress,
    target_ip: Ipv4Address,
) -> Vec<u8> {
    let arp_payload = build_arp_reply(src_mac, sender_ip, target_mac, target_ip);
    build_ethernet_frame(dst_mac, src_mac, EthernetProtocol::Arp, &arp_payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_ethernet() {
        // Minimal valid Ethernet frame (14 bytes header + some payload)
        let mut data = vec![0u8; 20];
        // Destination MAC
        data[0..6].copy_from_slice(&[0xff, 0xff, 0xff, 0xff, 0xff, 0xff]);
        // Source MAC
        data[6..12].copy_from_slice(&[0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);
        // EtherType (ARP = 0x0806)
        data[12..14].copy_from_slice(&[0x08, 0x06]);

        let frame = parse_ethernet(&data).unwrap();
        assert_eq!(
            frame.dst_addr(),
            EthernetAddress::from_bytes(&[0xff, 0xff, 0xff, 0xff, 0xff, 0xff])
        );
        assert_eq!(
            frame.src_addr(),
            EthernetAddress::from_bytes(&[0x52, 0x54, 0x00, 0x12, 0x34, 0x56])
        );
        assert_eq!(frame.ethertype(), EthernetProtocol::Arp);
    }

    #[test]
    fn test_build_arp_reply() {
        let sender_mac = EthernetAddress::from_bytes(&GATEWAY_MAC);
        let sender_ip = GATEWAY_IPV4;
        let target_mac = EthernetAddress::from_bytes(&[0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);
        let target_ip = Ipv4Address::new(10, 0, 0, 5);

        let packet = build_arp_reply(sender_mac, sender_ip, target_mac, target_ip);
        assert!(!packet.is_empty());

        // Parse it back
        let repr = parse_arp(&packet).unwrap();
        match repr {
            ArpRepr::EthernetIpv4 {
                operation,
                source_hardware_addr,
                source_protocol_addr,
                target_hardware_addr,
                target_protocol_addr,
            } => {
                assert_eq!(operation, ArpOperation::Reply);
                assert_eq!(source_hardware_addr, sender_mac);
                assert_eq!(source_protocol_addr, sender_ip);
                assert_eq!(target_hardware_addr, target_mac);
                assert_eq!(target_protocol_addr, target_ip);
            }
            _ => panic!("Expected EthernetIpv4 ARP"),
        }
    }

    #[test]
    fn test_gateway_constants() {
        assert_eq!(GATEWAY_IPV4, Ipv4Address::new(169, 254, 0, 1));
        assert_eq!(GATEWAY_MAC[0] & 0x02, 0x02); // Local bit set
    }
}
