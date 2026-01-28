//! DHCPv6, NDP, and ICMPv6 integration tests
//!
//! Tests the full IPv6 network stack initialization flow:
//! 1. RS (Router Solicitation) → RA (Router Advertisement)
//! 2. DHCPv6 SOLICIT → ADVERTISE
//! 3. DHCPv6 REQUEST → REPLY
//! 4. NS (Neighbor Solicitation) → NA (Neighbor Advertisement)
//! 5. Echo Request → Echo Reply (ping6 to gateway)

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;

use mvirt_net::router::{Router, VhostConfig};
use mvirt_net::test_util::{
    Dhcpv6MessageType, VhostUserFrontendDevice, create_dhcpv6_request, create_dhcpv6_solicit,
    create_icmpv6_echo_request, create_neighbor_solicitation, create_router_solicitation,
    generate_duid_ll, parse_dhcpv6_response, parse_icmpv6_echo_reply, parse_neighbor_advertisement,
    parse_router_advertisement,
};

/// Gateway MAC address used by the reactor
const GATEWAY_MAC: [u8; 6] = [0x02, 0x00, 0x00, 0x00, 0x00, 0x01];

/// Test the full RS → RA → DHCPv6 SOLICIT → ADVERTISE → REQUEST → REPLY flow
#[tokio::test]
async fn test_ipv6_rs_ra_dhcpv6() {
    let _ = tracing_subscriber::fmt::try_init();

    let socket_path = "/tmp/iou-dhcpv6-test.sock";

    // VM configuration
    let vm_mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x01];
    let vm_ipv6 = Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 0x10);
    let gateway_ipv6 = Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1);
    let gateway_ipv4 = Ipv4Addr::new(10, 50, 0, 1);

    // DNS servers to configure and verify
    let dns_servers: Vec<IpAddr> = vec![
        "2001:4860:4860::8888".parse().unwrap(), // Google DNS IPv6
        "2001:4860:4860::8844".parse().unwrap(),
    ];

    // Clean up any stale socket
    let _ = std::fs::remove_file(socket_path);

    // Start router with vhost-user backend and IPv6 configuration
    let vhost_config = VhostConfig::new(socket_path.to_string(), vm_mac)
        .with_ipv4(Ipv4Addr::new(10, 50, 0, 10), gateway_ipv4, 24)
        .with_ipv6(vm_ipv6, gateway_ipv6, 128)
        .with_dns(dns_servers.clone());

    let router = Router::with_config_and_vhost(
        "tun_dhcpv6_test",
        gateway_ipv4,
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
    // RS → RA (Router Solicitation → Router Advertisement)
    // =========================================================================
    println!("\n=== Router Solicitation ===");
    let rs = create_router_solicitation(vm_mac);
    println!("Sending RS ({} bytes)", rs.len());
    frontend.send_packet(&rs).expect("Failed to send RS");

    let _ = frontend.wait_for_tx(1000);
    frontend.wait_tx_complete().ok();

    // Wait for RA
    println!("Waiting for RA...");
    let _ = frontend.wait_for_rx(2000).expect("RX wait failed");

    let ra_packet = frontend
        .recv_packet()
        .expect("RX recv failed")
        .expect("No RA received");
    let ra = parse_router_advertisement(&ra_packet).expect("Failed to parse RA");

    println!("Received RA:");
    println!("  M flag (managed): {}", ra.managed_flag);
    println!("  O flag (other): {}", ra.other_flag);
    println!("  Router lifetime: {} seconds", ra.router_lifetime);
    if let Some(mac) = ra.router_mac {
        println!(
            "  Router MAC: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
        );
    }

    // Verify M flag is set (VMs must use DHCPv6)
    assert!(ra.managed_flag, "M flag should be set");
    assert!(ra.router_lifetime > 0, "Router lifetime should be non-zero");
    assert_eq!(ra.router_mac, Some(GATEWAY_MAC), "Router MAC mismatch");
    // O flag should be set when DNS servers are configured (Other Config for DNS via DHCPv6)
    assert!(
        ra.other_flag,
        "O flag should be set for DNS via DHCPv6 when DNS servers are configured"
    );

    println!("\nRA indicates: Use DHCPv6 for address configuration (M+O flags set)");

    // =========================================================================
    // DHCPv6 SOLICIT → ADVERTISE
    // =========================================================================
    println!("\n=== DHCPv6 SOLICIT ===");
    let client_duid = generate_duid_ll(vm_mac);
    let solicit = create_dhcpv6_solicit(vm_mac, &client_duid);
    println!("Sending DHCPv6 SOLICIT ({} bytes)", solicit.len());
    frontend
        .send_packet(&solicit)
        .expect("Failed to send SOLICIT");

    let _ = frontend.wait_for_tx(1000);
    frontend.wait_tx_complete().ok();

    // Wait for ADVERTISE
    println!("Waiting for DHCPv6 ADVERTISE...");
    let _ = frontend.wait_for_rx(2000).expect("RX wait failed");

    let advertise_packet = frontend
        .recv_packet()
        .expect("RX recv failed")
        .expect("No ADVERTISE received");
    let advertise = parse_dhcpv6_response(&advertise_packet).expect("Failed to parse ADVERTISE");

    assert_eq!(
        advertise.msg_type,
        Dhcpv6MessageType::Advertise,
        "Expected ADVERTISE"
    );
    println!("Received DHCPv6 ADVERTISE:");
    println!(
        "  XID: {:02x}{:02x}{:02x}",
        advertise.xid[0], advertise.xid[1], advertise.xid[2]
    );
    println!("  Server DUID: {:?}", advertise.server_duid);
    println!("  Offered addresses: {:?}", advertise.addresses);
    println!("  DNS servers: {:?}", advertise.dns_servers);

    assert!(!advertise.addresses.is_empty(), "No address offered");
    assert_eq!(advertise.addresses[0], vm_ipv6, "Offered address mismatch");
    assert!(!advertise.server_duid.is_empty(), "No server DUID");

    // Verify DNS servers are advertised
    let expected_dns_v6: Vec<Ipv6Addr> = dns_servers
        .iter()
        .filter_map(|ip| {
            if let IpAddr::V6(v6) = ip {
                Some(*v6)
            } else {
                None
            }
        })
        .collect();
    assert!(
        !advertise.dns_servers.is_empty(),
        "DNS servers should be advertised"
    );
    assert_eq!(
        advertise.dns_servers.len(),
        expected_dns_v6.len(),
        "All IPv6 DNS servers should be advertised"
    );
    for expected_dns in &expected_dns_v6 {
        assert!(
            advertise.dns_servers.contains(expected_dns),
            "DNS server {} should be in ADVERTISE",
            expected_dns
        );
    }

    // =========================================================================
    // DHCPv6 REQUEST → REPLY
    // =========================================================================
    println!("\n=== DHCPv6 REQUEST ===");
    let request = create_dhcpv6_request(
        vm_mac,
        &client_duid,
        &advertise.server_duid,
        advertise.addresses[0],
        1, // IAID
    );
    println!("Sending DHCPv6 REQUEST for {}", advertise.addresses[0]);
    frontend
        .send_packet(&request)
        .expect("Failed to send REQUEST");

    let _ = frontend.wait_for_tx(1000);
    frontend.wait_tx_complete().ok();

    // Wait for REPLY
    println!("Waiting for DHCPv6 REPLY...");
    let _ = frontend.wait_for_rx(2000).expect("RX wait failed");

    let reply_packet = frontend
        .recv_packet()
        .expect("RX recv failed")
        .expect("No REPLY received");
    let reply = parse_dhcpv6_response(&reply_packet).expect("Failed to parse REPLY");

    assert_eq!(reply.msg_type, Dhcpv6MessageType::Reply, "Expected REPLY");
    println!("Received DHCPv6 REPLY:");
    println!(
        "  XID: {:02x}{:02x}{:02x}",
        reply.xid[0], reply.xid[1], reply.xid[2]
    );
    println!("  Assigned addresses: {:?}", reply.addresses);
    println!("  DNS servers: {:?}", reply.dns_servers);

    assert!(!reply.addresses.is_empty(), "No address assigned");
    assert_eq!(reply.addresses[0], vm_ipv6, "Assigned address mismatch");

    // Verify DNS servers are in the REPLY
    assert!(
        !reply.dns_servers.is_empty(),
        "DNS servers should be in REPLY"
    );
    for expected_dns in &expected_dns_v6 {
        assert!(
            reply.dns_servers.contains(expected_dns),
            "DNS server {} should be in REPLY",
            expected_dns
        );
    }

    println!("\n=== Test PASSED ===");
    println!("Successfully completed: RS → RA → DHCPv6 SOLICIT → ADVERTISE → REQUEST → REPLY");
    println!("Assigned IPv6 address: {}", reply.addresses[0]);
    println!("DNS servers: {:?}", reply.dns_servers);

    // Cleanup
    router.prepare_shutdown();
    drop(frontend);
    router.shutdown().await.expect("Failed to shutdown router");
}

/// Test that NS for the gateway address (fe80::1) gets a valid NA response
#[tokio::test]
async fn test_neighbor_solicitation_for_gateway() {
    let _ = tracing_subscriber::fmt::try_init();

    let socket_path = "/tmp/iou-ns-test.sock";

    // VM configuration
    let vm_mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x02];
    let vm_ipv6 = Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 0x20);
    let gateway_ipv6 = Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1);
    let gateway_ipv4 = Ipv4Addr::new(10, 50, 0, 1);

    // Clean up any stale socket
    let _ = std::fs::remove_file(socket_path);

    // Start router with vhost-user backend and IPv6 configuration
    let vhost_config = VhostConfig::new(socket_path.to_string(), vm_mac)
        .with_ipv4(Ipv4Addr::new(10, 50, 0, 20), gateway_ipv4, 24)
        .with_ipv6(vm_ipv6, gateway_ipv6, 128);

    let router = Router::with_config_and_vhost(
        "tun_nstest",
        gateway_ipv4,
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
    // NS → NA (Neighbor Solicitation → Neighbor Advertisement)
    // =========================================================================
    println!("\n=== Neighbor Solicitation for gateway fe80::1 ===");
    let ns = create_neighbor_solicitation(vm_mac, gateway_ipv6);
    println!(
        "Sending NS ({} bytes) for target {}",
        ns.len(),
        gateway_ipv6
    );
    frontend.send_packet(&ns).expect("Failed to send NS");

    let _ = frontend.wait_for_tx(1000);
    frontend.wait_tx_complete().ok();

    // Wait for NA
    println!("Waiting for NA...");
    let _ = frontend.wait_for_rx(2000).expect("RX wait failed");

    let na_packet = frontend
        .recv_packet()
        .expect("RX recv failed")
        .expect("No NA received");
    let na = parse_neighbor_advertisement(&na_packet).expect("Failed to parse NA");

    println!("Received NA:");
    println!("  Target address: {}", na.target_addr);
    println!("  Router flag (R): {}", na.router_flag);
    println!("  Solicited flag (S): {}", na.solicited_flag);
    println!("  Override flag (O): {}", na.override_flag);
    if let Some(mac) = na.target_mac {
        println!(
            "  Target MAC (TLLAO): {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
        );
    }

    // Verify NA response
    assert_eq!(
        na.target_addr, gateway_ipv6,
        "Target address should match solicited address"
    );
    assert!(na.solicited_flag, "Solicited (S) flag should be set");
    assert!(na.override_flag, "Override (O) flag should be set");
    assert_eq!(
        na.target_mac,
        Some(GATEWAY_MAC),
        "Target MAC should be gateway MAC"
    );

    println!("\n=== Test PASSED ===");
    println!("Successfully completed: NS → NA for gateway");
    println!(
        "Gateway MAC resolved: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        GATEWAY_MAC[0],
        GATEWAY_MAC[1],
        GATEWAY_MAC[2],
        GATEWAY_MAC[3],
        GATEWAY_MAC[4],
        GATEWAY_MAC[5]
    );

    // Cleanup
    router.prepare_shutdown();
    drop(frontend);
    router.shutdown().await.expect("Failed to shutdown router");
}

/// Test that NS for a non-gateway address does NOT get an NA response
#[tokio::test]
async fn test_neighbor_solicitation_for_non_gateway() {
    let _ = tracing_subscriber::fmt::try_init();

    let socket_path = "/tmp/iou-ns-nongateway-test.sock";

    // VM configuration
    let vm_mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x03];
    let vm_ipv6 = Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 0x30);
    let gateway_ipv6 = Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1);
    let gateway_ipv4 = Ipv4Addr::new(10, 50, 0, 1);

    // Non-gateway address to solicit (some other address on the link)
    let other_ipv6 = Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 0x99);

    // Clean up any stale socket
    let _ = std::fs::remove_file(socket_path);

    // Start router with vhost-user backend and IPv6 configuration
    let vhost_config = VhostConfig::new(socket_path.to_string(), vm_mac)
        .with_ipv4(Ipv4Addr::new(10, 50, 0, 30), gateway_ipv4, 24)
        .with_ipv6(vm_ipv6, gateway_ipv6, 128);

    let router = Router::with_config_and_vhost(
        "tun_nsnogtw",
        gateway_ipv4,
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
    // NS for non-gateway → No NA expected
    // =========================================================================
    println!(
        "\n=== Neighbor Solicitation for non-gateway {} ===",
        other_ipv6
    );
    let ns = create_neighbor_solicitation(vm_mac, other_ipv6);
    println!("Sending NS ({} bytes) for target {}", ns.len(), other_ipv6);
    frontend.send_packet(&ns).expect("Failed to send NS");

    let _ = frontend.wait_for_tx(1000);
    frontend.wait_tx_complete().ok();

    // Wait briefly for any potential response
    println!("Waiting for potential NA (expecting none)...");
    let rx_result = frontend.wait_for_rx(500);

    // We expect no response - the router should not reply to NS for addresses it doesn't own
    let received_na = if rx_result.is_ok() {
        if let Ok(Some(packet)) = frontend.recv_packet() {
            // Check if it's actually an NA for our target
            if let Some(na) = parse_neighbor_advertisement(&packet) {
                if na.target_addr == other_ipv6 {
                    Some(na)
                } else {
                    println!("Received NA but for different target: {}", na.target_addr);
                    None
                }
            } else {
                println!("Received some packet but not an NA");
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    assert!(
        received_na.is_none(),
        "Should NOT receive NA for non-gateway address"
    );

    println!("\n=== Test PASSED ===");
    println!("Correctly received no NA response for non-gateway address");

    // Cleanup
    router.prepare_shutdown();
    drop(frontend);
    router.shutdown().await.expect("Failed to shutdown router");
}

/// Test that ping6 (ICMPv6 Echo Request) to gateway fe80::1 gets a reply
#[tokio::test]
async fn test_ping6_gateway() {
    let _ = tracing_subscriber::fmt::try_init();

    let socket_path = "/tmp/iou-ping6-test.sock";

    // VM configuration
    let vm_mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x04];
    let vm_ipv6 = Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 0x40);
    let gateway_ipv6 = Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1);
    let gateway_ipv4 = Ipv4Addr::new(10, 50, 0, 1);

    // Clean up any stale socket
    let _ = std::fs::remove_file(socket_path);

    // Start router with vhost-user backend and IPv6 configuration
    let vhost_config = VhostConfig::new(socket_path.to_string(), vm_mac)
        .with_ipv4(Ipv4Addr::new(10, 50, 0, 40), gateway_ipv4, 24)
        .with_ipv6(vm_ipv6, gateway_ipv6, 128);

    let router = Router::with_config_and_vhost(
        "tun_ping6",
        gateway_ipv4,
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
    // ping6 fe80::1
    // =========================================================================
    println!("\n=== ping6 to gateway fe80::1 ===");

    let ping_data = b"Hello from test!";
    let ping_id = 0x1234u16;
    let ping_seq = 1u16;

    let echo_req = create_icmpv6_echo_request(
        vm_mac,
        GATEWAY_MAC,
        gateway_ipv6,
        ping_id,
        ping_seq,
        ping_data,
    );
    println!(
        "Sending ICMPv6 Echo Request ({} bytes) to {}",
        echo_req.len(),
        gateway_ipv6
    );
    frontend
        .send_packet(&echo_req)
        .expect("Failed to send Echo Request");

    let _ = frontend.wait_for_tx(1000);
    frontend.wait_tx_complete().ok();

    // Wait for Echo Reply
    println!("Waiting for Echo Reply...");
    let _ = frontend.wait_for_rx(2000).expect("RX wait failed");

    let reply_packet = frontend
        .recv_packet()
        .expect("RX recv failed")
        .expect("No Echo Reply received");
    let reply = parse_icmpv6_echo_reply(&reply_packet).expect("Failed to parse Echo Reply");

    println!("Received Echo Reply:");
    println!("  Source: {}", reply.src_addr);
    println!("  ID: 0x{:04x}", reply.id);
    println!("  Seq: {}", reply.seq);
    println!("  Data: {:?}", String::from_utf8_lossy(&reply.data));

    // Verify Echo Reply
    assert_eq!(
        reply.src_addr, gateway_ipv6,
        "Reply should come from gateway"
    );
    assert_eq!(reply.id, ping_id, "ID should match request");
    assert_eq!(reply.seq, ping_seq, "Sequence should match request");
    assert_eq!(reply.data, ping_data, "Data should match request");

    println!("\n=== Test PASSED ===");
    println!("Successfully completed: ping6 to gateway");

    // Cleanup
    router.prepare_shutdown();
    drop(frontend);
    router.shutdown().await.expect("Failed to shutdown router");
}
