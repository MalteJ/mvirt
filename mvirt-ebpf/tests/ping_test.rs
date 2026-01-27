//! ICMP echo (ping) unit tests.
//!
//! Tests ICMP echo request handling.
//!
//! These tests verify the packet processing logic directly without needing
//! CAP_NET_ADMIN or actual TAP devices.

use mvirt_ebpf::process_packet_sync;
use mvirt_ebpf::test_util::{create_icmp_echo_request, test_network_config, test_nic_config};
use std::net::{IpAddr, Ipv4Addr};
use uuid::Uuid;

/// Test MAC address for VM
const VM_MAC: [u8; 6] = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];

/// Test IPv4 address for VM
const VM_IP: Ipv4Addr = Ipv4Addr::new(10, 0, 0, 100);

/// Gateway IPv4 address
const GATEWAY_IP: Ipv4Addr = Ipv4Addr::new(10, 0, 0, 1);

/// Test network subnet
const SUBNET: &str = "10.0.0.0/24";

/// Deterministic gateway MAC for test network.
/// This matches the gateway_mac_for_network() function in proto_handler.rs.
fn expected_gateway_mac(network_id: &Uuid) -> [u8; 6] {
    let id_bytes = network_id.as_bytes();
    [
        0x02,
        id_bytes[0],
        id_bytes[1],
        id_bytes[2],
        id_bytes[3],
        id_bytes[4],
    ]
}

/// ICMP echo request to gateway test.
///
/// Note: The current protocol handler doesn't respond to ICMP echo requests
/// directed at the gateway. This test verifies that no crash/panic occurs when
/// processing ping packets.
#[test]
fn test_icmp_to_gateway_no_crash() {
    let network = test_network_config(
        SUBNET.parse().unwrap(),
        IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
    );

    let gateway_mac = expected_gateway_mac(&network.id);
    let nic = test_nic_config(VM_MAC, VM_IP, network.id);

    // Create ICMP echo request to gateway
    let ping = create_icmp_echo_request(
        VM_MAC,
        gateway_mac,
        VM_IP.octets(),
        GATEWAY_IP.octets(),
        1,
        1,
    );

    // Process the packet - should not panic
    let response = process_packet_sync(&nic, &network, &ping);

    // The protocol handler doesn't respond to pings to the gateway,
    // so we expect None. If an implementation adds it later, Some is also OK.
    // Main check is no panic occurred.
    if response.is_some() {
        println!("Received ICMP echo reply (unexpected but not an error)");
    }
}

/// Test sending multiple ICMP packets doesn't cause issues.
#[test]
fn test_icmp_flood_no_crash() {
    let network = test_network_config(
        SUBNET.parse().unwrap(),
        IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
    );

    let gateway_mac = expected_gateway_mac(&network.id);
    let nic = test_nic_config(VM_MAC, VM_IP, network.id);

    // Process multiple ICMP echo requests
    for seq in 1..=10 {
        let ping = create_icmp_echo_request(
            VM_MAC,
            gateway_mac,
            VM_IP.octets(),
            GATEWAY_IP.octets(),
            1234,
            seq,
        );

        // Process the packet - should not panic
        let _ = process_packet_sync(&nic, &network, &ping);
    }

    // Test passes if no panic occurred
}

/// Test ICMP with corrupted data is handled gracefully.
#[test]
fn test_icmp_malformed_no_crash() {
    let network = test_network_config(
        SUBNET.parse().unwrap(),
        IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
    );

    let gateway_mac = expected_gateway_mac(&network.id);
    let nic = test_nic_config(VM_MAC, VM_IP, network.id);

    // Create a valid ICMP packet and then corrupt it
    let mut ping = create_icmp_echo_request(
        VM_MAC,
        gateway_mac,
        VM_IP.octets(),
        GATEWAY_IP.octets(),
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

/// Test ICMP to external IP (not gateway) doesn't crash.
#[test]
fn test_icmp_to_external_no_crash() {
    let network = test_network_config(
        SUBNET.parse().unwrap(),
        IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
    );

    let gateway_mac = expected_gateway_mac(&network.id);
    let nic = test_nic_config(VM_MAC, VM_IP, network.id);

    // ICMP to an external IP (8.8.8.8)
    let external_ip = [8, 8, 8, 8];
    let ping = create_icmp_echo_request(VM_MAC, gateway_mac, VM_IP.octets(), external_ip, 1, 1);

    // Process the packet - should not panic
    let response = process_packet_sync(&nic, &network, &ping);

    // External IPs are not handled by the protocol handler (they go to kernel)
    assert!(
        response.is_none(),
        "External ping should not generate a response from protocol handler"
    );
}
