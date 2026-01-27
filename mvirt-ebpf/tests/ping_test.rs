//! ICMP echo (ping) unit tests.
//!
//! Tests ICMP echo request handling.
//!
//! These tests verify the packet processing logic directly without needing
//! CAP_NET_ADMIN or actual TAP devices.

use mvirt_ebpf::process_packet_sync;
use mvirt_ebpf::test_util::{
    create_icmp_echo_request, parse_icmp_echo_reply, test_network_config, test_nic_config,
};
use mvirt_ebpf::{GATEWAY_IPV4_LINK_LOCAL, GATEWAY_MAC};
use std::net::{IpAddr, Ipv4Addr};

/// Test MAC address for VM
const VM_MAC: [u8; 6] = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];

/// Test IPv4 address for VM
const VM_IP: Ipv4Addr = Ipv4Addr::new(10, 0, 0, 100);

/// Test network subnet
const SUBNET: &str = "10.0.0.0/24";

/// ICMP echo request to gateway test.
///
/// The protocol handler now responds to ICMP echo requests directed at the
/// link-local gateway (169.254.0.1).
#[test]
fn test_icmp_to_gateway_reply() {
    let network = test_network_config(
        SUBNET.parse().unwrap(),
        IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
    );

    let nic = test_nic_config(VM_MAC, VM_IP, network.id);

    // Create ICMP echo request to link-local gateway
    let ping = create_icmp_echo_request(
        VM_MAC,
        GATEWAY_MAC,
        VM_IP.octets(),
        GATEWAY_IPV4_LINK_LOCAL.octets(),
        1234,
        1,
    );

    // Process the packet - should get echo reply
    let response = process_packet_sync(&nic, &network, &ping)
        .expect("Should get ICMP echo reply from gateway");

    // Parse and verify the reply
    let reply = parse_icmp_echo_reply(&response).expect("Should parse as ICMP echo reply");

    assert_eq!(
        Ipv4Addr::from(reply.src_ip),
        GATEWAY_IPV4_LINK_LOCAL,
        "Reply should be from link-local gateway"
    );
    assert_eq!(
        Ipv4Addr::from(reply.dst_ip),
        VM_IP,
        "Reply should be destined to VM"
    );
    assert_eq!(reply.id, 1234, "Reply ID should match request");
    assert_eq!(reply.seq, 1, "Reply sequence should match request");
}

/// Test sending multiple ICMP packets all get replies.
#[test]
fn test_icmp_flood_all_replies() {
    let network = test_network_config(
        SUBNET.parse().unwrap(),
        IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
    );

    let nic = test_nic_config(VM_MAC, VM_IP, network.id);

    // Process multiple ICMP echo requests
    for seq in 1..=10 {
        let ping = create_icmp_echo_request(
            VM_MAC,
            GATEWAY_MAC,
            VM_IP.octets(),
            GATEWAY_IPV4_LINK_LOCAL.octets(),
            1234,
            seq,
        );

        // Process the packet - should get reply
        let response = process_packet_sync(&nic, &network, &ping)
            .unwrap_or_else(|| panic!("Should get reply for seq {}", seq));

        let reply = parse_icmp_echo_reply(&response)
            .unwrap_or_else(|| panic!("Should parse reply for seq {}", seq));

        assert_eq!(reply.seq, seq, "Reply sequence should match request");
    }
}

/// Test ICMP with corrupted data is handled gracefully.
#[test]
fn test_icmp_malformed_no_crash() {
    let network = test_network_config(
        SUBNET.parse().unwrap(),
        IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
    );

    let nic = test_nic_config(VM_MAC, VM_IP, network.id);

    // Create a valid ICMP packet and then corrupt it
    let mut ping = create_icmp_echo_request(
        VM_MAC,
        GATEWAY_MAC,
        VM_IP.octets(),
        GATEWAY_IPV4_LINK_LOCAL.octets(),
        1,
        1,
    );

    // Corrupt the IP checksum
    if ping.len() > 24 {
        ping[24] ^= 0xFF;
        ping[25] ^= 0xFF;
    }

    // Process the corrupted packet - should not panic
    let _ = process_packet_sync(&nic, &network, &ping);

    // Test passes if no panic occurred
}

/// Test ICMP to external IP (not gateway) doesn't generate reply.
#[test]
fn test_icmp_to_external_no_reply() {
    let network = test_network_config(
        SUBNET.parse().unwrap(),
        IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
    );

    let nic = test_nic_config(VM_MAC, VM_IP, network.id);

    // ICMP to an external IP (8.8.8.8)
    let external_ip = [8, 8, 8, 8];
    let ping = create_icmp_echo_request(VM_MAC, GATEWAY_MAC, VM_IP.octets(), external_ip, 1, 1);

    // Process the packet - should not panic
    let response = process_packet_sync(&nic, &network, &ping);

    // External IPs are not handled by the protocol handler (they go to kernel)
    assert!(
        response.is_none(),
        "External ping should not generate a response from protocol handler"
    );
}

/// Test ICMP to old subnet-based gateway (10.0.0.1) doesn't generate reply.
/// We now use link-local gateway (169.254.0.1).
#[test]
fn test_icmp_to_old_gateway_no_reply() {
    let network = test_network_config(
        SUBNET.parse().unwrap(),
        IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
    );

    let nic = test_nic_config(VM_MAC, VM_IP, network.id);

    // ICMP to the old subnet-based gateway (10.0.0.1)
    let old_gateway = Ipv4Addr::new(10, 0, 0, 1);
    let ping = create_icmp_echo_request(
        VM_MAC,
        GATEWAY_MAC,
        VM_IP.octets(),
        old_gateway.octets(),
        1,
        1,
    );

    // Process the packet
    let response = process_packet_sync(&nic, &network, &ping);

    // Old subnet-based gateway should not get a reply
    assert!(
        response.is_none(),
        "Old subnet gateway ping should not generate a reply"
    );
}
