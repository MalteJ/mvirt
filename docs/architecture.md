# Architecture

mvirt is a modular VM management system built around loosely-coupled gRPC services.

## Overview

```
┌──────────────────────────────────────────────────────────────────────┐
│                          mvirt-cli (TUI/CLI)                         │
└───────┬──────────────┬──────────────┬──────────────┬─────────────────┘
        │              │              │              │
        │ gRPC         │ gRPC         │ gRPC         │ gRPC
        ▼              ▼              ▼              ▼
┌───────────────┐ ┌───────────────┐ ┌───────────────┐ ┌───────────────┐
│   mvirt-vmm   │ │   mvirt-zfs   │ │   mvirt-net   │ │   mvirt-log   │
│    :50051     │ │    :50053     │ │    :50054     │ │    :50052     │
│               │ │               │ │               │ │               │
│  VM lifecycle │ │ ZFS storage   │ │ vNIC + DHCP   │ │ Audit logs    │
│  SQLite state │ │ Volumes/Snaps │ │ vhost-user    │ │ Queries       │
└───────┬───────┘ └───────┬───────┘ └───────┬───────┘ └───────────────┘
        │                 │                 │                 ▲
        │                 │                 │                 │
        ▼                 ▼                 ▼                 │
┌───────────────┐ ┌───────────────┐ ┌───────────────┐        │
│ cloud-        │ │ ZFS Pool      │ │ TAP devices   │ ◀──────┘
│ hypervisor    │ │ (zvols)       │ │ Linux bridge  │   All services
└───────────────┘ └───────────────┘ └───────────────┘   log to mvirt-log
```

## Components

### mvirt-vmm (VM Manager)

The core daemon that manages virtual machine lifecycle.

- Spawns and monitors cloud-hypervisor processes
- Stores VM definitions in SQLite
- Provides console access via bidirectional gRPC streaming
- Recovers VMs on daemon restart

**Does not** handle storage or networking directly - accepts block device paths and socket paths from other services.

### mvirt-zfs (Storage)

Manages ZFS volumes (ZVOLs) as VM block devices.

- Creates thin-provisioned volumes
- Imports disk images from files or URLs
- Supports templates and instant cloning (CoW)
- Manages snapshots

**Loose coupling**: Returns `/dev/zvol/...` paths that mvirt-vmm uses directly.

### mvirt-net (Networking)

Manages virtual network interfaces and provides DHCP.

- Creates vhost-user backends for cloud-hypervisor
- Runs built-in DHCP server (IPv4 and IPv6)
- Routes traffic between VMs via Linux bridge

**Loose coupling**: Returns socket paths that mvirt-vmm passes to cloud-hypervisor.

### mvirt-log (Audit Logging)

Centralized logging service for audit trails.

- All state-changing operations are logged
- Many-to-many object relations (e.g., "volume attached to VM")
- Query logs by object ID

### mvirt-cli (Client)

TUI and CLI that orchestrates the other services.

- Connects to all four daemons
- Provides unified interface for VM operations
- Auto-refreshing TUI with ratatui

### mvirt-os (OS Builder)

Build system for the guest operating system.

- Compiles Linux kernel with minimal config
- Builds initramfs with pideisn (Rust init)
- Packages as UKI (Unified Kernel Image)

## Typical Workflow

Creating a VM with storage and networking:

```
1. mvirt-cli                           2. mvirt-zfs
   CreateVolume(name, size) ──────────▶ Creates ZVOL
                            ◀────────── Returns /dev/zvol/pool/name

3. mvirt-cli                           4. mvirt-net
   CreateNic(mac, network) ────────────▶ Creates vhost-user backend
                           ◀──────────── Returns socket path

5. mvirt-cli                           6. mvirt-vmm
   CreateVm(disk_path, nic_socket) ────▶ Stores in SQLite

7. mvirt-cli                           8. mvirt-vmm
   StartVm(id) ────────────────────────▶ Spawns cloud-hypervisor
                                         with disk and nic arguments
```

## Design Principles

1. **Loose coupling**: Services don't know about each other's internals
2. **CLI orchestration**: mvirt-cli coordinates the workflow
3. **gRPC everywhere**: Consistent API style across all services
4. **Audit everything**: All mutations logged to mvirt-log
5. **Stateless daemons**: State lives in SQLite/ZFS, daemons can restart

## Data Directories

| Service   | Data Location               | Contents                |
|-----------|-----------------------------|-----------------------  |
| mvirt-vmm | `/var/lib/mvirt/vmm`        | SQLite DB, sockets      |
| mvirt-zfs | ZFS pool (e.g., `vmpool`)   | Volumes, metadata DB    |
| mvirt-net | `/var/lib/mvirt/net`        | Metadata                |
| mvirt-log | `/var/lib/mvirt/log`        | Log database            |

See [reference/ports.md](reference/ports.md) for service ports.
