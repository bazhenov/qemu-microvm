#!/usr/bin/env bash

export PATH="/usr/local/bin:/usr/local/sbin:/usr/bin:/usr/sbin:/bin:/sbin"

mount_check() {
    local source="$1"
    local target="$2"
    local filesystemtype="$3"

    if [ ! -d "$target" ]; then
        echo "Creating $target"
        if ! mkdir -p "$target" 2>/dev/null; then
            echo "Creating directory failed" >&2
            exit 1
        fi
    fi

    echo "Mounting $target"
    if ! mount -t "$filesystemtype" "$source" "$target" 2>/dev/null; then
        echo "Mount failed" >&2
        exit 1
    fi
}

mount_check "none" "/proc" "proc"
mount_check "none" "/dev/pts" "devpts"
mount_check "none" "/dev/mqueue" "mqueue"
mount_check "none" "/dev/shm" "tmpfs"
mount_check "none" "/sys" "sysfs"
mount_check "none" "/sys/fs/cgroup" "cgroup"

hostname "microvm"

# Configuring network
ifconfig eth0 172.16.0.100 netmask 255.240.0.0
route add default gw 172.16.0.1
ifconfig eth0 up

/bin/bash
