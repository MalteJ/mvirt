//! DHCPv6 server for automatic IPv6 address assignment
//!
//! Assigns /128 addresses to VMs. Works with Router Advertisements
//! that have M=1, O=1 flags set.

use std::net::Ipv6Addr;

use dhcproto::v6::{DhcpOption, IAAddr, IANA, Message, MessageType, OptionCode};
use dhcproto::{Decodable, Encodable};
use smoltcp::wire::{
    EthernetAddress, EthernetProtocol, EthernetRepr, IpProtocol, Ipv6Address, Ipv6Packet, Ipv6Repr,
    UdpPacket, UdpRepr,
};
use tracing::debug;

use super::ndp::GATEWAY_IPV6;
use super::packet::{GATEWAY_MAC, parse_ethernet};

/// DHCPv6 server port
const DHCPV6_SERVER_PORT: u16 = 547;
/// DHCPv6 client port
const DHCPV6_CLIENT_PORT: u16 = 546;

/// DHCPv6 server configuration
pub struct Dhcpv6Server {
    /// IPv6 address to assign to this vNIC
    assigned_ip: Ipv6Addr,
    /// Preferred lifetime in seconds
    preferred_lifetime: u32,
    /// Valid lifetime in seconds
    valid_lifetime: u32,
    /// DNS servers
    dns_servers: Vec<Ipv6Addr>,
    /// Server DUID (using link-layer address)
    server_duid: Vec<u8>,
}

impl Dhcpv6Server {
    /// Create a new DHCPv6 server
    pub fn new(assigned_ip: Ipv6Addr) -> Self {
        // Build server DUID (DUID-LL: type 3, hardware type 1 for Ethernet)
        let mut server_duid = vec![0, 3, 0, 1]; // DUID-LL type + hardware type
        server_duid.extend_from_slice(&GATEWAY_MAC);

        Self {
            assigned_ip,
            preferred_lifetime: 14400, // 4 hours
            valid_lifetime: 86400,     // 24 hours
            dns_servers: Vec::new(),
            server_duid,
        }
    }

    /// Set DNS servers
    pub fn set_dns_servers(&mut self, servers: Vec<Ipv6Addr>) {
        self.dns_servers = servers;
    }

    /// Process an incoming packet and potentially generate a DHCPv6 response
    pub fn process(&self, packet: &[u8], client_mac: [u8; 6]) -> Option<Vec<u8>> {
        let frame = parse_ethernet(packet)?;

        // Only process IPv6 packets
        if frame.ethertype() != EthernetProtocol::Ipv6 {
            return None;
        }

        let ipv6 = Ipv6Packet::new_checked(frame.payload()).ok()?;

        // Only process UDP packets
        if ipv6.next_header() != IpProtocol::Udp {
            return None;
        }

        let udp = UdpPacket::new_checked(ipv6.payload()).ok()?;

        // Only process DHCPv6 server port
        if udp.dst_port() != DHCPV6_SERVER_PORT {
            return None;
        }

        // Parse DHCPv6 message
        let mut decoder = dhcproto::decoder::Decoder::new(udp.payload());
        let dhcp_msg = Message::decode(&mut decoder).ok()?;
        let src_addr = ipv6.src_addr();

        let mac_str = format!(
            "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            client_mac[0],
            client_mac[1],
            client_mac[2],
            client_mac[3],
            client_mac[4],
            client_mac[5]
        );
        let xid_str = format!(
            "{:02x}{:02x}{:02x}",
            dhcp_msg.xid()[0],
            dhcp_msg.xid()[1],
            dhcp_msg.xid()[2]
        );

        debug!(
            msg_type = ?dhcp_msg.msg_type(),
            xid = %xid_str,
            client_mac = %mac_str,
            client_ip = %src_addr,
            "DHCPv6 request received"
        );

        match dhcp_msg.msg_type() {
            MessageType::Solicit => self.handle_solicit(&dhcp_msg, client_mac, src_addr),
            MessageType::Request => self.handle_request(&dhcp_msg, client_mac, src_addr),
            MessageType::Renew => self.handle_renew(&dhcp_msg, client_mac, src_addr),
            MessageType::Rebind => self.handle_rebind(&dhcp_msg, client_mac, src_addr),
            _ => None,
        }
    }

    /// Handle SOLICIT -> send ADVERTISE
    fn handle_solicit(
        &self,
        request: &Message,
        client_mac: [u8; 6],
        client_addr: Ipv6Address,
    ) -> Option<Vec<u8>> {
        let xid = request.xid();
        debug!(
            xid = %format!("{:02x}{:02x}{:02x}", xid[0], xid[1], xid[2]),
            offered_ip = %self.assigned_ip,
            "Sending DHCPv6 ADVERTISE"
        );

        let response = self.build_response(request, MessageType::Advertise)?;
        Some(self.wrap_in_udp_ip_eth(response, client_mac, client_addr))
    }

    /// Handle REQUEST -> send REPLY
    fn handle_request(
        &self,
        request: &Message,
        client_mac: [u8; 6],
        client_addr: Ipv6Address,
    ) -> Option<Vec<u8>> {
        let xid = request.xid();
        debug!(
            xid = %format!("{:02x}{:02x}{:02x}", xid[0], xid[1], xid[2]),
            assigned_ip = %self.assigned_ip,
            "Sending DHCPv6 REPLY"
        );

        let response = self.build_response(request, MessageType::Reply)?;
        Some(self.wrap_in_udp_ip_eth(response, client_mac, client_addr))
    }

    /// Handle RENEW -> send REPLY
    fn handle_renew(
        &self,
        request: &Message,
        client_mac: [u8; 6],
        client_addr: Ipv6Address,
    ) -> Option<Vec<u8>> {
        let xid = request.xid();
        debug!(
            xid = %format!("{:02x}{:02x}{:02x}", xid[0], xid[1], xid[2]),
            renewed_ip = %self.assigned_ip,
            "Sending DHCPv6 REPLY (renew)"
        );

        let response = self.build_response(request, MessageType::Reply)?;
        Some(self.wrap_in_udp_ip_eth(response, client_mac, client_addr))
    }

    /// Handle REBIND -> send REPLY
    fn handle_rebind(
        &self,
        request: &Message,
        client_mac: [u8; 6],
        client_addr: Ipv6Address,
    ) -> Option<Vec<u8>> {
        let xid = request.xid();
        debug!(
            xid = %format!("{:02x}{:02x}{:02x}", xid[0], xid[1], xid[2]),
            rebound_ip = %self.assigned_ip,
            "Sending DHCPv6 REPLY (rebind)"
        );

        let response = self.build_response(request, MessageType::Reply)?;
        Some(self.wrap_in_udp_ip_eth(response, client_mac, client_addr))
    }

    /// Build a DHCPv6 response message
    fn build_response(&self, request: &Message, msg_type: MessageType) -> Option<Vec<u8>> {
        let mut response = Message::new(msg_type);

        // Copy transaction ID from request
        response.set_xid(request.xid());

        // Add server DUID
        response
            .opts_mut()
            .insert(DhcpOption::ServerId(self.server_duid.clone()));

        // Copy client DUID from request
        if let Some(DhcpOption::ClientId(client_duid)) = request.opts().get(OptionCode::ClientId) {
            response
                .opts_mut()
                .insert(DhcpOption::ClientId(client_duid.clone()));
        }

        // Extract IAID from client's IA_NA request (must echo it back)
        let client_iaid = request
            .opts()
            .get(OptionCode::IANA)
            .and_then(|opt| {
                if let DhcpOption::IANA(iana) = opt {
                    Some(iana.id)
                } else {
                    None
                }
            })
            .unwrap_or(1);

        // Add IA_NA with address
        let ia_addr = IAAddr {
            addr: self.assigned_ip,
            preferred_life: self.preferred_lifetime,
            valid_life: self.valid_lifetime,
            opts: Default::default(),
        };

        let iana = IANA {
            id: client_iaid,                       // Echo client's IAID
            t1: self.preferred_lifetime / 2,       // Renew at 50%
            t2: (self.preferred_lifetime * 4) / 5, // Rebind at 80%
            opts: vec![DhcpOption::IAAddr(ia_addr)].into_iter().collect(),
        };

        response.opts_mut().insert(DhcpOption::IANA(iana));

        // Add DNS servers using the RecursiveNameServer option (option 23)
        if !self.dns_servers.is_empty() {
            response
                .opts_mut()
                .insert(DhcpOption::DomainNameServers(self.dns_servers.clone()));
        }

        response.to_vec().ok()
    }

    /// Wrap DHCPv6 message in UDP/IPv6/Ethernet headers
    fn wrap_in_udp_ip_eth(
        &self,
        dhcp_data: Vec<u8>,
        client_mac: [u8; 6],
        client_addr: Ipv6Address,
    ) -> Vec<u8> {
        let gateway_mac = EthernetAddress::from_bytes(&GATEWAY_MAC);
        let client_eth = EthernetAddress::from_bytes(&client_mac);

        // Build UDP header
        let udp_repr = UdpRepr {
            src_port: DHCPV6_SERVER_PORT,
            dst_port: DHCPV6_CLIENT_PORT,
        };

        // Build IPv6 header
        let ipv6_repr = Ipv6Repr {
            src_addr: GATEWAY_IPV6,
            dst_addr: client_addr,
            next_header: IpProtocol::Udp,
            payload_len: udp_repr.header_len() + dhcp_data.len(),
            hop_limit: 64,
        };

        // Build Ethernet header
        let eth_repr = EthernetRepr {
            src_addr: gateway_mac,
            dst_addr: client_eth,
            ethertype: EthernetProtocol::Ipv6,
        };

        let total_len = eth_repr.buffer_len()
            + ipv6_repr.buffer_len()
            + udp_repr.header_len()
            + dhcp_data.len();

        let mut buffer = vec![0u8; total_len];

        // Emit Ethernet
        let mut eth_frame = smoltcp::wire::EthernetFrame::new_unchecked(&mut buffer);
        eth_repr.emit(&mut eth_frame);

        // Emit IPv6
        let mut ipv6_packet = Ipv6Packet::new_unchecked(eth_frame.payload_mut());
        ipv6_repr.emit(&mut ipv6_packet);

        // Emit UDP
        let mut udp_packet = UdpPacket::new_unchecked(ipv6_packet.payload_mut());
        udp_repr.emit(
            &mut udp_packet,
            &smoltcp::wire::IpAddress::Ipv6(GATEWAY_IPV6),
            &smoltcp::wire::IpAddress::Ipv6(client_addr),
            dhcp_data.len(),
            |buf| buf.copy_from_slice(&dhcp_data),
            &smoltcp::phy::ChecksumCapabilities::default(),
        );

        buffer
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dhcpv6_server_new() {
        let ip = Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 5);
        let server = Dhcpv6Server::new(ip);
        assert_eq!(server.assigned_ip, ip);
        assert!(!server.server_duid.is_empty());
    }

    #[test]
    fn test_build_response() {
        let ip = Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 5);
        let server = Dhcpv6Server::new(ip);

        // Build a minimal DHCPv6 solicit
        let mut request = Message::new(MessageType::Solicit);
        request.set_xid([0x12, 0x34, 0x56]);
        request.opts_mut().insert(DhcpOption::ClientId(vec![
            0, 1, 0, 1, 0x52, 0x54, 0x00, 0x12, 0x34, 0x56,
        ]));

        let response_bytes = server
            .build_response(&request, MessageType::Advertise)
            .unwrap();
        assert!(!response_bytes.is_empty());

        // Parse the response
        let mut decoder = dhcproto::decoder::Decoder::new(&response_bytes);
        let response = Message::decode(&mut decoder).unwrap();
        assert_eq!(response.msg_type(), MessageType::Advertise);
        assert_eq!(response.xid(), [0x12, 0x34, 0x56]);
    }

    #[test]
    fn test_set_dns_servers() {
        let ip = Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 5);
        let mut server = Dhcpv6Server::new(ip);
        let dns = Ipv6Addr::new(0x2001, 0x4860, 0x4860, 0, 0, 0, 0, 0x8844);
        server.set_dns_servers(vec![dns]);
        assert_eq!(server.dns_servers, vec![dns]);
    }
}
