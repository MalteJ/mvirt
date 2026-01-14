# Logging

mvirt uses centralized audit logging through mvirt-log.

## Overview

All state-changing operations are logged to mvirt-log, providing:
- **Audit Trail**: Who did what, when
- **Correlation**: Logs indexed by multiple objects (VM, volume, NIC)
- **Queryability**: Find all events related to a specific resource

## Log Levels

| Level   | Usage                                    | Examples                              |
|---------|------------------------------------------|---------------------------------------|
| AUDIT   | All state-changing operations            | VM created, volume deleted, NIC attached |
| INFO    | Informative events (no state change)     | Service started, connection established |
| WARN    | Degraded operations, retries             | Connection retry, fallback activated  |
| ERROR   | Failed operations                        | VM start failed, import failed        |
| DEBUG   | Developer diagnostics                    | Detailed trace info                   |

## Multi-Object Correlation

A single log entry can be indexed under multiple objects:

```
Event: "Volume attached to VM"
Indexed under:
  - vm-123 (the VM)
  - vol-456 (the volume)
```

Query by either ID to find the event:
```bash
# Find all events for a VM
mvirt logs --object vm-123

# Find all events for a volume
mvirt logs --object vol-456
```

## Architecture

```
┌───────────────┐  ┌───────────────┐  ┌───────────────┐
│   mvirt-vmm   │  │   mvirt-zfs   │  │   mvirt-net   │
└───────┬───────┘  └───────┬───────┘  └───────┬───────┘
        │                  │                  │
        │     gRPC Log()   │                  │
        └──────────────────┼──────────────────┘
                           │
                           ▼
                    ┌───────────────┐
                    │   mvirt-log   │
                    │   :50052      │
                    │               │
                    │  fjall DB     │
                    │  (LSM-Tree)   │
                    └───────────────┘
```

## Using AuditLogger

Components use the `AuditLogger` wrapper for dual logging:
- Local output via `tracing` (for debugging)
- Remote storage via `mvirt-log` (for audit trail)

```rust
use mvirt_log::{create_audit_logger, LogLevel};

// Create logger for a component
let audit = create_audit_logger("http://[::1]:50052", "vmm");

// Log an event with related objects
audit.log(
    LogLevel::Audit,
    "VM created",
    vec!["vm-123".into()]
).await;

// Log event related to multiple objects
audit.log(
    LogLevel::Audit,
    "Volume attached",
    vec!["vm-123".into(), "vol-456".into()]
).await;
```

## Querying Logs

```rust
use mvirt_log::{LogServiceClient, QueryRequest};

let mut client = LogServiceClient::connect("http://[::1]:50052").await?;

// Query logs for a specific object
let mut stream = client.query(QueryRequest {
    object_id: Some("vm-123".into()),
    limit: 100,
    ..Default::default()
}).await?.into_inner();

while let Some(entry) = stream.message().await? {
    println!("{}: {} - {}",
        entry.timestamp_ns,
        entry.level,
        entry.message
    );
}
```

## Storage

Logs are stored in a fjall LSM-Tree database with two partitions:

- **logs_data**: Full log entries, ordered by timestamp
- **index_objects**: Inverted index for object ID lookups

This design optimizes for:
- High-throughput writes (append-only)
- Efficient time-range queries
- Fast object-based lookups

## See Also

- [mvirt-log README](../mvirt-log/README.md) - Service documentation
- [Architecture](../architecture.md) - System overview
- [CLAUDE.md](../CLAUDE.md) - Log level guidelines
