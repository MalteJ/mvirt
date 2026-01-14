# mvirt mono-repo Makefile
# Run all commands from /home/malte/mvirt

MUSL_TARGET := x86_64-unknown-linux-musl

.PHONY: all build release os kernel initramfs uki iso menuconfig clean distclean check docker

# Include mvirt-os subsystem
include mvirt-os/mvirt-os.mk

# Default: build everything
all: release os

# ============ RUST ============

build:
	cargo build

release:
	cargo build --release --target $(MUSL_TARGET)

# Rust binary targets (for dependency tracking)
$(RUST_TARGET_DIR)/pideisn $(RUST_TARGET_DIR)/mvirt $(RUST_TARGET_DIR)/mvirt-vmm:
	cargo build --release --target $(MUSL_TARGET)

# ============ MVIRT-OS ============

os: uki

# ============ CLEAN ============

clean: os-clean
	cargo clean

distclean: os-distclean
	cargo clean

# ============ CHECK BUILD DEPENDENCIES ============

REQUIRED_CMDS := cargo curl tar cpio gzip ukify xorriso
REQUIRED_FILES := /usr/lib/systemd/boot/efi/linuxx64.efi.stub
ISO_FILES := /usr/lib/ISOLINUX/isolinux.bin /usr/lib/ISOLINUX/isohdpfx.bin \
             /usr/lib/syslinux/modules/bios/ldlinux.c32 \
             /usr/lib/syslinux/modules/bios/libcom32.c32 \
             /usr/lib/syslinux/modules/bios/libutil.c32 \
             /usr/lib/syslinux/modules/bios/menu.c32

check:
	@echo "Checking build dependencies..."
	@ok=1; \
	for cmd in $(REQUIRED_CMDS); do \
		if command -v $$cmd >/dev/null 2>&1; then \
			printf "  %-12s OK\n" "$$cmd"; \
		else \
			printf "  %-12s MISSING\n" "$$cmd"; ok=0; \
		fi; \
	done; \
	if rustup target list --installed 2>/dev/null | grep -q $(MUSL_TARGET); then \
		printf "  %-12s OK\n" "$(MUSL_TARGET)"; \
	else \
		printf "  %-12s MISSING (rustup target add $(MUSL_TARGET))\n" "$(MUSL_TARGET)"; ok=0; \
	fi; \
	for f in $(REQUIRED_FILES); do \
		if [ -f "$$f" ]; then \
			printf "  %-12s OK\n" "$$(basename $$f)"; \
		else \
			printf "  %-12s MISSING ($$f)\n" "$$(basename $$f)"; ok=0; \
		fi; \
	done; \
	echo ""; \
	echo "ISO dependencies (optional):"; \
	for f in $(ISO_FILES); do \
		if [ -f "$$f" ]; then \
			printf "  %-12s OK\n" "$$(basename $$f)"; \
		else \
			printf "  %-12s MISSING (apt install isolinux syslinux-common)\n" "$$(basename $$f)"; \
		fi; \
	done; \
	echo ""; \
	if [ $$ok -eq 1 ]; then \
		echo "All required dependencies available."; \
	else \
		echo "Some dependencies missing!"; exit 1; \
	fi

# ============ DOCKER BUILD ============

DOCKER_IMAGE := mvirt-builder

docker:
	docker build -t $(DOCKER_IMAGE) .
	docker run --rm \
		--user $$(id -u):$$(id -g) \
		-v $(CURDIR):/work \
		$(DOCKER_IMAGE) \
		make iso
