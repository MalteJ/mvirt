# mvirt Hypervisor Installation

## Prerequisites

- mvirt ISO booted (USB stick or VM)
- Target disk (will be completely overwritten!)
- SSH public key

## 1. Partition disk

### Option A: EFI + GPT (recommended)

| # | Name | Size | Type | Filesystem |
|---|------|------|------|------------|
| 1 | EFI | 512 MiB | EFI System | FAT32 |
| 2 | System | 64 GiB | Linux | ext4 |
| 3 | VMs | Rest | Linux | ZFS |

```bash
parted /dev/nvme0n1 -- mklabel gpt
parted /dev/nvme0n1 -- mkpart ESP fat32 1MiB 513MiB
parted /dev/nvme0n1 -- set 1 esp on
parted /dev/nvme0n1 -- mkpart primary ext4 513MiB 65GiB
parted /dev/nvme0n1 -- mkpart primary 65GiB 100%

mkfs.fat -F32 /dev/nvme0n1p1
mkfs.ext4 /dev/nvme0n1p2
zpool create -o ashift=12 mvirt /dev/nvme0n1p3
```

### Option B: Legacy BIOS + GPT

| # | Name | Size | Type | Filesystem |
|---|------|------|------|------------|
| 1 | BIOS Boot | 1 MiB | BIOS boot | - |
| 2 | System | 64 GiB | Linux | ext4 |
| 3 | VMs | Rest | Linux | ZFS |

```bash
parted /dev/nvme0n1 -- mklabel gpt
parted /dev/nvme0n1 -- mkpart bios 1MiB 2MiB
parted /dev/nvme0n1 -- set 1 bios_grub on
parted /dev/nvme0n1 -- mkpart primary ext4 2MiB 65GiB
parted /dev/nvme0n1 -- mkpart primary 65GiB 100%

mkfs.ext4 /dev/nvme0n1p2
zpool create -o ashift=12 mvirt /dev/nvme0n1p3
```

### Option C: Legacy BIOS + MBR

| # | Name | Size | Type | Filesystem |
|---|------|------|------|------------|
| 1 | System | 64 GiB | Linux | ext4 |
| 2 | VMs | Rest | Linux | ZFS |

```bash
parted /dev/nvme0n1 -- mklabel msdos
parted /dev/nvme0n1 -- mkpart primary ext4 1MiB 65GiB
parted /dev/nvme0n1 -- mkpart primary 65GiB 100%

mkfs.ext4 /dev/nvme0n1p1
zpool create -o ashift=12 mvirt /dev/nvme0n1p2
```

## 2. Mount

```bash
# Option A (EFI+GPT):
mount /dev/nvme0n1p2 /mnt
mkdir -p /mnt/boot
mount /dev/nvme0n1p1 /mnt/boot

# Option B (BIOS+GPT):
mount /dev/nvme0n1p2 /mnt

# Option C (BIOS+MBR):
mount /dev/nvme0n1p1 /mnt
```

## 3. Generate NixOS config

```bash
nixos-generate-config --root /mnt
```

## 4. Edit configuration

Edit `/mnt/etc/nixos/configuration.nix`:

```nix
{ config, pkgs, ... }:
{
  imports = [ ./hardware-configuration.nix ];

  # Boot - choose appropriate option:

  # Option A: EFI + GPT
  boot.loader.systemd-boot.enable = true;
  boot.loader.efi.canTouchEfiVariables = true;

  # Option B: BIOS + GPT
  # boot.loader.grub.enable = true;
  # boot.loader.grub.device = "/dev/nvme0n1";

  # Option C: BIOS + MBR
  # boot.loader.grub.enable = true;
  # boot.loader.grub.device = "/dev/nvme0n1";

  # ZFS for VM storage (mvirt pool)
  boot.supportedFilesystems = [ "zfs" ];
  boot.zfs.extraPools = [ "mvirt" ];
  networking.hostId = "12345678";  # Required for ZFS: head -c4 /dev/urandom | xxd -p

  # System
  networking.hostName = "mvirt-node-1";
  system.stateVersion = "24.11";

  # SSH
  services.openssh.enable = true;
  users.users.root.openssh.authorizedKeys.keys = [
    "ssh-ed25519 AAAA... user@host"
  ];

  # mvirt Hypervisor
  # TODO: services.mvirt.enable = true;
}
```

## 5. Install

```bash
nixos-install --no-root-passwd
```

## 6. Reboot

```bash
reboot
```
