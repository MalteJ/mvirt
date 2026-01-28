//! IPv6 protocol unit tests.
//!
//! Tests the full IPv6 network stack:
//! 1. RS (Router Solicitation) → RA (Router Advertisement) with M+O flags
//! 2. DHCPv6 SOLICIT → ADVERTISE with DNS servers
//! 3. DHCPv6 REQUEST → REPLY with assigned address
//! 4. NS (Neighbor Solicitation) → NA (Neighbor Advertisement)
//! 5. ICMPv6 Echo Request → Echo Reply (ping6 to gateway)
//!
//! These tests verify the packet processing logic directly without needing
//! CAP_NET_ADMIN or actual TAP devices.

use mvirt_ebpf::process_packet_sync;
use mvirt_ebpf::test_util::{
    Dhcpv6MessageType, create_dhcpv6_request, create_dhcpv6_solicit, create_icmpv6_echo_request,
    create_neighbor_solicitation, create_router_solicitation, generate_duid_ll,
    parse_dhcpv6_response, parse_icmpv6_echo_reply, parse_neighbor_advertisement,
    parse_router_advertisement, test_network_config_ipv6, test_nic_config_ipv6,
};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Test MAC address for VM
const TEST_MAC: [u8; 6] = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];

/// Gateway MAC address
const GATEWAY_MAC: [u8; 6] = [0x02, 0x00, 0x00, 0x00, 0x00, 0x01];

/// Test IPv4 address for VM
const TEST_IPV4: Ipv4Addr = Ipv4Addr::new(10, 0, 0, 100);

/// Test IPv6 address for VM
const TEST_IPV6: Ipv6Addr = Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 0x100);

/// Gateway IPv6 link-local address
const GATEWAY_IPV6_LL: Ipv6Addr = Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1);

/// Test network IPv4 subnet
const IPV4_SUBNET: &str = "10.0.0.0/24";

/// Test network IPv6 prefix
const IPV6_PREFIX: &str = "2001:db8::/64";

/// DNS servers for testing
fn test_dns_servers() -> Vec<IpAddr> {
    vec![
        "2001:4860:4860::8888".parse().unwrap(), // Google DNS IPv6
        "2001:4860:4860::8844".parse().unwrap(),
    ]
}

/// Test RS → RA with M+O flags.
#[test]
fn test_rs_ra_with_flags() {
    let network = test_network_config_ipv6(
        IPV4_SUBNET.parse().unwrap(),
        IPV6_PREFIX.parse().unwrap(),
        test_dns_servers(),
    );

    let nic = test_nic_config_ipv6(TEST_MAC, TEST_IPV4, TEST_IPV6, network.id);

    // Create Router Solicitation packet
    let rs = create_router_solicitation(TEST_MAC);

    // Process the packet
    let response = process_packet_sync(&nic, &network, &rs).expect("Should get RA for RS");

    // Parse and verify the RA
    let ra = parse_router_advertisement(&response).expect("Should parse as RA");

    // Verify M flag is set (VMs must use DHCPv6)
    assert!(ra.managed_flag, "M flag should be set");

    // Verify O flag is set when DNS servers are configured
    assert!(
        ra.other_flag,
        "O flag should be set when DNS servers are configured"
    );

    // Verify router lifetime is non-zero
    assert!(ra.router_lifetime > 0, "Router lifetime should be non-zero");

    // Verify router MAC
    assert_eq!(ra.router_mac, Some(GATEWAY_MAC), "Router MAC should match");
}

/// Test RS → RA without O flag when no DNS servers.
#[test]
fn test_rs_ra_without_dns() {
    let network = test_network_config_ipv6(
        IPV4_SUBNET.parse().unwrap(),
        IPV6_PREFIX.parse().unwrap(),
        vec![], // No DNS servers
    );

    let nic = test_nic_config_ipv6(TEST_MAC, TEST_IPV4, TEST_IPV6, network.id);

    let rs = create_router_solicitation(TEST_MAC);
    let response = process_packet_sync(&nic, &network, &rs).expect("Should get RA for RS");
    let ra = parse_router_advertisement(&response).expect("Should parse as RA");

    // M flag should still be set
    assert!(ra.managed_flag, "M flag should be set");

    // O flag should NOT be set when no DNS servers
    assert!(
        !ra.other_flag,
        "O flag should NOT be set when no DNS servers"
    );
}

/// Test DHCPv6 SOLICIT → ADVERTISE.
#[test]
fn test_dhcpv6_solicit_advertise() {
    let network = test_network_config_ipv6(
        IPV4_SUBNET.parse().unwrap(),
        IPV6_PREFIX.parse().unwrap(),
        test_dns_servers(),
    );

    let nic = test_nic_config_ipv6(TEST_MAC, TEST_IPV4, TEST_IPV6, network.id);

    // Create DHCPv6 SOLICIT
    let client_duid = generate_duid_ll(TEST_MAC);
    let solicit = create_dhcpv6_solicit(TEST_MAC, &client_duid);

    // Process the packet
    let response =
        process_packet_sync(&nic, &network, &solicit).expect("Should get ADVERTISE for SOLICIT");

    // Parse and verify the ADVERTISE
    let advertise = parse_dhcpv6_response(&response).expect("Should parse as DHCPv6 response");

    assert_eq!(
        advertise.msg_type,
        Dhcpv6MessageType::Advertise,
        "Expected ADVERTISE"
    );

    // Verify offered address
    assert!(!advertise.addresses.is_empty(), "Should offer an address");
    assert_eq!(
        advertise.addresses[0], TEST_IPV6,
        "Offered address should match configured"
    );

    // Verify server DUID is present
    assert!(!advertise.server_duid.is_empty(), "Should have server DUID");

    // Verify DNS servers
    assert!(
        !advertise.dns_servers.is_empty(),
        "Should include DNS servers"
    );
    assert_eq!(advertise.dns_servers.len(), 2, "Should have 2 DNS servers");
}

/// Test DHCPv6 REQUEST → REPLY.
#[test]
fn test_dhcpv6_request_reply() {
    let network = test_network_config_ipv6(
        IPV4_SUBNET.parse().unwrap(),
        IPV6_PREFIX.parse().unwrap(),
        test_dns_servers(),
    );

    let nic = test_nic_config_ipv6(TEST_MAC, TEST_IPV4, TEST_IPV6, network.id);

    // First, get ADVERTISE
    let client_duid = generate_duid_ll(TEST_MAC);
    let solicit = create_dhcpv6_solicit(TEST_MAC, &client_duid);
    let response =
        process_packet_sync(&nic, &network, &solicit).expect("Should get ADVERTISE for SOLICIT");
    let advertise = parse_dhcpv6_response(&response).expect("Should parse ADVERTISE");

    // Now send REQUEST
    let request = create_dhcpv6_request(
        TEST_MAC,
        &client_duid,
        &advertise.server_duid,
        advertise.addresses[0],
        1, // IAID
    );

    let response =
        process_packet_sync(&nic, &network, &request).expect("Should get REPLY for REQUEST");
    let reply = parse_dhcpv6_response(&response).expect("Should parse REPLY");

    assert_eq!(reply.msg_type, Dhcpv6MessageType::Reply, "Expected REPLY");

    // Verify assigned address
    assert!(!reply.addresses.is_empty(), "Should assign an address");
    assert_eq!(
        reply.addresses[0], TEST_IPV6,
        "Assigned address should match requested"
    );

    // Verify DNS servers in reply
    assert!(!reply.dns_servers.is_empty(), "Should include DNS servers");
    for expected_dns in test_dns_servers().iter().filter_map(|ip| {
        if let IpAddr::V6(v6) = ip {
            Some(*v6)
        } else {
            None
        }
    }) {
        assert!(
            reply.dns_servers.contains(&expected_dns),
            "DNS server {} should be in reply",
            expected_dns
        );
    }
}

/// Full DHCPv6 handshake: RS → RA → SOLICIT → ADVERTISE → REQUEST → REPLY.
#[test]
fn test_dhcpv6_full_handshake() {
    let network = test_network_config_ipv6(
        IPV4_SUBNET.parse().unwrap(),
        IPV6_PREFIX.parse().unwrap(),
        test_dns_servers(),
    );

    let nic = test_nic_config_ipv6(TEST_MAC, TEST_IPV4, TEST_IPV6, network.id);

    // === RS → RA ===
    let rs = create_router_solicitation(TEST_MAC);
    let response = process_packet_sync(&nic, &network, &rs).expect("Should get RA");
    let ra = parse_router_advertisement(&response).expect("Should parse RA");
    assert!(ra.managed_flag, "M flag should indicate DHCPv6");
    assert!(ra.other_flag, "O flag should indicate DNS via DHCPv6");

    // === SOLICIT → ADVERTISE ===
    let client_duid = generate_duid_ll(TEST_MAC);
    let solicit = create_dhcpv6_solicit(TEST_MAC, &client_duid);
    let response = process_packet_sync(&nic, &network, &solicit).expect("Should get ADVERTISE");
    let advertise = parse_dhcpv6_response(&response).expect("Should parse ADVERTISE");
    assert_eq!(advertise.msg_type, Dhcpv6MessageType::Advertise);
    assert_eq!(advertise.addresses[0], TEST_IPV6);

    // === REQUEST → REPLY ===
    let request = create_dhcpv6_request(
        TEST_MAC,
        &client_duid,
        &advertise.server_duid,
        advertise.addresses[0],
        1,
    );
    let response = process_packet_sync(&nic, &network, &request).expect("Should get REPLY");
    let reply = parse_dhcpv6_response(&response).expect("Should parse REPLY");
    assert_eq!(reply.msg_type, Dhcpv6MessageType::Reply);
    assert_eq!(reply.addresses[0], TEST_IPV6);
    assert!(!reply.dns_servers.is_empty());
}

/// Test NS → NA for gateway address.
#[test]
fn test_neighbor_solicitation_gateway() {
    let network = test_network_config_ipv6(
        IPV4_SUBNET.parse().unwrap(),
        IPV6_PREFIX.parse().unwrap(),
        test_dns_servers(),
    );

    let nic = test_nic_config_ipv6(TEST_MAC, TEST_IPV4, TEST_IPV6, network.id);

    // Create NS for gateway
    let ns = create_neighbor_solicitation(TEST_MAC, GATEWAY_IPV6_LL);

    // Process the packet
    let response = process_packet_sync(&nic, &network, &ns).expect("Should get NA for gateway NS");

    // Parse and verify the NA
    let na = parse_neighbor_advertisement(&response).expect("Should parse as NA");

    assert_eq!(
        na.target_addr, GATEWAY_IPV6_LL,
        "Target address should be gateway"
    );
    assert!(na.solicited_flag, "Solicited flag should be set");
    assert!(na.override_flag, "Override flag should be set");
    assert_eq!(
        na.target_mac,
        Some(GATEWAY_MAC),
        "Target MAC should be gateway MAC"
    );
}

/// Test NS for non-gateway address returns no response.
#[test]
fn test_neighbor_solicitation_non_gateway() {
    let network = test_network_config_ipv6(
        IPV4_SUBNET.parse().unwrap(),
        IPV6_PREFIX.parse().unwrap(),
        test_dns_servers(),
    );

    let nic = test_nic_config_ipv6(TEST_MAC, TEST_IPV4, TEST_IPV6, network.id);

    // Create NS for some other address (not gateway)
    let other_ip = Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 0x99);
    let ns = create_neighbor_solicitation(TEST_MAC, other_ip);

    // Process the packet - should return None
    let response = process_packet_sync(&nic, &network, &ns);

    assert!(
        response.is_none(),
        "Should NOT respond to NS for non-gateway address"
    );
}

/// Test ICMPv6 Echo Request → Echo Reply for gateway.
#[test]
fn test_ping6_gateway() {
    let network = test_network_config_ipv6(
        IPV4_SUBNET.parse().unwrap(),
        IPV6_PREFIX.parse().unwrap(),
        test_dns_servers(),
    );

    let nic = test_nic_config_ipv6(TEST_MAC, TEST_IPV4, TEST_IPV6, network.id);

    let ping_data = b"Hello IPv6!";
    let ping_id = 0x1234u16;
    let ping_seq = 1u16;

    // Create Echo Request to gateway
    let echo_req = create_icmpv6_echo_request(
        TEST_MAC,
        GATEWAY_MAC,
        GATEWAY_IPV6_LL,
        ping_id,
        ping_seq,
        ping_data,
    );

    // Process the packet
    let response =
        process_packet_sync(&nic, &network, &echo_req).expect("Should get Echo Reply for gateway");

    // Parse and verify the Echo Reply
    let reply = parse_icmpv6_echo_reply(&response).expect("Should parse as Echo Reply");

    assert_eq!(
        reply.src_addr, GATEWAY_IPV6_LL,
        "Reply should be from gateway"
    );
    assert_eq!(reply.id, ping_id, "ID should match request");
    assert_eq!(reply.seq, ping_seq, "Sequence should match request");
    assert_eq!(reply.data, ping_data, "Data should match request");
}

/// Test ICMPv6 Echo Request to non-gateway returns no response.
#[test]
fn test_ping6_non_gateway() {
    let network = test_network_config_ipv6(
        IPV4_SUBNET.parse().unwrap(),
        IPV6_PREFIX.parse().unwrap(),
        test_dns_servers(),
    );

    let nic = test_nic_config_ipv6(TEST_MAC, TEST_IPV4, TEST_IPV6, network.id);

    // Create Echo Request to some other address
    let other_ip = Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 0x99);
    let other_mac = [0x52, 0x54, 0x00, 0x99, 0x99, 0x99];
    let echo_req = create_icmpv6_echo_request(TEST_MAC, other_mac, other_ip, 0x1234, 1, b"test");

    // Process the packet - should return None
    let response = process_packet_sync(&nic, &network, &echo_req);

    assert!(
        response.is_none(),
        "Should NOT respond to ping for non-gateway address"
    );
}
