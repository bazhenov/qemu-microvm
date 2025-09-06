#!/usr/bin/env bash

rm -f ./console

exec qemu-system-aarch64 \
    `# General settings` \
    -accel hvf -M virt \
    -nodefaults -no-user-config -nographic \
    `# CPU settings` \
    -cpu host -smp cpus=1,sockets=1,cores=1,threads=1 \
    `# Serial port settings` \
    -device virtio-serial-device \
    -chardev stdio,signal=off,mux=on,id=virtiocon0 \
    -device virtconsole,chardev=virtiocon0 \
    -chardev pty,path=./console,id=pty1 \
    -device virtconsole,chardev=pty1 \
    -mon chardev=virtiocon0,mode=readline \
    `# Disk drive settings` \
    -drive id=root,file=rootfs.qcow2,format=qcow2,if=none \
    -device virtio-blk-device,drive=root \
    `# Network settings` \
    -device virtio-net-device,netdev=net1 \
    -netdev user,id=net1 \
    `# Realtime Clock settings settings. PL031 linux driver is required` \
    -rtc base=utc,clock=host \
    `# RNG support` \
    -device virtio-rng-pci \
    `# Linux kernel settings` \
    -kernel ./Image \
    -append "console=hvc0 reboot=t root=/dev/vda rw panic=-1"
