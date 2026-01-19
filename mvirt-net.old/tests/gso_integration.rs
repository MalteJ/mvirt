//! GSO/GRO integration tests
//!
//! Tests for TCP Segmentation Offload (TSO), checksum offload, and
//! Generic Receive Offload (GRO) features.
//!
//! Run all tests:
//!   cargo test -p mvirt-net --test gso_integration
//!
//! Run specific tests:
//!   cargo test -p mvirt-net --test gso_integration tso
//!   cargo test -p mvirt-net --test gso_integration csum

mod harness;

use harness::TestBackend;

/// Virtio-net feature flags for TSO/checksum offload
const VIRTIO_NET_F_CSUM: u64 = 1 << 0;
const VIRTIO_NET_F_GUEST_TSO4: u64 = 1 << 7;
const VIRTIO_NET_F_GUEST_TSO6: u64 = 1 << 8;
const VIRTIO_NET_F_HOST_TSO4: u64 = 1 << 11;
const VIRTIO_NET_F_HOST_TSO6: u64 = 1 << 12;
const VIRTIO_NET_F_MRG_RXBUF: u64 = 1 << 15;

// ============================================================================
// Feature Negotiation Tests
// ============================================================================

#[test]
fn test_tso_features_not_advertised() {
    // TSO is intentionally disabled because process_rx cannot handle packets
    // larger than a single descriptor chain (~MTU). Only CSUM is enabled.
    let backend = TestBackend::new("52:54:00:12:34:56", None);
    let client = backend.connect().expect("connect failed");

    // Check that checksum offload IS advertised
    assert!(
        client.has_feature(VIRTIO_NET_F_CSUM),
        "Should advertise CSUM feature"
    );

    // Check that TSO features are NOT advertised (disabled to prevent truncation)
    assert!(
        !client.has_feature(VIRTIO_NET_F_GUEST_TSO4),
        "Should NOT advertise GUEST_TSO4 feature"
    );
    assert!(
        !client.has_feature(VIRTIO_NET_F_HOST_TSO4),
        "Should NOT advertise HOST_TSO4 feature"
    );
    assert!(
        !client.has_feature(VIRTIO_NET_F_GUEST_TSO6),
        "Should NOT advertise GUEST_TSO6 feature"
    );
    assert!(
        !client.has_feature(VIRTIO_NET_F_HOST_TSO6),
        "Should NOT advertise HOST_TSO6 feature"
    );
}

#[test]
fn test_mergeable_rx_buffers_advertised() {
    let backend = TestBackend::new("52:54:00:12:34:56", None);
    let client = backend.connect().expect("connect failed");

    // MRG_RXBUF is needed for GRO to work efficiently
    assert!(
        client.has_feature(VIRTIO_NET_F_MRG_RXBUF),
        "Should advertise MRG_RXBUF feature for GRO support"
    );
}

#[test]
fn test_feature_negotiation_with_tun() {
    // Even with TUN configured, TSO should NOT be advertised
    // (disabled to prevent packet truncation in process_rx)
    let backend = TestBackend::new("52:54:00:12:34:56", Some("10.0.0.5"));
    let client = backend.connect().expect("connect failed");

    // CSUM should still be negotiated
    assert!(
        client.has_feature(VIRTIO_NET_F_CSUM),
        "CSUM should be negotiated"
    );

    // TSO should NOT be negotiated even with TUN
    assert!(
        !client.has_feature(VIRTIO_NET_F_GUEST_TSO4),
        "GUEST_TSO4 should NOT be negotiated"
    );
    assert!(
        !client.has_feature(VIRTIO_NET_F_GUEST_TSO6),
        "GUEST_TSO6 should NOT be negotiated"
    );
    assert!(
        !client.has_feature(VIRTIO_NET_F_HOST_TSO4),
        "HOST_TSO4 should NOT be negotiated"
    );
    assert!(
        !client.has_feature(VIRTIO_NET_F_HOST_TSO6),
        "HOST_TSO6 should NOT be negotiated"
    );
}

// ============================================================================
// GSO Header Tests
// ============================================================================

#[test]
fn test_send_packet_with_gso_none() {
    let backend = TestBackend::new("52:54:00:12:34:56", None);
    let mut client = backend.connect().expect("connect failed");

    // Build a packet with virtio-net header (GSO_NONE)
    let mut packet = Vec::new();

    // Virtio-net header (12 bytes): flags=0, gso_type=0 (none), rest zeros
    packet.extend_from_slice(&[0u8; 12]);

    // Ethernet header
    packet.extend_from_slice(&[0xff, 0xff, 0xff, 0xff, 0xff, 0xff]); // dst MAC (broadcast)
    packet.extend_from_slice(&[0x52, 0x54, 0x00, 0x12, 0x34, 0x56]); // src MAC
    packet.extend_from_slice(&[0x08, 0x00]); // EtherType (IPv4)

    // Minimal IP payload
    packet.extend_from_slice(&[0x45, 0x00, 0x00, 0x14]); // IP header start
    packet.extend_from_slice(&[0u8; 16]); // Rest of minimal IP header

    let result = client.send_packet(&packet);
    assert!(
        result.is_ok(),
        "Should be able to send packet with GSO_NONE: {:?}",
        result.err()
    );
}

#[test]
fn test_send_packet_with_csum_offload_request() {
    let backend = TestBackend::new("52:54:00:12:34:56", None);
    let mut client = backend.connect().expect("connect failed");

    // Build a packet with checksum offload request
    let mut packet = Vec::new();

    // Virtio-net header with NEEDS_CSUM flag
    let virtio_hdr = [
        0x01, // flags: VIRTIO_NET_HDR_F_NEEDS_CSUM
        0x00, // gso_type: GSO_NONE
        0x00, 0x00, // hdr_len
        0x00, 0x00, // gso_size
        0x22, 0x00, // csum_start: 34 (after eth+ip headers)
        0x10, 0x00, // csum_offset: 16 (TCP checksum offset)
        0x00, 0x00, // num_buffers
    ];
    packet.extend_from_slice(&virtio_hdr);

    // Ethernet header
    packet.extend_from_slice(&[0xff, 0xff, 0xff, 0xff, 0xff, 0xff]); // dst MAC
    packet.extend_from_slice(&[0x52, 0x54, 0x00, 0x12, 0x34, 0x56]); // src MAC
    packet.extend_from_slice(&[0x08, 0x00]); // EtherType (IPv4)

    // IP header (20 bytes)
    packet.extend_from_slice(&[
        0x45, 0x00, // Version, IHL, DSCP
        0x00, 0x28, // Total length: 40 bytes (20 IP + 20 TCP)
        0x00, 0x00, // ID
        0x40, 0x00, // Flags, Fragment offset (DF set)
        0x40, 0x06, // TTL=64, Protocol=TCP
        0x00, 0x00, // Header checksum (would be computed)
        0x0a, 0x00, 0x00, 0x01, // Src IP: 10.0.0.1
        0x0a, 0x00, 0x00, 0x02, // Dst IP: 10.0.0.2
    ]);

    // TCP header (20 bytes)
    packet.extend_from_slice(&[
        0x04, 0x00, // Src port: 1024
        0x00, 0x50, // Dst port: 80
        0x00, 0x00, 0x00, 0x01, // Seq number
        0x00, 0x00, 0x00, 0x00, // Ack number
        0x50, 0x02, // Data offset, flags (SYN)
        0xff, 0xff, // Window size
        0x00, 0x00, // Checksum (to be computed by host)
        0x00, 0x00, // Urgent pointer
    ]);

    let result = client.send_packet(&packet);
    assert!(
        result.is_ok(),
        "Should be able to send packet with NEEDS_CSUM: {:?}",
        result.err()
    );
}

// ============================================================================
// Large Packet Tests (GSO simulation)
// ============================================================================

#[test]
fn test_send_large_packet_gso_tcpv4() {
    let backend = TestBackend::new("52:54:00:12:34:56", None);
    let mut client = backend.connect().expect("connect failed");

    // Build a large packet with GSO_TCPV4
    let payload_size = 8000; // Larger than MTU
    let mut packet = Vec::with_capacity(12 + 14 + 20 + 20 + payload_size);

    // Virtio-net header with GSO_TCPV4
    let virtio_hdr = [
        0x01, // flags: VIRTIO_NET_HDR_F_NEEDS_CSUM
        0x01, // gso_type: VIRTIO_NET_HDR_GSO_TCPV4
        0x00, 0x00, // hdr_len
        0xdc, 0x05, // gso_size: 1500 (segment size)
        0x22, 0x00, // csum_start: 34
        0x10, 0x00, // csum_offset: 16
        0x00, 0x00, // num_buffers
    ];
    packet.extend_from_slice(&virtio_hdr);

    // Ethernet header
    packet.extend_from_slice(&[0xff, 0xff, 0xff, 0xff, 0xff, 0xff]);
    packet.extend_from_slice(&[0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);
    packet.extend_from_slice(&[0x08, 0x00]);

    // IP header
    let total_len = (20 + 20 + payload_size) as u16;
    packet.extend_from_slice(&[
        0x45,
        0x00,
        (total_len >> 8) as u8,
        (total_len & 0xff) as u8,
        0x00,
        0x00,
        0x40,
        0x00,
        0x40,
        0x06,
        0x00,
        0x00,
        0x0a,
        0x00,
        0x00,
        0x01,
        0x0a,
        0x00,
        0x00,
        0x02,
    ]);

    // TCP header
    packet.extend_from_slice(&[
        0x04, 0x00, 0x00, 0x50, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x50, 0x10, 0xff,
        0xff, 0x00, 0x00, 0x00, 0x00,
    ]);

    // Payload
    packet.extend(std::iter::repeat(0xAA).take(payload_size));

    let result = client.send_packet(&packet);
    assert!(
        result.is_ok(),
        "Should be able to send large GSO packet: {:?}",
        result.err()
    );
}

#[test]
fn test_send_large_packet_gso_tcpv6() {
    let backend = TestBackend::new("52:54:00:12:34:56", None);
    let mut client = backend.connect().expect("connect failed");

    // Build a large IPv6 packet with GSO_TCPV6
    let payload_size = 4000;
    let mut packet = Vec::with_capacity(12 + 14 + 40 + 20 + payload_size);

    // Virtio-net header with GSO_TCPV6
    let virtio_hdr = [
        0x01, // flags: VIRTIO_NET_HDR_F_NEEDS_CSUM
        0x04, // gso_type: VIRTIO_NET_HDR_GSO_TCPV6
        0x00, 0x00, // hdr_len
        0xdc, 0x05, // gso_size: 1500
        0x36, 0x00, // csum_start: 54 (14 eth + 40 ipv6)
        0x10, 0x00, // csum_offset: 16
        0x00, 0x00, // num_buffers
    ];
    packet.extend_from_slice(&virtio_hdr);

    // Ethernet header
    packet.extend_from_slice(&[0xff, 0xff, 0xff, 0xff, 0xff, 0xff]);
    packet.extend_from_slice(&[0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);
    packet.extend_from_slice(&[0x86, 0xdd]); // EtherType IPv6

    // IPv6 header (40 bytes)
    let payload_len = (20 + payload_size) as u16;
    packet.extend_from_slice(&[
        0x60,
        0x00,
        0x00,
        0x00, // Version, traffic class, flow label
        (payload_len >> 8) as u8,
        (payload_len & 0xff) as u8, // Payload length
        0x06,                       // Next header: TCP
        0x40,                       // Hop limit: 64
    ]);
    // Source address (::1)
    packet.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
    // Dest address (::2)
    packet.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2]);

    // TCP header
    packet.extend_from_slice(&[
        0x04, 0x00, 0x00, 0x50, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x50, 0x10, 0xff,
        0xff, 0x00, 0x00, 0x00, 0x00,
    ]);

    // Payload
    packet.extend(std::iter::repeat(0xBB).take(payload_size));

    let result = client.send_packet(&packet);
    assert!(
        result.is_ok(),
        "Should be able to send large GSO IPv6 packet: {:?}",
        result.err()
    );
}
