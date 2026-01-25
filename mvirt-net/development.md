# mvirt-net Development Guide

This document covers development topics for mvirt-net, including the test harness architecture and how to write integration tests.

## Test Harness

The integration tests use a custom test harness that simulates the VM/frontend side of vhost-user connections. This allows testing the vhost-user backend without running actual VMs.

### Architecture

```
src/
└── test_util/
    ├── mod.rs              # Module exports
    ├── frontend_device.rs  # VhostUserFrontendDevice (VM-side simulation)
    ├── virtqueue.rs        # Virtqueue driver implementation
    └── packets.rs          # Packet builders and parsers

tests/
├── dhcp_arp_ping_test.rs   # Full DHCP + ARP + ICMP flow
├── dhcpv6_test.rs          # DHCPv6 + NDP flow
├── ping_test.rs            # ICMP echo to gateway
├── vhost_test.rs           # vhost-user protocol tests
├── vm_to_vm_test.rs        # Inter-VM routing
└── vm_to_vm_ping_test.rs   # VM-to-VM ICMP
```

The test harness uses the **real** `Router`, `Reactor`, and protocol handlers - not mocks. This ensures integration tests exercise the actual production code paths.

### Key Components

**VhostUserFrontendDevice** - Simulates the VM side of vhost-user:

```rust
use mvirt_net::test_util::VhostUserFrontendDevice;

// Connect to backend
let mut frontend = VhostUserFrontendDevice::connect(socket_path)?;
frontend.setup()?;

// Provide RX buffers (like a real virtio driver)
for _ in 0..16 {
    frontend.provide_rx_buffer(4096)?;
}

// Send and receive packets
frontend.send_packet(&packet)?;
frontend.wait_for_tx(1000);

let rx_packet = frontend.recv_packet()?;
```

**Router** - Creates the backend with TUN + optional vhost:

```rust
use mvirt_net::router::{Router, VhostConfig};

let vhost_config = VhostConfig::new(socket_path, vm_mac)
    .with_ipv4(vm_ip, gateway_ip, 24)
    .with_dns(vec!["1.1.1.1".parse().unwrap()]);

let router = Router::with_config_and_vhost(
    "test_tun",
    gateway_ip,
    24,
    4096,   // buffer size
    256,    // RX buffer count
    256,    // TX buffer count
    Some(vhost_config),
).await?;
```

### Writing Tests

A typical test follows this pattern:

```rust
#[tokio::test]
async fn test_example() {
    let socket_path = "/tmp/test.sock";
    let vm_mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x01];
    let vm_ip = Ipv4Addr::new(10, 0, 0, 5);
    let gateway_ip = Ipv4Addr::new(10, 0, 0, 1);

    // Clean up stale socket
    let _ = std::fs::remove_file(socket_path);

    // 1. Start router with vhost-user backend
    let vhost_config = VhostConfig::new(socket_path, vm_mac)
        .with_ipv4(vm_ip, gateway_ip, 24);

    let router = Router::with_config_and_vhost(
        "test_tun", gateway_ip, 24, 4096, 256, 256,
        Some(vhost_config),
    ).await.expect("Router failed");

    // Give backend time to create socket
    tokio::time::sleep(Duration::from_millis(500)).await;

    // 2. Connect frontend (simulates VM)
    let mut frontend = VhostUserFrontendDevice::connect(socket_path)?;
    frontend.setup()?;

    // Provide RX buffers
    for _ in 0..16 {
        frontend.provide_rx_buffer(4096)?;
    }

    // 3. Build and send a packet
    let request = create_arp_request(
        vm_mac,                    // sender MAC
        [10, 0, 0, 5],             // sender IP
        [169, 254, 0, 1],          // target IP (gateway)
    );
    frontend.send_packet(&request)?;
    frontend.wait_for_tx(1000);

    // 4. Receive and verify response
    frontend.wait_for_rx(2000)?;
    let reply = frontend.recv_packet()?.expect("No reply");
    let arp = parse_arp_reply(&reply).expect("Parse failed");

    assert_eq!(arp.sender_ip, [169, 254, 0, 1]);
    assert_eq!(arp.sender_mac, GATEWAY_MAC);

    // 5. Cleanup
    router.shutdown().await?;
}
```

### Available Packet Builders

| Function | Description |
|----------|-------------|
| `create_arp_request(sender_mac, sender_ip, target_ip)` | ARP request |
| `create_dhcp_discover(client_mac, xid)` | DHCP Discover |
| `create_dhcp_request(client_mac, xid, requested_ip, server_ip)` | DHCP Request |
| `create_dhcpv6_solicit(client_mac, duid)` | DHCPv6 Solicit |
| `create_dhcpv6_request(client_mac, duid, server_duid, ia_na)` | DHCPv6 Request |
| `create_router_solicitation(src_mac, src_ip)` | ICMPv6 Router Solicitation |
| `create_neighbor_solicitation(src_mac, src_ip, target_ip)` | ICMPv6 Neighbor Solicitation |
| `create_icmp_echo_request(src_mac, dst_mac, src_ip, dst_ip, seq)` | ICMP Echo Request |
| `create_icmpv6_echo_request(src_mac, dst_mac, src_ip, dst_ip, seq)` | ICMPv6 Echo Request |
| `generate_duid_ll(mac)` | Generate DHCPv6 DUID-LL |

### Available Parsers

| Function | Returns | Description |
|----------|---------|-------------|
| `parse_arp_reply(frame)` | `ArpReply` | Parse ARP reply |
| `parse_dhcp_response(frame)` | `DhcpResponse` | Parse DHCP Offer/Ack |
| `parse_dhcpv6_response(frame)` | `Dhcpv6Response` | Parse DHCPv6 Advertise/Reply |
| `parse_router_advertisement(frame)` | `RaResponse` | Parse Router Advertisement |
| `parse_neighbor_advertisement(frame)` | `NaResponse` | Parse Neighbor Advertisement |
| `parse_icmp_echo_reply(frame)` | `IcmpEchoReply` | Parse ICMP Echo Reply |
| `parse_icmpv6_echo_reply(frame)` | `Icmpv6EchoReply` | Parse ICMPv6 Echo Reply |

### Response Types

```rust
pub struct DhcpResponse {
    pub msg_type: DhcpMessageType,  // Offer, Ack, Nak
    pub xid: u32,
    pub your_ip: [u8; 4],
    pub server_ip: [u8; 4],
    pub subnet_mask: Option<[u8; 4]>,
    pub router: Option<[u8; 4]>,
    pub dns_servers: Vec<[u8; 4]>,
    pub lease_time: Option<u32>,
}

pub struct ArpReply {
    pub sender_mac: [u8; 6],
    pub sender_ip: [u8; 4],
    pub target_mac: [u8; 6],
    pub target_ip: [u8; 4],
}

pub struct RaResponse {
    pub src_ip: [u8; 16],
    pub hop_limit: u8,
    pub managed: bool,      // M flag
    pub other: bool,        // O flag
    pub router_lifetime: u16,
    pub prefixes: Vec<PrefixInfo>,
}
```

### Constants

```rust
use mvirt_net::reactor::{GATEWAY_MAC, GATEWAY_IPV4_LINK_LOCAL, GATEWAY_IPV6_LINK_LOCAL};

// GATEWAY_MAC = [0x02, 0x00, 0x00, 0x00, 0x00, 0x01]
// GATEWAY_IPV4_LINK_LOCAL = 169.254.0.1
// GATEWAY_IPV6_LINK_LOCAL = fe80::1
```

## Running Tests

```bash
# All tests (requires root for TUN devices)
sudo -E cargo test -p mvirt-net

# With output (useful for debugging)
sudo -E cargo test -p mvirt-net -- --nocapture

# Specific test file
sudo -E cargo test -p mvirt-net --test dhcp_arp_ping_test

# Filter by test name
sudo -E cargo test -p mvirt-net dhcp
sudo -E cargo test -p mvirt-net vm_to_vm

# Unit tests only (no root required)
cargo test -p mvirt-net --lib
```

**Note**: Integration tests require root privileges because they create TUN devices.

## System Requirements

### Hugepages

Tests allocate hugepage-backed buffers. Ensure hugepages are available:

```bash
# Check current allocation
cat /proc/meminfo | grep Huge

# Allocate hugepages (64 x 2MB = 128MB)
echo 64 | sudo tee /proc/sys/vm/nr_hugepages

# Persistent (add to /etc/sysctl.conf)
vm.nr_hugepages = 64
```

### io_uring

Requires Linux kernel 5.6+ for io_uring support. Check with:

```bash
uname -r  # Should be >= 5.6
```

## Debugging

### Enable Tracing

Tests initialize tracing automatically. For more verbose output:

```bash
RUST_LOG=debug sudo -E cargo test -p mvirt-net -- --nocapture
```

### Common Issues

**"Failed to allocate huge pages"**
```bash
# Allocate more hugepages
echo 128 | sudo tee /proc/sys/vm/nr_hugepages
```

**"Permission denied" creating TUN**
```bash
# Run with sudo
sudo -E cargo test -p mvirt-net
```

**"Address already in use" for socket**
```bash
# Clean up stale sockets
rm -f /tmp/*.sock
```

**Test hangs on `wait_for_rx`**
- Check that RX buffers were provided (`frontend.provide_rx_buffer()`)
- Verify the packet format is correct (use Wireshark on the TUN device)
- Enable debug logging to see packet flow

### Packet Capture

To debug packet flow, capture on the TUN device:

```bash
# In another terminal, while test is running
sudo tcpdump -i test_tun -nn -vv
```

## Code Style

Before committing:

```bash
cargo fmt -p mvirt-net
cargo clippy -p mvirt-net
```

## Adding New Protocol Support

1. Add packet builder in `src/test_util/packets.rs`
2. Add parser in the same file
3. Export from `src/test_util/mod.rs`
4. Write integration test in `tests/`

Example for a new protocol:

```rust
// In packets.rs
pub fn create_new_protocol_request(...) -> Vec<u8> {
    // Build packet with virtio_net_hdr + Ethernet + payload
}

pub struct NewProtocolResponse { ... }

pub fn parse_new_protocol_response(frame: &[u8]) -> Option<NewProtocolResponse> {
    // Skip virtio_net_hdr (12 bytes) and Ethernet header (14 bytes)
    let payload = &frame[26..];
    // Parse protocol-specific data
}
```
