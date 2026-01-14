# pideisn

A minimal init process (PID 1) for lightweight Linux VMs, written in pure Rust.

## Name

The name "pideisn" is a happy typo that stuck. It sits somewhere between "PID eins" (German for "PID one", referring to the init process) and "PID Eisen" (German for "PID iron"). The iron connection is fitting: Eisen oxidizes into Rust, and this init system is written in Rust.

## Overview

pideisn is a monolithic init system designed as a systemd replacement for mini-Linux virtual machines. It handles all essential system initialization tasks without calling external binaries.

## Features

- **Filesystem Mounting**: Mounts essential kernel virtual filesystems (/proc, /sys, /dev, /run, /tmp)
- **Signal Handling**: Proper SIGCHLD handling for zombie process reaping
- **Network Configuration**:
  - Interface discovery via sysfs
  - DHCPv4 client (full DORA flow)
  - DHCPv6 client with Prefix Delegation (PD) support
  - SLAAC for IPv6 link-local and global addresses
  - All network configuration via netlink (no external tools)
- **Service Management**: Spawns and monitors child services (e.g., mvirt-vmm)
- **IPv6 Prefix Delegation Pool**: Manages delegated prefixes for nested VMs

## Architecture

```
src/
├── main.rs           # Entry point, async runtime, main loop
├── error.rs          # Error types
├── log.rs            # Serial console logging macros
├── mount.rs          # Filesystem mounting (nix::mount)
├── signals.rs        # Signal handling, zombie reaping
├── service.rs        # Service management
└── network/
    ├── mod.rs        # Network coordinator
    ├── interface.rs  # Interface discovery (/sys/class/net)
    ├── netlink.rs    # Netlink operations (rtnetlink)
    ├── slaac.rs      # SLAAC (Router Solicitation/Advertisement)
    ├── pd.rs         # Prefix delegation pool
    ├── dhcp4/
    │   ├── mod.rs
    │   └── client.rs # DHCPv4 state machine
    └── dhcp6/
        ├── mod.rs
        └── client.rs # DHCPv6 state machine with IA_NA/IA_PD
```

## Dependencies

- `nix` - Safe Rust bindings for Unix system calls
- `rtnetlink` - Netlink protocol for network configuration
- `dhcproto` - DHCP protocol encoding/decoding
- `tokio` - Async runtime
- `socket2` - Low-level socket operations

## Building

```bash
# From workspace root
cargo build --release -p pideisn

# For static linking (recommended for initramfs)
cargo build --release -p pideisn --target x86_64-unknown-linux-musl
```

## Boot Sequence

1. Mount virtual filesystems (/proc, /sys, /dev, /run, /tmp)
2. Setup signal handlers
3. Initialize async runtime
4. Discover network interfaces
5. For each interface:
   - Bring interface up
   - Configure IPv6 link-local via SLAAC
   - Attempt DHCPv4 configuration
   - Attempt DHCPv6 with prefix delegation
6. Start registered services (mvirt-vmm)
7. Enter main loop (reap zombies, monitor services)

## Network Protocol Support

### DHCPv4
- DISCOVER → OFFER → REQUEST → ACK flow
- Obtains: IP address, netmask, gateway, DNS servers
- Automatic retry with exponential backoff

### DHCPv6
- SOLICIT → ADVERTISE → REQUEST → REPLY flow
- Supports IA_NA (address assignment)
- Supports IA_PD (prefix delegation for nested VMs)
- Obtains: IPv6 address, delegated prefix, DNS servers

### SLAAC
- Generates link-local address from MAC (EUI-64)
- Sends Router Solicitation (ICMPv6)
- Processes Router Advertisements for global addresses
- Configures default gateway from RA source

## Design Principles

- **Pure Rust**: No external binary calls for core functionality
- **Minimal Dependencies**: Only essential crates
- **Never Exit**: PID 1 must never terminate
- **Graceful Degradation**: Network failures don't prevent boot
- **Static Linking**: Designed for musl-based initramfs

## License

Part of the mvirt project.
