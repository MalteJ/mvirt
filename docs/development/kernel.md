# Kernel Configuration

This guide explains how the kernel build works and how to customize it.

## Overview

mvirt-os uses a minimal Linux kernel optimized for virtualization. The kernel configuration uses a **fragment-based approach** rather than a full `.config` file.

## Config Fragment Approach

Instead of maintaining a full kernel config (~8000+ lines), we use a small fragment (`kernel.config`, ~200 lines) that contains only our customizations.

**Build process:**
1. `make tinyconfig` - Start with the smallest possible kernel
2. `merge_config.sh` - Merge our fragment on top
3. `make olddefconfig` - Fill in remaining options with defaults

**Benefits:**
- Easy to see what options we actually need
- Survives kernel upgrades better
- Clear documentation of requirements

## Current Configuration

The `mvirt-os/kernel.config` fragment includes:

| Category | Options |
|----------|---------|
| **Virtio** | virtio-blk, virtio-net, virtio-console |
| **Storage** | AHCI/SATA, NVMe, USB storage, SCSI |
| **Network** | Intel (e1000e, igb, ice), Realtek (r8169), Broadcom, Mellanox |
| **Input** | PS/2 keyboard/mouse, USB HID |
| **Filesystems** | ext4, tmpfs, proc, sysfs, devtmpfs |

## Customizing the Kernel

### Finding Options with menuconfig

```bash
# Download and extract kernel source (if not already done)
make os-download

# Open interactive config menu
make menuconfig
```

Navigate the menus to find the option you need. Press `/` to search by name.

### Testing Changes

Changes made in menuconfig are saved to `.config` and can be tested immediately:

```bash
make menuconfig    # Enable option, save to .config
make kernel        # Build kernel
make iso           # Build ISO for testing
```

### Making Changes Permanent

If the option works, add it to the fragment:

```bash
# Add to kernel.config
echo "CONFIG_MY_DRIVER=y" >> mvirt-os/kernel.config
```

Or edit `mvirt-os/kernel.config` directly.

**Important:** Changes only in `.config` are lost after `make clean` or when `kernel.config` is modified, because `.config` is regenerated from the fragment.

### Disabling Options

To explicitly disable an option:

```bash
# In kernel.config
# CONFIG_WIRELESS is not set
```

## Adding Hardware Support

### Adding a Network Driver

1. Find the driver name (e.g., from `lspci -k` on target hardware)
2. Search in menuconfig: `/` then search for driver name
3. Note the config option (e.g., `CONFIG_IXGBE`)
4. Add to `kernel.config`:
   ```
   # Intel 10GbE (ixgbe)
   CONFIG_IXGBE=y
   ```

### Adding a Storage Controller

Same process - find the driver, add the config option:

```
# Example: Add support for specific RAID controller
CONFIG_MEGARAID_SAS=y
```

### Checking Dependencies

Some drivers have dependencies. menuconfig shows these - look for options marked with `---` or `[*]` that appear when you enable something.

Common dependencies to check:
- `CONFIG_PCI=y` - Required for most hardware
- `CONFIG_SCSI=y` - Required for storage drivers
- `CONFIG_PHYLIB=y` - Required for some network drivers

## Kernel Version

The kernel version is specified in `mvirt-os/kernel.version`:

```bash
cat mvirt-os/kernel.version
```

To update the kernel:
1. Edit `kernel.version` with new version number
2. Run `make distclean` to remove old source
3. Run `make os` to download and build new version
4. Test thoroughly - config options may have changed

## Troubleshooting

### Option Not Taking Effect

Check if the option has unmet dependencies:

```bash
cd mvirt-os/kernel
./scripts/config --state CONFIG_MY_OPTION
```

### Kernel Too Large

Review enabled options. Common space savings:
- Disable unused network drivers
- Disable debug options (`CONFIG_DEBUG_*`)
- Disable unused filesystems

### Boot Fails

Ensure these core options are enabled:
- `CONFIG_BLK_DEV_INITRD=y` - Initramfs support
- `CONFIG_RD_GZIP=y` - Gzip decompression
- `CONFIG_DEVTMPFS=y` - Device filesystem
- `CONFIG_TTY=y` - Terminal support

## Reference

- [Kernel Configuration Documentation](https://www.kernel.org/doc/html/latest/kbuild/kconfig.html)
- [Kernel Newbies - Kernel Configuration](https://kernelnewbies.org/KernelBuild)
