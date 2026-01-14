# ZFS Storage Backend

`mvirt` utilizes **ZFS** as the primary storage backend for virtual machines. ZFS provides native capabilities for thin provisioning, transparent compression, snapshots, and clones, which are essential for a modern virtualization environment.

Currently, storage management is performed manually via the ZFS CLI. In the future, the **`mvirt-zfs`** component will handle these tasks and expose them via a gRPC API.

## 1. Pool Design & Requirements

`mvirt` expects a ZFS pool (default name: `vmpool`) to be available. Since modern NVMe SSDs are used, specific pool parameters are critical for performance and longevity:

* **Alignment (`ashift=12`)**: Mandatory for the 4k sectors of modern SSDs/NVMes to prevent *Write Amplification*.
* **Autotrim (`autotrim=on`)**: Automatically returns freed blocks to the SSD controller (crucial for NVMe performance).

### Recommended Pool Creation
```bash
# Example: Stripe across two partitions (Performance/Capacity focus)
# Note: Ensure partitions are unmounted before creation.
zpool create -o ashift=12 vmpool /dev/nvme0n1p2 /dev/nvme1n1

# Apply global optimizations
zpool set autotrim=on vmpool
zfs set compression=lz4 vmpool
zfs set xattr=sa vmpool       # Improves performance for Linux attributes
zfs set atime=off vmpool      # Reduces write operations

```

## 2. VM Volumes (ZVOLs)

Virtual hard disks are not stored as files (qcow2/raw) within a file system, but as **ZVOLs** (ZFS Volumes). A ZVOL is an emulated block device that is passed directly to `cloud-hypervisor`.

### Advantages of ZVOLs

1. **No Filesystem Overhead**: Eliminates the "Journal in Journal" penalty (ext4 on top of ZFS).
2. **Performance**: Direct I/O path through the ARC (Adaptive Replacement Cache).
3. **Snapshot Granularity**: Snapshots apply exactly to one disk.

### Volume Configuration Standards

When creating a volume for a VM, the following standards apply:

* **Thin Provisioning (`-s`)**: Space is reserved only when data is written (Sparse Volume).
* **Blocksize (`volblocksize`)**:
* *Recommendation:* **16k** (Default on many distros). Offers a good balance for LZ4 compression.
* *Alternative:* **4k**. Matches the page size exactly, eliminating Read-Modify-Write entirely, but significantly reduces compression ratios.


* **Compression**: **LZ4** is active (inherited from the pool). Saves I/O bandwidth and disk space, especially for OS images.

## 3. Manual Management (Interim)

Until `mvirt-zfs` is fully implemented, volumes are managed as follows:

### Creating a Volume

Creates a 50GB disk for the VM `docker-01`:

```bash
# -s = sparse (thin provisioning)
# -V = Volume size
zfs create -s -V 50G vmpool/docker-01

```

*Hypervisor Path:* `/dev/zvol/vmpool/docker-01`

### Snapshots & Rollbacks

Ideal for backups before updates or for experimental states.

```bash
# Create a snapshot
zfs snapshot vmpool/docker-01@pre-update

# Revert state (VM must be stopped!)
zfs rollback vmpool/docker-01@pre-update

```

### Cloning (Base Images)

To quickly provision multiple VMs from a template (Gold Master):

1. Create a snapshot of the master: `zfs snapshot vmpool/debian-base@v1`
2. Create a clone (initially consumes 0 bytes):
```bash
zfs clone vmpool/debian-base@v1 vmpool/worker-01

```



### Resizing

Expanding a VM disk while the host system is running:

```bash
zfs set volsize=100G vmpool/docker-01

```

*Note:* The guest OS must subsequently resize its partitions and filesystem (e.g., using `growpart` and `resize2fs`) after the hypervisor has signaled the new geometry (or after a reboot).

## 4. Planned Architecture: mvirt-zfs

A standalone daemon (**`mvirt-zfs`**, written in Rust) will encapsulate ZFS interactions.

* **API**: gRPC Service.
* **Library**: Usage of `libzfs-sys` or `zfs-grpc` (preferred over shelling out to `zfs` binaries to avoid parsing errors).
* **Responsibilities**:
* `CreateVolume(name, size)`
* `DeleteVolume(name)`
* `Snapshot(vol_name, snap_name)`
* `Clone(source_snap, new_vol_name)`
* `GetStats()` (Used space, Available space, Compression Ratio)

