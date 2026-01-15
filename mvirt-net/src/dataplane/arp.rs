//! ARP responder for gateway address
//!
//! Responds to ARP requests for the virtual gateway (169.254.0.1) with
//! a virtual MAC address.

use smoltcp::wire::{ArpOperation, ArpRepr, EthernetAddress, EthernetProtocol, Ipv4Address};
use tracing::debug;

use super::packet::{GATEWAY_IPV4, GATEWAY_MAC, build_arp_reply_frame, parse_arp, parse_ethernet};

/// ARP responder configuration
pub struct ArpResponder {
    /// Virtual MAC address for this vNIC (reserved for future use)
    #[allow(dead_code)]
    nic_mac: EthernetAddress,
    /// Additional IP addresses to respond for (besides gateway)
    additional_ips: Vec<Ipv4Address>,
}

impl ArpResponder {
    /// Create a new ARP responder
    pub fn new(nic_mac: [u8; 6]) -> Self {
        Self {
            nic_mac: EthernetAddress::from_bytes(&nic_mac),
            additional_ips: Vec::new(),
        }
    }

    /// Add an additional IP address to respond to
    pub fn add_ip(&mut self, ip: Ipv4Address) {
        if !self.additional_ips.contains(&ip) {
            self.additional_ips.push(ip);
        }
    }

    /// Process an incoming packet and potentially generate an ARP reply
    ///
    /// Returns `Some(frame)` if an ARP reply should be sent, `None` otherwise.
    pub fn process(&self, packet: &[u8]) -> Option<Vec<u8>> {
        let frame = parse_ethernet(packet)?;

        // Only process ARP packets
        if frame.ethertype() != EthernetProtocol::Arp {
            return None;
        }

        let arp = parse_arp(frame.payload())?;

        if let ArpRepr::EthernetIpv4 {
            operation: ArpOperation::Request,
            source_hardware_addr,
            source_protocol_addr,
            target_protocol_addr,
            ..
        } = arp
        {
            // Check if this is asking for the gateway or one of our IPs
            if target_protocol_addr == GATEWAY_IPV4
                || self.additional_ips.contains(&target_protocol_addr)
            {
                debug!(
                    target_ip = %target_protocol_addr,
                    source_ip = %source_protocol_addr,
                    source_mac = %source_hardware_addr,
                    "ARP request received"
                );

                // Respond with gateway MAC
                let gateway_mac = EthernetAddress::from_bytes(&GATEWAY_MAC);

                debug!(
                    reply_mac = %gateway_mac,
                    reply_ip = %target_protocol_addr,
                    "Sending ARP reply"
                );

                return Some(build_arp_reply_frame(
                    source_hardware_addr,
                    gateway_mac,
                    target_protocol_addr,
                    source_hardware_addr,
                    source_protocol_addr,
                ));
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_arp_request(sender_mac: [u8; 6], sender_ip: [u8; 4], target_ip: [u8; 4]) -> Vec<u8> {
        use smoltcp::wire::{ArpPacket, EthernetFrame, EthernetRepr};

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

    #[test]
    fn test_arp_request_for_gateway() {
        let responder = ArpResponder::new([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);

        // Build ARP request for gateway (169.254.0.1)
        let request = build_arp_request(
            [0x52, 0x54, 0x00, 0x12, 0x34, 0x56],
            [10, 0, 0, 5],
            [169, 254, 0, 1], // Gateway IP
        );

        let reply = responder.process(&request);
        assert!(reply.is_some(), "Should generate ARP reply for gateway");

        // Parse the reply
        let reply_data = reply.unwrap();
        let frame = parse_ethernet(&reply_data).unwrap();
        assert_eq!(frame.ethertype(), EthernetProtocol::Arp);

        let arp = parse_arp(frame.payload()).unwrap();
        match arp {
            ArpRepr::EthernetIpv4 {
                operation,
                source_hardware_addr,
                source_protocol_addr,
                target_protocol_addr,
                ..
            } => {
                assert_eq!(operation, ArpOperation::Reply);
                assert_eq!(source_hardware_addr.as_bytes(), &GATEWAY_MAC);
                assert_eq!(source_protocol_addr, GATEWAY_IPV4);
                assert_eq!(target_protocol_addr, Ipv4Address::new(10, 0, 0, 5));
            }
            _ => panic!("Expected EthernetIpv4 ARP reply"),
        }
    }

    #[test]
    fn test_arp_request_for_other_ip() {
        let responder = ArpResponder::new([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);

        // Build ARP request for some other IP (not gateway)
        let request = build_arp_request(
            [0x52, 0x54, 0x00, 0x12, 0x34, 0x56],
            [10, 0, 0, 5],
            [10, 0, 0, 10], // Not gateway
        );

        let reply = responder.process(&request);
        assert!(reply.is_none(), "Should not reply to non-gateway ARP");
    }

    #[test]
    fn test_non_arp_packet() {
        let responder = ArpResponder::new([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);

        // Build a non-ARP Ethernet frame (IPv4)
        let mut packet = vec![0u8; 20];
        packet[0..6].copy_from_slice(&[0xff, 0xff, 0xff, 0xff, 0xff, 0xff]); // dst
        packet[6..12].copy_from_slice(&[0x52, 0x54, 0x00, 0x12, 0x34, 0x56]); // src
        packet[12..14].copy_from_slice(&[0x08, 0x00]); // IPv4 ethertype

        let reply = responder.process(&packet);
        assert!(reply.is_none(), "Should not process non-ARP packets");
    }
}
