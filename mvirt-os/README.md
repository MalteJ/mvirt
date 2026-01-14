# mvirt-os

Build system for Linux kernel, initramfs, and UKI (Unified Kernel Image).

## Features

- Minimal Linux kernel for cloud-hypervisor
- initramfs with statically linked binaries
- UKI for direct EFI booting
- pideisn - Rust init (PID 1)

## Usage

All commands are run from the repository root:

```bash
# Build everything (kernel + initramfs + UKI)
make os

# Individual targets
make kernel
make initramfs
make uki

# Bootable ISO
make iso

# Configure kernel
make menuconfig
```

## Structure

```
mvirt-os/
├── kernel.version          # Kernel version (e.g. "6.18.4")
├── kernel.config           # Kconfig fragment
├── cmdline.txt             # Kernel cmdline
├── mvirt-os.mk             # Main Makefile
├── linux.mk                # Kernel build
├── initramfs/
│   └── rootfs/             # initramfs skeleton
│       ├── dev/
│       ├── proc/
│       ├── sys/
│       ├── usr/bin/
│       └── usr/sbin/
├── pideisn/                # Rust init
│   ├── Cargo.toml
│   └── src/main.rs
└── target/                 # Build output
    ├── cloud-hypervisor    # Downloaded binary
    ├── initramfs.cpio.gz
    └── mvirt.efi
```

## Configuration

### kernel.version

```
6.18.4
```

### kernel.config

Kconfig fragment applied on top of `tinyconfig`:

```
CONFIG_64BIT=y
CONFIG_VIRTIO_PCI=y
CONFIG_VIRTIO_BLK=y
CONFIG_VIRTIO_NET=y
CONFIG_EXT4_FS=y
...
```

### cmdline.txt

```
console=ttyS0 quiet
```

## initramfs Contents

| Path | Source |
|------|--------|
| `/init` | `pideisn` (Rust init) |
| `/usr/sbin/mvirt` | CLI |
| `/usr/sbin/mvirt-vmm` | Daemon |
| `/usr/bin/cloud-hypervisor` | cloud-hypervisor release |

## pideisn

Minimal init process in Rust:

1. Mounts `/proc`, `/sys`, `/dev`
2. Sets hostname
3. Starts `mvirt-vmm`
4. Waits for shutdown signal
5. Reboots

## Output

| File | Description |
|------|-------------|
| `kernel/arch/x86/boot/bzImage` | Linux kernel |
| `target/initramfs.cpio.gz` | Compressed initramfs |
| `target/mvirt.efi` | Bootable UKI |
| `target/cloud-hypervisor` | cloud-hypervisor binary |

## Cleanup

```bash
make clean       # Delete build artifacts
make distclean   # + kernel source + downloads
```

## Dependencies

```bash
# Kernel build
sudo apt install build-essential flex bison libncurses-dev \
    libssl-dev libelf-dev bc dwarves

# UKI
sudo apt install systemd-ukify systemd-boot-efi

# cloud-init ISO
sudo apt install genisoimage
```
