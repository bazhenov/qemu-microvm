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

SYSFS_IMAGE=alpine
SYSFS=images/sysfs.qcow2
VMCTL=target/release/client
cargo build --release

# Build a sysfs image ($2) using an existing rootfs ($1) as the build environment.
build_sysfs() {
	local source_fs=$1
	local target_fs=$2

    # Preparing rootfs from a OCI image
	qemu-img create -f qcow2 "$target_fs" 1G
	cat prepare-scripts/prepare-rootfs.sh | $VMCTL run --root-fs "$source_fs" --disk "$target_fs" -- sh -s "$SYSFS_IMAGE"

    # Installing all required dependencies in sysfs
	cat << EOF | $VMCTL run --root-fs "$target_fs" -- sh -s
apk add e2fsprogs podman rsync
EOF
}

# Build a candidate sysfs first, then use the candidate to build the final sysfs,
# proving the new sysfs is able to build sysfs itself.
build_sysfs "$SYSFS" target/sysfs-candidate.qcow2
build_sysfs target/sysfs-candidate.qcow2 target/sysfs.qcow2
rm ./target/sysfs-candidate.qcow2
mv -f target/sysfs.qcow2 "$SYSFS"
