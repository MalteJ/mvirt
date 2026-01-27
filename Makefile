# mvirt mono-repo Makefile
# Run all commands from /home/malte/mvirt

MUSL_TARGET := x86_64-unknown-linux-musl

.PHONY: all build release uos kernel initramfs menuconfig clean mrproper check docker deb deb-clean install iso

# Include subsystems
include mvirt-one/mvirt-one.mk
include mvirt-log/mvirt-log.mk
include mvirt-zfs/mvirt-zfs.mk
include mvirt-vmm/mvirt-vmm.mk

# Default: build everything
all: release one

# ============ RUST ============

build:
	cargo build

release:
	cargo build --release --target $(MUSL_TARGET)

# ============ TESTS ============

.PHONY: test test-unit test-integration test-all

# Run unit tests (no sudo required) - excludes integration_test
test-unit:
	cargo test --package mvirt-ebpf --features test-util --lib
	cargo test --package mvirt-ebpf --features test-util --test arp_test
	cargo test --package mvirt-ebpf --features test-util --test dhcp_test
	cargo test --package mvirt-ebpf --features test-util --test ping_test
	cargo test --package mvirt-ebpf --features test-util --test routing_test
	cargo test --package mvirt-ebpf --features test-util --test service_test

# Run integration tests (requires sudo for TAP devices)
test-integration:
	sudo -E cargo test --package mvirt-ebpf --test integration_test --features test-util

# Run all tests
test-all: test-unit test-integration

# Alias
test: test-unit

# Rust binary targets (for dependency tracking)
$(RUST_TARGET_DIR)/mvirt-one $(RUST_TARGET_DIR)/mvirt $(RUST_TARGET_DIR)/mvirt-vmm:
	cargo build --release --target $(MUSL_TARGET)

# ============ CLEAN ============

clean: one-clean
	cargo clean

mrproper: one-mrproper vmm-clean deb-clean
	cargo clean

# ============ CHECK BUILD DEPENDENCIES ============

REQUIRED_CMDS := cargo curl tar cpio gzip qemu-img ukify
REQUIRED_FILES := /usr/lib/systemd/boot/efi/linuxx64.efi.stub

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
		make uos

# ============ NIX ISO ============

iso:
	nix build .#hypervisor-image -o nix/result

# ============ DEBIAN PACKAGES ============

DEB_OUT := target/deb

deb: vmm-deps
	dpkg-buildpackage -us -uc -b
	mkdir -p $(DEB_OUT)
	mv -f ../mvirt*.deb $(DEB_OUT)/
	@echo ""
	@echo "Debian packages built in $(DEB_OUT)/:"
	@ls -1 $(DEB_OUT)/*.deb | xargs -I{} basename {}

deb-clean:
	rm -rf $(DEB_OUT)

install: deb
	sudo dpkg -i $(DEB_OUT)/*.deb
