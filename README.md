# mvirt

Ein leichtgewichtiger VM-Manager in Rust als moderne Alternative zu libvirt.

## Features

- **cloud-hypervisor** als Hypervisor (statt QEMU)
- **gRPC API** für einfache Integration
- **Rust** für Memory Safety und Performance
- **SQLite** für persistenten State

## Architektur

```
┌─────────────────────────────────────────────────────────┐
│                      Clients                            │
│              (CLI, Web-UI, andere Services)             │
└─────────────────────┬───────────────────────────────────┘
                      │ gRPC
┌─────────────────────▼───────────────────────────────────┐
│                    mvirt-vmm (Daemon)                   │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────────┐  │
│  │ gRPC Server │  │ VM Manager  │  │ Storage Manager │  │
│  └─────────────┘  └──────┬──────┘  └─────────────────┘  │
│                          │                              │
│  ┌───────────────────────▼──────────────────────────┐   │
│  └──────────────────────────────────────────────────┘   │
└─────────────────────┬───────────────────────────────────┘
                      │ HTTP API (Unix Socket)
┌─────────────────────▼───────────────────────────────────┐
│              cloud-hypervisor Prozesse                  │
│     ┌────────┐    ┌────────┐    ┌────────┐              │
│     │  VM 1  │    │  VM 2  │    │  VM n  │              │
│     └────────┘    └────────┘    └────────┘              │
└─────────────────────────────────────────────────────────┘
```

## Komponenten

| Komponente | Beschreibung |
|------------|--------------|
| `mvirt-vmm` | Daemon der VMs verwaltet |
| `mvirt` | CLI Client (geplant) |

## Voraussetzungen

- Rust 1.70+
- cloud-hypervisor (`/usr/bin/cloud-hypervisor`)
- Linux mit KVM-Unterstützung

## Build

```bash
cargo build --release
```

## Verwendung

```bash
# Daemon starten
./target/release/mvirt-vmm

# VM erstellen (via grpcurl)
grpcurl -plaintext -d '{
  "name": "test-vm",
  "config": {
    "vcpus": 2,
    "memory_mb": 1024,
    "kernel": "/path/to/vmlinux",
    "disk": "/path/to/disk.raw"
  }
}' localhost:50051 mvirt.VmService/CreateVm
```

## gRPC API

Die API ist in `proto/mvirt.proto` definiert. Hauptendpunkte:

- `CreateVm` - VM erstellen
- `StartVm` - VM starten
- `StopVm` - VM stoppen
- `DeleteVm` - VM löschen
- `GetVm` - VM-Details abrufen
- `ListVms` - Alle VMs auflisten

## Lizenz

MIT
