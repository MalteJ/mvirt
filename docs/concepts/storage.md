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

- **Delete a volume** → Template and other volumes are unaffected
- **Delete a snapshot** → Other snapshots and the volume are unaffected
- **Promote a snapshot to template** → Creates an independent copy (original snapshot remains)

Templates are stored as independent datasets. When you promote a snapshot to a template, the data is copied (not cloned), so templates have no dependencies on the source volume.

### No artificial constraints

- Volumes can be deleted without affecting templates
- Snapshots can be deleted independently

Note: Volume names must currently be unique (this may change in future versions).

### Efficient by default

Most operations use ZFS copy-on-write:

- Cloning a template to create a volume: instant, ~0 bytes
- Creating a snapshot: instant, ~0 bytes
- Promoting a snapshot to template: copies data (depends on snapshot size)

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

**Rollback behavior**: Rolling back to a snapshot destroys all snapshots newer than the target. For example, if you have snap-1, snap-2, snap-3 and rollback to snap-1, both snap-2 and snap-3 will be deleted.

## Architecture

Templates and volumes are stored in separate ZFS datasets:

```
mvirt/templates/<uuid>    # Template (independent dataset)
    └── @img              # Snapshot for cloning volumes
mvirt/volumes/<uuid>      # Volume (clone of template@img or standalone)
    └── @<snap-uuid>      # User-created snapshots
```

When you promote a snapshot to a template:
- The snapshot data is **copied** to a new independent template dataset using `zfs send | zfs receive`
- This creates a fully independent template with no dependencies on the source volume
- The original snapshot remains usable

This design ensures:
- Deleting a volume never affects templates
- Templates can always be deleted (with auto-promote for dependent volumes)

## Operations

| Action | TUI | CLI |
|--------|-----|-----|
| Import template | Storage → `i` | `mvirt import <name> <url>` |
| List templates | Storage tab | `mvirt template list` |
| Clone template | Templates → `c` | `mvirt template clone <template> <volume>` |
| Delete template | Templates → `d` | `mvirt template delete <name>` |
| List volumes | Storage tab | `mvirt volume list` |
| Create empty volume | - | `mvirt volume create <name> --size <GB>` |
| Resize volume | - | `mvirt volume resize <name> --size <GB>` |
| Delete volume | Volumes → `d` | `mvirt volume delete <name>` |
| List snapshots | Snapshots tab | `mvirt snapshot list <volume>` |
| Create snapshot | Volumes → `s` | `mvirt snapshot create <volume> <name>` |
| Rollback snapshot | Snapshot → `r` | `mvirt snapshot rollback <volume> <name>` |
| Delete snapshot | Snapshot → `d` | `mvirt snapshot delete <volume> <name>` |
| Promote to template | Snapshot → `t` | `mvirt snapshot promote <volume> <snap> <template>` |

## ZFS Layout

```
mvirt/
├── templates/
│   └── <template-uuid>                  # Template (independent dataset)
│       └── @img                         # Snapshot for cloning volumes
├── volumes/
│   └── <volume-uuid>                    # Volume (clone of template or standalone)
│       └── @<snapshot-uuid>             # User-created snapshots
└── .tmp/                                # Temporary files during import
```

Dataset names use UUIDs so entities can be renamed without ZFS operations.

## Deletion Behavior

**Deleting a volume**: Simply destroys the volume and its snapshots. Templates are not affected (they are independent copies).

**Deleting a template**: If volumes exist that were cloned from this template, one is promoted to become the new origin, then the template is deleted. This preserves the copy-on-write relationship between existing volumes.

## CLI Examples

### Import a Debian cloud image as template
```bash
mvirt import debian13 https://cloud.debian.org/images/cloud/trixie/latest/debian-13-generic-amd64.qcow2
```

### Create a VM disk from template
```bash
mvirt template clone debian13 my-vm-root
# Optionally resize:
mvirt volume resize my-vm-root --size 20
```

### Create a backup before updates
```bash
mvirt snapshot create my-vm-root before-update
# ... do updates ...
# If something goes wrong:
mvirt snapshot rollback my-vm-root before-update
```

### Create a golden image from a configured VM
```bash
# After configuring a VM, promote its snapshot to a new template
mvirt snapshot create my-vm-root configured
mvirt snapshot promote my-vm-root configured my-golden-image
# Now you can create new VMs from this template
mvirt template clone my-golden-image another-vm
```

### Create an empty data volume
```bash
mvirt volume create my-data --size 100
```

### List all storage
```bash
mvirt template list
mvirt volume list
mvirt snapshot list my-vm-root
```
