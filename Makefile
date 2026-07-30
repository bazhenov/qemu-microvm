# Downloads a predefined Linux kernel, unpacks it into the `linux/` directory
# and links the kernel .config from kernel-config.aarch64 in this directory.

KERNEL_VERSION := 7.1.3
KERNEL_MAJOR := $(firstword $(subst ., ,$(KERNEL_VERSION)))

CONFIG := kernel-config.aarch64
KERNEL_DIR := linux
TARGET := target

KERNEL_TAR := $(TARGET)/linux-$(KERNEL_VERSION).tar.xz
KERNEL_URL := https://cdn.kernel.org/pub/linux/kernel/v$(KERNEL_MAJOR).x/linux-$(KERNEL_VERSION).tar.xz

ALPINE_VERSION := 3.24.1
ALPINE_BRANCH := v3.24
ALPINE_TAR := $(TARGET)/alpine-minirootfs-$(ALPINE_VERSION)-aarch64.tar.gz
ALPINE_URL := https://dl-cdn.alpinelinux.org/alpine/$(ALPINE_BRANCH)/releases/aarch64/alpine-minirootfs-$(ALPINE_VERSION)-aarch64.tar.gz

INITRD_DIR := $(TARGET)/initrd
INITRD := $(TARGET)/initrd.gz
SERVER_BIN := $(TARGET)/aarch64-unknown-linux-musl/release/server

.PHONY: all clean distclean initrd

all: $(KERNEL_DIR)/.config

$(TARGET):
	mkdir $@

# Download the kernel tarball from kernel.org.
$(KERNEL_TAR): $(TARGET)
	curl -fL -o $@ $(KERNEL_URL)

# Unpack the tarball into the `linux/` directory.
$(KERNEL_DIR): $(KERNEL_TAR)
	rm -rf $@ $@.tmp
	mkdir -p $@.tmp
	tar -xf $(KERNEL_TAR) -C $@.tmp --strip-components=1
	mv $@.tmp $@

# Link the local kernel config as the kernel's .config.
$(KERNEL_DIR)/.config: $(KERNEL_DIR) $(CONFIG)
	ln -sf ../$(CONFIG) $(KERNEL_DIR)/.config
	# make olddefconfig generates a new config setting everything not defined in .config to be default
	$(MAKE) -C $(KERNEL_DIR) olddefconfig

initrd: $(INITRD)

# Download the Alpine minirootfs tarball.
$(ALPINE_TAR): | $(TARGET)
	curl -fL -o $@ $(ALPINE_URL)

# Unpack the Alpine minirootfs into `target/initrd/`.
$(INITRD_DIR): $(ALPINE_TAR)
	rm -rf $@ $@.tmp
	mkdir -p $@.tmp
	tar -xzf $(ALPINE_TAR) -C $@.tmp
	mv $@.tmp $@

# Build the guest server binary. Always runs; cargo does its own change tracking.
.PHONY: $(SERVER_BIN)
$(SERVER_BIN):
	cargo build --release --target=aarch64-unknown-linux-musl

# Assemble the initrd: Alpine minirootfs + init script + server binary.
$(INITRD): $(INITRD_DIR) initrd/init $(SERVER_BIN)
	cp initrd/init $(INITRD_DIR)/init
	cp $(SERVER_BIN) $(INITRD_DIR)/bin/server
	(cd $(INITRD_DIR) && find . -print0 | cpio --null --create --format=newc) | gzip - > $(INITRD)

# Remove the unpacked kernel tree (keeps the downloaded tarball).
clean:
	rm -rf $(KERNEL_DIR) $(KERNEL_DIR).tmp $(INITRD_DIR) $(INITRD_DIR).tmp $(INITRD)

cleanconfig:
	rm -f $(KERNEL_DIR)/.config

# Also remove the downloaded tarballs.
distclean: clean
	rm -f $(KERNEL_TAR) $(ALPINE_TAR)
