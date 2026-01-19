//! ICMP echo responder for gateway address
//!
//! Responds to ICMP echo requests (ping) for the virtual gateway (169.254.0.1)
//! to allow VMs to verify network connectivity.

use smoltcp::wire::{
    EthernetAddress, EthernetProtocol, Icmpv4Packet, Icmpv4Repr, IpProtocol, Ipv4Address,
    Ipv4Packet,
};
use tracing::debug;

use super::packet::{GATEWAY_IPV4, GATEWAY_MAC, parse_ethernet};

/// ICMP echo responder for the gateway
pub struct IcmpResponder {
    /// Gateway MAC address
    gateway_mac: EthernetAddress,
    /// Gateway IPv4 address
    gateway_ip: Ipv4Address,
}

impl IcmpResponder {
    /// Create a new ICMP responder for the default gateway
    pub fn new() -> Self {
        Self {
            gateway_mac: EthernetAddress::from_bytes(&GATEWAY_MAC),
            gateway_ip: GATEWAY_IPV4,
        }
    }

    /// Process an incoming packet and potentially generate an ICMP echo reply
    ///
    /// Returns `Some(frame)` if an ICMP echo reply should be sent, `None` otherwise.
    pub fn process(&self, packet: &[u8]) -> Option<Vec<u8>> {
        let frame = parse_ethernet(packet)?;

        // Only process IPv4 packets
        if frame.ethertype() != EthernetProtocol::Ipv4 {
            return None;
        }

        let ipv4 = Ipv4Packet::new_checked(frame.payload()).ok()?;

        // Only process ICMP packets destined to the gateway
        if ipv4.next_header() != IpProtocol::Icmp {
            return None;
        }

        if ipv4.dst_addr() != self.gateway_ip {
            return None;
        }

        let icmp = Icmpv4Packet::new_checked(ipv4.payload()).ok()?;
        let icmp_repr =
            Icmpv4Repr::parse(&icmp, &smoltcp::phy::ChecksumCapabilities::default()).ok()?;

        // Only respond to echo requests
        if let Icmpv4Repr::EchoRequest {
            ident,
            seq_no,
            data,
        } = icmp_repr
        {
            debug!(
                src_ip = %ipv4.src_addr(),
                dst_ip = %ipv4.dst_addr(),
                ident,
                seq_no,
                "ICMPv4 Echo Request received"
            );

            debug!(
                src_ip = %self.gateway_ip,
                dst_ip = %ipv4.src_addr(),
                ident,
                seq_no,
                "Sending ICMPv4 Echo Reply"
            );

            return Some(self.build_echo_reply(
                frame.src_addr(),
                ipv4.src_addr(),
                ident,
                seq_no,
                data,
            ));
        }

        None
    }

    /// Build an ICMP echo reply frame
    fn build_echo_reply(
        &self,
        dst_mac: EthernetAddress,
        dst_ip: Ipv4Address,
        ident: u16,
        seq_no: u16,
        data: &[u8],
    ) -> Vec<u8> {
        use smoltcp::wire::{EthernetFrame, EthernetRepr, Ipv4Repr};

        // ICMP reply
        let icmp_repr = Icmpv4Repr::EchoReply {
            ident,
            seq_no,
            data,
        };

        // IPv4 header
        let ipv4_repr = Ipv4Repr {
            src_addr: self.gateway_ip,
            dst_addr: dst_ip,
            next_header: IpProtocol::Icmp,
            payload_len: icmp_repr.buffer_len(),
            hop_limit: 64,
        };

        // Ethernet header
        let eth_repr = EthernetRepr {
            src_addr: self.gateway_mac,
            dst_addr: dst_mac,
            ethertype: EthernetProtocol::Ipv4,
        };

        // Calculate total size
        let total_len = eth_repr.buffer_len() + ipv4_repr.buffer_len() + icmp_repr.buffer_len();
        let mut buffer = vec![0u8; total_len];

        // Emit Ethernet frame
        let mut frame = EthernetFrame::new_unchecked(&mut buffer);
        eth_repr.emit(&mut frame);

        // Emit IPv4 packet
        let mut ipv4_packet = Ipv4Packet::new_unchecked(frame.payload_mut());
        ipv4_repr.emit(
            &mut ipv4_packet,
            &smoltcp::phy::ChecksumCapabilities::default(),
        );

        // Emit ICMP packet
        let mut icmp_packet = Icmpv4Packet::new_unchecked(ipv4_packet.payload_mut());
        icmp_repr.emit(
            &mut icmp_packet,
            &smoltcp::phy::ChecksumCapabilities::default(),
        );

        buffer
    }
}

impl Default for IcmpResponder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use smoltcp::wire::{EthernetFrame, EthernetRepr, Icmpv4Message, Ipv4Repr};

    fn build_icmp_echo_request(
        src_mac: [u8; 6],
        src_ip: [u8; 4],
        dst_ip: [u8; 4],
        ident: u16,
        seq_no: u16,
        data: &[u8],
    ) -> Vec<u8> {
        let icmp_repr = Icmpv4Repr::EchoRequest {
            ident,
            seq_no,
            data,
        };

        let ipv4_repr = Ipv4Repr {
            src_addr: Ipv4Address::from_octets(src_ip),
            dst_addr: Ipv4Address::from_octets(dst_ip),
            next_header: IpProtocol::Icmp,
            payload_len: icmp_repr.buffer_len(),
            hop_limit: 64,
        };

        let eth_repr = EthernetRepr {
            src_addr: EthernetAddress::from_bytes(&src_mac),
            dst_addr: EthernetAddress::from_bytes(&GATEWAY_MAC),
            ethertype: EthernetProtocol::Ipv4,
        };

        let total_len = eth_repr.buffer_len() + ipv4_repr.buffer_len() + icmp_repr.buffer_len();
        let mut buffer = vec![0u8; total_len];

        let mut frame = EthernetFrame::new_unchecked(&mut buffer);
        eth_repr.emit(&mut frame);

        let mut ipv4_packet = Ipv4Packet::new_unchecked(frame.payload_mut());
        ipv4_repr.emit(
            &mut ipv4_packet,
            &smoltcp::phy::ChecksumCapabilities::default(),
        );

        let mut icmp_packet = Icmpv4Packet::new_unchecked(ipv4_packet.payload_mut());
        icmp_repr.emit(
            &mut icmp_packet,
            &smoltcp::phy::ChecksumCapabilities::default(),
        );

        buffer
    }

    #[test]
    fn test_icmp_echo_request_to_gateway() {
        let responder = IcmpResponder::new();

        // Build ICMP echo request to gateway
        let request = build_icmp_echo_request(
            [0x52, 0x54, 0x00, 0x12, 0x34, 0x56],
            [10, 0, 0, 5],
            [169, 254, 0, 1], // Gateway IP
            1234,
            1,
            b"hello",
        );

        let reply = responder.process(&request);
        assert!(
            reply.is_some(),
            "Should generate ICMP echo reply for gateway"
        );

        // Parse the reply
        let reply_data = reply.unwrap();
        let frame = parse_ethernet(&reply_data).unwrap();
        assert_eq!(frame.ethertype(), EthernetProtocol::Ipv4);

        let ipv4 = Ipv4Packet::new_checked(frame.payload()).unwrap();
        assert_eq!(ipv4.src_addr(), GATEWAY_IPV4);
        assert_eq!(ipv4.dst_addr(), Ipv4Address::new(10, 0, 0, 5));
        assert_eq!(ipv4.next_header(), IpProtocol::Icmp);

        let icmp = Icmpv4Packet::new_checked(ipv4.payload()).unwrap();
        assert_eq!(icmp.msg_type(), Icmpv4Message::EchoReply);
    }

    #[test]
    fn test_icmp_echo_request_to_other_ip() {
        let responder = IcmpResponder::new();

        // Build ICMP echo request to some other IP
        let request = build_icmp_echo_request(
            [0x52, 0x54, 0x00, 0x12, 0x34, 0x56],
            [10, 0, 0, 5],
            [8, 8, 8, 8], // Not gateway
            1234,
            1,
            b"hello",
        );

        let reply = responder.process(&request);
        assert!(reply.is_none(), "Should not reply to non-gateway ICMP");
    }

    #[test]
    fn test_non_icmp_packet() {
        let responder = IcmpResponder::new();

        // Build a non-ICMP IPv4 packet (TCP)
        let eth_repr = EthernetRepr {
            src_addr: EthernetAddress::from_bytes(&[0x52, 0x54, 0x00, 0x12, 0x34, 0x56]),
            dst_addr: EthernetAddress::from_bytes(&GATEWAY_MAC),
            ethertype: EthernetProtocol::Ipv4,
        };

        let ipv4_repr = Ipv4Repr {
            src_addr: Ipv4Address::new(10, 0, 0, 5),
            dst_addr: GATEWAY_IPV4,
            next_header: IpProtocol::Tcp,
            payload_len: 20,
            hop_limit: 64,
        };

        let mut buffer = vec![0u8; eth_repr.buffer_len() + ipv4_repr.buffer_len() + 20];
        let mut frame = EthernetFrame::new_unchecked(&mut buffer);
        eth_repr.emit(&mut frame);

        let mut ipv4_packet = Ipv4Packet::new_unchecked(frame.payload_mut());
        ipv4_repr.emit(
            &mut ipv4_packet,
            &smoltcp::phy::ChecksumCapabilities::default(),
        );

        let reply = responder.process(&buffer);
        assert!(reply.is_none(), "Should not process non-ICMP packets");
    }
}
