# mvirt-vmm

Daemon for managing VMs via cloud-hypervisor.

## Features

- gRPC API on port 50051
- SQLite for VM definitions and runtime state
- Automatic TAP device and bridge management
- Cloud-init support (user-data, meta-data, network-config)
- Graceful shutdown with timeout
- Crash recovery (finds running VMs after daemon restart)

## Usage

```bash
# With default settings (/var/lib/mvirt)
mvirt-vmm

# Development (local directory)
mvirt-vmm --data-dir ./tmp

# Custom bridge
mvirt-vmm --bridge br0
```

## Options

| Option | Default | Description |
|--------|---------|-------------|
| `--data-dir` | `/var/lib/mvirt` | Directory for DB and sockets |
| `--bridge` | `mvirt0` | Linux bridge for VM network |
| `--listen` | `[::1]:50051` | gRPC listen address |

## Data Directory

```
<data-dir>/
├── mvirt.db                # SQLite database
└── vm/
    └── <vm-id>/
        ├── api.sock        # cloud-hypervisor API socket
        ├── serial.sock     # Serial console socket
        ├── cloudinit.iso   # Generated cloud-init ISO
        └── *.log           # Process logs
```

## gRPC API

Defined in `proto/mvirt.proto`:

### System
- `GetSystemInfo` - CPU/RAM total and allocated

### CRUD
- `CreateVm` - Create VM
- `GetVm` - Get VM details
- `ListVms` - List all VMs
- `DeleteVm` - Delete VM (must be stopped)

### Lifecycle
- `StartVm` - Start VM
- `StopVm` - Graceful shutdown (with timeout)
- `KillVm` - Force kill (SIGKILL)

### Console
- `Console` - Bidirectional serial console stream

## Architecture

```
┌─────────────────────────────────────────────────────┐
│                     grpc.rs                         │
│                  (gRPC Handlers)                    │
└──────────┬─────────────────────────┬────────────────┘
           │                         │
┌──────────▼──────────┐   ┌──────────▼──────────┐
│      store.rs       │   │    hypervisor.rs    │
│  (SQLite: VMs,      │   │  (cloud-hypervisor  │
│   Runtime Info)     │   │   processes, TAPs)  │
└─────────────────────┘   └──────────┬──────────┘
                                     │
                          ┌──────────▼──────────┐
                          │   Background Task   │
                          │  (Process Watcher)  │
                          └─────────────────────┘
```

## SQLite Schema

```sql
-- VM definitions
CREATE TABLE vms (
    id TEXT PRIMARY KEY,
    name TEXT,
    state TEXT NOT NULL DEFAULT 'stopped',
    config_json TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    started_at INTEGER
);

-- Runtime info (PID, sockets)
CREATE TABLE vm_runtime (
    vm_id TEXT PRIMARY KEY REFERENCES vms(id),
    pid INTEGER NOT NULL,
    api_socket TEXT NOT NULL,
    serial_socket TEXT NOT NULL
);
```

## cloud-hypervisor Command

```bash
cloud-hypervisor \
    --api-socket path=<data-dir>/vm/<id>/api.sock \
    --serial socket=<data-dir>/vm/<id>/serial.sock \
    --console off \
    --kernel <kernel_path> \
    --cpus boot=<vcpus> \
    --memory size=<memory_mb>M \
    --disk path=<disk_path> [path=<cloudinit.iso>,readonly=on] \
    --net tap=<tap_name>
```
