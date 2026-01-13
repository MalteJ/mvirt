# mvirt-os

Build-System für Linux-Kernel, initramfs und UKI (Unified Kernel Image).

## Features

- Minimaler Linux-Kernel für cloud-hypervisor
- initramfs mit statisch gelinkten Binaries
- UKI für direktes EFI-Booten
- pideins - Rust init (PID 1)

## Verwendung

```bash
# Alles bauen (Kernel + initramfs + UKI)
make -C mvirt-os

# Einzelne Targets
make -C mvirt-os kernel
make -C mvirt-os initramfs
make -C mvirt-os uki

# Kernel konfigurieren
make -C mvirt-os menuconfig
```

## Struktur

```
mvirt-os/
├── kernel.version          # Kernel-Version (z.B. "6.18.4")
├── kernel.config           # Kconfig Fragment
├── cmdline.txt             # Kernel Cmdline
├── mvirt-os.mk             # Haupt-Makefile
├── linux.mk                # Kernel-Build
├── initramfs/
│   └── rootfs/             # initramfs Skeleton
│       ├── dev/
│       ├── proc/
│       ├── sys/
│       ├── usr/bin/
│       └── usr/sbin/
├── pideins/                # Rust init
│   ├── Cargo.toml
│   └── src/main.rs
└── target/                 # Build-Output
    ├── cloud-hypervisor    # Downloaded Binary
    ├── initramfs.cpio.gz
    └── mvirt.efi
```

## Konfiguration

### kernel.version

```
6.18.4
```

### kernel.config

Kconfig-Fragment das auf `tinyconfig` angewendet wird:

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

## initramfs Inhalt

| Pfad | Quelle |
|------|--------|
| `/init` | `pideins` (Rust init) |
| `/usr/sbin/mvirt` | CLI |
| `/usr/sbin/mvirt-vmm` | Daemon |
| `/usr/bin/cloud-hypervisor` | cloud-hypervisor Release |

## pideins

Minimaler init-Prozess in Rust:

1. Mountet `/proc`, `/sys`, `/dev`
2. Setzt Hostname
3. Startet `mvirt-vmm`
4. Wartet auf Shutdown-Signal
5. Rebootet

## Output

| Datei | Beschreibung |
|-------|--------------|
| `kernel/arch/x86/boot/bzImage` | Linux Kernel |
| `target/initramfs.cpio.gz` | Komprimiertes initramfs |
| `target/mvirt.efi` | Bootbares UKI |
| `target/cloud-hypervisor` | cloud-hypervisor Binary |

## Aufräumen

```bash
make -C mvirt-os clean       # Build-Artefakte löschen
make -C mvirt-os distclean   # + Kernel-Source + Downloads
```

## Abhängigkeiten

```bash
# Kernel-Build
sudo apt install build-essential flex bison libncurses-dev \
    libssl-dev libelf-dev bc dwarves

# UKI
sudo apt install systemd-ukify systemd-boot-efi

# cloud-init ISO
sudo apt install genisoimage
```
