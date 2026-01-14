# Development Guide

This guide explains the mvirt build system and development workflow.

## Project Structure

```
mvirt/
├── mvirt-cli/           # CLI client + TUI
├── mvirt-vmm/           # Daemon (VM Manager)
├── mvirt-log/           # Centralized audit logging
├── mvirt-zfs/           # ZFS storage management
├── mvirt-net/           # Virtual networking
├── mvirt-os/            # Mini-Linux for VMs
│   ├── pideisn/         # Rust init process (PID 1)
│   ├── initramfs/       # rootfs skeleton
│   ├── kernel.config    # Kernel config fragment
│   └── mvirt-os.mk      # OS build rules
├── docs/                # Documentation
└── Makefile             # Main build orchestration
```

Each service has its own `proto/` subdirectory with gRPC definitions.

## Build System

The build system uses GNU Make with a dependency-based approach. Targets only rebuild when their dependencies change.

### Main Targets

| Target | Description |
|--------|-------------|
| `make` | Build everything (Rust + OS) |
| `make release` | Build Rust binaries (musl, static) |
| `make os` | Build kernel + initramfs + UKI |
| `make iso` | Build bootable ISO (BIOS + UEFI) |
| `make clean` | Remove build artifacts |
| `make distclean` | Remove everything including kernel source |
| `make check` | Verify build dependencies are installed |
| `make docker` | Build ISO in Docker (no local deps needed) |

### Dependency Chain

The build system automatically resolves dependencies:

```
make iso
  └── $(ISO)
        └── $(UKI)                    # Unified Kernel Image
              ├── $(BZIMAGE)          # Linux kernel
              │     └── .config
              │           └── kernel.config (fragment)
              └── $(INITRAMFS)        # Root filesystem
                    ├── pideisn       # Init process
                    ├── mvirt         # CLI
                    ├── mvirt-vmm     # Daemon
                    ├── cloud-hypervisor
                    └── hypervisor-fw
```

Running `make iso` automatically builds all dependencies in the correct order.

### Rust Binaries

Rust binaries are cross-compiled for musl to produce fully static executables:

```bash
cargo build --release --target x86_64-unknown-linux-musl
```

The binaries are:
- `pideisn` - Init process (PID 1) for the mini-Linux
- `mvirt` - CLI client
- `mvirt-vmm` - VM manager daemon
- `mvirt-log` - Audit logging service
- `mvirt-zfs` - ZFS storage daemon
- `mvirt-net` - Networking daemon

### Output Artifacts

All build outputs go to `mvirt-os/target/`:

| File | Description |
|------|-------------|
| `initramfs.cpio.gz` | Compressed root filesystem |
| `mvirt.efi` | Unified Kernel Image (bootable) |
| `mvirt-os.iso` | Bootable ISO image |
| `cloud-hypervisor` | Downloaded hypervisor binary |
| `hypervisor-fw` | Downloaded UEFI firmware |

## Development Workflow

### Code Changes

1. Edit code in any module (`mvirt-cli/`, `mvirt-vmm/`, `mvirt-zfs/`, `mvirt-net/`, `mvirt-log/`, `mvirt-os/pideisn/`)
2. Run `cargo fmt && cargo clippy --workspace` to check formatting and lints
3. Run `make iso` to rebuild with changes (for mvirt-os), or `cargo build` for daemons only

### Testing in VM

```bash
# Build ISO
make iso

# Test with QEMU (UEFI)
qemu-system-x86_64 -bios /usr/share/ovmf/OVMF.fd \
    -cdrom mvirt-os/target/mvirt-os.iso \
    -m 2G -enable-kvm

# Test with QEMU (BIOS)
qemu-system-x86_64 -cdrom mvirt-os/target/mvirt-os.iso \
    -m 2G -enable-kvm
```

### Adding Dependencies

Build dependencies can be checked with:

```bash
make check
```

Required packages (Debian/Ubuntu):
```bash
apt install build-essential flex bison libelf-dev libssl-dev
apt install systemd-ukify          # For UKI building
apt install isolinux syslinux-common xorriso  # For ISO building
rustup target add x86_64-unknown-linux-musl   # Rust musl target
```

## Docker Build

Build without installing dependencies locally:

```bash
make docker
```

This builds a Docker image with all dependencies (Rust, musl, kernel build tools, etc.) and runs `make iso` inside the container. Files are owned by your user, not root.

The Docker image is cached, so subsequent builds are fast.

### What's in the Container

- Debian Trixie
- Rust + musl target
- Kernel build tools (flex, bison, libelf, etc.)
- UKI tools (systemd-ukify)
- ISO tools (isolinux, xorriso)
- protobuf compiler

### Manual Docker Usage

```bash
# Build image only
docker build -t mvirt-builder .

# Run any make target
docker run --rm --user $(id -u):$(id -g) -v $(pwd):/work mvirt-builder make os
```

## Code Quality

Always run before committing:

```bash
cargo fmt && cargo clippy
```

No warnings allowed in CI.
