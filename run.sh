#!/usr/bin/env bash

rm -f ./console

if [[ ! -f ./rootfs-overlay.qcow2 ]]; then
    qemu-img create -o backing_file=rootfs.qcow2,backing_fmt=qcow2 -f qcow2 rootfs-overlay.qcow2
fi

exec qemu-system-aarch64 \
    `# General settings. Using Hypervisor.framework` \
        -accel hvf -cpu host \
    `# General settings. Emulation` \
        `#-cpu cortex-a72` \
        -nodefaults -no-user-config -nographic -no-reboot \
    `# CPU settings` \
        -M virt -smp cpus=1,sockets=1,cores=1,threads=1 -m 512M \
    `# Serial port settings` \
        -device virtio-serial-device \
    `# hvc0 serial device with QEMU monitor in a multiplexed mode` \
        -chardev stdio,signal=off,id=console-hvc0 \
        -device virtconsole,chardev=console-hvc0 \
    `# hvc1 serial device for a ./console pty` \
        -chardev pty,signal=off,path=./console,id=console-hvc1 \
        -device virtserialport,chardev=console-hvc1 \
    `# Root disk drive` \
        -drive id=root,file=rootfs-overlay.qcow2,format=qcow2,if=none \
        -device virtio-blk-device,drive=root \
    `# Network` \
        -device virtio-net-device,netdev=net1 \
        -netdev user,id=net1 \
    `# Realtime Clock. PL031 linux driver is required` \
        -rtc base=utc,clock=host \
    `# RNG support` \
        -device virtio-rng-pci \
    `# VirtIO FS` \
        -virtfs local,path="$PWD",mount_tag=qemu,security_model=mapped \
    `# mount -t 9p -o trans=virtio qemu /mnt -oversion=9p2000.L,msize=512k ` \
    `# Kernel must be compiled with approriate options. See. https://wiki.qemu.org/Documentation/9psetup` \
    `# Linux kernel settings` \
        -kernel ./Image \
        -initrd initrd.gz \
        -append "console=hvc0 reboot=t rdinit=/init panic=-1"
