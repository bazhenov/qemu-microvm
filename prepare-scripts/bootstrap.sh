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

# Because we don't have fully working vmctl yet, we need to specify initrd and kernel explicitly
BOOTSTRAP_ARGS=(--initrd target/initrd.gz --kernel ./linux/arch/arm64/boot/Image)

if [[ "$OSTYPE" == "linux-gnu"* ]]; then
    EMULATE="--emulate"
fi

SYSFS_IMAGE=alpine
SYSFS=images/sysfs.qcow2
VMCTL=target/release/vmctl
ALPINE_URL=https://dl-cdn.alpinelinux.org/alpine/v3.24/releases/aarch64/alpine-minirootfs-3.24.1-aarch64.tar.gz

cargo build --release

# Preparing rootfs from a OCI image
qemu-img create -f qcow2 "$SYSFS" 1G

# Installing Alpine minirootfs on the new image
cat << EOF | $VMCTL run $EMULATE --recovery --root-fs "$SYSFS" "${BOOTSTRAP_ARGS[@]}" -- sh -sexo pipefail
# There is no DNS in recovery mode and there is no intent to support it fully, because
# it might negatively impact resilence of the recovery mode. So we need to enable it in ad hoc manner
echo "nameserver 8.8.8.8" > /etc/resolv.conf

apk add e2fsprogs
mkfs.ext4 /dev/vda
mkdir -p /rootfs
mount /dev/vda /rootfs

wget -qO- https://dl-cdn.alpinelinux.org/alpine/v3.24/releases/aarch64/alpine-minirootfs-3.24.1-aarch64.tar.gz | gunzip | tar xf - -C /rootfs
sync
EOF

# Installing all required dependencies in a sysfs
cat << EOF | $VMCTL run $EMULATE --root-fs "$SYSFS" "${BOOTSTRAP_ARGS[@]}" -- sh -sexo pipefail
apk add e2fsprogs podman rsync
EOF
