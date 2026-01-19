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
    DhcpMessageType, arp_request, dhcp_discover, dhcp_request, icmp_echo_request, is_arp_reply,
    is_icmp_echo_reply, parse_arp_reply, parse_dhcp_response, parse_icmp_echo_reply,
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
        if let Ok(n) = client.wait_tx_completion(100) {
            completed += n; // Timeout on this poll is expected, continue
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
// ICMP Tests
// ============================================================================

#[test]
fn test_icmp_ping_gateway() {
    let backend = TestBackend::new("52:54:00:12:34:56", Some("10.0.0.5"));

    let mut client = backend.connect().expect("connect failed");

    // First, get the gateway MAC via ARP (like a real VM would)
    let arp_req = arp_request(
        [0x52, 0x54, 0x00, 0x12, 0x34, 0x56],
        [10, 0, 0, 5],
        [169, 254, 0, 1],
    );
    client.send_packet(&arp_req).expect("ARP send failed");
    let arp_reply = client.recv_packet(2000).expect("ARP reply expected");
    let arp = parse_arp_reply(&arp_reply).expect("parse ARP failed");
    assert_eq!(arp.sender_mac, GATEWAY_MAC);

    // Now send ICMP echo request to gateway
    let ping = icmp_echo_request(
        [0x52, 0x54, 0x00, 0x12, 0x34, 0x56], // src MAC
        GATEWAY_MAC,                          // dst MAC (gateway)
        [10, 0, 0, 5],                        // src IP
        GATEWAY_IP,                           // dst IP (gateway)
        0x1234,                               // ident
        1,                                    // seq_no
        b"ping test",                         // data
    );

    client.send_packet(&ping).expect("ICMP send failed");

    let reply = client.recv_packet(2000);
    assert!(
        reply.is_ok(),
        "Should receive ICMP echo reply: {:?}",
        reply.err()
    );

    let reply = reply.unwrap();
    assert!(is_icmp_echo_reply(&reply), "Should be ICMP echo reply");

    let icmp = parse_icmp_echo_reply(&reply).expect("parse ICMP failed");
    assert_eq!(icmp.src_ip, GATEWAY_IP, "Reply should come from gateway");
    assert_eq!(
        icmp.dst_ip,
        [10, 0, 0, 5],
        "Reply should be addressed to VM"
    );
    assert_eq!(icmp.ident, 0x1234, "Ident should match");
    assert_eq!(icmp.seq_no, 1, "Seq should match");
    assert_eq!(icmp.data, b"ping test", "Data should match");
}

#[test]
fn test_icmp_ping_non_gateway_ignored() {
    let backend = TestBackend::new("52:54:00:12:34:56", Some("10.0.0.5"));

    let mut client = backend.connect().expect("connect failed");

    // Send ICMP echo request to non-gateway IP (should not get reply)
    let ping = icmp_echo_request(
        [0x52, 0x54, 0x00, 0x12, 0x34, 0x56],
        [0xff, 0xff, 0xff, 0xff, 0xff, 0xff], // broadcast
        [10, 0, 0, 5],
        [8, 8, 8, 8], // Not gateway
        0x5678,
        1,
        b"test",
    );

    client.send_packet(&ping).expect("send failed");

    // Should not receive a reply (use short timeout)
    let reply = client.recv_packet(500);
    assert!(
        reply.is_err(),
        "Should NOT receive ICMP reply for non-gateway IP"
    );
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

// ============================================================================
// NDP Tests (IPv6 Neighbor Discovery)
// ============================================================================

use harness::packets::{
    Dhcpv6MsgType, GATEWAY_IPV6, dhcpv6_request, dhcpv6_solicit, icmpv6_echo_request,
    neighbor_solicitation, parse_dhcpv6_response, parse_icmpv6_echo_reply,
    parse_neighbor_advertisement, parse_router_advertisement, router_solicitation,
};

#[test]
fn test_ndp_neighbor_solicitation_gets_advertisement() {
    let backend =
        TestBackend::new_with_ipv6("52:54:00:12:34:56", Some("10.0.0.5"), Some("fd00::5"));

    let mut client = backend.connect().expect("connect failed");

    // VM's link-local address (derived from MAC using EUI-64)
    let vm_link_local = [
        0xfe, 0x80, 0, 0, 0, 0, 0, 0, 0x50, 0x54, 0x00, 0xff, 0xfe, 0x12, 0x34, 0x56,
    ];

    // Send NS for gateway (fe80::1)
    let ns = neighbor_solicitation(
        [0x52, 0x54, 0x00, 0x12, 0x34, 0x56],
        vm_link_local,
        GATEWAY_IPV6,
    );

    client.send_packet(&ns).expect("NS send failed");

    let reply = client.recv_packet(2000);
    assert!(
        reply.is_ok(),
        "Should receive Neighbor Advertisement: {:?}",
        reply.err()
    );

    let na = parse_neighbor_advertisement(&reply.unwrap()).expect("parse NA failed");
    assert_eq!(na.target_ip, GATEWAY_IPV6, "NA should be for gateway");
    assert_eq!(na.target_mac, GATEWAY_MAC, "Gateway MAC should match");
    assert!(na.router, "Gateway should have ROUTER flag");
    assert!(na.solicited, "NA should have SOLICITED flag");
}

#[test]
fn test_ndp_router_solicitation_gets_advertisement() {
    let backend =
        TestBackend::new_with_ipv6("52:54:00:12:34:56", Some("10.0.0.5"), Some("fd00::5"));

    let mut client = backend.connect().expect("connect failed");

    // VM's link-local address
    let vm_link_local = [
        0xfe, 0x80, 0, 0, 0, 0, 0, 0, 0x50, 0x54, 0x00, 0xff, 0xfe, 0x12, 0x34, 0x56,
    ];

    // Send RS
    let rs = router_solicitation([0x52, 0x54, 0x00, 0x12, 0x34, 0x56], vm_link_local);

    client.send_packet(&rs).expect("RS send failed");

    let reply = client.recv_packet(2000);
    assert!(
        reply.is_ok(),
        "Should receive Router Advertisement: {:?}",
        reply.err()
    );

    let ra = parse_router_advertisement(&reply.unwrap()).expect("parse RA failed");
    assert_eq!(ra.src_ip, GATEWAY_IPV6, "RA should come from gateway");
    assert_eq!(ra.src_mac, GATEWAY_MAC, "Gateway MAC should match");
    assert!(ra.managed, "RA should have M flag (managed via DHCPv6)");
    assert!(ra.other_config, "RA should have O flag");
    assert!(ra.router_lifetime > 0, "Router lifetime should be > 0");
    // No prefix should be advertised - all addresses via DHCPv6 only
    // This ensures VMs route all IPv6 traffic through the gateway
    assert!(
        ra.prefix.is_none(),
        "RA should NOT contain prefix (forces routing through gateway)"
    );
}

// ============================================================================
// DHCPv6 Tests
// ============================================================================

#[test]
fn test_dhcpv6_solicit_gets_advertise() {
    let backend =
        TestBackend::new_with_ipv6("52:54:00:12:34:56", Some("10.0.0.5"), Some("fd00::5"));

    let mut client = backend.connect().expect("connect failed");

    // VM's link-local address
    let vm_link_local = [
        0xfe, 0x80, 0, 0, 0, 0, 0, 0, 0x50, 0x54, 0x00, 0xff, 0xfe, 0x12, 0x34, 0x56,
    ];

    let xid = [0x12, 0x34, 0x56];
    let iaid = 0xDEADBEEF_u32; // Use non-trivial IAID to verify echoing
    let solicit = dhcpv6_solicit(
        [0x52, 0x54, 0x00, 0x12, 0x34, 0x56],
        vm_link_local,
        xid,
        iaid,
    );

    client.send_packet(&solicit).expect("SOLICIT send failed");

    let reply = client.recv_packet(2000);
    assert!(
        reply.is_ok(),
        "Should receive DHCPv6 ADVERTISE: {:?}",
        reply.err()
    );

    let advertise = parse_dhcpv6_response(&reply.unwrap()).expect("parse ADVERTISE failed");
    assert_eq!(advertise.message_type, Dhcpv6MsgType::Advertise);
    assert_eq!(advertise.xid, xid);

    // IAID should be echoed back
    assert_eq!(
        advertise.iaid,
        Some(iaid),
        "IAID should be echoed from client request"
    );

    // Should offer fd00::5
    let expected_ip = [0xfd, 0x00, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x05];
    assert_eq!(
        advertise.assigned_ip,
        Some(expected_ip),
        "Should offer configured IPv6 address"
    );

    // Server DUID should be present
    assert!(
        !advertise.server_duid.is_empty(),
        "Server DUID should be present"
    );
}

#[test]
fn test_dhcpv6_full_flow() {
    let backend =
        TestBackend::new_with_ipv6("52:54:00:12:34:56", Some("10.0.0.5"), Some("fd00::5"));

    let mut client = backend.connect().expect("connect failed");

    let mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
    let vm_link_local = [
        0xfe, 0x80, 0, 0, 0, 0, 0, 0, 0x50, 0x54, 0x00, 0xff, 0xfe, 0x12, 0x34, 0x56,
    ];
    let xid = [0xde, 0xad, 0xbe];
    let iaid = 0xCAFEBABE_u32; // Use non-trivial IAID to verify echoing

    // 1. SOLICIT -> ADVERTISE
    let solicit = dhcpv6_solicit(mac, vm_link_local, xid, iaid);
    client.send_packet(&solicit).expect("SOLICIT send failed");

    let reply = client.recv_packet(2000).expect("no ADVERTISE received");
    let advertise = parse_dhcpv6_response(&reply).expect("parse ADVERTISE failed");
    assert_eq!(advertise.message_type, Dhcpv6MsgType::Advertise);
    assert_eq!(advertise.xid, xid);
    assert_eq!(
        advertise.iaid,
        Some(iaid),
        "IAID should be echoed in ADVERTISE"
    );

    let server_duid = advertise.server_duid.clone();
    let expected_ip = [0xfd, 0x00, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x05];
    assert_eq!(advertise.assigned_ip, Some(expected_ip));

    // 2. REQUEST -> REPLY
    let request = dhcpv6_request(mac, vm_link_local, xid, server_duid, iaid);
    client.send_packet(&request).expect("REQUEST send failed");

    let reply = client.recv_packet(2000).expect("no REPLY received");
    let ack = parse_dhcpv6_response(&reply).expect("parse REPLY failed");

    assert_eq!(ack.message_type, Dhcpv6MsgType::Reply);
    assert_eq!(ack.xid, xid);
    assert_eq!(ack.iaid, Some(iaid), "IAID should be echoed in REPLY");
    assert_eq!(ack.assigned_ip, Some(expected_ip));

    // Check lifetimes
    assert!(ack.preferred_lifetime.is_some());
    assert!(ack.valid_lifetime.is_some());
    assert!(ack.valid_lifetime.unwrap() >= ack.preferred_lifetime.unwrap());

    // Check DNS servers
    assert!(!ack.dns_servers.is_empty(), "Should have DNS servers");
}

// ============================================================================
// ICMPv6 Tests (IPv6 Ping)
// ============================================================================

#[test]
fn test_icmpv6_ping_gateway() {
    let backend =
        TestBackend::new_with_ipv6("52:54:00:12:34:56", Some("10.0.0.5"), Some("fd00::5"));

    let mut client = backend.connect().expect("connect failed");

    let mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];

    // First, resolve gateway MAC via NDP (like a real VM would)
    let vm_link_local = [
        0xfe, 0x80, 0, 0, 0, 0, 0, 0, 0x50, 0x54, 0x00, 0xff, 0xfe, 0x12, 0x34, 0x56,
    ];
    let ns = neighbor_solicitation(mac, vm_link_local, GATEWAY_IPV6);
    client.send_packet(&ns).expect("NS send failed");

    let na_reply = client.recv_packet(2000).expect("NA expected");
    let na = parse_neighbor_advertisement(&na_reply).expect("parse NA failed");
    assert_eq!(na.target_mac, GATEWAY_MAC);

    // Now send ICMPv6 echo request to gateway
    let ping = icmpv6_echo_request(
        mac,
        GATEWAY_MAC,   // dst MAC (gateway)
        vm_link_local, // src IP
        GATEWAY_IPV6,  // dst IP (gateway)
        0x1234,        // ident
        1,             // seq_no
        b"ipv6 ping",  // data
    );

    client.send_packet(&ping).expect("ICMPv6 send failed");

    let reply = client.recv_packet(2000);
    assert!(
        reply.is_ok(),
        "Should receive ICMPv6 echo reply: {:?}",
        reply.err()
    );

    let icmp = parse_icmpv6_echo_reply(&reply.unwrap()).expect("parse ICMPv6 failed");
    assert_eq!(icmp.src_ip, GATEWAY_IPV6, "Reply should come from gateway");
    assert_eq!(
        icmp.dst_ip, vm_link_local,
        "Reply should be addressed to VM"
    );
    assert_eq!(icmp.ident, 0x1234, "Ident should match");
    assert_eq!(icmp.seq_no, 1, "Seq should match");
    assert_eq!(icmp.data, b"ipv6 ping", "Data should match");
}

#[test]
fn test_icmpv6_ping_non_gateway_ignored() {
    let backend =
        TestBackend::new_with_ipv6("52:54:00:12:34:56", Some("10.0.0.5"), Some("fd00::5"));

    let mut client = backend.connect().expect("connect failed");

    let mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
    let vm_link_local = [
        0xfe, 0x80, 0, 0, 0, 0, 0, 0, 0x50, 0x54, 0x00, 0xff, 0xfe, 0x12, 0x34, 0x56,
    ];

    // Send ICMPv6 echo request to non-gateway IP (should not get reply)
    let non_gateway = [
        0x20, 0x01, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x01,
    ];
    let ping = icmpv6_echo_request(
        mac,
        [0x33, 0x33, 0xff, 0x00, 0x00, 0x01], // multicast MAC
        vm_link_local,
        non_gateway, // Not gateway
        0x5678,
        1,
        b"test",
    );

    client.send_packet(&ping).expect("send failed");

    // Should not receive a reply (use short timeout)
    let reply = client.recv_packet(500);
    assert!(
        reply.is_err(),
        "Should NOT receive ICMPv6 reply for non-gateway IP"
    );
}

// ============================================================================
// Inter-vNIC Routing Tests
// ============================================================================

use harness::packets::{is_icmp_echo_request, parse_icmp_echo_request};
use harness::{RoutingNicConfig, RoutingTestBackend};

#[test]
fn test_routing_packet_from_a_to_b() {
    // Set up two vNICs with routing
    let backend = RoutingTestBackend::new(
        RoutingNicConfig {
            id: "nic-a".to_string(),
            mac: "52:54:00:00:00:01".to_string(),
            ipv4: "10.0.0.1".to_string(),
            ipv6: None,
        },
        RoutingNicConfig {
            id: "nic-b".to_string(),
            mac: "52:54:00:00:00:02".to_string(),
            ipv4: "10.0.0.2".to_string(),
            ipv6: None,
        },
    );

    // Connect both clients
    let mut client_a = backend.connect_a().expect("connect A failed");
    let mut client_b = backend.connect_b().expect("connect B failed");

    // Client A sends ICMP echo request to client B's IP (10.0.0.2)
    // Use gateway MAC as dst since we're routing through the virtual gateway
    let ping = icmp_echo_request(
        [0x52, 0x54, 0x00, 0x00, 0x00, 0x01], // src MAC (client A)
        GATEWAY_MAC,                          // dst MAC (gateway - will route)
        [10, 0, 0, 1],                        // src IP (client A)
        [10, 0, 0, 2],                        // dst IP (client B)
        0xABCD,                               // ident
        1,                                    // seq_no
        b"routing test",                      // data
    );

    client_a.send_packet(&ping).expect("send from A failed");

    // Client B should receive the routed ICMP echo request
    let received = client_b.recv_packet(3000);
    assert!(
        received.is_ok(),
        "Client B should receive routed packet: {:?}",
        received.err()
    );

    let received = received.unwrap();
    assert!(
        is_icmp_echo_request(&received),
        "Should be ICMP echo request"
    );

    let icmp = parse_icmp_echo_request(&received).expect("parse ICMP failed");
    assert_eq!(icmp.src_ip, [10, 0, 0, 1], "Src IP should be client A");
    assert_eq!(icmp.dst_ip, [10, 0, 0, 2], "Dst IP should be client B");
    assert_eq!(icmp.ident, 0xABCD, "Ident should match");
    assert_eq!(icmp.seq_no, 1, "Seq should match");
    assert_eq!(icmp.data, b"routing test", "Data should match");

    // Verify MAC addresses were rewritten by the router
    assert_eq!(
        icmp.dst_mac,
        [0x52, 0x54, 0x00, 0x00, 0x00, 0x02],
        "Dst MAC should be client B's MAC"
    );
    assert_eq!(
        icmp.src_mac, GATEWAY_MAC,
        "Src MAC should be gateway MAC (router)"
    );
}

#[test]
fn test_routing_bidirectional() {
    // Set up two vNICs with routing
    let backend = RoutingTestBackend::new(
        RoutingNicConfig {
            id: "nic-a".to_string(),
            mac: "52:54:00:00:00:11".to_string(),
            ipv4: "10.0.0.11".to_string(),
            ipv6: None,
        },
        RoutingNicConfig {
            id: "nic-b".to_string(),
            mac: "52:54:00:00:00:22".to_string(),
            ipv4: "10.0.0.22".to_string(),
            ipv6: None,
        },
    );

    let mut client_a = backend.connect_a().expect("connect A failed");
    let mut client_b = backend.connect_b().expect("connect B failed");

    // A -> B
    let ping_a_to_b = icmp_echo_request(
        [0x52, 0x54, 0x00, 0x00, 0x00, 0x11],
        GATEWAY_MAC,
        [10, 0, 0, 11],
        [10, 0, 0, 22],
        0x1111,
        1,
        b"a to b",
    );

    client_a
        .send_packet(&ping_a_to_b)
        .expect("send A->B failed");

    let received_b = client_b.recv_packet(3000).expect("B should receive packet");
    let icmp_b = parse_icmp_echo_request(&received_b).expect("parse failed");
    assert_eq!(icmp_b.src_ip, [10, 0, 0, 11]);
    assert_eq!(icmp_b.dst_ip, [10, 0, 0, 22]);
    assert_eq!(icmp_b.data, b"a to b");
    // Verify MAC rewriting
    assert_eq!(
        icmp_b.dst_mac,
        [0x52, 0x54, 0x00, 0x00, 0x00, 0x22],
        "Dst MAC should be B's MAC"
    );
    assert_eq!(icmp_b.src_mac, GATEWAY_MAC, "Src MAC should be gateway");

    // B -> A
    let ping_b_to_a = icmp_echo_request(
        [0x52, 0x54, 0x00, 0x00, 0x00, 0x22],
        GATEWAY_MAC,
        [10, 0, 0, 22],
        [10, 0, 0, 11],
        0x2222,
        2,
        b"b to a",
    );

    client_b
        .send_packet(&ping_b_to_a)
        .expect("send B->A failed");

    let received_a = client_a.recv_packet(3000).expect("A should receive packet");
    let icmp_a = parse_icmp_echo_request(&received_a).expect("parse failed");
    assert_eq!(icmp_a.src_ip, [10, 0, 0, 22]);
    assert_eq!(icmp_a.dst_ip, [10, 0, 0, 11]);
    assert_eq!(icmp_a.data, b"b to a");
    // Verify MAC rewriting
    assert_eq!(
        icmp_a.dst_mac,
        [0x52, 0x54, 0x00, 0x00, 0x00, 0x11],
        "Dst MAC should be A's MAC"
    );
    assert_eq!(icmp_a.src_mac, GATEWAY_MAC, "Src MAC should be gateway");
}

#[test]
fn test_routing_ttl_decrement() {
    // Verify TTL is decremented when routing
    let backend = RoutingTestBackend::new(
        RoutingNicConfig {
            id: "nic-ttl-a".to_string(),
            mac: "52:54:00:00:00:AA".to_string(),
            ipv4: "10.0.0.100".to_string(),
            ipv6: None,
        },
        RoutingNicConfig {
            id: "nic-ttl-b".to_string(),
            mac: "52:54:00:00:00:BB".to_string(),
            ipv4: "10.0.0.200".to_string(),
            ipv6: None,
        },
    );

    let mut client_a = backend.connect_a().expect("connect A failed");
    let mut client_b = backend.connect_b().expect("connect B failed");

    // Send packet with TTL=64 (default)
    let ping = icmp_echo_request(
        [0x52, 0x54, 0x00, 0x00, 0x00, 0xAA],
        GATEWAY_MAC,
        [10, 0, 0, 100],
        [10, 0, 0, 200],
        0x5555,
        1,
        b"ttl test",
    );

    client_a.send_packet(&ping).expect("send failed");

    let received = client_b.recv_packet(3000).expect("should receive packet");

    // Parse raw IPv4 packet to check TTL
    // Ethernet header is 14 bytes, IPv4 TTL is at offset 8
    let ttl = received[14 + 8];
    assert_eq!(ttl, 63, "TTL should be decremented from 64 to 63");
}

// ============================================================================
// IPv6 Routing Tests
// ============================================================================

use harness::packets::{is_icmpv6_echo_request, parse_icmpv6_echo_request};

#[test]
fn test_routing_ipv6_packet_from_a_to_b() {
    // Set up two vNICs with IPv6 addresses
    let backend = RoutingTestBackend::new(
        RoutingNicConfig {
            id: "nic-v6-a".to_string(),
            mac: "52:54:00:00:00:A1".to_string(),
            ipv4: "10.0.0.1".to_string(),
            ipv6: Some("fd00::1".to_string()),
        },
        RoutingNicConfig {
            id: "nic-v6-b".to_string(),
            mac: "52:54:00:00:00:B2".to_string(),
            ipv4: "10.0.0.2".to_string(),
            ipv6: Some("fd00::2".to_string()),
        },
    );

    let mut client_a = backend.connect_a().expect("connect A failed");
    let mut client_b = backend.connect_b().expect("connect B failed");

    // fd00::1 and fd00::2 as byte arrays
    let src_ip: [u8; 16] = [0xfd, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1];
    let dst_ip: [u8; 16] = [0xfd, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2];

    // Client A sends ICMPv6 echo request to client B's IPv6 (fd00::2)
    let ping = icmpv6_echo_request(
        [0x52, 0x54, 0x00, 0x00, 0x00, 0xA1], // src MAC (client A)
        GATEWAY_MAC,                          // dst MAC (gateway - will route)
        src_ip,                               // src IP (client A)
        dst_ip,                               // dst IP (client B)
        0xABCD,                               // ident
        1,                                    // seq_no
        b"ipv6 routing test",                 // data
    );

    client_a.send_packet(&ping).expect("send from A failed");

    // Client B should receive the routed ICMPv6 echo request
    let received = client_b.recv_packet(3000);
    assert!(
        received.is_ok(),
        "Client B should receive routed IPv6 packet: {:?}",
        received.err()
    );

    let received = received.unwrap();
    assert!(
        is_icmpv6_echo_request(&received),
        "Should be ICMPv6 echo request"
    );

    let icmp = parse_icmpv6_echo_request(&received).expect("parse ICMPv6 failed");
    assert_eq!(icmp.src_ip, src_ip, "Src IP should be client A");
    assert_eq!(icmp.dst_ip, dst_ip, "Dst IP should be client B");
    assert_eq!(icmp.ident, 0xABCD, "Ident should match");
    assert_eq!(icmp.seq_no, 1, "Seq should match");
    assert_eq!(icmp.data, b"ipv6 routing test", "Data should match");

    // Verify MAC addresses were rewritten by the router
    assert_eq!(
        icmp.dst_mac,
        [0x52, 0x54, 0x00, 0x00, 0x00, 0xB2],
        "Dst MAC should be client B's MAC"
    );
    assert_eq!(
        icmp.src_mac, GATEWAY_MAC,
        "Src MAC should be gateway MAC (router)"
    );

    // Verify Hop Limit was decremented (default is 64)
    assert_eq!(
        icmp.hop_limit, 63,
        "Hop Limit should be decremented from 64 to 63"
    );
}

#[test]
fn test_routing_ipv6_bidirectional() {
    let backend = RoutingTestBackend::new(
        RoutingNicConfig {
            id: "nic-v6-bidir-a".to_string(),
            mac: "52:54:00:00:00:AA".to_string(),
            ipv4: "10.0.0.11".to_string(),
            ipv6: Some("fd00::11".to_string()),
        },
        RoutingNicConfig {
            id: "nic-v6-bidir-b".to_string(),
            mac: "52:54:00:00:00:BB".to_string(),
            ipv4: "10.0.0.22".to_string(),
            ipv6: Some("fd00::22".to_string()),
        },
    );

    let mut client_a = backend.connect_a().expect("connect A failed");
    let mut client_b = backend.connect_b().expect("connect B failed");

    let ip_a: [u8; 16] = [0xfd, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x11];
    let ip_b: [u8; 16] = [0xfd, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x22];

    // A -> B
    let ping_a_to_b = icmpv6_echo_request(
        [0x52, 0x54, 0x00, 0x00, 0x00, 0xAA],
        GATEWAY_MAC,
        ip_a,
        ip_b,
        0x1111,
        1,
        b"a to b v6",
    );

    client_a
        .send_packet(&ping_a_to_b)
        .expect("send A->B failed");

    let received_b = client_b
        .recv_packet(3000)
        .expect("B should receive IPv6 packet");
    let icmp_b = parse_icmpv6_echo_request(&received_b).expect("parse failed");
    assert_eq!(icmp_b.src_ip, ip_a);
    assert_eq!(icmp_b.dst_ip, ip_b);
    assert_eq!(icmp_b.data, b"a to b v6");
    assert_eq!(icmp_b.dst_mac, [0x52, 0x54, 0x00, 0x00, 0x00, 0xBB]);
    assert_eq!(icmp_b.src_mac, GATEWAY_MAC);
    assert_eq!(icmp_b.hop_limit, 63);

    // B -> A
    let ping_b_to_a = icmpv6_echo_request(
        [0x52, 0x54, 0x00, 0x00, 0x00, 0xBB],
        GATEWAY_MAC,
        ip_b,
        ip_a,
        0x2222,
        2,
        b"b to a v6",
    );

    client_b
        .send_packet(&ping_b_to_a)
        .expect("send B->A failed");

    let received_a = client_a
        .recv_packet(3000)
        .expect("A should receive IPv6 packet");
    let icmp_a = parse_icmpv6_echo_request(&received_a).expect("parse failed");
    assert_eq!(icmp_a.src_ip, ip_b);
    assert_eq!(icmp_a.dst_ip, ip_a);
    assert_eq!(icmp_a.data, b"b to a v6");
    assert_eq!(icmp_a.dst_mac, [0x52, 0x54, 0x00, 0x00, 0x00, 0xAA]);
    assert_eq!(icmp_a.src_mac, GATEWAY_MAC);
    assert_eq!(icmp_a.hop_limit, 63);
}
