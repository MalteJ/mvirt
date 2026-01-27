# mvirt-one Makefile (MicroVM Init System)
# Included from root Makefile
# Builds UKI (Unified Kernel Image) for direct boot

MVIRT_ONE_DIR := mvirt-one

KERNEL_VERSION := $(shell cat $(MVIRT_ONE_DIR)/kernel.version)
KERNEL_MAJOR := $(shell echo $(KERNEL_VERSION) | cut -d. -f1)
KERNEL_URL := https://cdn.kernel.org/pub/linux/kernel/v$(KERNEL_MAJOR).x/linux-$(KERNEL_VERSION).tar.xz
KERNEL_DIR := $(MVIRT_ONE_DIR)/kernel
KERNEL_TARBALL := $(MVIRT_ONE_DIR)/target/linux-$(KERNEL_VERSION).tar.xz

# Config fragment
KERNEL_CONFIG := $(MVIRT_ONE_DIR)/kernel.config

NPROC := $(shell nproc)

# Rust binaries
RUST_TARGET_DIR := target/$(MUSL_TARGET)/release

# Youki OCI runtime
YOUKI_VERSION := 0.5.0
YOUKI_URL := https://github.com/youki-dev/youki/releases/download/v$(YOUKI_VERSION)/youki-$(YOUKI_VERSION)-x86_64-musl.tar.gz
YOUKI_TARBALL := $(MVIRT_ONE_DIR)/target/youki-$(YOUKI_VERSION).tar.gz
YOUKI_BIN := $(MVIRT_ONE_DIR)/target/youki

# UKI settings
EFI_STUB := /usr/lib/systemd/boot/efi/linuxx64.efi.stub

# Output paths
BZIMAGE := $(KERNEL_DIR)/arch/x86/boot/bzImage
INITRAMFS := $(MVIRT_ONE_DIR)/target/initramfs.cpio.gz
INITRAMFS_ROOTFS := $(MVIRT_ONE_DIR)/initramfs/rootfs
UKI := $(MVIRT_ONE_DIR)/target/mvirt-one.efi
ROOTFS_RAW := $(MVIRT_ONE_DIR)/target/rootfs.raw
ROOTFS_SIZE_MB := 32

# ============ KERNEL ============

$(KERNEL_TARBALL): | $(MVIRT_ONE_DIR)/target
	curl -L -o $(KERNEL_TARBALL) $(KERNEL_URL)

$(KERNEL_DIR): $(KERNEL_TARBALL)
	mkdir -p $(KERNEL_DIR)
	tar -xf $(KERNEL_TARBALL) -C $(KERNEL_DIR) --strip-components=1

one-download: $(KERNEL_DIR)

$(KERNEL_DIR)/.config: $(KERNEL_DIR) $(KERNEL_CONFIG)
	cd $(KERNEL_DIR) && make tinyconfig
	cd $(KERNEL_DIR) && ./scripts/kconfig/merge_config.sh -m .config $(abspath $(KERNEL_CONFIG))
	cd $(KERNEL_DIR) && make olddefconfig

$(BZIMAGE): $(KERNEL_DIR)/.config
	cd $(KERNEL_DIR) && make -j$(NPROC)

.PHONY: kernel
kernel: $(BZIMAGE)

menuconfig: $(KERNEL_DIR)
	cd $(KERNEL_DIR) && make menuconfig

# ============ YOUKI ============

$(YOUKI_TARBALL): | $(MVIRT_ONE_DIR)/target
	curl -L -o $(YOUKI_TARBALL) $(YOUKI_URL)

$(YOUKI_BIN): $(YOUKI_TARBALL)
	tar -xzf $(YOUKI_TARBALL) -C $(MVIRT_ONE_DIR)/target youki
	chmod +x $(YOUKI_BIN)

.PHONY: youki
youki: $(YOUKI_BIN)

# ============ INITRAMFS ============

$(INITRAMFS): $(RUST_TARGET_DIR)/mvirt-one $(YOUKI_BIN) | $(MVIRT_ONE_DIR)/target
	mkdir -p $(INITRAMFS_ROOTFS)/usr/bin
	cp $(RUST_TARGET_DIR)/mvirt-one $(INITRAMFS_ROOTFS)/init
	cp $(YOUKI_BIN) $(INITRAMFS_ROOTFS)/usr/bin/youki
	chmod +x $(INITRAMFS_ROOTFS)/init $(INITRAMFS_ROOTFS)/usr/bin/youki
	cd $(INITRAMFS_ROOTFS) && find . -print0 | cpio --null -ov --format=newc | gzip -9 > ../../../$(INITRAMFS)

.PHONY: initramfs
initramfs: $(INITRAMFS)

# ============ ROOTFS RAW IMAGE ============

$(ROOTFS_RAW): $(RUST_TARGET_DIR)/mvirt-one $(YOUKI_BIN) | $(MVIRT_ONE_DIR)/target
	@echo "Building rootfs.raw ($(ROOTFS_SIZE_MB)MB ext4 image)..."
	mkdir -p $(INITRAMFS_ROOTFS)/usr/bin
	cp $(RUST_TARGET_DIR)/mvirt-one $(INITRAMFS_ROOTFS)/init
	cp $(YOUKI_BIN) $(INITRAMFS_ROOTFS)/usr/bin/youki
	chmod +x $(INITRAMFS_ROOTFS)/init $(INITRAMFS_ROOTFS)/usr/bin/youki
	truncate -s $(ROOTFS_SIZE_MB)M $(ROOTFS_RAW)
	/usr/sbin/mkfs.ext4 -d $(INITRAMFS_ROOTFS) $(ROOTFS_RAW)

.PHONY: rootfs
rootfs: $(ROOTFS_RAW)

# ============ UKI (Unified Kernel Image) ============

$(UKI): $(BZIMAGE) $(INITRAMFS) $(MVIRT_ONE_DIR)/cmdline.txt | $(MVIRT_ONE_DIR)/target
	ukify build \
		--linux=$(BZIMAGE) \
		--initrd=$(INITRAMFS) \
		--cmdline=@$(MVIRT_ONE_DIR)/cmdline.txt \
		--uname=$(KERNEL_VERSION) \
		--stub=$(EFI_STUB) \
		--output=$(UKI)

.PHONY: one
one: $(UKI) $(ROOTFS_RAW)
	cp $(BZIMAGE) $(MVIRT_ONE_DIR)/target/bzImage
	cp $(MVIRT_ONE_DIR)/cmdline.txt $(MVIRT_ONE_DIR)/target/cmdline

# Keep 'uos' as alias for backwards compatibility
.PHONY: uos
uos: one

# ============ TARGET DIR ============

$(MVIRT_ONE_DIR)/target:
	mkdir -p $(MVIRT_ONE_DIR)/target

# ============ CLEAN ============

.PHONY: one-clean one-mrproper uos-clean uos-mrproper
one-clean:
	rm -rf $(MVIRT_ONE_DIR)/target
	rm -f $(INITRAMFS_ROOTFS)/init
	rm -f $(INITRAMFS_ROOTFS)/usr/bin/youki
	-cd $(KERNEL_DIR) 2>/dev/null && make clean

one-mrproper: one-clean
	rm -rf $(KERNEL_DIR)

# Aliases for backwards compatibility
uos-clean: one-clean
uos-mrproper: one-mrproper
