# mvirt-vmm Makefile
# Included from root Makefile

VMM_DIR := mvirt-vmm
VMM_TARGET := $(VMM_DIR)/target

# Versionen (sync mit mvirt-os.mk)
CH_VERSION := v50.0
FW_VERSION := ch-a54f262b09

# Downloads
CH_URL := https://github.com/cloud-hypervisor/cloud-hypervisor/releases/download/$(CH_VERSION)/cloud-hypervisor-static
FW_URL := https://github.com/cloud-hypervisor/edk2/releases/download/$(FW_VERSION)/CLOUDHV.fd

VMM_CH_BIN := $(VMM_TARGET)/cloud-hypervisor
VMM_FW_BIN := $(VMM_TARGET)/CLOUDHV.fd

# ============ DOWNLOAD TARGETS ============

$(VMM_TARGET):
	mkdir -p $(VMM_TARGET)

$(VMM_CH_BIN): | $(VMM_TARGET)
	curl -L -o $(VMM_CH_BIN) $(CH_URL)
	chmod +x $(VMM_CH_BIN)

$(VMM_FW_BIN): | $(VMM_TARGET)
	curl -L -o $(VMM_FW_BIN) $(FW_URL)

vmm-deps: $(VMM_CH_BIN) $(VMM_FW_BIN)

# ============ CLEAN ============

vmm-clean:
	rm -f $(VMM_CH_BIN) $(VMM_FW_BIN)

.PHONY: vmm-deps vmm-clean
