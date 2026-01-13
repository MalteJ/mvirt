# mvirt Development Guidelines

## Project Structure

```
mvirt/
├── mvirt/           # CLI client + TUI
│   └── src/
│       ├── main.rs  # CLI commands + entry point
│       └── tui.rs   # ratatui TUI with async channels
├── mvirt-vmm/       # Daemon (VM Manager)
│   └── src/
│       ├── main.rs      # CLI args, server startup
│       ├── grpc.rs      # gRPC service implementation
│       ├── store.rs     # SQLite persistence
│       └── hypervisor.rs # cloud-hypervisor process management
├── proto/
│   └── mvirt.proto  # gRPC API definition
└── images/          # Kernel and disk images (not in git)
```

## Build & Run

```bash
# Development (uses ./tmp for data)
cargo run --bin mvirt-vmm -- --data-dir ./tmp

# Client CLI
cargo run --bin mvirt -- list
cargo run --bin mvirt -- create --name test --kernel images/hypervisor-fw --disk images/ubuntu-noble.raw

# TUI (no subcommand)
cargo run --bin mvirt
```

## Code Quality

**ALWAYS run before committing:**
```bash
cargo fmt && cargo clippy
```

No warnings allowed in either crate.

## Architecture

### gRPC API (proto/mvirt.proto)
- `CreateVm`, `GetVm`, `ListVms`, `DeleteVm` - CRUD
- `StartVm`, `StopVm`, `KillVm` - Lifecycle
- `Console` - Bidirectional streaming for serial console
- `AttachDisk`, `DetachDisk`, `AttachNic`, `DetachNic` - Hot-plug (not yet implemented)

### SQLite Schema (store.rs)
- `vms` table: VM definitions (id, name, state, config_json, timestamps)
- `vm_runtime` table: Runtime info for running VMs (pid, sockets)

### Hypervisor (hypervisor.rs)
- Spawns cloud-hypervisor processes
- Creates TAP devices and attaches to bridge
- Generates cloud-init ISO with user-data, meta-data, network-config
- Background watcher task monitors child processes for unexpected exits
- `recover_vms()` on daemon startup cleans up stale VMs

### TUI (tui.rs)
- Non-blocking async architecture with channels
- Background worker handles gRPC calls
- Auto-refresh every 2 seconds
- Delete requires y/n confirmation

## VM Runtime Directory

```
<data-dir>/vm/<vm-id>/
├── api.sock         # cloud-hypervisor HTTP API socket
├── serial.sock      # Serial console Unix socket
├── cloudinit.img    # cloud-init ISO (NoCloud datasource)
└── stdout.log       # cloud-hypervisor stdout/stderr
```

## Networking

- Each VM gets a TAP device (`mvirt-<short-id>`)
- TAPs attached to bridge (default: br0)
- DHCP/DHCPv6 on bridge for VM IPs
- cloud-init configures DHCP for all interfaces

## Key Decisions

- **SQLite** for persistence from start (not in-memory)
- **cloud-hypervisor** instead of QEMU (simpler, modern)
- **TAP + bridge** networking (no user-mode networking)
- **Async channels** in TUI (no blocking gRPC calls)
- **Console escape**: Ctrl+a t (not Ctrl+])

## Common Issues

### VM won't start
- Check kernel path exists
- Check disk images exist
- Run daemon with `RUST_LOG=debug` for details

### VM state stuck
- Daemon recovery should fix on restart
- Check `ps aux | grep cloud-hypervisor`
- Manual cleanup: remove runtime from DB, kill process

### TUI hangs
- Should not happen with async architecture
- If it does, check channel capacity
