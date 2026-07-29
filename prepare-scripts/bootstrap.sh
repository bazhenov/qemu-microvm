#!/usr/bin/env bash
# This script prepares sysroot fs which is used to prepare user rootfs images.
#
# Basically sysroot fs is a minimal rootfs that can do following:
#
# 1. pull docker image from a registry
# 2. format block device with ext4
# 3. copy content of OCI image to a newly formatted block device
#
# Because sysfs is used to prepare sysfs itself we have a bootstrapping problem.
# For those reasons:
#
# 1. sysfs is stored in git, so that we could not loose it
# 2. when building sysfs we do it twice, to make sure that a new candidate sysfs is able to
#    build sysfs itself.

set -exo pipefail

VMCTL=target/release/client
cargo build --release

qemu-img create -f qcow2 sysfs.qcow2 1G
cat prepare-scripts/prepare-rootfs.sh | $VMCTL run --root-fs ./alpine.qcow2 --disk sysfs.qcow2 -- sh -s alpine

cat << EOF | $VMCTL run --root-fs ./sysfs.qcow2 -- sh -s
apk add e2fsprogs podman rsync
EOF
