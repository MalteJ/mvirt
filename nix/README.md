# mvirt Nix Integration

This directory contains the Nix/NixOS configuration for building mvirt packages and a bootable hypervisor image.

## Structure

```
nix/
├── README.md                # This file
├── packages/
│   ├── mvirt.nix            # Rust workspace build (all mvirt binaries)
│   ├── cloud-hypervisor.nix # Static cloud-hypervisor binary
│   └── hypervisor-fw.nix    # Firmware files (hypervisor-fw, CLOUDHV.fd)
├── modules/
│   └── mvirt.nix            # NixOS service module
└── images/
    └── hypervisor.nix       # Bootable hypervisor ISO configuration
```

## Quick Start

### Prerequisites

- Nix with flakes enabled
- For building: `nix build`
- For development: `nix develop`

### Build Commands

```bash
# Enter development shell
nix develop

# Build all mvirt packages
nix build .#mvirt

# Build individual components
nix build .#mvirt-cli
nix build .#mvirt-vmm
nix build .#mvirt-zfs
nix build .#mvirt-net
nix build .#mvirt-log

# Build cloud-hypervisor
nix build .#cloud-hypervisor

# Build firmware
nix build .#hypervisor-fw
nix build .#edk2-cloudhv

# Build hypervisor ISO image
nix build .#hypervisor-image
```

### First Build

On first build, you'll need to update the SHA256 hashes in:

- `nix/packages/cloud-hypervisor.nix`
- `nix/packages/hypervisor-fw.nix`

Nix will tell you the correct hash when the build fails.

## Using the NixOS Module

Add mvirt to your NixOS configuration:

```nix
{
  inputs.mvirt.url = "github:your-org/mvirt";

  outputs = { self, nixpkgs, mvirt, ... }: {
    nixosConfigurations.myhost = nixpkgs.lib.nixosSystem {
      system = "x86_64-linux";
      modules = [
        mvirt.nixosModules.mvirt
        {
          services.mvirt = {
            enable = true;
            package = mvirt.packages.x86_64-linux.mvirt;
            cloudHypervisor = mvirt.packages.x86_64-linux.cloud-hypervisor;
            firmware = mvirt.packages.x86_64-linux.hypervisor-fw;

            # Enable/disable individual services
            vmm.enable = true;
            vmm.port = 50051;

            log.enable = true;
            log.port = 50052;

            net.enable = true;
            net.port = 50054;

            # ZFS support (requires ZFS pool)
            zfs.enable = false;
            zfs.pool = "tank";
            zfs.port = 50053;
          };
        }
      ];
    };
  };
}
```

## Module Options

| Option | Default | Description |
|--------|---------|-------------|
| `services.mvirt.enable` | `false` | Enable mvirt services |
| `services.mvirt.package` | - | The mvirt package to use |
| `services.mvirt.cloudHypervisor` | - | The cloud-hypervisor package |
| `services.mvirt.firmware` | - | The firmware package |
| `services.mvirt.dataDir` | `/var/lib/mvirt` | Data directory |
| `services.mvirt.vmm.enable` | `true` | Enable VMM daemon |
| `services.mvirt.vmm.port` | `50051` | VMM gRPC port |
| `services.mvirt.log.enable` | `true` | Enable logging service |
| `services.mvirt.log.port` | `50052` | Log service gRPC port |
| `services.mvirt.net.enable` | `true` | Enable network manager |
| `services.mvirt.net.port` | `50054` | Network manager gRPC port |
| `services.mvirt.zfs.enable` | `false` | Enable ZFS storage manager |
| `services.mvirt.zfs.port` | `50053` | ZFS manager gRPC port |
| `services.mvirt.zfs.pool` | `"tank"` | ZFS pool name |

## Hypervisor Image

The hypervisor image is a bootable NixOS ISO with all mvirt services pre-configured.

### Building the Image

```bash
nix build .#hypervisor-image
ls -la result/iso/
```

### Testing with QEMU

```bash
qemu-system-x86_64 \
  -enable-kvm \
  -m 4G \
  -smp 4 \
  -cdrom result/iso/mvirt-hypervisor-*.iso \
  -boot d \
  -net nic \
  -net user,hostfwd=tcp::2222-:22
```

Then SSH in:

```bash
ssh -p 2222 admin@localhost
```

### Image Features

- KVM modules pre-loaded (kvm-intel, kvm-amd, vhost_net)
- Bridge networking (br0) configured
- IP forwarding enabled
- All mvirt services running
- SSH access enabled
- Serial console support for headless operation

### Customizing the Image

Create your own configuration:

```nix
# my-hypervisor.nix
{ config, pkgs, ... }:

{
  imports = [ ./nix/images/hypervisor.nix ];

  # Add your SSH keys
  users.users.root.openssh.authorizedKeys.keys = [
    "ssh-ed25519 AAAA... your-key"
  ];

  users.users.admin.openssh.authorizedKeys.keys = [
    "ssh-ed25519 AAAA... your-key"
  ];

  # Enable ZFS
  services.mvirt.zfs.enable = true;
  services.mvirt.zfs.pool = "mypool";

  # Custom networking
  networking.interfaces.br0.ipv4.addresses = [{
    address = "192.168.1.100";
    prefixLength = 24;
  }];
}
```

## Development

### Development Shell

The flake provides a development shell with all necessary tools:

```bash
nix develop
```

This includes:

- Rust toolchain with musl target
- protobuf compiler
- pkg-config and OpenSSL
- SQLite
- QEMU for testing

### Building Manually

Inside the dev shell:

```bash
# Build all packages
cargo build --release

# Build for musl (static)
cargo build --release --target x86_64-unknown-linux-musl
```

## Troubleshooting

### Hash Mismatch

If you get hash mismatch errors for downloaded files, Nix will show the correct hash. Update the `sha256` field in the relevant package file.

### Build Failures

Check that:

1. You have flakes enabled in your Nix config
2. protobuf is available (included in dev shell)
3. musl is available for static builds

### Service Issues

Check service status:

```bash
systemctl status mvirt-vmm mvirt-log mvirt-net mvirt-zfs
journalctl -u mvirt-vmm -f
```

### KVM Access

Ensure KVM is available:

```bash
ls -la /dev/kvm
lsmod | grep kvm
```
