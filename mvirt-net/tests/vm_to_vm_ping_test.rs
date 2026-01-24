//! VM-to-VM Ping integration test
//!
//! Tests that packets between two VMs on the same network are routed
//! directly between reactors (not through TUN).
//!
//! This test verifies the fix for the routing bug where VM-to-VM routes
//! were added to a non-existent routing table.

use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;

use ipnet::Ipv4Net;
use mvirt_net::reactor::ReactorRegistry;
use mvirt_net::router::{Router, VhostConfig};
use mvirt_net::routing::{IpPrefix, RouteTarget};
use mvirt_net::test_util::{
    VhostUserFrontendDevice, create_arp_request, create_icmp_echo_request, parse_arp_reply,
};
use uuid::Uuid;

/// Gateway MAC address used by all reactors
const GATEWAY_MAC: [u8; 6] = [0x02, 0x00, 0x00, 0x00, 0x00, 0x01];

/// Link-local gateway IP (like AWS/GCP)
const GATEWAY_IP: [u8; 4] = [169, 254, 0, 1];

/// Test VM-to-VM ping through inter-reactor routing
#[tokio::test]
async fn test_vm_to_vm_ping() {
    let _ = tracing_subscriber::fmt::try_init();

    // Socket paths for the two VMs
    let socket1 = "/tmp/vm2vm-test-vm1.sock";
    let socket2 = "/tmp/vm2vm-test-vm2.sock";

    // VM configurations
    let vm1_mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x01];
    let vm1_ip = Ipv4Addr::new(10, 50, 0, 10);

    let vm2_mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x02];
    let vm2_ip = Ipv4Addr::new(10, 50, 0, 20);

    let gateway_ip = Ipv4Addr::new(10, 50, 0, 1);

    // Clean up any stale sockets
    let _ = std::fs::remove_file(socket1);
    let _ = std::fs::remove_file(socket2);

    // Create a shared reactor registry for VM-to-VM communication
    let registry = Arc::new(ReactorRegistry::new());

    // Create Router 1 (VM1)
    let vhost_config1 =
        VhostConfig::new(socket1.to_string(), vm1_mac).with_ipv4(vm1_ip, gateway_ip, 24);

    let router1 = Router::with_shared_registry(
        "vm2vm_r1",
        Some((gateway_ip, 24)),
        4096,
        256,
        256,
        Some(vhost_config1),
        Arc::clone(&registry),
    )
    .await
    .expect("Failed to start router1");

    // Create Router 2 (VM2)
    let vhost_config2 =
        VhostConfig::new(socket2.to_string(), vm2_mac).with_ipv4(vm2_ip, gateway_ip, 24);

    let router2 = Router::with_shared_registry(
        "vm2vm_r2",
        Some((gateway_ip, 24)),
        4096,
        256,
        256,
        Some(vhost_config2),
        Arc::clone(&registry),
    )
    .await
    .expect("Failed to start router2");

    // Set up routing tables for VM-to-VM communication
    // Router1: route to VM2's IP → Router2's reactor
    // Router2: route to VM1's IP → Router1's reactor
    let table1_id = Uuid::new_v4();
    let table2_id = Uuid::new_v4();

    // Create and configure routing table for Router1
    router1
        .reactor_handle()
        .create_table(table1_id, "vm1-routes");
    router1.reactor_handle().set_default_table(table1_id);

    // Route for VM1's own IP (local)
    router1.reactor_handle().add_route(
        table1_id,
        IpPrefix::V4(Ipv4Net::new(vm1_ip, 32).unwrap()),
        RouteTarget::reactor(router1.reactor_id()),
    );

    // Route to VM2
    router1.reactor_handle().add_route(
        table1_id,
        IpPrefix::V4(Ipv4Net::new(vm2_ip, 32).unwrap()),
        RouteTarget::reactor(router2.reactor_id()),
    );

    // Create and configure routing table for Router2
    router2
        .reactor_handle()
        .create_table(table2_id, "vm2-routes");
    router2.reactor_handle().set_default_table(table2_id);

    // Route for VM2's own IP (local)
    router2.reactor_handle().add_route(
        table2_id,
        IpPrefix::V4(Ipv4Net::new(vm2_ip, 32).unwrap()),
        RouteTarget::reactor(router2.reactor_id()),
    );

    // Route to VM1
    router2.reactor_handle().add_route(
        table2_id,
        IpPrefix::V4(Ipv4Net::new(vm1_ip, 32).unwrap()),
        RouteTarget::reactor(router1.reactor_id()),
    );

    // Give backends time to create sockets
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Connect frontends (simulating VMs)
    let mut frontend1 =
        VhostUserFrontendDevice::connect(socket1).expect("Failed to connect frontend1");
    frontend1.setup().expect("Failed to setup frontend1");

    let mut frontend2 =
        VhostUserFrontendDevice::connect(socket2).expect("Failed to connect frontend2");
    frontend2.setup().expect("Failed to setup frontend2");

    // Provide RX buffers for both VMs
    for _ in 0..16 {
        frontend1
            .provide_rx_buffer(4096)
            .expect("Failed to provide RX buffer for VM1");
        frontend2
            .provide_rx_buffer(4096)
            .expect("Failed to provide RX buffer for VM2");
    }

    // =========================================================================
    // ARP Resolution: VM1 resolves gateway MAC
    // =========================================================================
    println!("\n=== VM1: ARP Resolution for Gateway ===");
    let arp_request = create_arp_request(vm1_mac, vm1_ip.octets(), GATEWAY_IP);
    println!("VM1: Sending ARP request: who-has {:?}?", GATEWAY_IP);
    frontend1
        .send_packet(&arp_request)
        .expect("Failed to send ARP request");

    let _ = frontend1.wait_for_tx(1000);
    frontend1.wait_tx_complete().ok();

    // Wait for ARP reply
    let _ = frontend1.wait_for_rx(2000).expect("VM1: RX wait failed");

    let arp_reply_packet = frontend1
        .recv_packet()
        .expect("VM1: RX recv failed")
        .expect("VM1: No ARP reply received");
    let arp_reply = parse_arp_reply(&arp_reply_packet).expect("Failed to parse ARP reply");

    println!("VM1: Received ARP reply:");
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

    assert_eq!(arp_reply.sender_ip, GATEWAY_IP, "ARP sender IP mismatch");
    assert_eq!(arp_reply.sender_mac, GATEWAY_MAC, "Gateway MAC mismatch");

    // =========================================================================
    // ICMP Ping: VM1 → VM2
    // =========================================================================
    println!("\n=== VM1 → VM2: ICMP Ping ===");
    let icmp_request = create_icmp_echo_request(
        vm1_mac,
        GATEWAY_MAC,     // Send to gateway MAC (router will forward)
        vm1_ip.octets(), // Source: VM1
        vm2_ip.octets(), // Destination: VM2
        0xABCD,          // ID
        1,               // Sequence
    );
    println!("VM1: Sending ICMP echo request to {:?}", vm2_ip);
    frontend1
        .send_packet(&icmp_request)
        .expect("Failed to send ICMP request");

    let _ = frontend1.wait_for_tx(1000);
    frontend1.wait_tx_complete().ok();

    // =========================================================================
    // VM2 receives the ping request
    // =========================================================================
    println!("\n=== VM2: Receiving ICMP Request ===");
    let _ = frontend2.wait_for_rx(2000).expect("VM2: RX wait failed");

    let icmp_req_packet = frontend2
        .recv_packet()
        .expect("VM2: RX recv failed")
        .expect("VM2: No ICMP request received - VM-to-VM routing may be broken!");

    // Parse the received packet to verify it's the ICMP request
    // Note: parse_icmp_echo_reply works for requests too (same structure)
    let received = parse_icmp_echo_request(&icmp_req_packet)
        .expect("Failed to parse ICMP request received by VM2");

    println!("VM2: Received ICMP echo request:");
    println!("  From: {:?}", received.src_ip);
    println!("  To: {:?}", received.dst_ip);
    println!("  ID: 0x{:04x}, Seq: {}", received.id, received.seq);
    println!(
        "  Ethernet dst_mac: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        received.dst_mac[0],
        received.dst_mac[1],
        received.dst_mac[2],
        received.dst_mac[3],
        received.dst_mac[4],
        received.dst_mac[5]
    );
    println!(
        "  Ethernet src_mac: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        received.src_mac[0],
        received.src_mac[1],
        received.src_mac[2],
        received.src_mac[3],
        received.src_mac[4],
        received.src_mac[5]
    );

    assert_eq!(
        received.src_ip,
        vm1_ip.octets(),
        "ICMP source IP should be VM1"
    );
    assert_eq!(
        received.dst_ip,
        vm2_ip.octets(),
        "ICMP dest IP should be VM2"
    );
    assert_eq!(received.id, 0xABCD, "ICMP ID mismatch");
    assert_eq!(received.seq, 1, "ICMP sequence mismatch");

    // Verify MAC address rewriting
    // dst_mac should be VM2's MAC (the receiver), NOT the gateway MAC
    assert_eq!(
        received.dst_mac, vm2_mac,
        "VM-to-VM routing should rewrite dst_mac to receiver's MAC"
    );
    // src_mac should be the router's MAC (GATEWAY_MAC)
    assert_eq!(
        received.src_mac, GATEWAY_MAC,
        "VM-to-VM routing should set src_mac to router's MAC"
    );

    println!("\n=== Test PASSED ===");
    println!("Successfully completed: VM1 → VM2 ping via inter-reactor routing");

    // Cleanup
    router1.prepare_shutdown();
    router2.prepare_shutdown();
    drop(frontend1);
    drop(frontend2);
    router1
        .shutdown()
        .await
        .expect("Failed to shutdown router1");
    router2
        .shutdown()
        .await
        .expect("Failed to shutdown router2");
}

/// Parse an ICMP echo request packet (same structure as reply but type=8)
fn parse_icmp_echo_request(packet: &[u8]) -> Option<IcmpEchoRequest> {
    use mvirt_net::test_util::VIRTIO_NET_HDR_SIZE;
    use smoltcp::wire::{
        EthernetFrame, EthernetProtocol, Icmpv4Message, Icmpv4Packet, IpProtocol, Ipv4Packet,
    };

    let eth_frame = EthernetFrame::new_checked(&packet[VIRTIO_NET_HDR_SIZE..]).ok()?;
    if eth_frame.ethertype() != EthernetProtocol::Ipv4 {
        return None;
    }

    // Extract MAC addresses from Ethernet frame
    let dst_mac = eth_frame.dst_addr().0;
    let src_mac = eth_frame.src_addr().0;

    let ip_packet = Ipv4Packet::new_checked(eth_frame.payload()).ok()?;
    if ip_packet.next_header() != IpProtocol::Icmp {
        return None;
    }

    let icmp_packet = Icmpv4Packet::new_checked(ip_packet.payload()).ok()?;
    if icmp_packet.msg_type() != Icmpv4Message::EchoRequest {
        return None;
    }

    Some(IcmpEchoRequest {
        src_ip: ip_packet.src_addr().0,
        dst_ip: ip_packet.dst_addr().0,
        id: icmp_packet.echo_ident(),
        seq: icmp_packet.echo_seq_no(),
        dst_mac,
        src_mac,
    })
}

/// Parsed ICMP echo request
struct IcmpEchoRequest {
    src_ip: [u8; 4],
    dst_ip: [u8; 4],
    id: u16,
    seq: u16,
    /// Ethernet destination MAC
    dst_mac: [u8; 6],
    /// Ethernet source MAC
    src_mac: [u8; 6],
}
