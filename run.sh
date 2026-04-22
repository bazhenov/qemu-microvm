#!/usr/bin/env bash

rm -f ./console

exec qemu-system-aarch64 \
    `# General settings. Using Hypervisor.framework` \
        -accel hvf -M virt \
        -nodefaults -no-user-config -nographic -no-reboot \
    `# CPU settings` \
        -cpu host -smp cpus=1,sockets=1,cores=1,threads=1 \
    `# Serial port settings` \
        -device virtio-serial-device \
    `# hvc0 serial device with QEMU monitor in a multiplexed mode` \
        -chardev stdio,signal=off,mux=on,id=console-hvc0 \
        -device virtconsole,chardev=console-hvc0 \
        -mon chardev=console-hvc0,mode=readline \
    `# hvc1 serial device for a ./console pty` \
        -chardev pty,path=./console,id=console-hvc1 \
        -device virtconsole,chardev=console-hvc1 \
    `# Root disk drive` \
        -drive id=root,file=rootfs.qcow2,format=qcow2,if=none \
        -device virtio-blk-device,drive=root \
    `# Network` \
        -device virtio-net-device,netdev=net1 \
        -netdev user,id=net1 \
    `# Realtime Clock. PL031 linux driver is required` \
        -rtc base=utc,clock=host \
    `# RNG support` \
        -device virtio-rng-pci \
    `# VirtIO FS` \
        -virtfs local,path=/Users/bazhenov/Developer/qemu-microvm,mount_tag=qemu,security_model=mapped \
    `# mount -t 9p -o trans=virtio qemu /mnt -oversion=9p2000.L ` \
    `# Kernel must be compiled with approriate options. See. https://wiki.qemu.org/Documentation/9psetup` \
    `# Linux kernel settings` \
        -kernel ./Image \
        -initrd initrd.gz \
        -append "console=hvc0 console=hvc1 reboot=t root=/dev/vda rdinit=/init panic=-1"
