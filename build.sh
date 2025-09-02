#!/usr/bin/env bash
set -e
docker build --output "type=tar,dest=rootfs.tar" rootfs
echo "Building rootfs..."
orb sudo virt-make-fs --format=qcow2 --size=+200M rootfs.tar rootfs.qcow2
