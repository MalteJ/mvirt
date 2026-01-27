//! ARP protocol unit tests.
//!
//! Tests ARP request → reply for gateway resolution.
//!
//! These tests verify the packet processing logic directly without needing
//! CAP_NET_ADMIN or actual TAP devices.

use mvirt_ebpf::process_packet_sync;
use mvirt_ebpf::test_util::{
    create_arp_request, parse_arp_reply, test_network_config, test_nic_config,
};
use mvirt_ebpf::{GATEWAY_IPV4_LINK_LOCAL, GATEWAY_MAC};
use std::net::{IpAddr, Ipv4Addr};

/// Test MAC address for VM
const VM_MAC: [u8; 6] = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];

/// Test IPv4 address for VM
const VM_IP: Ipv4Addr = Ipv4Addr::new(10, 0, 0, 100);

/// Test network subnet
const SUBNET: &str = "10.0.0.0/24";

/// ARP request for gateway test.
#[test]
fn test_arp_gateway_resolution() {
    let network = test_network_config(
        SUBNET.parse().unwrap(),
        IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
    );

    let nic = test_nic_config(VM_MAC, VM_IP, network.id);

    // Create ARP request for link-local gateway (169.254.0.1)
    let arp_request = create_arp_request(VM_MAC, VM_IP.octets(), GATEWAY_IPV4_LINK_LOCAL.octets());

    // Process the ARP request
    let response = process_packet_sync(&nic, &network, &arp_request)
        .expect("Should get ARP reply for gateway");

    // Parse and verify the reply
    let reply = parse_arp_reply(&response).expect("Should parse as ARP reply");

    // Verify the reply is for the link-local gateway
    assert_eq!(
        Ipv4Addr::from(reply.sender_ip),
        GATEWAY_IPV4_LINK_LOCAL,
        "ARP reply sender IP should be link-local gateway"
    );

    // Gateway MAC should be the fixed GATEWAY_MAC
    assert_eq!(
        reply.sender_mac, GATEWAY_MAC,
        "Gateway MAC should be the fixed GATEWAY_MAC"
    );

    // Target should be our VM
    assert_eq!(reply.target_mac, VM_MAC, "Target MAC should be VM MAC");
    assert_eq!(
        Ipv4Addr::from(reply.target_ip),
        VM_IP,
        "Target IP should be VM IP"
    );
}

/// ARP request for non-gateway IP should not get a reply.
#[test]
fn test_arp_non_gateway() {
    let network = test_network_config(
        SUBNET.parse().unwrap(),
        IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
    );

    let nic = test_nic_config(VM_MAC, VM_IP, network.id);

    // Create ARP request for a random IP (not the link-local gateway)
    let random_ip = Ipv4Addr::new(10, 0, 0, 50);
    let arp_request = create_arp_request(VM_MAC, VM_IP.octets(), random_ip.octets());

    // Process the ARP request - should not get a response for non-gateway
    let response = process_packet_sync(&nic, &network, &arp_request);

    // We shouldn't get a reply for non-gateway IPs
    assert!(
        response.is_none(),
        "Should not receive ARP reply for non-gateway IP"
    );
}

/// ARP request for old subnet-based gateway should not get a reply.
/// We now use link-local gateway (169.254.0.1), not subnet-based (10.0.0.1).
#[test]
fn test_arp_old_subnet_gateway_no_reply() {
    let network = test_network_config(
        SUBNET.parse().unwrap(),
        IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
    );

    let nic = test_nic_config(VM_MAC, VM_IP, network.id);

    // Create ARP request for the old subnet-based gateway (10.0.0.1)
    let old_gateway = Ipv4Addr::new(10, 0, 0, 1);
    let arp_request = create_arp_request(VM_MAC, VM_IP.octets(), old_gateway.octets());

    // Process the ARP request - should not get a response
    let response = process_packet_sync(&nic, &network, &arp_request);

    // We shouldn't get a reply for the old subnet-based gateway
    assert!(
        response.is_none(),
        "Should not receive ARP reply for old subnet-based gateway"
    );
}

/// Multiple ARP requests should all get replies.
#[test]
fn test_arp_multiple_requests() {
    let network = test_network_config(
        SUBNET.parse().unwrap(),
        IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
    );

    let nic = test_nic_config(VM_MAC, VM_IP, network.id);

    // Process multiple ARP requests
    for i in 0..3 {
        let arp_request =
            create_arp_request(VM_MAC, VM_IP.octets(), GATEWAY_IPV4_LINK_LOCAL.octets());

        let response = process_packet_sync(&nic, &network, &arp_request)
            .unwrap_or_else(|| panic!("No ARP reply for request {}", i));

        let reply =
            parse_arp_reply(&response).unwrap_or_else(|| panic!("Failed to parse ARP reply {}", i));

        assert_eq!(
            Ipv4Addr::from(reply.sender_ip),
            GATEWAY_IPV4_LINK_LOCAL,
            "Reply {} should be from link-local gateway",
            i
        );
    }
}
