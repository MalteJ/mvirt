# mvirt Storage Model

This document describes the storage model used by mvirt-zfs for managing VM disk images.

## Concepts

### Template

A **Template** is an immutable base image used for creating VM volumes. Templates are typically imported from cloud images (e.g., Debian cloud images) and can be cloned multiple times to create independent volumes.

- Imported from raw or qcow2 images (local files or URLs)
- Immutable after creation
- Can be cloned to create multiple volumes
- Deletion only removes metadata; underlying ZFS data persists until all dependent volumes are deleted

### Volume

A **Volume** is a block device that can be attached to a VM. Volumes can be:

- **Empty**: Created directly with a specified size
- **Cloned**: Created from a template (copy-on-write, efficient)

Volumes are mutable and represent the actual disk state of a VM.

### Snapshot

A **Snapshot** is a point-in-time capture of a volume's state. Snapshots belong to their parent volume and are deleted when the volume is deleted.

- Used for backup/restore (rollback)
- Can be promoted to a new template
- Automatically deleted with parent volume

## Hierarchy

```
Template
    └── Volume (cloned)
            ├── Snapshot A
            └── Snapshot B
```

## Operations

### Import (creates Template)

```
TUI: Storage → [i] Import
CLI: mvirt-zfs import <name> <url>
```

Downloads/converts an image and creates a template from it.

### Create Empty Volume

```
TUI: Storage → [n] New
CLI: mvirt-zfs volume create <name> <size>
```

Creates a new empty volume (not from template).

### Clone Template → Volume

```
TUI: Templates → [c] Clone
CLI: mvirt-zfs clone <template-name> <volume-name>
```

Creates a new volume from a template using ZFS clone (copy-on-write).

### Create Snapshot

```
TUI: Volumes → [s] Snapshot
CLI: mvirt-zfs snapshot create <volume-name> <snapshot-name>
```

Creates a point-in-time snapshot of a volume.

### Rollback to Snapshot

```
TUI: Volume → Snapshots → [r] Rollback
CLI: mvirt-zfs snapshot rollback <volume-name> <snapshot-name>
```

Reverts a volume to a previous snapshot state.

### Promote Snapshot → Template

```
TUI: Volume → Snapshots → [t] Promote to Template
CLI: mvirt-zfs snapshot promote <volume-name> <snapshot-name> <template-name>
```

Creates a new template from an existing snapshot.

### Delete Volume

Deletes the volume and all its snapshots. If the volume was cloned from a template and no other volumes depend on that template's base data, the underlying ZFS resources are garbage collected.

### Delete Template

Only removes the template metadata. The underlying ZFS base ZVOL remains until all cloned volumes are deleted (garbage collection).

## ZFS Implementation

Under the hood, mvirt-zfs uses UUIDs for ZFS dataset names to allow renaming without ZFS operations.

```
vmpool/.base/<template-uuid>           # Base ZVOL (hidden)
vmpool/.base/<template-uuid>@img       # Template snapshot (cloneable)
vmpool/<volume-uuid>                   # Volume (may be clone or standalone)
vmpool/<volume-uuid>@<snapshot-uuid>   # Volume snapshot
```

### Garbage Collection

When a template is deleted, only the database entry is removed. The base ZVOL persists because dependent volumes (clones) still reference its snapshot.

When a volume is deleted:
1. The volume and its snapshots are destroyed (`zfs destroy -r`)
2. If the volume had an origin template:
   - Check if template entry still exists in DB → if yes, do nothing
   - Check if other volumes depend on same base → if yes, do nothing
   - If orphaned (no template entry, no other volumes): destroy base ZVOL

This ensures:
- Deleting a template doesn't break existing volumes
- Storage is reclaimed only when truly unused
- Copy-on-write benefits are preserved across all clones
