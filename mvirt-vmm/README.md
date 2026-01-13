# mvirt-vmm

Daemon zur Verwaltung von VMs via cloud-hypervisor.

## Features

- gRPC API auf Port 50051
- SQLite für VM-Definitionen und Runtime-State
- Automatische TAP-Device und Bridge-Verwaltung
- Cloud-init Support (user-data, meta-data, network-config)
- Graceful Shutdown mit Timeout
- Crash Recovery (findet laufende VMs nach Daemon-Restart)

## Verwendung

```bash
# Mit Default-Einstellungen (/var/lib/mvirt)
mvirt-vmm

# Development (lokales Verzeichnis)
mvirt-vmm --data-dir ./tmp

# Eigene Bridge
mvirt-vmm --bridge br0
```

## Optionen

| Option | Default | Beschreibung |
|--------|---------|--------------|
| `--data-dir` | `/var/lib/mvirt` | Verzeichnis für DB und Sockets |
| `--bridge` | `mvirt0` | Linux Bridge für VM-Netzwerk |
| `--listen` | `[::1]:50051` | gRPC Listen-Adresse |

## Datenverzeichnis

```
<data-dir>/
├── mvirt.db                # SQLite Datenbank
└── vm/
    └── <vm-id>/
        ├── api.sock        # cloud-hypervisor API Socket
        ├── serial.sock     # Serial Console Socket
        ├── cloudinit.iso   # Generated cloud-init ISO
        └── *.log           # Prozess-Logs
```

## gRPC API

Definiert in `proto/mvirt.proto`:

### System
- `GetSystemInfo` - CPU/RAM total und allocated

### CRUD
- `CreateVm` - VM erstellen
- `GetVm` - VM-Details abrufen
- `ListVms` - Alle VMs auflisten
- `DeleteVm` - VM löschen (muss gestoppt sein)

### Lifecycle
- `StartVm` - VM starten
- `StopVm` - Graceful Shutdown (mit Timeout)
- `KillVm` - Force Kill (SIGKILL)

### Console
- `Console` - Bidirektionaler Serial Console Stream

## Architektur

```
┌─────────────────────────────────────────────────────┐
│                     grpc.rs                         │
│                  (gRPC Handler)                     │
└──────────┬─────────────────────────┬────────────────┘
           │                         │
┌──────────▼──────────┐   ┌──────────▼──────────┐
│      store.rs       │   │    hypervisor.rs    │
│  (SQLite: VMs,      │   │  (cloud-hypervisor  │
│   Runtime-Info)     │   │   Prozesse, TAPs)   │
└─────────────────────┘   └──────────┬──────────┘
                                     │
                          ┌──────────▼──────────┐
                          │   Background Task   │
                          │  (Process Watcher)  │
                          └─────────────────────┘
```

## SQLite Schema

```sql
-- VM-Definitionen
CREATE TABLE vms (
    id TEXT PRIMARY KEY,
    name TEXT,
    state TEXT NOT NULL DEFAULT 'stopped',
    config_json TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    started_at INTEGER
);

-- Runtime-Info (PID, Sockets)
CREATE TABLE vm_runtime (
    vm_id TEXT PRIMARY KEY REFERENCES vms(id),
    pid INTEGER NOT NULL,
    api_socket TEXT NOT NULL,
    serial_socket TEXT NOT NULL
);
```

## cloud-hypervisor Kommando

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
