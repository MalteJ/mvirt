//! vhost-user integration tests
//!
//! Tests the vhost-user backend by simulating the VM/frontend side.
//!
//! Run all tests:
//!   cargo test -p mvirt-net --test vhost_integration
//!
//! Run specific tests:
//!   cargo test -p mvirt-net --test vhost_integration handshake
//!   cargo test -p mvirt-net --test vhost_integration arp
//!   cargo test -p mvirt-net --test vhost_integration dhcp

mod harness;

use harness::packets::{
    DhcpMessageType, arp_request, dhcp_discover, dhcp_request, is_arp_reply, parse_arp_reply,
    parse_dhcp_response,
};
use harness::{GATEWAY_IP, GATEWAY_MAC, TestBackend};

/// Virtio feature flags for assertions
const VIRTIO_F_VERSION_1: u64 = 1 << 32;
const VIRTIO_NET_F_MAC: u64 = 1 << 5;
const VIRTIO_RING_F_EVENT_IDX: u64 = 1 << 29;

// ============================================================================
// Handshake Tests
// ============================================================================

#[test]
fn test_handshake_and_feature_negotiation() {
    let backend = TestBackend::new("52:54:00:12:34:56", None);

    let client = backend.connect();
    assert!(
        client.is_ok(),
        "Handshake should succeed: {:?}",
        client.err()
    );

    let client = client.unwrap();
    assert!(
        client.has_feature(VIRTIO_F_VERSION_1),
        "Should have VERSION_1"
    );
    assert!(
        client.has_feature(VIRTIO_NET_F_MAC),
        "Should have MAC feature"
    );
}

#[test]
fn test_config_returns_mac() {
    let backend = TestBackend::new("52:54:00:aa:bb:cc", None);

    let mut client = backend.connect().expect("connect failed");

    let mac = client.get_mac_from_config().expect("get_config failed");
    assert_eq!(mac, [0x52, 0x54, 0x00, 0xaa, 0xbb, 0xcc]);
}

// ============================================================================
// Virtio TX/RX Tests
// ============================================================================

#[test]
fn test_send_packet() {
    let backend = TestBackend::new("52:54:00:12:34:56", None);

    let mut client = backend.connect().expect("connect failed");

    let frame = harness::packets::ethernet_frame(
        [0xff, 0xff, 0xff, 0xff, 0xff, 0xff],
        [0x52, 0x54, 0x00, 0x12, 0x34, 0x56],
        0x0800,
        &[0u8; 64],
    );

    let result = client.send_packet(&frame);
    assert!(result.is_ok(), "TX should work: {:?}", result.err());
}

#[test]
fn test_tx_completion() {
    let backend = TestBackend::new("52:54:00:12:34:56", None);

    let mut client = backend.connect().expect("connect failed");

    let frame = harness::packets::ethernet_frame(
        [0xff, 0xff, 0xff, 0xff, 0xff, 0xff],
        [0x52, 0x54, 0x00, 0x12, 0x34, 0x56],
        0x0800,
        &[0u8; 64],
    );

    // Send packet without waiting
    client.send_packet_nowait(&frame).expect("send failed");

    // Wait for TX completion with timeout (simulates what a real driver does)
    let completed = client.wait_tx_completion(1000);
    assert!(
        completed.is_ok(),
        "TX completion should succeed: {:?}",
        completed.err()
    );
    assert_eq!(completed.unwrap(), 1, "Should complete 1 descriptor");
}

#[test]
fn test_tx_burst_completion() {
    let backend = TestBackend::new("52:54:00:12:34:56", None);

    let mut client = backend.connect().expect("connect failed");

    // Create multiple frames
    let frames: Vec<Vec<u8>> = (0..10)
        .map(|i| {
            harness::packets::ethernet_frame(
                [0xff, 0xff, 0xff, 0xff, 0xff, 0xff],
                [0x52, 0x54, 0x00, 0x12, 0x34, i as u8],
                0x0800,
                &[i as u8; 64],
            )
        })
        .collect();

    // Send burst and wait for all completions
    let completed = client.send_burst(&frames, 2000);
    assert!(
        completed.is_ok(),
        "TX burst should complete: {:?}",
        completed.err()
    );
    assert_eq!(completed.unwrap(), 10, "Should complete all 10 descriptors");
}

#[test]
fn test_tx_large_burst() {
    let backend = TestBackend::new("52:54:00:12:34:56", None);

    let mut client = backend.connect().expect("connect failed");

    // Create a larger burst (50 packets)
    let frames: Vec<Vec<u8>> = (0..50)
        .map(|i| {
            harness::packets::ethernet_frame(
                [0xff, 0xff, 0xff, 0xff, 0xff, 0xff],
                [0x52, 0x54, 0x00, 0x12, 0x34, (i % 256) as u8],
                0x0800,
                &[0u8; 128],
            )
        })
        .collect();

    let completed = client.send_burst(&frames, 5000);
    assert!(
        completed.is_ok(),
        "Large TX burst should complete: {:?}",
        completed.err()
    );
    assert_eq!(completed.unwrap(), 50, "Should complete all 50 descriptors");
}

#[test]
fn test_tx_sequential_packets() {
    let backend = TestBackend::new("52:54:00:12:34:56", None);

    let mut client = backend.connect().expect("connect failed");

    // Send packets one by one and verify each completes
    for i in 0..5 {
        let frame = harness::packets::ethernet_frame(
            [0xff, 0xff, 0xff, 0xff, 0xff, 0xff],
            [0x52, 0x54, 0x00, 0x12, 0x34, i as u8],
            0x0800,
            &[i as u8; 64],
        );

        client.send_packet_nowait(&frame).expect("send failed");
        let completed = client.wait_tx_completion(500);
        assert!(
            completed.is_ok(),
            "TX {} should complete: {:?}",
            i,
            completed.err()
        );
    }
}

// ============================================================================
// EVENT_IDX Tests (realistic notification suppression)
// ============================================================================

#[test]
fn test_tx_with_event_idx_suppression() {
    // This test simulates realistic EVENT_IDX behavior:
    // - Guest sends multiple packets
    // - Guest sets used_event to control when it wants interrupts
    // - Device should respect the used_event value

    let backend = TestBackend::new("52:54:00:12:34:56", None);
    let mut client = backend.connect().expect("connect failed");

    // Verify EVENT_IDX is negotiated
    assert!(
        client.has_feature(VIRTIO_RING_F_EVENT_IDX),
        "EVENT_IDX should be negotiated"
    );

    // Send burst of packets - this exercises the device's EVENT_IDX handling
    let frames: Vec<Vec<u8>> = (0..20)
        .map(|i| {
            harness::packets::ethernet_frame(
                [0xff, 0xff, 0xff, 0xff, 0xff, 0xff],
                [0x52, 0x54, 0x00, 0x12, 0x34, i as u8],
                0x0800,
                &[i as u8; 64],
            )
        })
        .collect();

    // Send all packets and wait for completions
    let completed = client.send_burst(&frames, 3000);
    assert!(
        completed.is_ok(),
        "TX burst with EVENT_IDX should complete: {:?}",
        completed.err()
    );
    assert_eq!(
        completed.unwrap(),
        20,
        "All 20 packets should complete with EVENT_IDX"
    );
}

#[test]
fn test_tx_rapid_fire() {
    // Stress test: send packets as fast as possible without waiting
    // This mimics real network workloads

    let backend = TestBackend::new("52:54:00:12:34:56", None);
    let mut client = backend.connect().expect("connect failed");

    // Send 100 packets rapidly
    for i in 0..100u8 {
        let frame = harness::packets::ethernet_frame(
            [0xff, 0xff, 0xff, 0xff, 0xff, 0xff],
            [0x52, 0x54, 0x00, 0x12, 0x34, i],
            0x0800,
            &[i; 64],
        );

        client.send_packet_nowait(&frame).expect("send failed");
    }

    // Now wait for all completions
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_secs(5);
    let mut completed = 0u32;

    while completed < 100 && start.elapsed() < timeout {
        match client.wait_tx_completion(100) {
            Ok(n) => completed += n,
            Err(_) => {} // Timeout on this poll, continue
        }
    }

    assert_eq!(completed, 100, "All 100 rapid-fire packets should complete");
}

// ============================================================================
// ARP Tests
// ============================================================================

#[test]
fn test_arp_request_gets_reply() {
    let backend = TestBackend::new("52:54:00:12:34:56", Some("10.0.0.5"));

    let mut client = backend.connect().expect("connect failed");

    let arp_req = arp_request(
        [0x52, 0x54, 0x00, 0x12, 0x34, 0x56],
        [10, 0, 0, 100],
        [169, 254, 0, 1],
    );

    client.send_packet(&arp_req).expect("send failed");

    let reply = client.recv_packet(2000);
    assert!(reply.is_ok(), "Should receive ARP reply: {:?}", reply.err());

    let reply = reply.unwrap();
    assert!(is_arp_reply(&reply), "Should be ARP reply");

    let arp = parse_arp_reply(&reply).expect("parse ARP failed");
    assert_eq!(arp.sender_ip, [169, 254, 0, 1]);
    assert_eq!(arp.sender_mac, GATEWAY_MAC);
}

// ============================================================================
// DHCP Tests
// ============================================================================

#[test]
fn test_dhcp_discover_gets_offer() {
    let backend = TestBackend::new("52:54:00:12:34:56", Some("10.0.0.5"));

    let mut client = backend.connect().expect("connect failed");

    let discover = dhcp_discover([0x52, 0x54, 0x00, 0x12, 0x34, 0x56], 0x12345678);
    client.send_packet(&discover).expect("send failed");

    let reply = client.recv_packet(2000);
    assert!(
        reply.is_ok(),
        "Should receive DHCP reply: {:?}",
        reply.err()
    );

    let reply = reply.unwrap();
    let dhcp = parse_dhcp_response(&reply).expect("parse DHCP failed");

    // Message type and transaction ID
    assert_eq!(dhcp.message_type, DhcpMessageType::Offer);
    assert_eq!(dhcp.xid, 0x12345678);

    // Assigned IP address
    assert_eq!(dhcp.your_ip, [10, 0, 0, 5]);

    // Server identifier (gateway)
    assert_eq!(dhcp.server_ip, GATEWAY_IP);

    // Subnet mask (/32 for point-to-point)
    assert_eq!(dhcp.subnet_mask, Some([255, 255, 255, 255]));

    // Router (gateway)
    assert_eq!(dhcp.router, Some(GATEWAY_IP));

    // DNS servers
    assert_eq!(dhcp.dns_servers, vec![[1, 1, 1, 1], [8, 8, 8, 8]]);

    // Lease time (24 hours)
    assert_eq!(dhcp.lease_time, Some(86400));
}

#[test]
fn test_dhcp_full_flow() {
    let backend = TestBackend::new("52:54:00:12:34:56", Some("10.0.0.5"));

    let mut client = backend.connect().expect("connect failed");
    let mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
    let xid = 0xdeadbeef_u32;

    // 1. DISCOVER -> OFFER
    let discover = dhcp_discover(mac, xid);
    client.send_packet(&discover).expect("send discover failed");

    let reply = client.recv_packet(2000).expect("no offer received");
    let offer = parse_dhcp_response(&reply).expect("parse offer failed");
    assert_eq!(offer.message_type, DhcpMessageType::Offer);
    assert_eq!(offer.xid, xid);
    assert_eq!(offer.your_ip, [10, 0, 0, 5]);

    // 2. REQUEST -> ACK
    let request = dhcp_request(mac, xid, offer.your_ip, offer.server_ip);
    client.send_packet(&request).expect("send request failed");

    let reply = client.recv_packet(2000).expect("no ack received");
    let ack = parse_dhcp_response(&reply).expect("parse ack failed");

    // Message type and transaction ID
    assert_eq!(ack.message_type, DhcpMessageType::Ack);
    assert_eq!(ack.xid, xid);

    // Same IP as offered
    assert_eq!(ack.your_ip, offer.your_ip);

    // Server identifier
    assert_eq!(ack.server_ip, GATEWAY_IP);

    // Subnet mask (/32)
    assert_eq!(ack.subnet_mask, Some([255, 255, 255, 255]));

    // Router (gateway)
    assert_eq!(ack.router, Some(GATEWAY_IP));

    // DNS servers
    assert_eq!(ack.dns_servers, vec![[1, 1, 1, 1], [8, 8, 8, 8]]);

    // Lease time (24 hours = 86400 seconds)
    assert_eq!(ack.lease_time, Some(86400));
}
