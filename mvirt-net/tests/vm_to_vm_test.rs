//! VM-to-VM routing integration test
//!
//! Tests packet forwarding between two vhost-user devices through the router.
//! VM A sends a packet destined for VM B, which should be routed through
//! the shared reactor registry.

use std::sync::Arc;
use std::time::Duration;

use mvirt_net::reactor::ReactorRegistry;
use mvirt_net::router::{Router, VhostConfig};
use mvirt_net::routing::{IpPrefix, RouteTarget};
use mvirt_net::test_util::{
    VhostUserFrontendDevice,
    frontend_device::{ETHERNET_HDR_SIZE, VIRTIO_NET_HDR_SIZE, create_icmp_echo_request},
};

/// Test VM-to-VM packet forwarding through the router.
///
/// Setup:
/// - VM A (10.200.1.2) connected via vhost socket A
/// - VM B (10.200.2.2) connected via vhost socket B
/// - Shared reactor registry between both routers
/// - Routes configured so VM A's traffic to 10.200.2.0/24 goes to VM B's reactor
///
/// Test:
/// - VM A sends ICMP echo request to VM B (10.200.2.2)
/// - Verify VM B receives the packet
#[tokio::test]
async fn test_vm_to_vm_routing() {
    let _ = tracing_subscriber::fmt::try_init();

    let socket_a = "/tmp/iou-vm-a.sock";
    let socket_b = "/tmp/iou-vm-b.sock";

    // IP addresses
    let router_a_ip = std::net::Ipv4Addr::new(10, 200, 1, 1);
    let router_b_ip = std::net::Ipv4Addr::new(10, 200, 2, 1);
    let vm_a_ip: [u8; 4] = [10, 200, 1, 2];
    let vm_b_ip: [u8; 4] = [10, 200, 2, 2];

    // Router vhost-user interface MACs (presented to guests)
    let router_mac_a = [0x52, 0x54, 0x00, 0xAA, 0xBB, 0x01];
    let router_mac_b = [0x52, 0x54, 0x00, 0xAA, 0xBB, 0x02];

    // VM (guest) interface MACs
    let vm_mac_a = [0x52, 0x54, 0x00, 0xCC, 0xDD, 0x01];
    let vm_mac_b = [0x52, 0x54, 0x00, 0xCC, 0xDD, 0x02];

    // Clean up any stale sockets
    let _ = std::fs::remove_file(socket_a);
    let _ = std::fs::remove_file(socket_b);

    // Create shared registry for VM-to-VM communication
    let registry = Arc::new(ReactorRegistry::new());

    // Start router A with vhost-user backend
    println!("Starting Router A...");
    let router_a = Router::with_shared_registry(
        "tun_vm_a",
        Some((router_a_ip, 24)),
        4096,
        256,
        256,
        Some(VhostConfig::new(socket_a.to_string(), router_mac_a)),
        Arc::clone(&registry),
    )
    .await
    .expect("Failed to start router A");

    let reactor_a_id = router_a.reactor_id();
    println!("Router A started with reactor ID: {}", reactor_a_id);

    // Start router B with vhost-user backend
    println!("Starting Router B...");
    let router_b = Router::with_shared_registry(
        "tun_vm_b",
        Some((router_b_ip, 24)),
        4096,
        256,
        256,
        Some(VhostConfig::new(socket_b.to_string(), router_mac_b)),
        Arc::clone(&registry),
    )
    .await
    .expect("Failed to start router B");

    let reactor_b_id = router_b.reactor_id();
    println!("Router B started with reactor ID: {}", reactor_b_id);

    // Configure routes:
    // Router A: 10.200.2.0/24 -> Router B's reactor
    // Router B: 10.200.1.0/24 -> Router A's reactor
    println!("Configuring routes...");

    // Create routing tables and add routes
    let table_id = uuid::Uuid::new_v4();
    router_a.reactor_handle().create_table(table_id, "default");
    router_a.reactor_handle().add_route(
        table_id,
        IpPrefix::V4(ipnet::Ipv4Net::new(std::net::Ipv4Addr::new(10, 200, 2, 0), 24).unwrap()),
        RouteTarget::Reactor { id: reactor_b_id },
    );
    router_a.reactor_handle().set_default_table(table_id);

    let table_id_b = uuid::Uuid::new_v4();
    router_b
        .reactor_handle()
        .create_table(table_id_b, "default");
    router_b.reactor_handle().add_route(
        table_id_b,
        IpPrefix::V4(ipnet::Ipv4Net::new(std::net::Ipv4Addr::new(10, 200, 1, 0), 24).unwrap()),
        RouteTarget::Reactor { id: reactor_a_id },
    );
    router_b.reactor_handle().set_default_table(table_id_b);

    // Give routers time to process route updates
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Give backends time to create sockets
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Connect VM A frontend
    println!("Connecting VM A frontend...");
    let mut frontend_a =
        VhostUserFrontendDevice::connect(socket_a).expect("Failed to connect frontend A");
    frontend_a.setup().expect("Failed to setup frontend A");

    // Connect VM B frontend
    println!("Connecting VM B frontend...");
    let mut frontend_b =
        VhostUserFrontendDevice::connect(socket_b).expect("Failed to connect frontend B");
    frontend_b.setup().expect("Failed to setup frontend B");

    // Provide RX buffers for VM B to receive the packet
    println!("Providing RX buffers for VM B...");
    for _ in 0..8 {
        frontend_b
            .provide_rx_buffer(4096)
            .expect("Failed to provide RX buffer");
    }

    // VM A sends ICMP echo request to VM B
    // L2 frame: src=VM_A_MAC, dst=Router_A_MAC (gateway), payload=IP packet
    let packet = create_icmp_echo_request(1, vm_mac_a, router_mac_a, vm_a_ip, vm_b_ip);
    println!(
        "VM A sending ICMP echo request to VM B ({} bytes)...",
        packet.len()
    );
    frontend_a
        .send_packet(&packet)
        .expect("Failed to send packet");

    // For VM-to-VM routing, the TX completion on VM A comes AFTER:
    // 1. Router A routes packet to Router B
    // 2. Router B receives and copies to VM B's RX
    // 3. Router B sends CompletionNotify back to Router A
    // 4. Router A returns descriptor to VM A
    //
    // So we first wait for VM B to receive, then check TX completion on VM A

    // Wait for RX on VM B
    println!("Waiting for VM B to receive packet...");
    let rx_signaled = frontend_b.wait_for_rx(5000).expect("RX call wait failed");

    if rx_signaled {
        println!("VM B RX call signaled");
    }

    // Try to receive the packet on VM B
    if let Some(received) = frontend_b.recv_packet().expect("RX recv failed") {
        println!("VM B received packet: {} bytes", received.len());

        // Verify it's the ICMP echo request we sent
        // Packet format: [virtio-net hdr][Ethernet hdr][IP hdr][ICMP]
        let eth_start = VIRTIO_NET_HDR_SIZE;
        let ip_start = eth_start + ETHERNET_HDR_SIZE;
        let icmp_start = ip_start + 20;

        assert!(
            received.len() >= icmp_start + 8,
            "Received packet too short: {} bytes",
            received.len()
        );

        // Check IP source and destination
        let src_ip = &received[ip_start + 12..ip_start + 16];
        let dst_ip = &received[ip_start + 16..ip_start + 20];

        assert_eq!(src_ip, &vm_a_ip, "Source IP mismatch");
        assert_eq!(dst_ip, &vm_b_ip, "Destination IP mismatch");

        // Check ICMP type (should be Echo Request = 8)
        assert_eq!(
            received[icmp_start], 8,
            "Expected ICMP Echo Request (type 8), got {}",
            received[icmp_start]
        );

        println!("VM-to-VM routing test PASSED: VM B received ICMP Echo Request from VM A");

        // Now check TX completion on VM A (should be complete after RX delivery)
        println!("Checking VM A TX completion...");

        // Give a little time for completion notification to propagate
        std::thread::sleep(Duration::from_millis(100));

        // Check for TX call signal (completion notification triggers this)
        let tx_signaled = frontend_a.wait_for_tx(2000).expect("TX call wait failed");
        if tx_signaled {
            println!("VM A TX call signaled");
        }

        if frontend_a.wait_tx_complete().expect("TX check failed") {
            println!("VM A TX completed (descriptor returned)");
        } else {
            println!("Warning: VM A TX descriptor not yet returned");
        }
    } else {
        panic!("VM B did not receive any packet - routing may have failed");
    }

    // Cleanup - signal shutdown before dropping frontends to suppress
    // expected "Disconnected" error messages
    router_a.prepare_shutdown();
    router_b.prepare_shutdown();

    drop(frontend_a);
    drop(frontend_b);

    router_a
        .shutdown()
        .await
        .expect("Failed to shutdown router A");
    router_b
        .shutdown()
        .await
        .expect("Failed to shutdown router B");
}
