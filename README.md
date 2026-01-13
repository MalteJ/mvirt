# mvirt

> [!WARNING]
> This project was vibe-coded with Claude Code. Not for production use!

Lightweight VM manager in Rust as a modern alternative to libvirt.

## Features

- **cloud-hypervisor** as hypervisor (instead of QEMU)
- **gRPC API** for easy integration
- **TUI** with ratatui
- **SQLite** for persistent state
- **Statically linked** with musl for easy deployment

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                        mvirt (CLI/TUI)                      │
└─────────────────────────┬───────────────────────────────────┘
                          │ gRPC
┌─────────────────────────▼───────────────────────────────────┐
│                      mvirt-vmm (Daemon)                     │
│  ┌─────────────┐  ┌──────────────┐  ┌───────────────────┐   │
│  │ gRPC Server │  │  Hypervisor  │  │  SQLite Store     │   │
│  └─────────────┘  └──────┬───────┘  └───────────────────┘   │
└──────────────────────────┼──────────────────────────────────┘
                           │ HTTP API (Unix Socket)
┌──────────────────────────▼──────────────────────────────────┐
│                 cloud-hypervisor Processes                  │
│       ┌────────┐    ┌────────┐    ┌────────┐                │
│       │  VM 1  │    │  VM 2  │    │  VM n  │                │
│       └────────┘    └────────┘    └────────┘                │
└─────────────────────────────────────────────────────────────┘
```

## Components

| Directory | Description |
|-----------|-------------|
| `mvirt-vmm/` | Daemon that manages VMs |
| `mvirt-cli/` | CLI and TUI client |
| `mvirt-os/` | Linux kernel, initramfs, UKI build system |

## Prerequisites

```bash
# Rust with musl target
rustup target add x86_64-unknown-linux-musl

# Build tools
sudo apt install build-essential musl-tools

# For mvirt-os (Kernel/UKI)
sudo apt install flex bison libncurses-dev libssl-dev libelf-dev bc dwarves
sudo apt install systemd-ukify systemd-boot-efi genisoimage
```

## Build

```bash
# Build everything (Rust + Kernel + initramfs + UKI)
make

# Rust binaries only
make release

# mvirt-os only
make os
```

## Development

```bash
# Debug build
cargo build

# Start daemon (development)
cargo run --bin mvirt-vmm -- --data-dir ./tmp

# Start CLI/TUI
cargo run --bin mvirt

# Tests
cargo test --workspace

# Formatting & linting
cargo fmt && cargo clippy --workspace
```

## Directory Structure

```
mvirt/
├── Cargo.toml              # Workspace
├── Makefile                # Build orchestration
├── mvirt-cli/              # CLI + TUI
│   ├── src/
│   │   ├── main.rs         # CLI commands
│   │   └── tui.rs          # TUI with ratatui
│   └── proto/              # gRPC proto (client)
├── mvirt-vmm/              # Daemon
│   ├── src/
│   │   ├── main.rs         # Server startup
│   │   ├── grpc.rs         # gRPC handlers
│   │   ├── hypervisor.rs   # cloud-hypervisor management
│   │   └── store.rs        # SQLite persistence
│   └── proto/              # gRPC proto (server)
├── mvirt-os/               # OS build system
│   ├── pideins/            # Rust init (PID 1)
│   ├── initramfs/          # initramfs skeleton
│   ├── kernel.config       # Kernel Kconfig fragment
│   └── *.mk                # Make includes
├── images/                 # VM disk images
└── tmp/                    # Development data dir
```

## Output

| File | Description |
|------|-------------|
| `target/x86_64-unknown-linux-musl/release/mvirt` | CLI binary |
| `target/x86_64-unknown-linux-musl/release/mvirt-vmm` | Daemon binary |
| `mvirt-os/mvirt.efi` | Bootable UKI |

## License

Apache-2.0
