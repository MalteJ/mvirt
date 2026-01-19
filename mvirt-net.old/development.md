# mvirt-net Development Guide

This document covers development topics for mvirt-net, including the test harness architecture and how to write integration tests.

## Test Harness

The integration tests use a custom test harness that simulates the VM/frontend side of vhost-user connections. This allows testing the vhost-user backend without running actual VMs.

### Architecture

```
tests/
├── harness/
│   ├── mod.rs          # Module exports
│   ├── backend.rs      # TestBackend (uses real VhostNetBackend)
│   ├── client.rs       # VhostTestClient (VM-side simulation)
│   ├── memory.rs       # Shared memory via memfd
│   ├── virtio.rs       # Virtio queue management
│   └── packets.rs      # Packet builders and parsers
└── vhost_integration.rs  # Test cases
```

The test harness uses the **real** `VhostNetBackend`, `ArpResponder`, and `Dhcpv4Server` from `src/dataplane/` - not mocks. This ensures integration tests exercise the actual production code paths.

### Key Components

**TestBackend** - Spawns a vhost-user daemon in a background thread:
```rust
let backend = TestBackend::new("52:54:00:12:34:56", Some("10.0.0.5"));
let mut client = backend.connect().expect("connect failed");
```

**VhostTestClient** - Simulates the VM side:
- Performs vhost-user handshake (feature negotiation, memory setup, vring config)
- Provides `send_packet()` and `recv_packet()` for TX/RX operations
- Uses shared memory via memfd for zero-copy packet transfer

**Packet Helpers** - Build and parse network packets:
```rust
use harness::packets::{arp_request, dhcp_discover, parse_dhcp_response};
```

### Writing Tests

A typical test follows this pattern:

```rust
#[test]
fn test_example() {
    // 1. Create backend with MAC and optional IPv4 for ARP/DHCP
    let backend = TestBackend::new("52:54:00:12:34:56", Some("10.0.0.5"));

    // 2. Connect client (performs vhost-user handshake)
    let mut client = backend.connect().expect("connect failed");

    // 3. Build and send a packet
    let request = arp_request(
        [0x52, 0x54, 0x00, 0x12, 0x34, 0x56],  // sender MAC
        [10, 0, 0, 100],                        // sender IP
        [169, 254, 0, 1],                       // target IP (gateway)
    );
    client.send_packet(&request).expect("send failed");

    // 4. Receive and verify response
    let reply = client.recv_packet(2000).expect("timeout");
    let arp = parse_arp_reply(&reply).expect("parse failed");
    assert_eq!(arp.sender_ip, [169, 254, 0, 1]);
}
```

### Available Packet Builders

| Function | Description |
|----------|-------------|
| `ethernet_frame(dst, src, ethertype, payload)` | Raw Ethernet frame |
| `arp_request(sender_mac, sender_ip, target_ip)` | ARP request |
| `dhcp_discover(client_mac, xid)` | DHCP Discover |
| `dhcp_request(client_mac, xid, requested_ip, server_ip)` | DHCP Request |

### Available Parsers

| Function | Description |
|----------|-------------|
| `is_arp_reply(frame)` | Check if frame is ARP reply |
| `parse_arp_reply(frame)` | Parse ARP reply |
| `parse_dhcp_response(frame)` | Parse DHCP Offer/Ack |

### Test Backend Behavior

When created with an IPv4 address, the backend uses the real `ArpResponder` and `Dhcpv4Server` implementations:

- **ARP**: `ArpResponder` responds to requests for gateway IP (169.254.0.1)
- **DHCP**: `Dhcpv4Server` responds to Discover with Offer, Request with Ack
  - Assigned IP: The IPv4 passed to `TestBackend::new()`
  - Subnet mask: /32 (255.255.255.255)
  - Router: 169.254.0.1
  - DNS: 1.1.1.1, 8.8.8.8
  - Lease time: 86400 seconds (24h)

### Constants

```rust
use harness::{GATEWAY_MAC, GATEWAY_IP};

// GATEWAY_MAC = [0x02, 0x00, 0x00, 0x00, 0x00, 0x01]
// GATEWAY_IP  = [169, 254, 0, 1]
```

## Running Tests

```bash
# All integration tests
cargo test -p mvirt-net --test vhost_integration

# With output (useful for debugging)
cargo test -p mvirt-net --test vhost_integration -- --nocapture

# Filter by keyword
cargo test -p mvirt-net --test vhost_integration handshake
cargo test -p mvirt-net --test vhost_integration arp
cargo test -p mvirt-net --test vhost_integration dhcp

# Unit tests only
cargo test -p mvirt-net --lib
```

## Dependencies

The test harness uses these dev-dependencies:

```toml
[dev-dependencies]
tempfile = "3"
vhost = { version = "0.13", features = ["vhost-user-frontend"] }
nix = { version = "0.29", features = ["poll", "event"] }
```

The `vhost-user-frontend` feature provides the `Frontend` type for simulating the VM side of the vhost-user protocol.
