//! DHCPv4 server for automatic IP assignment
//!
//! Assigns /32 addresses to VMs with gateway 169.254.0.1.
//! Uses the dhcproto library for message parsing/building.

use std::net::Ipv4Addr;

use dhcproto::v4::{DhcpOption, Message, MessageType, Opcode, OptionCode};
use dhcproto::{Decodable, Encodable};
use smoltcp::wire::{
    EthernetAddress, EthernetProtocol, EthernetRepr, IpProtocol, Ipv4Address, Ipv4Packet, Ipv4Repr,
    UdpPacket, UdpRepr,
};
use tracing::debug;

use super::packet::{GATEWAY_MAC, parse_ethernet};

/// DHCP server port
const DHCP_SERVER_PORT: u16 = 67;
/// DHCP client port
const DHCP_CLIENT_PORT: u16 = 68;

/// DHCPv4 server configuration
pub struct Dhcpv4Server {
    /// IP address to assign to this vNIC
    assigned_ip: Ipv4Addr,
    /// Subnet mask (always /32 = 255.255.255.255)
    subnet_mask: Ipv4Addr,
    /// Gateway address
    gateway: Ipv4Addr,
    /// DNS servers
    dns_servers: Vec<Ipv4Addr>,
    /// Lease time in seconds
    lease_time: u32,
    /// Server identifier (gateway IP)
    server_id: Ipv4Addr,
    /// Whether to announce default route (only for public networks)
    announce_default_route: bool,
}

impl Dhcpv4Server {
    /// Create a new DHCPv4 server
    ///
    /// # Arguments
    /// * `assigned_ip` - IPv4 address to assign to the VM
    /// * `is_public` - If true, announces a default route via the gateway.
    ///   If false, no Router option is sent (network isolation).
    pub fn new(assigned_ip: Ipv4Addr, is_public: bool) -> Self {
        Self {
            assigned_ip,
            subnet_mask: Ipv4Addr::new(255, 255, 255, 255), // /32
            gateway: Ipv4Addr::new(169, 254, 0, 1),
            dns_servers: Vec::new(),
            lease_time: 86400, // 24 hours
            server_id: Ipv4Addr::new(169, 254, 0, 1),
            announce_default_route: is_public,
        }
    }

    /// Set DNS servers
    pub fn set_dns_servers(&mut self, servers: Vec<Ipv4Addr>) {
        self.dns_servers = servers;
    }

    /// Process an incoming packet and potentially generate a DHCP response
    pub fn process(&self, packet: &[u8], client_mac: [u8; 6]) -> Option<Vec<u8>> {
        let frame = parse_ethernet(packet)?;

        // Only process IPv4 packets
        if frame.ethertype() != EthernetProtocol::Ipv4 {
            return None;
        }

        let ipv4 = Ipv4Packet::new_checked(frame.payload()).ok()?;

        // Only process UDP packets
        if ipv4.next_header() != IpProtocol::Udp {
            return None;
        }

        let udp = UdpPacket::new_checked(ipv4.payload()).ok()?;

        // Only process DHCP server port
        if udp.dst_port() != DHCP_SERVER_PORT {
            return None;
        }

        // Parse DHCP message
        let mut decoder = dhcproto::decoder::Decoder::new(udp.payload());
        let dhcp_msg = Message::decode(&mut decoder).ok()?;

        // Only process requests (BOOTREQUEST)
        if dhcp_msg.opcode() != Opcode::BootRequest {
            return None;
        }

        // Get message type
        let msg_type = dhcp_msg
            .opts()
            .get(OptionCode::MessageType)
            .and_then(|opt| {
                if let DhcpOption::MessageType(t) = opt {
                    Some(t)
                } else {
                    None
                }
            })?;

        let mac_str = format!(
            "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            client_mac[0],
            client_mac[1],
            client_mac[2],
            client_mac[3],
            client_mac[4],
            client_mac[5]
        );

        debug!(
            msg_type = ?msg_type,
            xid = %format!("{:08x}", dhcp_msg.xid()),
            client_mac = %mac_str,
            "DHCPv4 request received"
        );

        match msg_type {
            MessageType::Discover => self.handle_discover(&dhcp_msg, client_mac),
            MessageType::Request => self.handle_request(&dhcp_msg, client_mac),
            _ => None,
        }
    }

    /// Handle DHCPDISCOVER -> send DHCPOFFER
    fn handle_discover(&self, request: &Message, client_mac: [u8; 6]) -> Option<Vec<u8>> {
        debug!(
            xid = %format!("{:08x}", request.xid()),
            offered_ip = %self.assigned_ip,
            "Sending DHCPv4 OFFER"
        );

        let response = self.build_response(request, MessageType::Offer)?;
        Some(self.wrap_in_udp_ip_eth(response, client_mac))
    }

    /// Handle DHCPREQUEST -> send DHCPACK
    fn handle_request(&self, request: &Message, client_mac: [u8; 6]) -> Option<Vec<u8>> {
        // Verify the request is for us (check server identifier if present)
        if let Some(DhcpOption::ServerIdentifier(server_id)) =
            request.opts().get(OptionCode::ServerIdentifier)
            && *server_id != self.server_id
        {
            return None; // Not for us
        }

        debug!(
            xid = %format!("{:08x}", request.xid()),
            assigned_ip = %self.assigned_ip,
            "Sending DHCPv4 ACK"
        );

        let response = self.build_response(request, MessageType::Ack)?;
        Some(self.wrap_in_udp_ip_eth(response, client_mac))
    }

    /// Build a DHCP response message
    fn build_response(&self, request: &Message, msg_type: MessageType) -> Option<Vec<u8>> {
        let mut response = Message::default();

        response.set_opcode(Opcode::BootReply);
        response.set_htype(request.htype());
        response.set_xid(request.xid());
        response.set_flags(request.flags());
        response.set_yiaddr(self.assigned_ip);
        response.set_siaddr(self.server_id);
        response.set_chaddr(request.chaddr());

        // Set options
        response
            .opts_mut()
            .insert(DhcpOption::MessageType(msg_type));
        response
            .opts_mut()
            .insert(DhcpOption::ServerIdentifier(self.server_id));
        response
            .opts_mut()
            .insert(DhcpOption::AddressLeaseTime(self.lease_time));
        response
            .opts_mut()
            .insert(DhcpOption::SubnetMask(self.subnet_mask));

        // Only announce default route for public networks
        // Non-public networks are isolated - no gateway/default route
        if self.announce_default_route {
            response
                .opts_mut()
                .insert(DhcpOption::Router(vec![self.gateway]));
        }

        if !self.dns_servers.is_empty() {
            response
                .opts_mut()
                .insert(DhcpOption::DomainNameServer(self.dns_servers.clone()));
        }

        response.to_vec().ok()
    }

    /// Wrap DHCP message in UDP/IP/Ethernet headers
    fn wrap_in_udp_ip_eth(&self, dhcp_data: Vec<u8>, client_mac: [u8; 6]) -> Vec<u8> {
        let gateway_mac = EthernetAddress::from_bytes(&GATEWAY_MAC);
        let client_eth = EthernetAddress::from_bytes(&client_mac);

        // Build UDP header
        let udp_repr = UdpRepr {
            src_port: DHCP_SERVER_PORT,
            dst_port: DHCP_CLIENT_PORT,
        };

        // Build IPv4 header (broadcast to client)
        let ipv4_repr = Ipv4Repr {
            src_addr: Ipv4Address::from_octets(self.server_id.octets()),
            dst_addr: Ipv4Address::BROADCAST,
            next_header: IpProtocol::Udp,
            payload_len: udp_repr.header_len() + dhcp_data.len(),
            hop_limit: 64,
        };

        // Build Ethernet header
        let eth_repr = EthernetRepr {
            src_addr: gateway_mac,
            dst_addr: client_eth,
            ethertype: EthernetProtocol::Ipv4,
        };

        let total_len = eth_repr.buffer_len()
            + ipv4_repr.buffer_len()
            + udp_repr.header_len()
            + dhcp_data.len();

        let mut buffer = vec![0u8; total_len];

        // Emit Ethernet
        let mut eth_frame = smoltcp::wire::EthernetFrame::new_unchecked(&mut buffer);
        eth_repr.emit(&mut eth_frame);

        // Emit IPv4
        let mut ipv4_packet = Ipv4Packet::new_unchecked(eth_frame.payload_mut());
        ipv4_repr.emit(
            &mut ipv4_packet,
            &smoltcp::phy::ChecksumCapabilities::default(),
        );

        // Emit UDP
        let mut udp_packet = UdpPacket::new_unchecked(ipv4_packet.payload_mut());
        udp_repr.emit(
            &mut udp_packet,
            &ipv4_repr.src_addr.into(),
            &ipv4_repr.dst_addr.into(),
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
    fn test_dhcpv4_server_new() {
        let server = Dhcpv4Server::new(Ipv4Addr::new(10, 0, 0, 5), true);
        assert_eq!(server.assigned_ip, Ipv4Addr::new(10, 0, 0, 5));
        assert_eq!(server.subnet_mask, Ipv4Addr::new(255, 255, 255, 255));
        assert_eq!(server.gateway, Ipv4Addr::new(169, 254, 0, 1));
        assert!(server.announce_default_route);
    }

    #[test]
    fn test_dhcpv4_server_non_public() {
        let server = Dhcpv4Server::new(Ipv4Addr::new(10, 0, 0, 5), false);
        assert!(!server.announce_default_route);
    }

    #[test]
    fn test_build_response() {
        let server = Dhcpv4Server::new(Ipv4Addr::new(10, 0, 0, 5), true);

        // Build a minimal DHCP discover
        let mut request = Message::default();
        request.set_opcode(Opcode::BootRequest);
        request.set_xid(0x12345678);
        request.set_chaddr(&[
            0x52, 0x54, 0x00, 0x12, 0x34, 0x56, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ]);
        request
            .opts_mut()
            .insert(DhcpOption::MessageType(MessageType::Discover));

        let response_bytes = server.build_response(&request, MessageType::Offer).unwrap();
        assert!(!response_bytes.is_empty());

        // Parse the response
        let mut decoder = dhcproto::decoder::Decoder::new(&response_bytes);
        let response = Message::decode(&mut decoder).unwrap();
        assert_eq!(response.opcode(), Opcode::BootReply);
        assert_eq!(response.xid(), 0x12345678);
        assert_eq!(response.yiaddr(), Ipv4Addr::new(10, 0, 0, 5));
    }

    #[test]
    fn test_set_dns_servers() {
        let mut server = Dhcpv4Server::new(Ipv4Addr::new(10, 0, 0, 5), true);
        server.set_dns_servers(vec![Ipv4Addr::new(9, 9, 9, 9)]);
        assert_eq!(server.dns_servers, vec![Ipv4Addr::new(9, 9, 9, 9)]);
    }
}
