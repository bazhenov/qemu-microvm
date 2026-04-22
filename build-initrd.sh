#!/usr/bin/env bash

set -eo pipefail

#orb sh -c "cd init && cargo build --release --target=aarch64-unknown-linux-musl"
#cp init/target/aarch64-unknown-linux-musl/release/init ./initrd/init
cp rootfs2/init rootfs2/sysroot/init
mkdir -p rootfs2/sysroot/proc
mkdir -p rootfs2/sysroot/sys
(cd rootfs2/sysroot && find . -print0 | cpio --null --create --format=newc) | gzip - > initrd.gz
