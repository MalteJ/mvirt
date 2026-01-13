# Linux subsystem Makefile
# Included from root Makefile

LINUX_DIR := linux

KERNEL_VERSION := $(shell cat $(LINUX_DIR)/kernel.version)
KERNEL_MAJOR := $(shell echo $(KERNEL_VERSION) | cut -d. -f1)
KERNEL_URL := https://cdn.kernel.org/pub/linux/kernel/v$(KERNEL_MAJOR).x/linux-$(KERNEL_VERSION).tar.xz
KERNEL_DIR := $(LINUX_DIR)/kernel
KERNEL_TARBALL := $(LINUX_DIR)/target/linux-$(KERNEL_VERSION).tar.xz

# Cloud-Hypervisor
CH_VERSION := v50.0
CH_URL := https://github.com/cloud-hypervisor/cloud-hypervisor/releases/download/$(CH_VERSION)/cloud-hypervisor-static
CH_BIN := $(LINUX_DIR)/target/cloud-hypervisor

NPROC := $(shell nproc)

# Rust binaries
RUST_TARGET_DIR := target/$(MUSL_TARGET)/release

# UKI settings
EFI_STUB := /usr/lib/systemd/boot/efi/linuxx64.efi.stub

# Output
INITRAMFS := $(LINUX_DIR)/target/initramfs.cpio.gz
BZIMAGE := $(KERNEL_DIR)/arch/x86/boot/bzImage
UKI := $(LINUX_DIR)/target/mvirt.efi
INITRAMFS_ROOTFS := $(LINUX_DIR)/initramfs/rootfs

# ============ KERNEL ============

$(KERNEL_TARBALL): | $(LINUX_DIR)/target
	curl -L -o $(KERNEL_TARBALL) $(KERNEL_URL)

$(KERNEL_DIR): $(KERNEL_TARBALL)
	mkdir -p $(KERNEL_DIR)
	tar -xf $(KERNEL_TARBALL) -C $(KERNEL_DIR) --strip-components=1

linux-download: $(KERNEL_DIR)

$(KERNEL_DIR)/.config: $(KERNEL_DIR) $(LINUX_DIR)/kernel.config
	cd $(KERNEL_DIR) && make tinyconfig
	cd $(KERNEL_DIR) && ./scripts/kconfig/merge_config.sh -m .config ../kernel.config
	cd $(KERNEL_DIR) && make olddefconfig

$(BZIMAGE): $(KERNEL_DIR)/.config
	cd $(KERNEL_DIR) && make -j$(NPROC)

kernel: $(BZIMAGE)

menuconfig: $(KERNEL_DIR)
	cd $(KERNEL_DIR) && make menuconfig

# ============ CLOUD-HYPERVISOR ============

$(CH_BIN): | $(LINUX_DIR)/target
	curl -L -o $(CH_BIN) $(CH_URL)
	chmod +x $(CH_BIN)

# ============ INITRAMFS ============

$(INITRAMFS): $(CH_BIN) $(RUST_TARGET_DIR)/pideins $(RUST_TARGET_DIR)/mvirt-cli $(RUST_TARGET_DIR)/mvirt-vmm | $(LINUX_DIR)/target
	cp $(RUST_TARGET_DIR)/pideins $(INITRAMFS_ROOTFS)/init
	chmod +x $(INITRAMFS_ROOTFS)/init
	cp $(RUST_TARGET_DIR)/mvirt-cli $(INITRAMFS_ROOTFS)/usr/sbin/
	cp $(RUST_TARGET_DIR)/mvirt-vmm $(INITRAMFS_ROOTFS)/usr/sbin/
	chmod +x $(INITRAMFS_ROOTFS)/usr/sbin/*
	cp $(CH_BIN) $(INITRAMFS_ROOTFS)/usr/bin/cloud-hypervisor
	chmod +x $(INITRAMFS_ROOTFS)/usr/bin/cloud-hypervisor
	cd $(INITRAMFS_ROOTFS) && find . -print0 | cpio --null -ov --format=newc | gzip -9 > ../../../$(INITRAMFS)

initramfs: $(INITRAMFS)

# ============ UKI ============

$(UKI): $(BZIMAGE) $(INITRAMFS) $(LINUX_DIR)/cmdline.txt $(LINUX_DIR)/kernel.version | $(LINUX_DIR)/target
	ukify build \
		--linux=$(BZIMAGE) \
		--initrd=$(INITRAMFS) \
		--cmdline=@$(LINUX_DIR)/cmdline.txt \
		--uname=$(KERNEL_VERSION) \
		--stub=$(EFI_STUB) \
		--output=$(UKI)

uki: $(UKI)

# ============ TARGET DIR ============

$(LINUX_DIR)/target:
	mkdir -p $(LINUX_DIR)/target

# ============ CLEAN ============

linux-clean:
	-cd $(KERNEL_DIR) && make clean
	rm -rf $(LINUX_DIR)/target
	rm -f $(INITRAMFS_ROOTFS)/init
	rm -f $(INITRAMFS_ROOTFS)/usr/sbin/mvirt-cli
	rm -f $(INITRAMFS_ROOTFS)/usr/sbin/mvirt-vmm
	rm -f $(INITRAMFS_ROOTFS)/usr/bin/cloud-hypervisor

linux-distclean: linux-clean
	rm -rf $(KERNEL_DIR)
