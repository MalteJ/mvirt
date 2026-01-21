//! DHCP + ARP + Ping integration test
//!
//! Tests the full network stack initialization flow:
//! 1. DHCP DISCOVER → OFFER → REQUEST → ACK
//! 2. ARP resolution for gateway
//! 3. ICMP ping to gateway

use std::net::Ipv4Addr;
use std::time::Duration;

use mvirt_net::router::{Router, VhostConfig};
use mvirt_net::test_util::{
    DhcpMessageType, VhostUserFrontendDevice, create_arp_request, create_dhcp_discover,
    create_dhcp_request, create_icmp_echo_request, parse_arp_reply, parse_dhcp_response,
    parse_icmp_echo_reply,
};

/// Gateway MAC address used by the reactor
const GATEWAY_MAC: [u8; 6] = [0x02, 0x00, 0x00, 0x00, 0x00, 0x01];

/// Test the full DHCP + ARP + Ping flow
#[tokio::test]
async fn test_dhcp_arp_ping() {
    let _ = tracing_subscriber::fmt::try_init();

    let socket_path = "/tmp/iou-dhcp-test.sock";

    // VM configuration
    let vm_mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x01];
    let vm_ip = Ipv4Addr::new(10, 50, 0, 10);
    let gateway_ip = Ipv4Addr::new(10, 50, 0, 1);

    // Clean up any stale socket
    let _ = std::fs::remove_file(socket_path);

    // Start router with vhost-user backend and DHCP configuration
    let vhost_config =
        VhostConfig::new(socket_path.to_string(), vm_mac).with_ipv4(vm_ip, gateway_ip, 24);

    let router = Router::with_config_and_vhost(
        "tun_dhcp_test",
        gateway_ip, // Router IP is the gateway
        24,
        4096,
        256,
        256,
        Some(vhost_config),
    )
    .await
    .expect("Failed to start router");

    // Give backend time to create socket
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Connect frontend
    let mut frontend =
        VhostUserFrontendDevice::connect(socket_path).expect("Failed to connect frontend");
    frontend.setup().expect("Failed to setup frontend");

    // Provide RX buffers
    for _ in 0..16 {
        frontend
            .provide_rx_buffer(4096)
            .expect("Failed to provide RX buffer");
    }

    // =========================================================================
    // DHCP DISCOVER
    // =========================================================================
    println!("\n=== DHCP DISCOVER ===");
    let xid: u32 = 0x12345678;
    let discover = create_dhcp_discover(vm_mac, xid);
    println!(
        "Sending DHCP DISCOVER ({} bytes, xid=0x{:08x})",
        discover.len(),
        xid
    );
    frontend
        .send_packet(&discover)
        .expect("Failed to send DHCP DISCOVER");

    // Wait for TX completion
    let _ = frontend.wait_for_tx(1000);
    frontend.wait_tx_complete().ok();

    // Wait for DHCP OFFER
    println!("Waiting for DHCP OFFER...");
    let _ = frontend.wait_for_rx(2000).expect("RX wait failed");

    let offer_packet = frontend
        .recv_packet()
        .expect("RX recv failed")
        .expect("No DHCP OFFER received");
    let offer = parse_dhcp_response(&offer_packet).expect("Failed to parse DHCP OFFER");

    assert_eq!(
        offer.msg_type,
        DhcpMessageType::Offer,
        "Expected DHCP OFFER"
    );
    assert_eq!(offer.xid, xid, "XID mismatch");
    println!("Received DHCP OFFER:");
    println!("  Your IP: {:?}", offer.your_ip);
    println!("  Server IP: {:?}", offer.server_ip);
    println!("  Subnet mask: {:?}", offer.subnet_mask);
    println!("  Router: {:?}", offer.router);

    // Verify offered IP matches our configuration
    assert_eq!(offer.your_ip, vm_ip.octets(), "Offered IP mismatch");

    // =========================================================================
    // DHCP REQUEST
    // =========================================================================
    println!("\n=== DHCP REQUEST ===");
    let request = create_dhcp_request(vm_mac, xid, offer.your_ip, offer.server_ip);
    println!("Sending DHCP REQUEST for {:?}", offer.your_ip);
    frontend
        .send_packet(&request)
        .expect("Failed to send DHCP REQUEST");

    let _ = frontend.wait_for_tx(1000);
    frontend.wait_tx_complete().ok();

    // Wait for DHCP ACK
    println!("Waiting for DHCP ACK...");
    let _ = frontend.wait_for_rx(2000).expect("RX wait failed");

    let ack_packet = frontend
        .recv_packet()
        .expect("RX recv failed")
        .expect("No DHCP ACK received");
    let ack = parse_dhcp_response(&ack_packet).expect("Failed to parse DHCP ACK");

    assert_eq!(ack.msg_type, DhcpMessageType::Ack, "Expected DHCP ACK");
    assert_eq!(ack.xid, xid, "XID mismatch");
    println!("Received DHCP ACK:");
    println!("  Assigned IP: {:?}", ack.your_ip);
    println!("  Lease time: {:?}", ack.lease_time);

    let assigned_ip = ack.your_ip;
    let gateway = ack.router.expect("No gateway in DHCP ACK");

    println!(
        "\nDHCP complete! Assigned IP: {:?}, Gateway: {:?}",
        assigned_ip, gateway
    );

    // =========================================================================
    // ARP Resolution
    // =========================================================================
    println!("\n=== ARP Resolution ===");
    let arp_request = create_arp_request(vm_mac, assigned_ip, gateway);
    println!("Sending ARP request: who-has {:?}?", gateway);
    frontend
        .send_packet(&arp_request)
        .expect("Failed to send ARP request");

    let _ = frontend.wait_for_tx(1000);
    frontend.wait_tx_complete().ok();

    // Wait for ARP reply
    println!("Waiting for ARP reply...");
    let _ = frontend.wait_for_rx(2000).expect("RX wait failed");

    let arp_reply_packet = frontend
        .recv_packet()
        .expect("RX recv failed")
        .expect("No ARP reply received");
    let arp_reply = parse_arp_reply(&arp_reply_packet).expect("Failed to parse ARP reply");

    println!("Received ARP reply:");
    println!(
        "  {:?} is at {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        arp_reply.sender_ip,
        arp_reply.sender_mac[0],
        arp_reply.sender_mac[1],
        arp_reply.sender_mac[2],
        arp_reply.sender_mac[3],
        arp_reply.sender_mac[4],
        arp_reply.sender_mac[5]
    );

    assert_eq!(arp_reply.sender_ip, gateway, "ARP sender IP mismatch");
    assert_eq!(arp_reply.sender_mac, GATEWAY_MAC, "Gateway MAC mismatch");

    // =========================================================================
    // ICMP Ping
    // =========================================================================
    println!("\n=== ICMP Ping ===");
    let icmp_request = create_icmp_echo_request(
        vm_mac,
        arp_reply.sender_mac, // Use resolved gateway MAC
        assigned_ip,
        gateway,
        0x1234, // ID
        1,      // Sequence
    );
    println!("Sending ICMP echo request to {:?}", gateway);
    frontend
        .send_packet(&icmp_request)
        .expect("Failed to send ICMP request");

    let _ = frontend.wait_for_tx(1000);
    frontend.wait_tx_complete().ok();

    // Wait for ICMP reply
    println!("Waiting for ICMP echo reply...");
    let _ = frontend.wait_for_rx(2000).expect("RX wait failed");

    let icmp_reply_packet = frontend
        .recv_packet()
        .expect("RX recv failed")
        .expect("No ICMP reply received");
    let icmp_reply = parse_icmp_echo_reply(&icmp_reply_packet).expect("Failed to parse ICMP reply");

    println!("Received ICMP echo reply:");
    println!("  From: {:?}", icmp_reply.src_ip);
    println!("  ID: 0x{:04x}, Seq: {}", icmp_reply.id, icmp_reply.seq);

    assert_eq!(icmp_reply.src_ip, gateway, "ICMP source IP mismatch");
    assert_eq!(icmp_reply.dst_ip, assigned_ip, "ICMP dest IP mismatch");
    assert_eq!(icmp_reply.id, 0x1234, "ICMP ID mismatch");
    assert_eq!(icmp_reply.seq, 1, "ICMP sequence mismatch");

    println!("\n=== Test PASSED ===");
    println!("Successfully completed: DHCP → ARP → Ping");

    // Cleanup
    router.prepare_shutdown();
    drop(frontend);
    router.shutdown().await.expect("Failed to shutdown router");
}
