# mvirt-zfs Makefile
# Included from root Makefile

MVIRT_ZFS_DIR := mvirt-zfs
MVIRT_ZFS_BIN := $(RUST_TARGET_DIR)/mvirt-zfs

# ============ BUILD ============

$(MVIRT_ZFS_BIN):
	cargo build --release --target $(MUSL_TARGET) -p mvirt-zfs

mvirt-zfs: $(MVIRT_ZFS_BIN)

# ============ CLEAN ============

mvirt-zfs-clean:
	cargo clean -p mvirt-zfs
