# mvirt-log Makefile
# Included from root Makefile

MVIRT_LOG_DIR := mvirt-log
MVIRT_LOG_BIN := $(RUST_TARGET_DIR)/mvirt-log

# ============ BUILD ============

$(MVIRT_LOG_BIN):
	cargo build --release --target $(MUSL_TARGET) -p mvirt-log

mvirt-log: $(MVIRT_LOG_BIN)

# ============ CLEAN ============

mvirt-log-clean:
	cargo clean -p mvirt-log
