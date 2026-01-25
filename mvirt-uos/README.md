# mvirt-uos

µOS (Micro OS) - A minimal Linux guest OS for MicroVMs.

Builds a UKI (Unified Kernel Image) containing kernel + initramfs + cmdline. Just ~3MB.

## Features

- Minimal Linux kernel optimized for virtio
- pideisn - Rust init (PID 1) with built-in DHCP
- UKI for simple deployment (one file)
- No unnecessary binaries - just init

## Usage

```bash
# Build UKI
make uos

# Individual targets
make kernel
make initramfs

# Configure kernel
make menuconfig
```

## Structure

```
mvirt-uos/
├── kernel.version          # Kernel version
├── kernel.config           # Kconfig fragment (virtio-only)
├── cmdline.txt             # Kernel cmdline (embedded in UKI)
├── mvirt-uos.mk            # Makefile
├── initramfs/
│   └── rootfs/             # initramfs skeleton
│       ├── etc/            # Basic config files
│       └── init            # pideisn binary (built)
├── pideisn/                # Rust init source
│   ├── Cargo.toml
│   └── src/
└── target/                 # Build output
    ├── mvirt-uos.efi       # UKI (~3MB)
    └── initramfs.cpio.gz   # Intermediate
```

## What's in the UKI

| Component | Size | Description |
|-----------|------|-------------|
| Kernel | ~2MB | Minimal Linux with virtio |
| Initramfs | ~700KB | pideisn + /etc |
| Cmdline | few bytes | `console=ttyS0` |

## pideisn

Minimal init process in Rust:

1. Mounts `/proc`, `/sys`, `/dev`
2. Sets hostname
3. Configures network (DHCP)
4. Runs workload (TBD)
5. Handles shutdown

## Testing

```bash
# With cloud-hypervisor
cloud-hypervisor \
    --kernel mvirt-uos/target/mvirt-uos.efi \
    --cpus boot=1 --memory size=128M \
    --serial tty --console off
```

## Cleanup

```bash
make clean       # Delete build artifacts
make distclean   # + kernel source
```

## Dependencies

```bash
# Kernel build
sudo apt install build-essential flex bison libncurses-dev \
    libssl-dev libelf-dev bc dwarves

# UKI
sudo apt install systemd-ukify systemd-boot-efi
```
