#!/usr/bin/env bash

qemu-system-aarch64 \
    -accel hvf -M virt -cpu host -nodefaults -no-user-config -nographic -no-reboot \
    -device virtio-serial-device \
    -chardev stdio,id=virtiocon0 \
    -device virtconsole,chardev=virtiocon0 \
    -drive id=root,file=rootfs.qcow2,format=qcow2,if=none \
    -device virtio-blk-device,drive=root \
    -device virtio-net-device,mac=82:FC:AE:F7:21:BF,netdev=net0 \
    -netdev vmnet-shared,id=net0,start-address=172.16.0.1,end-address=172.31.255.254,subnet-mask=255.240.0.0 \
    -kernel ./Image \
    -append "console=hvc0 reboot=t root=/dev/vda rw panic=-1"
