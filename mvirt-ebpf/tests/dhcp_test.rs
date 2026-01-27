//! DHCP protocol unit tests.
//!
//! Tests DHCP DISCOVER → OFFER → REQUEST → ACK flow.
//!
//! These tests verify the packet processing logic directly without needing
//! CAP_NET_ADMIN or actual TAP devices.

use mvirt_ebpf::GATEWAY_IPV4_LINK_LOCAL;
use mvirt_ebpf::process_packet_sync;
use mvirt_ebpf::test_util::{
    DhcpMessageType, create_dhcp_discover, create_dhcp_request, parse_dhcp_response,
    test_network_config, test_nic_config,
};
use std::net::{IpAddr, Ipv4Addr};

/// Test MAC address for VM
const TEST_MAC: [u8; 6] = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];

/// Test IPv4 address for VM
const TEST_IP: Ipv4Addr = Ipv4Addr::new(10, 0, 0, 100);

/// Test network subnet
const SUBNET: &str = "10.0.0.0/24";

/// Test DHCP transaction ID
const XID: u32 = 0x12345678;

/// DHCP DISCOVER → OFFER test.
#[test]
fn test_dhcp_discover_offer() {
    let network = test_network_config(
        SUBNET.parse().unwrap(),
        IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
    );

    let nic = test_nic_config(TEST_MAC, TEST_IP, network.id);

    // Create DHCP DISCOVER packet
    let discover = create_dhcp_discover(TEST_MAC, XID);

    // Process the packet
    let response =
        process_packet_sync(&nic, &network, &discover).expect("Should get DHCP OFFER for DISCOVER");

    // Parse and verify the OFFER
    let offer = parse_dhcp_response(&response).expect("Should parse as DHCP response");

    assert_eq!(
        offer.msg_type,
        DhcpMessageType::Offer,
        "Expected DHCP OFFER, got {:?}",
        offer.msg_type
    );
    assert_eq!(offer.xid, XID, "XID mismatch");
    assert_eq!(
        Ipv4Addr::from(offer.your_ip),
        TEST_IP,
        "Offered IP should match NIC's configured IP"
    );

    // Verify options are present
    assert!(offer.subnet_mask.is_some(), "Missing subnet mask");
    assert!(offer.router.is_some(), "Missing router option");

    // Verify router is the link-local gateway (169.254.0.1)
    if let Some(router) = offer.router {
        assert_eq!(
            Ipv4Addr::from(router),
            GATEWAY_IPV4_LINK_LOCAL,
            "Router should be link-local gateway"
        );
    }
}

/// Full DHCP handshake: DISCOVER → OFFER → REQUEST → ACK.
#[test]
fn test_dhcp_full_handshake() {
    let network = test_network_config(
        SUBNET.parse().unwrap(),
        IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
    );

    let nic = test_nic_config(TEST_MAC, TEST_IP, network.id);

    // === DHCP DISCOVER ===
    let discover = create_dhcp_discover(TEST_MAC, XID);
    let response =
        process_packet_sync(&nic, &network, &discover).expect("Should get DHCP OFFER for DISCOVER");

    let offer = parse_dhcp_response(&response).expect("Should parse DHCP OFFER");
    assert_eq!(offer.msg_type, DhcpMessageType::Offer);
    assert_eq!(offer.xid, XID);
    assert_eq!(Ipv4Addr::from(offer.your_ip), TEST_IP);

    // === DHCP REQUEST ===
    let server_id = offer.router.unwrap_or(GATEWAY_IPV4_LINK_LOCAL.octets());
    let request = create_dhcp_request(TEST_MAC, XID, offer.your_ip, server_id);

    let response =
        process_packet_sync(&nic, &network, &request).expect("Should get DHCP ACK for REQUEST");

    let ack = parse_dhcp_response(&response).expect("Should parse DHCP ACK");

    assert_eq!(
        ack.msg_type,
        DhcpMessageType::Ack,
        "Expected DHCP ACK, got {:?}",
        ack.msg_type
    );
    assert_eq!(ack.xid, XID, "XID mismatch in ACK");
    assert_eq!(
        Ipv4Addr::from(ack.your_ip),
        TEST_IP,
        "Acknowledged IP mismatch"
    );

    // Verify lease time is set
    assert!(ack.lease_time.is_some(), "Missing lease time in ACK");
}

/// Test that DHCP returns DNS servers from network config.
#[test]
fn test_dhcp_dns_servers() {
    // Create network with specific DNS servers
    let mut network = test_network_config(
        SUBNET.parse().unwrap(),
        IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)),
    );
    network.dns_servers = vec!["1.1.1.1".parse().unwrap(), "8.8.8.8".parse().unwrap()];

    let nic = test_nic_config(TEST_MAC, TEST_IP, network.id);

    // Send DISCOVER
    let discover = create_dhcp_discover(TEST_MAC, XID);
    let response =
        process_packet_sync(&nic, &network, &discover).expect("Should get DHCP OFFER for DISCOVER");

    // Get OFFER
    let offer = parse_dhcp_response(&response).expect("Should parse DHCP OFFER");
    assert_eq!(offer.msg_type, DhcpMessageType::Offer);

    // Verify DNS servers are included
    assert!(
        !offer.dns_servers.is_empty(),
        "DNS servers should be included"
    );
    assert_eq!(offer.dns_servers.len(), 2, "Should have 2 DNS servers");
}

/// Multiple DHCP requests should all get responses.
#[test]
fn test_dhcp_multiple_requests() {
    let network = test_network_config(
        SUBNET.parse().unwrap(),
        IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
    );

    let nic = test_nic_config(TEST_MAC, TEST_IP, network.id);

    // Process multiple DHCP DISCOVER requests
    for i in 0..3 {
        let xid = XID + i;
        let discover = create_dhcp_discover(TEST_MAC, xid);

        let response = process_packet_sync(&nic, &network, &discover)
            .unwrap_or_else(|| panic!("No DHCP OFFER for request {}", i));

        let offer = parse_dhcp_response(&response)
            .unwrap_or_else(|| panic!("Failed to parse DHCP OFFER {}", i));

        assert_eq!(
            offer.msg_type,
            DhcpMessageType::Offer,
            "Request {} should get OFFER",
            i
        );
        assert_eq!(offer.xid, xid, "XID mismatch for request {}", i);
    }
}
