# mvirt-net

Virtual network daemon for mvirt. Provides L3 networking for VMs using vhost-user virtio-net backends.

## Documentation

- **[Networking Concepts](../docs/concepts/networking.md)** - User-facing documentation on networks, vNICs, and IP addressing
- **[Architecture](architecture.md)** - Technical deep-dive into the implementation
- **[Development](development.md)** - Test harness and how to write integration tests

## Features

- **L3 Networking**: Pure Layer 3 routing between VMs (no L2 switching)
- **DHCPv4/DHCPv6**: Automatic IP address assignment
- **Dual-Stack**: IPv4-only, IPv6-only, or dual-stack networks
- **Network Isolation**: VMs in different networks are isolated (multi-tenant)
- **Routed Prefixes**: Additional prefixes can be routed to vNICs for VMs acting as routers
- **vhost-user**: High-performance virtio-net using shared memory
- **io_uring**: Asynchronous I/O for efficient packet processing
- **Hugepages**: Optional hugepage-backed buffers for reduced TLB misses

## Architecture

mvirt-net is a standalone gRPC daemon that manages virtual networking independently from mvirt-vmm:

```
┌─────────────┐     ┌─────────────┐     ┌─────────────┐
│  mvirt-cli  │────▶│  mvirt-net  │     │  mvirt-vmm  │
│   (TUI)     │     │   :50054    │     │   :50051    │
└─────────────┘     └─────────────┘     └─────────────┘
      │                   │                   ▲
      │  1. CreateNic     │                   │
      │──────────────────▶│                   │
      │                   │                   │
      │  Nic { socket: "/run/mvirt/net/..." } │
      │◀──────────────────│                   │
      │                                       │
      │  2. StartVm { net_socket: ... }       │
      └───────────────────────────────────────┘
```

This loose coupling means:
- mvirt-vmm accepts any vhost-user socket path (doesn't know about networks)
- mvirt-net manages virtual networking (doesn't know about VMs)
- mvirt-cli orchestrates the workflow

## Usage

```bash
# Start the daemon
mvirt-net --listen [::1]:50054 --socket-dir /run/mvirt/net

# With custom metadata directory
mvirt-net --listen [::1]:50054 --socket-dir /run/mvirt/net --metadata-dir /var/lib/mvirt/net
```

## gRPC API

The daemon exposes `NetService` on port 50054:

### Network Operations
- `CreateNetwork` - Create a new isolated network
- `GetNetwork` - Get network details
- `ListNetworks` - List all networks
- `UpdateNetwork` - Update network configuration
- `DeleteNetwork` - Delete a network (and all its vNICs)

### vNIC Operations
- `CreateNic` - Create a vNIC in a network (returns socket path)
- `GetNic` - Get vNIC details including assigned IP
- `ListNics` - List vNICs (optionally filtered by network)
- `UpdateNic` - Update vNIC (e.g., add/remove routed prefixes)
- `DeleteNic` - Delete a vNIC

## Quick Start

### 1. Create a Network

```bash
# Create a dual-stack network
mvirt network create --name prod \
    --ipv4-subnet 10.0.0.0/24 \
    --ipv6-prefix fd00::/64 \
    --dns 1.1.1.1
```

### 2. Create vNICs

```bash
# Create vNICs for two VMs
mvirt nic create --network prod --name vm1-eth0
mvirt nic create --network prod --name vm2-eth0
```

### 3. Start VMs

```bash
mvirt vm start vm1 --nic <nic-id-1>
mvirt vm start vm2 --nic <nic-id-2>
```

### 4. Test Connectivity

```bash
# Inside vm1
ping <vm2-ip>
```

## Data Plane

Each vNIC is handled by a Reactor using io_uring for efficient async I/O:

- **ARP Responder**: Responds to ARP requests for gateway (169.254.0.1)
- **ICMPv6/NDP Responder**: Responds to Neighbor Solicitations for fe80::1
- **Router Advertisements**: Periodic RAs for IPv6 (M=1, O=1)
- **DHCPv4 Server**: Assigns /32 addresses
- **DHCPv6 Server**: Assigns /128 addresses
- **L3 Router**: Routes packets between vNICs via inter-reactor messaging

## Building

```bash
# From workspace root
cargo build -p mvirt-net

# Release build
cargo build -p mvirt-net --release
```

## Testing

```bash
# Run all tests
cargo test -p mvirt-net

# Run with output
cargo test -p mvirt-net -- --nocapture
```

See [development.md](development.md) for details on the test harness.

## System Requirements

- Linux kernel 5.6+ (io_uring support)
- Hugepages recommended for production (2MB pages)

```bash
# Allocate hugepages
echo 64 | sudo tee /proc/sys/vm/nr_hugepages
```
