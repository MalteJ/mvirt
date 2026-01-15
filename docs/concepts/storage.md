# mvirt Storage Model

This document describes the storage model used by mvirt-zfs for managing VM disk images.

## UX Philosophy

### Simplicity over ZFS complexity

Users should not need to understand ZFS internals. The UI exposes three simple concepts:

- **Templates**: Base images you import once and clone many times
- **Volumes**: Actual disks attached to VMs
- **Snapshots**: Point-in-time backups of volumes

### Independence of entities

Each entity can be managed independently:

- **Delete a template** → Volumes cloned from it continue to work
- **Delete a snapshot** → Other snapshots and the volume are unaffected
- **Promote a snapshot to template** → Original snapshot remains usable

This is achieved through reference counting: the underlying ZFS resources are only cleaned up when nothing references them anymore.

### No artificial constraints

- Volume names don't need to be unique (multiple VMs can have a "root" volume)
- Templates can be deleted even when volumes depend on them
- Snapshots can be deleted even when templates reference them

### Efficient by default

All operations use ZFS copy-on-write:

- Cloning a 3GB template to create a volume: instant, ~0 bytes
- Creating a snapshot: instant, ~0 bytes
- Promoting a snapshot to template: instant, just a database entry

## Concepts

### Template

A **Template** is a base image used for creating VM volumes. Templates are typically imported from cloud images (e.g., Debian cloud images).

- Imported from raw or qcow2 images (local files or URLs)
- Can be cloned to create volumes (copy-on-write)
- Can be deleted anytime; underlying ZFS data persists until all clones are gone

### Volume

A **Volume** is a block device attached to a VM. Volumes can be:

- **Cloned**: Created from a template (efficient, copy-on-write)
- **Empty**: Created directly with a specified size

### Snapshot

A **Snapshot** is a point-in-time capture of a volume's state.

- Used for backup/restore (rollback)
- Can be promoted to a new template (no data copy, just a reference)
- Deleted with parent volume

## Reference Counting

Under the hood, a single ZFS snapshot can be referenced by multiple mvirt entities:

```
ZFS Snapshot (vmpool/vol-123@snap-456)
    ├── mvirt Snapshot "before-update"
    └── mvirt Template "my-golden-image" (promoted from snapshot)
```

When you promote a snapshot to a template:
- No ZFS operation happens
- A new template entry is created pointing to the same ZFS snapshot
- Both the original snapshot and the new template remain usable

When you delete a snapshot or template:
- Only the database entry is removed
- The ZFS snapshot is deleted only when ref count reaches 0

## Operations

| Action | TUI | CLI |
|--------|-----|-----|
| Import template | Storage → `i` | `mvirt import <name> <url>` |
| Clone template | Templates → `c` | - |
| Create snapshot | Volumes → `s` | - |
| Rollback snapshot | Snapshot → `r` | - |
| Promote to template | Snapshot → `t` | - |
| Delete | Any → `d` | - |

## ZFS Layout

```
vmpool/
├── .templates/                          # Imported base images
│   └── <template-uuid>                  # Base ZVOL
│       └── @img                         # Snapshot for cloning
├── .tmp/                                # Temporary files during import
└── <volume-uuid>                        # Volume (clone or standalone)
    └── @<snapshot-uuid>                 # Volume snapshots
```

Dataset names use UUIDs so entities can be renamed without ZFS operations.

## Garbage Collection

ZFS resources are cleaned up lazily:

1. **Template deleted**: Only DB entry removed. Base ZVOL persists (clones depend on it).
2. **Volume deleted**: Volume and snapshots destroyed. If no other volumes share the origin template's base ZVOL, it's garbage collected.
3. **Snapshot deleted**: DB entry removed. ZFS snapshot deleted only if ref count = 0.

This ensures:
- Deleting entities never breaks other entities
- Storage is reclaimed when truly unused
- Copy-on-write benefits preserved across all clones
