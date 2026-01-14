# mvirt-os Makefile
# Included from root Makefile

MVIRT_OS_DIR := mvirt-os

KERNEL_VERSION := $(shell cat $(MVIRT_OS_DIR)/kernel.version)
KERNEL_MAJOR := $(shell echo $(KERNEL_VERSION) | cut -d. -f1)
KERNEL_URL := https://cdn.kernel.org/pub/linux/kernel/v$(KERNEL_MAJOR).x/linux-$(KERNEL_VERSION).tar.xz
KERNEL_DIR := $(MVIRT_OS_DIR)/kernel
KERNEL_TARBALL := $(MVIRT_OS_DIR)/target/linux-$(KERNEL_VERSION).tar.xz

# Cloud-Hypervisor
CH_VERSION := v50.0
CH_URL := https://github.com/cloud-hypervisor/cloud-hypervisor/releases/download/$(CH_VERSION)/cloud-hypervisor-static
CH_BIN := $(MVIRT_OS_DIR)/target/cloud-hypervisor

# Firmware (hypervisor-fw for UEFI boot)
FW_VERSION := 0.5.0
FW_URL := https://github.com/cloud-hypervisor/rust-hypervisor-firmware/releases/download/$(FW_VERSION)/hypervisor-fw
FW_BIN := $(MVIRT_OS_DIR)/target/hypervisor-fw

NPROC := $(shell nproc)

# Rust binaries
RUST_TARGET_DIR := target/$(MUSL_TARGET)/release

# UKI settings
EFI_STUB := /usr/lib/systemd/boot/efi/linuxx64.efi.stub

# Output
INITRAMFS := $(MVIRT_OS_DIR)/target/initramfs.cpio.gz
BZIMAGE := $(KERNEL_DIR)/arch/x86/boot/bzImage
UKI := $(MVIRT_OS_DIR)/target/mvirt.efi
INITRAMFS_ROOTFS := $(MVIRT_OS_DIR)/initramfs/rootfs

# ============ KERNEL ============

$(KERNEL_TARBALL): | $(MVIRT_OS_DIR)/target
	curl -L -o $(KERNEL_TARBALL) $(KERNEL_URL)

$(KERNEL_DIR): $(KERNEL_TARBALL)
	mkdir -p $(KERNEL_DIR)
	tar -xf $(KERNEL_TARBALL) -C $(KERNEL_DIR) --strip-components=1

os-download: $(KERNEL_DIR)

$(KERNEL_DIR)/.config: $(KERNEL_DIR) $(MVIRT_OS_DIR)/kernel.config
	cd $(KERNEL_DIR) && make tinyconfig
	cd $(KERNEL_DIR) && ./scripts/kconfig/merge_config.sh -m .config ../kernel.config
	cd $(KERNEL_DIR) && make olddefconfig

$(BZIMAGE): $(KERNEL_DIR)/.config
	cd $(KERNEL_DIR) && make -j$(NPROC)

kernel: $(BZIMAGE)

menuconfig: $(KERNEL_DIR)
	cd $(KERNEL_DIR) && make menuconfig

# ============ CLOUD-HYPERVISOR ============

$(CH_BIN): | $(MVIRT_OS_DIR)/target
	curl -L -o $(CH_BIN) $(CH_URL)
	chmod +x $(CH_BIN)

# ============ FIRMWARE ============

$(FW_BIN): | $(MVIRT_OS_DIR)/target
	curl -L -o $(FW_BIN) $(FW_URL)

firmware: $(FW_BIN)

# ============ INITRAMFS ============

$(INITRAMFS): $(CH_BIN) $(FW_BIN) $(RUST_TARGET_DIR)/pideisn $(RUST_TARGET_DIR)/mvirt $(RUST_TARGET_DIR)/mvirt-vmm | $(MVIRT_OS_DIR)/target
	cp $(RUST_TARGET_DIR)/pideisn $(INITRAMFS_ROOTFS)/init
	chmod +x $(INITRAMFS_ROOTFS)/init
	mkdir -p $(INITRAMFS_ROOTFS)/usr/sbin $(INITRAMFS_ROOTFS)/usr/bin
	cp $(RUST_TARGET_DIR)/mvirt $(INITRAMFS_ROOTFS)/usr/sbin/mvirt
	cp $(RUST_TARGET_DIR)/mvirt-vmm $(INITRAMFS_ROOTFS)/usr/sbin/
	chmod +x $(INITRAMFS_ROOTFS)/usr/sbin/*
	cp $(CH_BIN) $(INITRAMFS_ROOTFS)/usr/bin/cloud-hypervisor
	chmod +x $(INITRAMFS_ROOTFS)/usr/bin/cloud-hypervisor
	mkdir -p $(INITRAMFS_ROOTFS)/var/lib/mvirt
	cp $(FW_BIN) $(INITRAMFS_ROOTFS)/var/lib/mvirt/hypervisor-fw
	cd $(INITRAMFS_ROOTFS) && find . -print0 | cpio --null -ov --format=newc | gzip -9 > ../../../$(INITRAMFS)

initramfs: $(INITRAMFS)

# ============ UKI ============

$(UKI): $(BZIMAGE) $(INITRAMFS) $(MVIRT_OS_DIR)/cmdline.txt $(MVIRT_OS_DIR)/kernel.version | $(MVIRT_OS_DIR)/target
	ukify build \
		--linux=$(BZIMAGE) \
		--initrd=$(INITRAMFS) \
		--cmdline=@$(MVIRT_OS_DIR)/cmdline.txt \
		--uname=$(KERNEL_VERSION) \
		--stub=$(EFI_STUB) \
		--output=$(UKI)

uki: $(UKI)

# ============ ISO (UEFI + Legacy BIOS) ============

ISO := $(MVIRT_OS_DIR)/target/mvirt-os.iso
ISO_DIR := $(MVIRT_OS_DIR)/target/iso

CMDLINE := $(shell cat $(MVIRT_OS_DIR)/cmdline.txt 2>/dev/null)

$(ISO): $(UKI) $(MVIRT_OS_DIR)/cmdline.txt | $(MVIRT_OS_DIR)/target
	rm -rf $(ISO_DIR)
	mkdir -p $(ISO_DIR)/boot/grub
	mkdir -p $(ISO_DIR)/EFI/BOOT
	mkdir -p $(ISO_DIR)/isolinux
	# Copy kernel and initramfs
	cp $(BZIMAGE) $(ISO_DIR)/boot/vmlinuz
	cp $(INITRAMFS) $(ISO_DIR)/boot/initramfs.cpio.gz
	# UEFI: Copy UKI as EFI bootloader
	cp $(UKI) $(ISO_DIR)/EFI/BOOT/BOOTX64.EFI
	# Legacy BIOS: ISOLINUX
	cp /usr/lib/ISOLINUX/isolinux.bin $(ISO_DIR)/isolinux/
	cp /usr/lib/syslinux/modules/bios/ldlinux.c32 $(ISO_DIR)/isolinux/
	cp /usr/lib/syslinux/modules/bios/libcom32.c32 $(ISO_DIR)/isolinux/
	cp /usr/lib/syslinux/modules/bios/libutil.c32 $(ISO_DIR)/isolinux/
	cp /usr/lib/syslinux/modules/bios/menu.c32 $(ISO_DIR)/isolinux/
	# ISOLINUX config
	printf 'DEFAULT mvirt\nTIMEOUT 30\nPROMPT 0\n\nLABEL mvirt\n    KERNEL /boot/vmlinuz\n    INITRD /boot/initramfs.cpio.gz\n    APPEND $(CMDLINE)\n' > $(ISO_DIR)/isolinux/isolinux.cfg
	# GRUB config for UEFI (fallback)
	printf 'set timeout=3\nset default=0\n\nmenuentry "mvirt-os" {\n    linux /boot/vmlinuz $(CMDLINE)\n    initrd /boot/initramfs.cpio.gz\n}\n' > $(ISO_DIR)/boot/grub/grub.cfg
	# Create hybrid ISO (UEFI + BIOS bootable)
	xorriso -as mkisofs \
		-o $(ISO) \
		-isohybrid-mbr /usr/lib/ISOLINUX/isohdpfx.bin \
		-c isolinux/boot.cat \
		-b isolinux/isolinux.bin \
		-no-emul-boot \
		-boot-load-size 4 \
		-boot-info-table \
		-eltorito-alt-boot \
		-e EFI/BOOT/BOOTX64.EFI \
		-no-emul-boot \
		-isohybrid-gpt-basdat \
		$(ISO_DIR)

iso: $(ISO)

# ============ TARGET DIR ============

$(MVIRT_OS_DIR)/target:
	mkdir -p $(MVIRT_OS_DIR)/target

# ============ CLEAN ============

os-clean:
	-cd $(KERNEL_DIR) && make clean
	rm -rf $(MVIRT_OS_DIR)/target
	rm -rf $(ISO_DIR)
	rm -f $(INITRAMFS_ROOTFS)/init
	rm -f $(INITRAMFS_ROOTFS)/usr/sbin/mvirt-cli
	rm -f $(INITRAMFS_ROOTFS)/usr/sbin/mvirt-vmm
	rm -f $(INITRAMFS_ROOTFS)/usr/bin/cloud-hypervisor
	rm -rf $(INITRAMFS_ROOTFS)/var/lib/mvirt

os-distclean: os-clean
	rm -rf $(KERNEL_DIR)
