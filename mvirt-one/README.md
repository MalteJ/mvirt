# mvirt-one

MicroVM Init System for isolated Pods - A minimal Linux guest OS for MicroVMs.

Builds a UKI (Unified Kernel Image) containing kernel + initramfs + cmdline. Just ~3MB.

## Features

- Minimal Linux kernel optimized for virtio
- Rust init (PID 1) with built-in DHCP and gRPC API
- OCI container runtime (via youki)
- UKI for simple deployment (one file)

## Usage

```bash
# Build UKI
make one

# Individual targets
make kernel
make initramfs

# Configure kernel
make menuconfig
```

## Structure

```
mvirt-one/
├── kernel.version          # Kernel version
├── kernel.config           # Kconfig fragment (virtio-only)
├── cmdline.txt             # Kernel cmdline (embedded in UKI)
├── mvirt-one.mk            # Makefile
├── src/                    # Rust init source
│   ├── main.rs
│   └── services/           # Pod, Image, Task services
├── proto/                  # gRPC API definition
├── initramfs/
│   └── rootfs/             # initramfs skeleton
│       ├── etc/            # Basic config files
│       └── init            # init binary (built)
└── target/                 # Build output
    ├── mvirt-one.efi       # UKI (~3MB)
    └── initramfs.cpio.gz   # Intermediate
```

## What's in the UKI

| Component | Size | Description |
|-----------|------|-------------|
| Kernel | ~2MB | Minimal Linux with virtio |
| Initramfs | ~700KB | init + /etc |
| Cmdline | few bytes | `console=ttyS0` |

## Init Process

Minimal init process in Rust (PID 1):

1. Mounts `/proc`, `/sys`, `/dev`, `/run`, `/tmp`, cgroups
2. Configures network (DHCP via rtnetlink)
3. Starts gRPC API server (vsock)
4. Manages pods and containers
5. Handles shutdown

## Testing

```bash
# With cloud-hypervisor
cloud-hypervisor \
    --kernel mvirt-one/target/mvirt-one.efi \
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
