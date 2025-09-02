#!/usr/bin/env bash

qemu-system-aarch64 \
    `# General settings` \
    -accel hvf -M virt \
    -nodefaults -no-user-config -nographic -no-reboot \
    `# CPU settings` \
    -cpu host -smp cpus=1,sockets=1,cores=1,threads=1 \
    `# Serial port settings` \
    -device virtio-serial-device \
    -chardev stdio,id=virtiocon0 \
    -chardev socket,path=./console,server=on,wait=off,id=pty1 \
    -device virtconsole,chardev=virtiocon0 \
    -device virtconsole,chardev=pty1 \
    `# Disk drive settings` \
    -drive id=root,file=rootfs.qcow2,format=qcow2,if=none \
    -device virtio-blk-device,drive=root \
    `# Network settings` \
    -device virtio-net-device,netdev=net1 \
    -netdev vmnet-shared,id=net1,start-address=172.16.0.1,end-address=172.31.255.254,subnet-mask=255.240.0.0 \
    `# Realtime Clock settings settings. PL031 linux driver is required` \
    -rtc base=utc,clock=host \
    `# RNG support` \
    -device virtio-rng-pci \
    `# Linux kernel settings` \
    -kernel ./Image \
    -append "console=hvc0 reboot=t root=/dev/vda rw panic=-1"
