//! ICMPv6 echo responder for gateway address
//!
//! Responds to ICMPv6 echo requests (ping) for the virtual gateway (fe80::1)
//! to allow VMs to verify IPv6 network connectivity.

use smoltcp::wire::{
    EthernetAddress, EthernetFrame, EthernetProtocol, EthernetRepr, Icmpv6Packet, Icmpv6Repr,
    IpAddress, IpProtocol, Ipv6Packet, Ipv6Repr,
};
use tracing::debug;

use super::ndp::GATEWAY_IPV6;
use super::packet::{GATEWAY_MAC, parse_ethernet};

/// ICMPv6 echo responder for the gateway
pub struct Icmpv6Responder {
    /// Gateway MAC address
    gateway_mac: EthernetAddress,
}

impl Icmpv6Responder {
    /// Create a new ICMPv6 responder for the default gateway
    pub fn new() -> Self {
        Self {
            gateway_mac: EthernetAddress::from_bytes(&GATEWAY_MAC),
        }
    }

    /// Process an incoming packet and potentially generate an ICMPv6 echo reply
    ///
    /// Returns `Some(frame)` if an ICMPv6 echo reply should be sent, `None` otherwise.
    pub fn process(&self, packet: &[u8]) -> Option<Vec<u8>> {
        let frame = parse_ethernet(packet)?;

        // Only process IPv6 packets
        if frame.ethertype() != EthernetProtocol::Ipv6 {
            return None;
        }

        let ipv6 = Ipv6Packet::new_checked(frame.payload()).ok()?;

        // Only process ICMPv6 packets destined to the gateway
        if ipv6.next_header() != IpProtocol::Icmpv6 {
            return None;
        }

        if ipv6.dst_addr() != GATEWAY_IPV6 {
            return None;
        }

        let icmp = Icmpv6Packet::new_checked(ipv6.payload()).ok()?;
        let icmp_repr = Icmpv6Repr::parse(
            &IpAddress::Ipv6(ipv6.src_addr()),
            &IpAddress::Ipv6(ipv6.dst_addr()),
            &icmp,
            &smoltcp::phy::ChecksumCapabilities::default(),
        )
        .ok()?;

        // Only respond to echo requests
        if let Icmpv6Repr::EchoRequest {
            ident,
            seq_no,
            data,
        } = icmp_repr
        {
            debug!(
                src_ip = %ipv6.src_addr(),
                dst_ip = %ipv6.dst_addr(),
                ident,
                seq_no,
                "ICMPv6 Echo Request received"
            );

            debug!(
                src_ip = %GATEWAY_IPV6,
                dst_ip = %ipv6.src_addr(),
                ident,
                seq_no,
                "Sending ICMPv6 Echo Reply"
            );

            return Some(self.build_echo_reply(
                frame.src_addr(),
                ipv6.src_addr(),
                ident,
                seq_no,
                data,
            ));
        }

        None
    }

    /// Build an ICMPv6 echo reply frame
    fn build_echo_reply(
        &self,
        dst_mac: EthernetAddress,
        dst_ip: smoltcp::wire::Ipv6Address,
        ident: u16,
        seq_no: u16,
        data: &[u8],
    ) -> Vec<u8> {
        // ICMPv6 reply
        let icmp_repr = Icmpv6Repr::EchoReply {
            ident,
            seq_no,
            data,
        };

        // IPv6 header
        let ipv6_repr = Ipv6Repr {
            src_addr: GATEWAY_IPV6,
            dst_addr: dst_ip,
            next_header: IpProtocol::Icmpv6,
            payload_len: icmp_repr.buffer_len(),
            hop_limit: 64,
        };

        // Ethernet header
        let eth_repr = EthernetRepr {
            src_addr: self.gateway_mac,
            dst_addr: dst_mac,
            ethertype: EthernetProtocol::Ipv6,
        };

        // Calculate total size
        let total_len = eth_repr.buffer_len() + ipv6_repr.buffer_len() + icmp_repr.buffer_len();
        let mut buffer = vec![0u8; total_len];

        // Emit Ethernet frame
        let mut frame = EthernetFrame::new_unchecked(&mut buffer);
        eth_repr.emit(&mut frame);

        // Emit IPv6 packet
        let mut ipv6_packet = Ipv6Packet::new_unchecked(frame.payload_mut());
        ipv6_repr.emit(&mut ipv6_packet);

        // Emit ICMPv6 packet
        let mut icmp_packet = Icmpv6Packet::new_unchecked(ipv6_packet.payload_mut());
        icmp_repr.emit(
            &IpAddress::Ipv6(GATEWAY_IPV6),
            &IpAddress::Ipv6(dst_ip),
            &mut icmp_packet,
            &smoltcp::phy::ChecksumCapabilities::default(),
        );

        buffer
    }
}

impl Default for Icmpv6Responder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use smoltcp::wire::Icmpv6Message;

    fn build_icmpv6_echo_request(
        src_mac: [u8; 6],
        src_ip: [u8; 16],
        dst_ip: [u8; 16],
        ident: u16,
        seq_no: u16,
        data: &[u8],
    ) -> Vec<u8> {
        let src_addr = smoltcp::wire::Ipv6Address::from_bytes(&src_ip);
        let dst_addr = smoltcp::wire::Ipv6Address::from_bytes(&dst_ip);

        let icmp_repr = Icmpv6Repr::EchoRequest {
            ident,
            seq_no,
            data,
        };

        let ipv6_repr = Ipv6Repr {
            src_addr,
            dst_addr,
            next_header: IpProtocol::Icmpv6,
            payload_len: icmp_repr.buffer_len(),
            hop_limit: 64,
        };

        let eth_repr = EthernetRepr {
            src_addr: EthernetAddress::from_bytes(&src_mac),
            dst_addr: EthernetAddress::from_bytes(&GATEWAY_MAC),
            ethertype: EthernetProtocol::Ipv6,
        };

        let total_len = eth_repr.buffer_len() + ipv6_repr.buffer_len() + icmp_repr.buffer_len();
        let mut buffer = vec![0u8; total_len];

        let mut frame = EthernetFrame::new_unchecked(&mut buffer);
        eth_repr.emit(&mut frame);

        let mut ipv6_packet = Ipv6Packet::new_unchecked(frame.payload_mut());
        ipv6_repr.emit(&mut ipv6_packet);

        let mut icmp_packet = Icmpv6Packet::new_unchecked(ipv6_packet.payload_mut());
        icmp_repr.emit(
            &IpAddress::Ipv6(src_addr),
            &IpAddress::Ipv6(dst_addr),
            &mut icmp_packet,
            &smoltcp::phy::ChecksumCapabilities::default(),
        );

        buffer
    }

    #[test]
    fn test_icmpv6_echo_request_to_gateway() {
        let responder = Icmpv6Responder::new();

        // Client link-local address
        let src_ip = [0xfe, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x02];
        // Gateway link-local address
        let dst_ip = [0xfe, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x01];

        let request = build_icmpv6_echo_request(
            [0x52, 0x54, 0x00, 0x12, 0x34, 0x56],
            src_ip,
            dst_ip,
            1234,
            1,
            b"hello",
        );

        let reply = responder.process(&request);
        assert!(
            reply.is_some(),
            "Should generate ICMPv6 echo reply for gateway"
        );

        // Parse the reply
        let reply_data = reply.unwrap();
        let frame = parse_ethernet(&reply_data).unwrap();
        assert_eq!(frame.ethertype(), EthernetProtocol::Ipv6);

        let ipv6 = Ipv6Packet::new_checked(frame.payload()).unwrap();
        assert_eq!(ipv6.src_addr(), GATEWAY_IPV6);
        assert_eq!(
            ipv6.dst_addr(),
            smoltcp::wire::Ipv6Address::from_bytes(&src_ip)
        );
        assert_eq!(ipv6.next_header(), IpProtocol::Icmpv6);

        let icmp = Icmpv6Packet::new_checked(ipv6.payload()).unwrap();
        assert_eq!(icmp.msg_type(), Icmpv6Message::EchoReply);
    }

    #[test]
    fn test_icmpv6_echo_request_to_other_ip() {
        let responder = Icmpv6Responder::new();

        // Client link-local address
        let src_ip = [0xfe, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x02];
        // Some other address (not gateway)
        let dst_ip = [
            0x20, 0x01, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x01,
        ];

        let request = build_icmpv6_echo_request(
            [0x52, 0x54, 0x00, 0x12, 0x34, 0x56],
            src_ip,
            dst_ip,
            1234,
            1,
            b"hello",
        );

        let reply = responder.process(&request);
        assert!(reply.is_none(), "Should not reply to non-gateway ICMPv6");
    }
}
