#!/usr/bin/env bash

set -eo pipefail

#orb sh -c "cd init && cargo build --release --target=aarch64-unknown-linux-musl"
#cp init/target/aarch64-unknown-linux-musl/release/init ./initrd/init
cp rootfs/init rootfs/sysroot/init
cargo build --release --target=aarch64-unknown-linux-musl
cp target/aarch64-unknown-linux-musl/release/server rootfs/sysroot/bin/server
(cd rootfs/sysroot && find . -print0 | cpio --null --create --format=newc) | gzip - > initrd.gz
