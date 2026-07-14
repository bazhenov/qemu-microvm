# Downloads a predefined Linux kernel, unpacks it into the `linux/` directory
# and links the kernel .config from linux-kernlel-config in this directory.

KERNEL_VERSION := 5.12.10
KERNEL_MAJOR   := $(firstword $(subst ., ,$(KERNEL_VERSION)))

TARBALL     := linux-$(KERNEL_VERSION).tar.xz
KERNEL_URL  := https://cdn.kernel.org/pub/linux/kernel/v$(KERNEL_MAJOR).x/$(TARBALL)

CONFIG      := kernel-config.aarch64
LINUX_DIR   := linux

.PHONY: all clean distclean

all: $(LINUX_DIR)/.config

# Download the kernel tarball from kernel.org.
$(TARBALL):
	curl -fL -o $@ $(KERNEL_URL)

# Unpack the tarball into the `linux/` directory.
$(LINUX_DIR): $(TARBALL)
	rm -rf $@ $@.tmp
	mkdir -p $@.tmp
	tar -xf $(TARBALL) -C $@.tmp --strip-components=1
	mv $@.tmp $@

# Link the local kernel config as the kernel's .config.
$(LINUX_DIR)/.config: $(LINUX_DIR) $(CONFIG)
	ln -sf ../$(CONFIG) $@

# Remove the unpacked kernel tree (keeps the downloaded tarball).
clean:
	rm -rf $(LINUX_DIR) $(LINUX_DIR).tmp

# Also remove the downloaded tarball.
distclean: clean
	rm -f $(TARBALL)
