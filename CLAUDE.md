# mvirt Development Guidelines

See [README.md](README.md) for build commands and project overview.

## Project Structure

```
mvirt/
├── mvirt-cli/       # CLI client + TUI
├── mvirt-vmm/       # Daemon (VM Manager)
├── mvirt-os/        # Mini-Linux for VMs
│   ├── pideins/     # Rust init process (PID 1)
│   └── initramfs/   # rootfs skeleton
├── proto/           # gRPC API definition
└── images/          # Kernel and disk images (not in git)
```

## Code Quality

**ALWAYS run before committing:**
```bash
cargo fmt && cargo clippy
```

No warnings allowed.

## Architecture

### gRPC API (proto/mvirt.proto)
- `CreateVm`, `GetVm`, `ListVms`, `DeleteVm` - CRUD
- `StartVm`, `StopVm`, `KillVm` - Lifecycle
- `Console` - Bidirectional streaming for serial console

### SQLite Schema (store.rs)
- `vms` table: VM definitions (id, name, state, config_json, timestamps)
- `vm_runtime` table: Runtime info for running VMs (pid, sockets)

### Hypervisor (hypervisor.rs)
- Spawns cloud-hypervisor processes
- Creates TAP devices and attaches to bridge
- Generates cloud-init ISO
- Background watcher monitors child processes
- `recover_vms()` on daemon startup cleans up stale VMs

### TUI (tui.rs)
- Non-blocking async with channels
- Background worker handles gRPC calls
- Auto-refresh every 2 seconds

## Key Decisions

- **SQLite** for persistence (not in-memory)
- **cloud-hypervisor** instead of QEMU
- **TAP + bridge** networking
- **Async channels** in TUI
- **Console escape**: Ctrl+a t
- **musl** for static linking
- **UKI** for simple VM boot

## Common Issues

### VM won't start
- Check kernel/disk paths exist
- Run daemon with `RUST_LOG=debug`

### VM state stuck
- Daemon recovery should fix on restart
- Check `ps aux | grep cloud-hypervisor`
