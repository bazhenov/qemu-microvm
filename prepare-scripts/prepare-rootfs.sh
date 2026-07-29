#!/usr/bin/env bash
set -euxo pipefail

DISK="/dev/vdb"
DISK_MOUNT="/rootfs"
DOCKER_IMAGE="$1"

echo "Preparing rootfs from Docker image: $DOCKER_IMAGE"

mkfs.ext4 "$DISK"

mkdir -p $DISK_MOUNT
mount "$DISK" "$DISK_MOUNT"

podman pull "$DOCKER_IMAGE"
IMAGE_MOUNT=$(podman image mount "$DOCKER_IMAGE")

# Notice end slash on IMAGE_MOUNT. It's important for rsync not to create source directory in the root fs itself
rsync -a --numeric-ids "$IMAGE_MOUNT/" "$DISK_MOUNT"
