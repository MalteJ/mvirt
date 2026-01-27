//! Integration tests requiring TAP devices.
//!
//! These tests verify TAP device creation, NIC registration, and handler
//! infrastructure without racing for packet reads.
//!
//! Note: Full packet flow testing requires eBPF redirect to be active.
//! These tests focus on infrastructure and no-crash verification.
//!
//! Requires CAP_NET_ADMIN capability - run with:
//!   sudo -E cargo test --package mvirt-ebpf --test integration_test --features test-util

use mvirt_ebpf::test_util::{
    TapTestDevice, create_arp_request, create_dhcp_discover, create_icmp_echo_request,
    test_network_config, test_nic_config,
};
use mvirt_ebpf::{GATEWAY_IPV4_LINK_LOCAL, GATEWAY_MAC, ProtocolHandler, Storage};
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

/// VM A configuration
const VM_A_MAC: [u8; 6] = [0x52, 0x54, 0x00, 0xaa, 0xaa, 0x01];
const VM_A_IP: Ipv4Addr = Ipv4Addr::new(10, 0, 0, 100);

/// VM B configuration
const VM_B_MAC: [u8; 6] = [0x52, 0x54, 0x00, 0xbb, 0xbb, 0x02];
const VM_B_IP: Ipv4Addr = Ipv4Addr::new(10, 0, 0, 101);

/// Test network subnet
const SUBNET: &str = "10.0.0.0/24";

/// Integration test: TAP device creation and basic operations.
#[tokio::test]
async fn test_tap_device_creation() {
    let tap_name = format!("tap_test_{}", &Uuid::new_v4().to_string()[..8]);

    // Create TAP device
    let tap = TapTestDevice::create(&tap_name).expect("Failed to create TAP device");

    // Verify basic properties
    assert_eq!(tap.name(), tap_name);
    assert!(tap.if_index() > 0, "Interface index should be valid");
    assert!(tap.as_raw_fd() >= 0, "File descriptor should be valid");

    // TAP is automatically cleaned up on drop
}

/// Integration test: Two TAP devices can coexist.
#[tokio::test]
async fn test_multiple_tap_devices() {
    let tap_name_a = format!("tap_a_{}", &Uuid::new_v4().to_string()[..8]);
    let tap_name_b = format!("tap_b_{}", &Uuid::new_v4().to_string()[..8]);

    let tap_a = TapTestDevice::create(&tap_name_a).expect("Failed to create TAP A");
    let tap_b = TapTestDevice::create(&tap_name_b).expect("Failed to create TAP B");

    // Both should have unique interface indices
    assert_ne!(
        tap_a.if_index(),
        tap_b.if_index(),
        "TAP devices should have different interface indices"
    );

    // Both should be functional
    assert!(tap_a.as_raw_fd() >= 0);
    assert!(tap_b.as_raw_fd() >= 0);
}

/// Integration test: NIC registration with TAP devices.
#[tokio::test]
async fn test_nic_registration_with_tap() {
    let tap_name = format!("tap_reg_{}", &Uuid::new_v4().to_string()[..8]);

    let tap = TapTestDevice::create(&tap_name).expect("Failed to create TAP");

    let storage = Arc::new(Storage::in_memory().expect("Failed to create storage"));

    let network = test_network_config(
        SUBNET.parse().unwrap(),
        IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
    );
    storage
        .create_network(&network)
        .expect("Failed to create network");

    let mut nic = test_nic_config(VM_A_MAC, VM_A_IP, network.id);
    nic.tap_name = tap_name.clone();

    let handler = ProtocolHandler::new();

    // Register NIC
    handler
        .register_nic(tap.if_index(), nic.clone(), network.clone())
        .await;

    // Verify we can spawn a handler (it will use AF_PACKET socket)
    let _handler_task = handler.spawn_handler(tap_name.clone(), tap.if_index());

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Unregister should work without error
    handler.unregister_nic(tap.if_index()).await;
}

/// Integration test: Two NICs on same network with handlers.
#[tokio::test]
async fn test_two_nics_same_network() {
    let tap_name_a = format!("tap_vma_{}", &Uuid::new_v4().to_string()[..8]);
    let tap_name_b = format!("tap_vmb_{}", &Uuid::new_v4().to_string()[..8]);

    let tap_a = TapTestDevice::create(&tap_name_a).expect("Failed to create TAP A");
    let tap_b = TapTestDevice::create(&tap_name_b).expect("Failed to create TAP B");

    let storage = Arc::new(Storage::in_memory().expect("Failed to create storage"));

    let network = test_network_config(
        SUBNET.parse().unwrap(),
        IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
    );
    storage
        .create_network(&network)
        .expect("Failed to create network");

    let mut nic_a = test_nic_config(VM_A_MAC, VM_A_IP, network.id);
    nic_a.tap_name = tap_name_a.clone();

    let mut nic_b = test_nic_config(VM_B_MAC, VM_B_IP, network.id);
    nic_b.tap_name = tap_name_b.clone();

    let handler = ProtocolHandler::new();

    // Register both NICs
    handler
        .register_nic(tap_a.if_index(), nic_a.clone(), network.clone())
        .await;
    handler
        .register_nic(tap_b.if_index(), nic_b.clone(), network.clone())
        .await;

    // Spawn handlers for both
    let _handler_a = handler.spawn_handler(tap_name_a.clone(), tap_a.if_index());
    let _handler_b = handler.spawn_handler(tap_name_b.clone(), tap_b.if_index());

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Both handlers should be running
    // Test passes if no crash during setup

    handler.unregister_nic(tap_a.if_index()).await;
    handler.unregister_nic(tap_b.if_index()).await;
}

/// Integration test: Packet sending doesn't crash handler.
#[tokio::test]
async fn test_packet_sending_no_crash() {
    let tap_name = format!("tap_pkt_{}", &Uuid::new_v4().to_string()[..8]);

    let tap = TapTestDevice::create(&tap_name).expect("Failed to create TAP");

    let storage = Arc::new(Storage::in_memory().expect("Failed to create storage"));

    let network = test_network_config(
        SUBNET.parse().unwrap(),
        IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
    );
    storage
        .create_network(&network)
        .expect("Failed to create network");

    let mut nic = test_nic_config(VM_A_MAC, VM_A_IP, network.id);
    nic.tap_name = tap_name.clone();

    let handler = ProtocolHandler::new();

    handler
        .register_nic(tap.if_index(), nic.clone(), network.clone())
        .await;

    let _handler_task = handler.spawn_handler(tap_name.clone(), tap.if_index());

    tokio::time::sleep(Duration::from_millis(50)).await;

    let _ = tap.drain();

    // Send various packet types - handler should process without crashing
    let arp = create_arp_request(VM_A_MAC, VM_A_IP.octets(), GATEWAY_IPV4_LINK_LOCAL.octets());
    tap.send_packet(&arp).expect("Send ARP");

    let dhcp = create_dhcp_discover(VM_A_MAC, 0x12345678);
    tap.send_packet(&dhcp).expect("Send DHCP");

    let ping = create_icmp_echo_request(
        VM_A_MAC,
        GATEWAY_MAC,
        VM_A_IP.octets(),
        GATEWAY_IPV4_LINK_LOCAL.octets(),
        1,
        1,
    );
    tap.send_packet(&ping).expect("Send ICMP");

    // Give handler time to process
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Test passes if no crash
    handler.unregister_nic(tap.if_index()).await;
}

/// Integration test: VM-to-VM ICMP packet flow.
///
/// Tests that a packet from VM A destined for VM B can be sent.
/// Note: Without eBPF redirect, the packet won't actually reach VM B's TAP,
/// but we verify the infrastructure doesn't crash.
#[tokio::test]
async fn test_vm_to_vm_packet_no_crash() {
    let tap_name_a = format!("tap_icmpa_{}", &Uuid::new_v4().to_string()[..8]);
    let tap_name_b = format!("tap_icmpb_{}", &Uuid::new_v4().to_string()[..8]);

    let tap_a = TapTestDevice::create(&tap_name_a).expect("Failed to create TAP A");
    let tap_b = TapTestDevice::create(&tap_name_b).expect("Failed to create TAP B");

    let storage = Arc::new(Storage::in_memory().expect("Failed to create storage"));

    let network = test_network_config(
        SUBNET.parse().unwrap(),
        IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
    );
    storage
        .create_network(&network)
        .expect("Failed to create network");

    let mut nic_a = test_nic_config(VM_A_MAC, VM_A_IP, network.id);
    nic_a.tap_name = tap_name_a.clone();

    let mut nic_b = test_nic_config(VM_B_MAC, VM_B_IP, network.id);
    nic_b.tap_name = tap_name_b.clone();

    let handler = ProtocolHandler::new();

    handler
        .register_nic(tap_a.if_index(), nic_a.clone(), network.clone())
        .await;
    handler
        .register_nic(tap_b.if_index(), nic_b.clone(), network.clone())
        .await;

    let _handler_a = handler.spawn_handler(tap_name_a.clone(), tap_a.if_index());
    let _handler_b = handler.spawn_handler(tap_name_b.clone(), tap_b.if_index());

    tokio::time::sleep(Duration::from_millis(50)).await;

    let _ = tap_a.drain();
    let _ = tap_b.drain();

    // VM A sends ICMP to VM B via gateway MAC
    for seq in 1..=5 {
        let ping = create_icmp_echo_request(
            VM_A_MAC,
            GATEWAY_MAC,
            VM_A_IP.octets(),
            VM_B_IP.octets(),
            1234,
            seq,
        );
        tap_a.send_packet(&ping).expect("Send ICMP from A");
    }

    // Give handler time to process
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Check if packet arrived at VM B (unlikely without eBPF redirect)
    let response = tap_b
        .recv_packet(Duration::from_millis(100))
        .expect("recv failed");

    if response.is_some() {
        println!("Packet reached VM B - eBPF routing may be active");
    } else {
        println!("No packet at VM B (expected without eBPF redirect)");
    }

    // Test passes if no crash
    handler.unregister_nic(tap_a.if_index()).await;
    handler.unregister_nic(tap_b.if_index()).await;
}

/// Integration test: Dynamic NIC lifecycle.
#[tokio::test]
async fn test_dynamic_nic_lifecycle() {
    let tap_name = format!("tap_dyn_{}", &Uuid::new_v4().to_string()[..8]);

    let tap = TapTestDevice::create(&tap_name).expect("Failed to create TAP");

    let storage = Arc::new(Storage::in_memory().expect("Failed to create storage"));

    let network = test_network_config(
        SUBNET.parse().unwrap(),
        IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
    );
    storage
        .create_network(&network)
        .expect("Failed to create network");

    let mut nic = test_nic_config(VM_A_MAC, VM_A_IP, network.id);
    nic.tap_name = tap_name.clone();

    let handler = ProtocolHandler::new();

    // Register
    handler
        .register_nic(tap.if_index(), nic.clone(), network.clone())
        .await;

    let _handler_task = handler.spawn_handler(tap_name.clone(), tap.if_index());
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Send some packets
    let arp = create_arp_request(VM_A_MAC, VM_A_IP.octets(), GATEWAY_IPV4_LINK_LOCAL.octets());
    tap.send_packet(&arp).expect("Send ARP");
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Unregister
    handler.unregister_nic(tap.if_index()).await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Re-register with different IP
    let mut nic2 = test_nic_config(VM_A_MAC, Ipv4Addr::new(10, 0, 0, 200), network.id);
    nic2.tap_name = tap_name.clone();

    handler
        .register_nic(tap.if_index(), nic2, network.clone())
        .await;

    // Send packet with new config
    let arp2 = create_arp_request(VM_A_MAC, [10, 0, 0, 200], GATEWAY_IPV4_LINK_LOCAL.octets());
    tap.send_packet(&arp2).expect("Send ARP 2");

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Test passes if no crash during lifecycle changes
    handler.unregister_nic(tap.if_index()).await;
}

/// Integration test: Flood packets without crash.
#[tokio::test]
async fn test_packet_flood_no_crash() {
    let tap_name = format!("tap_flood_{}", &Uuid::new_v4().to_string()[..8]);

    let tap = TapTestDevice::create(&tap_name).expect("Failed to create TAP");

    let storage = Arc::new(Storage::in_memory().expect("Failed to create storage"));

    let network = test_network_config(
        SUBNET.parse().unwrap(),
        IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
    );
    storage
        .create_network(&network)
        .expect("Failed to create network");

    let mut nic = test_nic_config(VM_A_MAC, VM_A_IP, network.id);
    nic.tap_name = tap_name.clone();

    let handler = ProtocolHandler::new();

    handler
        .register_nic(tap.if_index(), nic.clone(), network.clone())
        .await;

    let _handler_task = handler.spawn_handler(tap_name.clone(), tap.if_index());

    tokio::time::sleep(Duration::from_millis(50)).await;

    let _ = tap.drain();

    // Send many packets rapidly
    for i in 0..100 {
        let ping = create_icmp_echo_request(
            VM_A_MAC,
            GATEWAY_MAC,
            VM_A_IP.octets(),
            GATEWAY_IPV4_LINK_LOCAL.octets(),
            1234,
            i,
        );
        tap.send_packet(&ping).expect("Send flood packet");
    }

    // Give handler time to process
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Test passes if no crash during flood
    handler.unregister_nic(tap.if_index()).await;
}

/// Integration test: Both networks use same link-local gateway.
#[tokio::test]
async fn test_networks_same_gateway_tap() {
    let tap_name_1 = format!("tap_net1_{}", &Uuid::new_v4().to_string()[..8]);
    let tap_name_2 = format!("tap_net2_{}", &Uuid::new_v4().to_string()[..8]);

    let tap_1 = TapTestDevice::create(&tap_name_1).expect("Failed to create TAP 1");
    let tap_2 = TapTestDevice::create(&tap_name_2).expect("Failed to create TAP 2");

    let storage = Arc::new(Storage::in_memory().expect("Failed to create storage"));

    // Two different networks
    let network1 = test_network_config(
        "10.0.1.0/24".parse().unwrap(),
        IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
    );
    let network2 = test_network_config(
        "10.0.2.0/24".parse().unwrap(),
        IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)),
    );

    storage
        .create_network(&network1)
        .expect("Failed to create network1");
    storage
        .create_network(&network2)
        .expect("Failed to create network2");

    let mut nic_1 = test_nic_config(VM_A_MAC, Ipv4Addr::new(10, 0, 1, 100), network1.id);
    nic_1.tap_name = tap_name_1.clone();

    let mut nic_2 = test_nic_config(VM_B_MAC, Ipv4Addr::new(10, 0, 2, 100), network2.id);
    nic_2.tap_name = tap_name_2.clone();

    let handler = ProtocolHandler::new();

    handler
        .register_nic(tap_1.if_index(), nic_1.clone(), network1.clone())
        .await;
    handler
        .register_nic(tap_2.if_index(), nic_2.clone(), network2.clone())
        .await;

    let _handler_1 = handler.spawn_handler(tap_name_1.clone(), tap_1.if_index());
    let _handler_2 = handler.spawn_handler(tap_name_2.clone(), tap_2.if_index());

    tokio::time::sleep(Duration::from_millis(50)).await;

    let _ = tap_1.drain();
    let _ = tap_2.drain();

    // Both networks use the same link-local gateway
    let arp_1 = create_arp_request(VM_A_MAC, [10, 0, 1, 100], GATEWAY_IPV4_LINK_LOCAL.octets());
    let arp_2 = create_arp_request(VM_B_MAC, [10, 0, 2, 100], GATEWAY_IPV4_LINK_LOCAL.octets());

    tap_1.send_packet(&arp_1).expect("Send ARP 1");
    tap_2.send_packet(&arp_2).expect("Send ARP 2");

    tokio::time::sleep(Duration::from_millis(200)).await;

    // Test passes if no crash
    handler.unregister_nic(tap_1.if_index()).await;
    handler.unregister_nic(tap_2.if_index()).await;
}
