# mvirt Development Guidelines

See [README.md](README.md) for build commands and project overview.

## Project Structure

```
mvirt/
├── mvirt-api/       # Raft-based distributed control plane
│   ├── src/
│   │   ├── grpc/    # gRPC NodeService server
│   │   ├── rest/    # REST API (handlers per resource)
│   │   ├── store/   # Raft storage, event sourcing
│   │   ├── state.rs # State machine + command handling
│   │   └── scheduler.rs
│   └── proto/       # node.proto
├── mvirt-node/      # Node agent (reconciles desired state from API)
│   ├── src/
│   │   ├── clients/ # gRPC clients for local services (vmm, ebpf, zfs)
│   │   └── reconciler/ # Per-resource reconcilers (vm, nic, volume, network, route, security_group, template)
├── mvirt-vmm/       # Local hypervisor daemon (VM + Pod management)
│   ├── src/
│   │   ├── grpc.rs        # VmService implementation
│   │   ├── hypervisor.rs  # cloud-hypervisor process management
│   │   ├── pod_service.rs # PodService implementation
│   │   └── store.rs       # SQLite state
│   └── proto/       # mvirt.proto (VmService + PodService)
├── mvirt-ebpf/      # eBPF-based networking (replaces mvirt-net)
│   ├── src/
│   │   ├── ebpf_loader.rs   # eBPF program loading
│   │   ├── proto_handler.rs # IPv4/IPv6/DHCP/ICMP handling
│   │   ├── tap.rs           # TAP device management
│   │   └── nat.rs           # NAT + conntrack
│   └── programs/    # eBPF kernel-space programs
├── mvirt-zfs/       # ZFS storage daemon
│   └── proto/       # zfs.proto
├── mvirt-log/       # Centralized audit logging service
│   └── proto/       # log.proto
├── mvirt-one/       # MicroVM Init System (PID 1 for Pods)
│   ├── src/
│   ├── proto/       # one.proto
│   └── initramfs/   # rootfs skeleton
├── mvirt-cli/       # CLI client + TUI (ratatui)
├── mvirt-ui/        # Web UI (React + Vite + Tailwind)
├── nix/             # NixOS modules, packages, images
│   ├── modules/     # mvirt.nix (service definitions)
│   ├── packages/    # Build derivations
│   └── images/      # hypervisor.nix, node.nix
└── proto/           # Legacy shared proto (per-crate protos preferred)
```

## Code Quality

**ALWAYS run before committing:**
```bash
cargo fmt && cargo clippy
```

No warnings allowed.

## Architecture

### Distributed Control Plane (mvirt-api)

Raft-based consensus for multi-node cluster management.

- **Raft** via `mraft` for leader election and log replication
- **REST API** on port 8080 for external clients and UI
- **gRPC NodeService** on port 50056 for node agents
- **Raft** inter-node communication on port 6001
- **Event-sourced state machine** (`state.rs`) processes all commands
- **Scheduler** assigns resources to nodes

REST handlers are split per resource: `controlplane.rs`, `nodes.rs`, `vms.rs`, `networks.rs`, `nics.rs`.

### Node Agent (mvirt-node)

Runs on each hypervisor node, connects to mvirt-api.

- Registers with the API, sends heartbeats
- Watches spec stream (`WatchSpecs`) for desired state
- Reconciles locally via per-resource reconcilers (VM, NIC, volume, network, route, security group, template)
- Reports status back via `UpdateResourceStatus`

### Local Hypervisor (mvirt-vmm)

gRPC services: **VmService** + **PodService** on port 50051.

- Spawns cloud-hypervisor processes
- Creates TAP devices and attaches to bridge
- Generates cloud-init ISO
- Background watcher monitors child processes
- `recover_vms()` on daemon startup cleans up stale VMs
- Pod lifecycle via vsock communication with mvirt-one

### eBPF Networking (mvirt-ebpf)

Replaces legacy mvirt-net. In-kernel packet processing via eBPF.

- IPv4/IPv6 routing, DHCP, ICMP handling
- NAT and connection tracking
- TAP device management
- Security groups

### ZFS Storage (mvirt-zfs)

gRPC **ZfsService** for volume management.

- Volume CRUD and resize
- Snapshot management
- Template import with progress tracking (HTTP/file)
- Clone from templates

### Event Logging (mvirt-log)

Centralized audit log. SQLite with many-to-many object relations.

**Schema:**
```sql
logs (id INTEGER PRIMARY KEY, timestamp_ns, message, level, component)
log_objects (log_id, object_id)  -- junction table
```

**AuditLogger Usage** (recommended):
```rust
use mvirt_log::{AuditLogger, LogLevel, create_audit_logger};

let audit = create_audit_logger("http://[::1]:50052", "vmm");
audit.log(LogLevel::Audit, "VM created", vec![vm_id]).await;
```

### MicroVM Init (mvirt-one)

PID 1 init process running inside MicroVMs for container pods. Provides OneService gRPC for container lifecycle, log streaming, exec sessions.

### TUI (mvirt-cli)

ratatui-based terminal UI with non-blocking async and background gRPC workers.

### Web UI (mvirt-ui)

React + Vite + Tailwind dashboard. Communicates with mvirt-api REST API on port 8080.

## gRPC Services

| Proto | Service | Port | Key RPCs |
|-------|---------|------|----------|
| `mvirt-api/proto/node.proto` | NodeService | 50056 | Register, Heartbeat, Deregister, WatchSpecs, UpdateResourceStatus |
| `mvirt-vmm/proto/mvirt.proto` | VmService, PodService | 50051 | CreateVm, StartVm, StopVm, Console, PodLogs, PodExec |
| `mvirt-one/proto/one.proto` | OneService | (vsock) | CreatePod, StartPod, StopPod, Logs, Exec |
| `mvirt-zfs/proto/zfs.proto` | ZfsService | 50053 | CreateVolume, ImportTemplate, CreateSnapshot, CloneFromTemplate |
| `mvirt-log/proto/log.proto` | LogService | 50052 | Log, Query |

## Log-Level Guidelines

| Level | Usage | Examples |
|-------|-------|---------|
| **AUDIT** | State-changing operations (CRUD + Lifecycle) | VM/Volume/NIC created/deleted/started/stopped |
| **INFO** | Informational events without state change | Service started, connection established |
| **WARN** | Degraded operations, retries | Connection retry, fallback activated |
| **ERROR** | Failed operations | VM start failed, import failed |
| **DEBUG** | Developer diagnostics | Detailed trace info |

## Key Decisions

- **Raft consensus** for distributed cluster state (mraft)
- **Event sourcing** in mvirt-api state machine
- **Reconciliation loop** pattern (desired state vs actual state)
- **SQLite** for local persistence
- **cloud-hypervisor** instead of QEMU
- **eBPF** for in-kernel networking (replaces bridge-based mvirt-net)
- **musl** for static linking
- **Direct kernel boot** for MicroVMs
- **NixOS** for reproducible builds and deployment (flake.nix + crane)
- **Console escape**: Ctrl+a t

## Common Issues

### VM won't start
- Check kernel/disk paths exist
- Run daemon with `RUST_LOG=debug`

### VM state stuck
- Daemon recovery should fix on restart
- Check `ps aux | grep cloud-hypervisor`
