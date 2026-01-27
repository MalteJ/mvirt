//! VM-to-VM routing unit tests.
//!
//! Tests packet routing logic between VMs on the same network.
//!
//! These tests verify the packet processing logic directly without needing
//! CAP_NET_ADMIN or actual TAP devices.

use mvirt_ebpf::process_packet_sync;
use mvirt_ebpf::test_util::{
    create_arp_request, create_icmp_echo_request, parse_arp_reply, test_network_config,
    test_nic_config,
};
use std::net::{IpAddr, Ipv4Addr};
use uuid::Uuid;

/// VM A configuration
const VM_A_MAC: [u8; 6] = [0x52, 0x54, 0x00, 0xaa, 0xaa, 0x01];
const VM_A_IP: Ipv4Addr = Ipv4Addr::new(10, 0, 0, 100);

/// VM B configuration
const VM_B_MAC: [u8; 6] = [0x52, 0x54, 0x00, 0xbb, 0xbb, 0x02];
const VM_B_IP: Ipv4Addr = Ipv4Addr::new(10, 0, 0, 101);

/// Gateway IPv4 address
const GATEWAY_IP: Ipv4Addr = Ipv4Addr::new(10, 0, 0, 1);

/// Test network subnet
const SUBNET: &str = "10.0.0.0/24";

/// Deterministic gateway MAC for test network.
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

/// Test that different VMs on same network get same gateway responses.
#[test]
fn test_same_network_gateway_consistency() {
    let network = test_network_config(
        SUBNET.parse().unwrap(),
        IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
    );

    let nic_a = test_nic_config(VM_A_MAC, VM_A_IP, network.id);
    let nic_b = test_nic_config(VM_B_MAC, VM_B_IP, network.id);

    // Both VMs ARP for gateway
    let arp_a = create_arp_request(VM_A_MAC, VM_A_IP.octets(), GATEWAY_IP.octets());
    let arp_b = create_arp_request(VM_B_MAC, VM_B_IP.octets(), GATEWAY_IP.octets());

    let response_a =
        process_packet_sync(&nic_a, &network, &arp_a).expect("VM A should get ARP reply");
    let response_b =
        process_packet_sync(&nic_b, &network, &arp_b).expect("VM B should get ARP reply");

    let reply_a = parse_arp_reply(&response_a).expect("Should parse ARP reply A");
    let reply_b = parse_arp_reply(&response_b).expect("Should parse ARP reply B");

    // Both should get the same gateway MAC
    assert_eq!(
        reply_a.sender_mac, reply_b.sender_mac,
        "Gateway MAC should be consistent across VMs"
    );

    // Gateway MAC should match expected
    let expected_mac = expected_gateway_mac(&network.id);
    assert_eq!(
        reply_a.sender_mac, expected_mac,
        "Gateway MAC should be deterministic from network ID"
    );
}

/// Test VM-to-VM packet doesn't generate protocol handler response.
///
/// Packets destined for other VMs are handled by eBPF redirect,
/// not by the userspace protocol handler.
#[test]
fn test_vm_to_vm_no_handler_response() {
    let network = test_network_config(
        SUBNET.parse().unwrap(),
        IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
    );

    let gateway_mac = expected_gateway_mac(&network.id);
    let nic_a = test_nic_config(VM_A_MAC, VM_A_IP, network.id);

    // VM A sends ICMP to VM B via gateway
    // dst MAC is gateway (L2 next hop), dst IP is VM B
    let ping = create_icmp_echo_request(
        VM_A_MAC,
        gateway_mac,
        VM_A_IP.octets(),
        VM_B_IP.octets(),
        1,
        1,
    );

    // Protocol handler shouldn't respond to VM-to-VM traffic
    // (this is handled by eBPF redirect in production)
    let response = process_packet_sync(&nic_a, &network, &ping);
    assert!(
        response.is_none(),
        "VM-to-VM traffic should not generate protocol handler response"
    );
}

/// Test ARP for non-gateway IP returns nothing.
#[test]
fn test_arp_for_other_vm_no_response() {
    let network = test_network_config(
        SUBNET.parse().unwrap(),
        IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
    );

    let nic_a = test_nic_config(VM_A_MAC, VM_A_IP, network.id);

    // VM A ARPs for VM B's IP (not gateway)
    let arp = create_arp_request(VM_A_MAC, VM_A_IP.octets(), VM_B_IP.octets());

    // Protocol handler only responds to gateway ARP
    let response = process_packet_sync(&nic_a, &network, &arp);
    assert!(
        response.is_none(),
        "ARP for VM B should not get response from protocol handler"
    );
}

/// Test packets to external IPs don't generate handler response.
#[test]
fn test_external_traffic_no_response() {
    let network = test_network_config(
        SUBNET.parse().unwrap(),
        IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
    );

    let gateway_mac = expected_gateway_mac(&network.id);
    let nic_a = test_nic_config(VM_A_MAC, VM_A_IP, network.id);

    // Traffic to external IP (8.8.8.8)
    let external_ip = [8, 8, 8, 8];
    let ping = create_icmp_echo_request(VM_A_MAC, gateway_mac, VM_A_IP.octets(), external_ip, 1, 1);

    // External traffic goes through kernel networking, not protocol handler
    let response = process_packet_sync(&nic_a, &network, &ping);
    assert!(
        response.is_none(),
        "External traffic should not generate protocol handler response"
    );
}

/// Test multiple VMs can process packets independently.
#[test]
fn test_multiple_vms_independent_processing() {
    let network = test_network_config(
        SUBNET.parse().unwrap(),
        IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
    );

    let nic_a = test_nic_config(VM_A_MAC, VM_A_IP, network.id);
    let nic_b = test_nic_config(VM_B_MAC, VM_B_IP, network.id);

    // Multiple ARP requests from different VMs
    for _ in 0..5 {
        let arp_a = create_arp_request(VM_A_MAC, VM_A_IP.octets(), GATEWAY_IP.octets());
        let arp_b = create_arp_request(VM_B_MAC, VM_B_IP.octets(), GATEWAY_IP.octets());

        let response_a = process_packet_sync(&nic_a, &network, &arp_a);
        let response_b = process_packet_sync(&nic_b, &network, &arp_b);

        assert!(response_a.is_some(), "VM A should get response");
        assert!(response_b.is_some(), "VM B should get response");

        let reply_a = parse_arp_reply(&response_a.unwrap()).expect("Parse A");
        let reply_b = parse_arp_reply(&response_b.unwrap()).expect("Parse B");

        // Responses should be addressed to the correct VMs
        assert_eq!(reply_a.target_mac, VM_A_MAC, "Reply A should target VM A");
        assert_eq!(reply_b.target_mac, VM_B_MAC, "Reply B should target VM B");
    }
}

/// Test packet processing with different networks is isolated.
#[test]
fn test_network_isolation() {
    let network1 = test_network_config(
        "10.0.1.0/24".parse().unwrap(),
        IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
    );
    let network2 = test_network_config(
        "10.0.2.0/24".parse().unwrap(),
        IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)),
    );

    let gateway1 = Ipv4Addr::new(10, 0, 1, 1);
    let gateway2 = Ipv4Addr::new(10, 0, 2, 1);

    let nic1 = test_nic_config(VM_A_MAC, Ipv4Addr::new(10, 0, 1, 100), network1.id);
    let nic2 = test_nic_config(VM_B_MAC, Ipv4Addr::new(10, 0, 2, 100), network2.id);

    // ARP for each network's gateway
    let arp1 = create_arp_request(VM_A_MAC, [10, 0, 1, 100], gateway1.octets());
    let arp2 = create_arp_request(VM_B_MAC, [10, 0, 2, 100], gateway2.octets());

    let response1 = process_packet_sync(&nic1, &network1, &arp1);
    let response2 = process_packet_sync(&nic2, &network2, &arp2);

    assert!(response1.is_some(), "Network1 should respond");
    assert!(response2.is_some(), "Network2 should respond");

    let reply1 = parse_arp_reply(&response1.unwrap()).expect("Parse 1");
    let reply2 = parse_arp_reply(&response2.unwrap()).expect("Parse 2");

    // Different networks should have different gateway MACs
    assert_ne!(
        reply1.sender_mac, reply2.sender_mac,
        "Different networks should have different gateway MACs"
    );
}
