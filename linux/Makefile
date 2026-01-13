KERNEL_VERSION := 6.18.4
KERNEL_MAJOR := $(shell echo $(KERNEL_VERSION) | cut -d. -f1)
KERNEL_URL := https://cdn.kernel.org/pub/linux/kernel/v$(KERNEL_MAJOR).x/linux-$(KERNEL_VERSION).tar.xz
KERNEL_DIR := kernel
KERNEL_TARBALL := linux-$(KERNEL_VERSION).tar.xz

NPROC := $(shell nproc)

.PHONY: all clean distclean download build menuconfig

all: build

# Download kernel tarball
$(KERNEL_TARBALL):
	curl -LO $(KERNEL_URL)

# Extract kernel
$(KERNEL_DIR): $(KERNEL_TARBALL)
	mkdir -p $(KERNEL_DIR)
	tar -xf $(KERNEL_TARBALL) -C $(KERNEL_DIR) --strip-components=1

download: $(KERNEL_DIR)

# Configure kernel for cloud-hypervisor (only if no .config exists)
$(KERNEL_DIR)/.config: $(KERNEL_DIR) cloud-hypervisor.config
	cd $(KERNEL_DIR) && make tinyconfig
	cd $(KERNEL_DIR) && ./scripts/kconfig/merge_config.sh -m .config ../cloud-hypervisor.config
	cd $(KERNEL_DIR) && make olddefconfig

# Build kernel
build: $(KERNEL_DIR)/.config
	cd $(KERNEL_DIR) && make -j$(NPROC)

# Configure kernel manually
menuconfig: $(KERNEL_DIR)
	cd $(KERNEL_DIR) && make menuconfig

clean: $(KERNEL_DIR)
	cd $(KERNEL_DIR) && make clean

distclean:
	rm -rf $(KERNEL_DIR)
	rm -f $(KERNEL_TARBALL)
