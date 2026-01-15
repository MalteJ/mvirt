# Data Directories

Each mvirt service stores its state in a dedicated directory.

## Service Data Locations

| Service   | Default Location          | Contents                         |
|-----------|---------------------------|----------------------------------|
| mvirt-vmm | `/var/lib/mvirt/vmm`      | SQLite DB, API sockets           |
| mvirt-log | `/var/lib/mvirt/log`      | fjall LSM-Tree (logs + indexes)  |
| mvirt-zfs | ZFS pool metadata         | `.mvirt-zfs/metadata.db`         |
| mvirt-net | `/var/lib/mvirt/net`      | Metadata                         |

## Runtime Directories

| Service   | Location             | Contents                    |
|-----------|----------------------|-----------------------------|
| mvirt-vmm | `/var/lib/mvirt/vmm` | VM sockets (per-VM subdirs) |
| mvirt-net | `/run/mvirt/net`     | vhost-user sockets          |

## mvirt-vmm

```
/var/lib/mvirt/vmm/
├── mvirt.db                # SQLite database (VMs, runtime)
└── vm/
    └── <vm-id>/
        ├── api.sock        # cloud-hypervisor API socket
        ├── serial.sock     # Serial console socket
        └── cloudinit.iso   # Cloud-init ISO
```

**Customize:** `mvirt-vmm --data-dir /custom/path`

## mvirt-log

```
/var/lib/mvirt/log/
└── fjall/                  # LSM-Tree storage
    ├── logs_data/          # Log entries (timestamp + ID → protobuf)
    └── index_objects/      # Inverted index (object ID → log IDs)
```

**Customize:** `mvirt-log --data-dir /custom/path`

## mvirt-zfs

Data is stored within the ZFS pool itself:

```
<pool>/
├── .mvirt-zfs/
│   └── metadata.db         # SQLite metadata (templates, import jobs)
├── .tmp/                   # Temporary files during imports
├── <volume-name>           # ZVOL block devices
└── <volume-name>@<snap>    # Snapshots
```

**Customize:** `mvirt-zfs --pool <pool-name>`

## mvirt-net

```
/var/lib/mvirt/net/
└── metadata.db             # NIC definitions, network config

/run/mvirt/net/
└── <nic-id>.sock           # vhost-user sockets for cloud-hypervisor
```

**Customize:**
- `mvirt-net --metadata-dir /custom/path`
- `mvirt-net --socket-dir /custom/run/path`

## Permissions

| Service   | User          | Notes                              |
|-----------|---------------|-------------------------------------|
| mvirt-vmm | Non-root OK   | Needs access to /dev/kvm            |
| mvirt-log | Non-root OK   | -                                   |
| mvirt-zfs | Root          | ZFS operations require root         |
| mvirt-net | Root          | TAP device creation requires root   |

## Backup Considerations

- **mvirt-vmm**: Back up SQLite DB
- **mvirt-log**: Back up fjall directory (stop service first for consistency)
- **mvirt-zfs**: Use ZFS snapshots for volumes; metadata DB is regenerable
- **mvirt-net**: NIC config is ephemeral, recreated on startup
