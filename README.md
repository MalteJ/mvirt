# mvirt

> [!WARNING]
> Dieses Projekt wurde vibe-coded mit Claude Code. Not for production use!

Leichtgewichtiger VM-Manager in Rust als moderne Alternative zu libvirt.

## Features

- **cloud-hypervisor** als Hypervisor (statt QEMU)
- **gRPC API** für einfache Integration
- **TUI** mit ratatui
- **SQLite** für persistenten State
- **Statisch gelinkt** mit musl für einfaches Deployment

## Architektur

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
│                 cloud-hypervisor Prozesse                   │
│       ┌────────┐    ┌────────┐    ┌────────┐                │
│       │  VM 1  │    │  VM 2  │    │  VM n  │                │
│       └────────┘    └────────┘    └────────┘                │
└─────────────────────────────────────────────────────────────┘
```

## Komponenten

| Verzeichnis | Beschreibung |
|-------------|--------------|
| `mvirt-vmm/` | Daemon der VMs verwaltet |
| `mvirt-cli/` | CLI und TUI Client |
| `mvirt-os/` | Linux-Kernel, initramfs, UKI Build-System |

## Voraussetzungen

```bash
# Rust mit musl target
rustup target add x86_64-unknown-linux-musl

# Build-Tools
sudo apt install build-essential musl-tools

# Für mvirt-os (Kernel/UKI)
sudo apt install flex bison libncurses-dev libssl-dev libelf-dev bc dwarves
sudo apt install systemd-ukify systemd-boot-efi genisoimage
```

## Build

```bash
# Alles bauen (Rust + Kernel + initramfs + UKI)
make

# Nur Rust-Binaries
make release

# Nur mvirt-os
make os
```

## Entwicklung

```bash
# Debug-Build
cargo build

# Daemon starten (Development)
cargo run --bin mvirt-vmm -- --data-dir ./tmp

# CLI/TUI starten
cargo run --bin mvirt

# Tests
cargo test --workspace

# Formatierung & Linting
cargo fmt && cargo clippy --workspace
```

## Verzeichnisstruktur

```
mvirt/
├── Cargo.toml              # Workspace
├── Makefile                # Build-Orchestrierung
├── mvirt-cli/              # CLI + TUI
│   ├── src/
│   │   ├── main.rs         # CLI Commands
│   │   └── tui.rs          # TUI mit ratatui
│   └── proto/              # gRPC Proto (Client)
├── mvirt-vmm/              # Daemon
│   ├── src/
│   │   ├── main.rs         # Server-Start
│   │   ├── grpc.rs         # gRPC Handler
│   │   ├── hypervisor.rs   # cloud-hypervisor Management
│   │   └── store.rs        # SQLite Persistence
│   └── proto/              # gRPC Proto (Server)
├── mvirt-os/               # OS Build-System
│   ├── pideins/            # Rust init (PID 1)
│   ├── initramfs/          # initramfs Skeleton
│   ├── kernel.config       # Kernel Kconfig Fragment
│   └── *.mk                # Make-Includes
├── images/                 # VM Disk Images
└── tmp/                    # Development Data-Dir
```

## Output

| Datei | Beschreibung |
|-------|--------------|
| `target/x86_64-unknown-linux-musl/release/mvirt` | CLI Binary |
| `target/x86_64-unknown-linux-musl/release/mvirt-vmm` | Daemon Binary |
| `mvirt-os/mvirt.efi` | Bootbares UKI |

## Lizenz

MIT
