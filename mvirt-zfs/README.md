# mvirt-zfs

ZFS volume manager daemon for mvirt. Manages ZVOLs (ZFS Volumes) as block devices for virtual machines.

## Features

- **Volume Management**: Create, list, resize, and delete thin-provisioned ZVOLs
- **Image Import**: Import raw and qcow2 disk images from local files or HTTP(S) URLs
- **Templates**: Create snapshots as templates for rapid VM provisioning
- **Cloning**: Instantly clone VMs from templates (copy-on-write)
- **Snapshots**: Create and rollback volume snapshots
- **Statistics**: Pool utilization, provisioned vs. actual usage, compression ratios

## Architecture

mvirt-zfs is a standalone gRPC daemon that manages ZFS storage independently from mvirt-vmm:

```
┌─────────────┐     ┌─────────────┐     ┌─────────────┐
│  mvirt-cli  │────▶│  mvirt-zfs  │     │  mvirt-vmm  │
│   (TUI)     │     │   :50053    │     │   :50051    │
└─────────────┘     └─────────────┘     └─────────────┘
      │                   │                   ▲
      │  1. CreateVolume  │                   │
      │──────────────────▶│                   │
      │                   │                   │
      │  Volume { path: "/dev/zvol/..." }     │
      │◀──────────────────│                   │
      │                                       │
      │  2. CreateVm { disks: [{ path }] }    │
      └───────────────────────────────────────┘
```

This loose coupling means:
- mvirt-vmm accepts any block device path (doesn't know about ZFS)
- mvirt-zfs manages storage (doesn't know about VMs)
- mvirt-cli orchestrates the workflow

## Requirements

- ZFS pool (default: `mvirt`)
- libzfs development headers (`libzfs-dev` on Debian/Ubuntu)

## Usage

```bash
# Start the daemon
mvirt-zfs --pool mvirt --listen [::1]:50053

# With custom pool
mvirt-zfs --pool mypool --listen 0.0.0.0:50053
```

## gRPC API

The daemon exposes `ZfsService` on port 50052:

### Pool Operations
- `GetPoolStats` - Get pool size, usage, and compression stats

### Volume Operations
- `CreateVolume` - Create a new thin-provisioned ZVOL
- `ListVolumes` - List all managed volumes
- `GetVolume` - Get volume details
- `DeleteVolume` - Delete a volume
- `ResizeVolume` - Expand a volume

### Import Operations
- `ImportVolume` - Start async import from file/URL (returns job ID)
- `GetImportJob` - Check import progress
- `ListImportJobs` - List all import jobs
- `CancelImportJob` - Cancel a running import

### Snapshot Operations
- `CreateSnapshot` - Create a snapshot
- `ListSnapshots` - List snapshots for a volume
- `DeleteSnapshot` - Delete a snapshot
- `RollbackSnapshot` - Rollback to a snapshot

### Template Operations
- `CreateTemplate` - Create a template from a volume snapshot
- `ListTemplates` - List available templates
- `DeleteTemplate` - Delete a template
- `CloneFromTemplate` - Clone a new volume from a template

## Storage Layout

```
mvirt/
├── .tmp/                 # Temporary files for imports
├── <uuid>                # Template ZVOL
├── <uuid>                # Volume ZVOL (clone of template@img)
└── <uuid>                # Volume ZVOL (clone of template@img)
```

Snapshots: `mvirt/<uuid>@img`, `mvirt/<uuid>@<snap-uuid>`

## Import Workflow

### Local Raw File
```
Source: /tmp/disk.raw → Stream → /dev/zvol/mvirt/<uuid>
```

### Local qcow2 File
```
Source: /tmp/disk.qcow2 → Read (random access) → Convert → /dev/zvol/mvirt/<uuid>
```

### HTTP(S) Raw
```
Source: https://example.com/disk.raw → Stream → /dev/zvol/mvirt/<uuid>
```

### HTTP(S) qcow2
```
Source: https://example.com/disk.qcow2 → Download → /mvirt/.tmp/import-{uuid}.qcow2
                                       → Convert → /dev/zvol/mvirt/<uuid>
                                       → Cleanup temp file
```

## Building

```bash
# From workspace root
cargo build -p mvirt-zfs

# Release build
cargo build -p mvirt-zfs --release
```

## Development Status

Work in progress. See the implementation plan for details.
