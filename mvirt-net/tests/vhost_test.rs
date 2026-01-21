//! vhost-user frontend test - simulates the VM side to test the backend

use std::time::Duration;

use mvirt_net::router::{Router, VhostConfig};
use mvirt_net::test_util::{
    VhostUserFrontendDevice,
    frontend_device::{VIRTIO_NET_HDR_SIZE, create_icmp_echo_request},
};

// NOTE: This test is ignored because local IP handling (ICMP echo to router IP)
// was removed as part of the L3-only routing refactor.
#[tokio::test]
#[ignore = "Local IP handling removed in L3-only refactor"]
async fn test_vhost_user_ping() {
    let _ = tracing_subscriber::fmt::try_init();

    let socket_path = "/tmp/iou-vhost-test.sock";
    let local_ip = std::net::Ipv4Addr::new(10, 99, 100, 1);
    let mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];

    // Clean up any stale socket
    let _ = std::fs::remove_file(socket_path);

    // Start router with vhost-user backend
    let router = Router::with_config_and_vhost(
        "tun_vhost",
        local_ip,
        24,
        4096,
        256,
        256,
        Some(VhostConfig::new(socket_path.to_string(), mac)),
    )
    .await
    .expect("Failed to start router");

    // Give backend time to create socket
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Connect frontend
    let mut frontend =
        VhostUserFrontendDevice::connect(socket_path).expect("Failed to connect frontend");

    if let Err(e) = frontend.setup() {
        eprintln!("Frontend setup failed: {:?}", e);
        // Give time for backend logs to appear
        tokio::time::sleep(Duration::from_millis(500)).await;
        router.shutdown().await.expect("Failed to shutdown router");
        panic!("Frontend setup failed: {}", e);
    }

    // Provide RX buffers for receiving the echo reply
    for _ in 0..8 {
        frontend
            .provide_rx_buffer(4096)
            .expect("Failed to provide RX buffer");
    }

    // Create and send an ICMP echo request
    // Source: 10.99.100.2 (simulated VM), Dest: 10.99.100.1 (router)
    let packet = create_icmp_echo_request(1, [10, 99, 100, 2], [10, 99, 100, 1]);
    println!("Sending ICMP echo request ({} bytes)", packet.len());
    frontend
        .send_packet(&packet)
        .expect("Failed to send packet");

    // Wait for TX call eventfd (backend signals completion)
    println!("Waiting for TX completion...");
    let tx_signaled = frontend.wait_for_tx(1000).expect("TX call wait failed");

    if tx_signaled {
        println!("TX call signaled");
        assert!(
            frontend.wait_tx_complete().expect("TX check failed"),
            "TX not completed"
        );
        println!("TX completed");
    } else {
        // Check if maybe it completed without signal
        if frontend.wait_tx_complete().expect("TX check failed") {
            println!("TX completed (no signal)");
        } else {
            panic!("TX not completed - no signal and no used buffers");
        }
    }

    // Wait for RX call eventfd (backend sends echo reply)
    println!("Waiting for RX (echo reply)...");
    let rx_signaled = frontend.wait_for_rx(2000).expect("RX call wait failed");

    if rx_signaled {
        println!("RX call signaled");
    }

    // Try to receive the echo reply
    if let Some(reply) = frontend.recv_packet().expect("RX recv failed") {
        println!("Received reply: {} bytes", reply.len());

        // Verify it's an ICMP echo reply
        // Skip virtio-net header (10 bytes), IP header starts at offset 10
        let ip_start = VIRTIO_NET_HDR_SIZE;
        let icmp_start = ip_start + 20;

        assert!(reply.len() >= icmp_start + 8, "Reply too short");
        assert_eq!(
            reply[icmp_start], 0,
            "Expected ICMP Echo Reply (type 0), got {}",
            reply[icmp_start]
        );
        println!("ICMP Echo Reply received!");
    } else {
        panic!("No echo reply received");
    }

    // Drop frontend first to close the connection
    // This allows the vhost daemon to exit cleanly
    drop(frontend);

    // Cleanup
    router.shutdown().await.expect("Failed to shutdown router");
}
