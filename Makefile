# mvirt mono-repo Makefile
# Run all commands from /home/malte/mvirt

MUSL_TARGET := x86_64-unknown-linux-musl

.PHONY: all build release os kernel initramfs uki menuconfig clean distclean

# Include mvirt-os subsystem
include mvirt-os/mvirt-os.mk

# Default: build everything
all: release os

# ============ RUST ============

build:
	cargo build

release:
	cargo build --release --target $(MUSL_TARGET)

# ============ MVIRT-OS ============

os: release kernel initramfs uki

# ============ CLEAN ============

clean: os-clean
	cargo clean

distclean: os-distclean
	cargo clean
