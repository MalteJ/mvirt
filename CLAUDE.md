# mvirt Development Guidelines

See [README.md](README.md) for build commands and project overview.

## Project Structure

```
mvirt/
├── mvirt-cli/       # CLI client + TUI
├── mvirt-vmm/       # Daemon (VM Manager)
├── mvirt-log/       # Centralized logging service
├── mvirt-zfs/       # ZFS storage management
├── mvirt-uos/       # µOS - Minimal Linux for MicroVMs
│   ├── pideisn/     # Rust init process (PID 1)
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

### Event Logging (mvirt-log)

Centralized audit log for all mvirt components. Logs are stored in SQLite with many-to-many object relations.

**Schema:**
```sql
logs (id INTEGER PRIMARY KEY, timestamp_ns, message, level, component)
log_objects (log_id, object_id)  -- junction table for many-to-many
```

**Proto API** (`mvirt-log/proto/log.proto`):
```protobuf
service LogService {
  rpc Log(LogRequest) returns (LogResponse);      // append log
  rpc Query(QueryRequest) returns (stream LogEntry);  // query logs
}

message LogEntry {
  string id = 1;
  int64 timestamp_ns = 2;
  string message = 3;
  LogLevel level = 4;           // INFO, WARN, ERROR, DEBUG, AUDIT
  string component = 5;         // "vmm", "zfs", "cli"
  repeated string related_object_ids = 6;  // ["vm-123", "vol-456"]
}
```

**Dependency** (in component's Cargo.toml):
```toml
mvirt-log = { path = "../mvirt-log" }
```

**Logging from components:**
```rust
use mvirt_log::{LogServiceClient, LogEntry, LogLevel, LogRequest};

// Connect to mvirt-log
let mut client = LogServiceClient::connect("http://[::1]:50052").await?;

// Log an event with related objects
client.log(LogRequest {
    entry: Some(LogEntry {
        message: "VM started".into(),
        level: LogLevel::Info as i32,
        component: "vmm".into(),
        related_object_ids: vec![vm_id.clone()],
        ..Default::default()  // id and timestamp auto-generated
    }),
}).await?;

// Log event related to multiple objects (e.g., disk attached to VM)
client.log(LogRequest {
    entry: Some(LogEntry {
        message: "Volume attached".into(),
        level: LogLevel::Audit as i32,
        component: "zfs".into(),
        related_object_ids: vec![vm_id, volume_id],  // indexed under both
        ..Default::default()
    }),
}).await?;
```

**Querying logs:**
```rust
use mvirt_log::{LogServiceClient, QueryRequest};

let mut stream = client.query(QueryRequest {
    object_id: Some("vm-123".into()),
    limit: 100,
    ..Default::default()
}).await?.into_inner();

while let Some(entry) = stream.message().await? {
    println!("{}: {}", entry.timestamp_ns, entry.message);
}
```

### Log-Level Guidelines

| Level | Verwendung | Beispiele |
|-------|------------|-----------|
| **AUDIT** | Alle State-ändernden Operationen (CRUD + Lifecycle) | VM/Volume/NIC created/deleted/started/stopped/killed |
| **INFO** | Informative Events ohne State-Änderung | Service gestartet, Connection hergestellt |
| **WARN** | Degraded Operations, Retries | Connection retry, Fallback aktiviert |
| **ERROR** | Fehlgeschlagene Operationen | VM start failed, Import failed |
| **DEBUG** | Entwickler-Diagnostik | Detaillierte Trace-Infos |

**AuditLogger Usage** (empfohlen statt direktem Client):
```rust
use mvirt_log::{AuditLogger, LogLevel, create_audit_logger};

// In main.rs: Create shared audit logger
let audit = create_audit_logger("http://[::1]:50052", "vmm");

// Automatisch: Dual-Logging (lokal via tracing + remote via mvirt-log)
audit.log(LogLevel::Audit, "VM created", vec![vm_id]).await;
```

**Component-specific wrappers** (in `mvirt-zfs`, `mvirt-net`):
```rust
// ZfsAuditLogger wraps AuditLogger with domain-specific methods
use crate::audit::{create_audit_logger, ZfsAuditLogger};

let audit = create_audit_logger(&args.log_endpoint);
audit.volume_created(&volume_id, &volume_name, size).await;
```

## Key Decisions

- **SQLite** for persistence (not in-memory)
- **cloud-hypervisor** instead of QEMU
- **TAP + bridge** networking
- **Async channels** in TUI
- **Console escape**: Ctrl+a t
- **musl** for static linking
- **Direct kernel boot** for MicroVMs

## Common Issues

### VM won't start
- Check kernel/disk paths exist
- Run daemon with `RUST_LOG=debug`

### VM state stuck
- Daemon recovery should fix on restart
- Check `ps aux | grep cloud-hypervisor`
