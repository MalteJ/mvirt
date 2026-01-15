# Getting Started

Quick guide to running mvirt.

## Prerequisites

- Linux with KVM support (`/dev/kvm`)
- cloud-hypervisor binary in PATH
- For storage: ZFS pool (optional but recommended)
- For networking: Root privileges (for TAP devices)

## Build

```bash
# Build all binaries
make release

# Or just Rust binaries (no kernel build)
cargo build --workspace --release
```

## Start Services

### Minimal Setup (VM Manager only)

```bash
# Terminal 1: Start the VM manager
mvirt-vmm --data-dir ./data

# Terminal 2: Start the TUI
mvirt
```

### Full Setup (with storage and networking)

```bash
# Terminal 1: Logging service
mvirt-log --data-dir ./log-data

# Terminal 2: VM manager
mvirt-vmm --data-dir ./vmm-data

# Terminal 3: ZFS storage (requires ZFS pool)
mvirt-zfs --pool vmpool

# Terminal 4: Networking (requires root)
sudo mvirt-net --socket-dir /run/mvirt/net

# Terminal 5: TUI
mvirt
```

## Create a VM

### Via TUI

1. Start the TUI: `mvirt`
2. Press `c` to open the create dialog
3. Fill in: name, kernel path, disk path
4. Press Enter to create

### Via CLI

```bash
# Create a VM
mvirt create my-vm \
  --kernel /path/to/kernel \
  --disk /path/to/disk.raw \
  --memory 1024 \
  --cpus 2

# List VMs
mvirt list

# Start VM
mvirt start my-vm

# Connect to console
mvirt console my-vm
# Exit console: Ctrl+a t

# Stop VM
mvirt stop my-vm
```

## With ZFS Storage

```bash
# Import a disk image
mvirt volume import debian-base --source /path/to/debian.qcow2

# Create a template
mvirt template create debian-base --snapshot initial

# Clone from template
mvirt volume clone my-vm-disk --template debian-base --snapshot initial

# Create VM using the cloned volume
mvirt create my-vm \
  --kernel /path/to/kernel \
  --disk /dev/zvol/vmpool/my-vm-disk
```

## With Networking

```bash
# Create a NIC
mvirt nic create my-vm-nic --network default

# Create VM with NIC
mvirt create my-vm \
  --kernel /path/to/kernel \
  --disk /dev/zvol/vmpool/my-vm-disk \
  --nic my-vm-nic
```

## Development Setup

For development with a temporary data directory:

```bash
# Start VM manager with debug logging
RUST_LOG=debug cargo run --bin mvirt-vmm -- --data-dir ./tmp

# In another terminal
cargo run --bin mvirt
```

## Next Steps

- [Architecture](architecture.md) - How components work together
- [Networking](concepts/networking.md) - vNIC configuration and IP addressing
- [Storage](concepts/storage.md) - Templates, snapshots, and cloning
- [Development](development/building.md) - Build system and contributing
