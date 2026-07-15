# Downloads a predefined Linux kernel, unpacks it into the `linux/` directory
# and links the kernel .config from kernel-config.aarch64 in this directory.

KERNEL_VERSION := 7.1.3
KERNEL_MAJOR := $(firstword $(subst ., ,$(KERNEL_VERSION)))

CONFIG := kernel-config.aarch64
KERNEL_DIR := linux
TARGET := target

KERNEL_TAR := $(TARGET)/linux-$(KERNEL_VERSION).tar.xz
KERNEL_URL := https://cdn.kernel.org/pub/linux/kernel/v$(KERNEL_MAJOR).x/linux-$(KERNEL_VERSION).tar.xz

.PHONY: all clean distclean

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

# Remove the unpacked kernel tree (keeps the downloaded tarball).
clean:
	rm -rf $(KERNEL_DIR) $(KERNEL_DIR).tmp

cleanconfig:
	rm -f $(KERNEL_DIR)/.config

# Also remove the downloaded tarball.
distclean: clean
	rm -f $(KERNEL_TAR)
