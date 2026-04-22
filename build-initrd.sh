#!/usr/bin/env bash

set -eo pipefail

orb sh -c "cd init && cargo build --release --target=aarch64-unknown-linux-musl"
cp init/target/aarch64-unknown-linux-musl/release/init ./initrd/init
(cd initrd && find . -print0 | cpio --null --create --format=newc) | gzip - > initrd.gz
