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
├── mvirt-uos/           # µOS - Minimal Linux for MicroVMs
│   ├── pideisn/         # Rust init process (PID 1)
│   ├── initramfs/       # rootfs skeleton
│   ├── kernel.config    # Kernel config fragment
│   └── mvirt-uos.mk     # OS build rules
├── docs/                # Documentation
└── Makefile             # Main build orchestration
```

Each service has its own `proto/` subdirectory with gRPC definitions.

## Build System

The build system uses GNU Make with a dependency-based approach. Targets only rebuild when their dependencies change.

### Main Targets

| Target | Description |
|--------|-------------|
| `make` | Build everything (Rust + UKI) |
| `make release` | Build Rust binaries (musl, static) |
| `make uos` | Build mvirt-uos (UKI) |
| `make kernel` | Build kernel only |
| `make initramfs` | Build initramfs only |
| `make clean` | Remove build artifacts |
| `make distclean` | Remove everything including kernel source |
| `make check` | Verify build dependencies are installed |
| `make docker` | Build in Docker (no local deps needed) |

### Dependency Chain

The build system automatically resolves dependencies:

```
make uos
  └── $(UKI)                      # Unified Kernel Image
        ├── $(BZIMAGE)            # Linux kernel
        │     └── .config
        │           └── kernel.config (fragment)
        ├── $(INITRAMFS)          # Root filesystem
        │     ├── pideisn         # Init process
        │     ├── mvirt           # CLI
        │     ├── mvirt-vmm       # Daemon
        │     ├── cloud-hypervisor
        │     └── hypervisor-fw
        └── cmdline.txt           # Kernel command line
```

Running `make uos` automatically builds all dependencies in the correct order.

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

Build outputs:

| File | Description |
|------|-------------|
| `mvirt-uos/target/mvirt-uos.efi` | UKI (kernel + initramfs + cmdline) |
| `mvirt-uos/target/cloud-hypervisor` | Downloaded hypervisor binary |
| `mvirt-uos/target/hypervisor-fw` | Downloaded firmware |

## Development Workflow

### Code Changes

1. Edit code in any module (`mvirt-cli/`, `mvirt-vmm/`, `mvirt-zfs/`, `mvirt-net/`, `mvirt-log/`, `mvirt-uos/pideisn/`)
2. Run `cargo fmt && cargo clippy --workspace` to check formatting and lints
3. Run `make uos` to rebuild UKI, or `cargo build` for daemons only

### Testing with cloud-hypervisor

```bash
# Build UKI
make uos

# Test with cloud-hypervisor (direct kernel boot)
cloud-hypervisor \
    --kernel mvirt-uos/target/mvirt-uos.efi \
    --cpus boot=1 --memory size=512M \
    --console off --serial tty
```

### Adding Dependencies

Build dependencies can be checked with:

```bash
make check
```

Required packages (Debian/Ubuntu):
```bash
apt install build-essential flex bison libelf-dev libssl-dev bc
apt install systemd-ukify systemd-boot-efi    # For UKI building
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
