# mvirt-uos Makefile (µOS for MicroVMs)
# Included from root Makefile
# Builds UKI (Unified Kernel Image) for direct boot

MVIRT_UOS_DIR := mvirt-uos

KERNEL_VERSION := $(shell cat $(MVIRT_UOS_DIR)/kernel.version)
KERNEL_MAJOR := $(shell echo $(KERNEL_VERSION) | cut -d. -f1)
KERNEL_URL := https://cdn.kernel.org/pub/linux/kernel/v$(KERNEL_MAJOR).x/linux-$(KERNEL_VERSION).tar.xz
KERNEL_DIR := $(MVIRT_UOS_DIR)/kernel
KERNEL_TARBALL := $(MVIRT_UOS_DIR)/target/linux-$(KERNEL_VERSION).tar.xz

# Config fragment
KERNEL_CONFIG := $(MVIRT_UOS_DIR)/kernel.config

NPROC := $(shell nproc)

# Rust binaries
RUST_TARGET_DIR := target/$(MUSL_TARGET)/release

# UKI settings
EFI_STUB := /usr/lib/systemd/boot/efi/linuxx64.efi.stub

# Output paths
BZIMAGE := $(KERNEL_DIR)/arch/x86/boot/bzImage
INITRAMFS := $(MVIRT_UOS_DIR)/target/initramfs.cpio.gz
INITRAMFS_ROOTFS := $(MVIRT_UOS_DIR)/initramfs/rootfs
UKI := $(MVIRT_UOS_DIR)/target/mvirt-uos.efi

# ============ KERNEL ============

$(KERNEL_TARBALL): | $(MVIRT_UOS_DIR)/target
	curl -L -o $(KERNEL_TARBALL) $(KERNEL_URL)

$(KERNEL_DIR): $(KERNEL_TARBALL)
	mkdir -p $(KERNEL_DIR)
	tar -xf $(KERNEL_TARBALL) -C $(KERNEL_DIR) --strip-components=1

os-download: $(KERNEL_DIR)

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

# ============ INITRAMFS ============

$(INITRAMFS): $(RUST_TARGET_DIR)/pideisn | $(MVIRT_UOS_DIR)/target
	cp $(RUST_TARGET_DIR)/pideisn $(INITRAMFS_ROOTFS)/init
	chmod +x $(INITRAMFS_ROOTFS)/init
	cd $(INITRAMFS_ROOTFS) && find . -print0 | cpio --null -ov --format=newc | gzip -9 > ../../../$(INITRAMFS)

.PHONY: initramfs
initramfs: $(INITRAMFS)

# ============ UKI (Unified Kernel Image) ============

$(UKI): $(BZIMAGE) $(INITRAMFS) $(MVIRT_UOS_DIR)/cmdline.txt | $(MVIRT_UOS_DIR)/target
	ukify build \
		--linux=$(BZIMAGE) \
		--initrd=$(INITRAMFS) \
		--cmdline=@$(MVIRT_UOS_DIR)/cmdline.txt \
		--uname=$(KERNEL_VERSION) \
		--stub=$(EFI_STUB) \
		--output=$(UKI)

.PHONY: uos
uos: $(UKI)

# ============ TARGET DIR ============

$(MVIRT_UOS_DIR)/target:
	mkdir -p $(MVIRT_UOS_DIR)/target

# ============ CLEAN ============

.PHONY: uos-clean uos-mrproper
uos-clean:
	rm -rf $(MVIRT_UOS_DIR)/target
	rm -f $(INITRAMFS_ROOTFS)/init
	-cd $(KERNEL_DIR) 2>/dev/null && make clean

uos-mrproper: uos-clean
	rm -rf $(KERNEL_DIR)
