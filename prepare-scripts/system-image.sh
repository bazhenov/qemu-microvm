#!/usr/bin/env bash

# This system image is used to compile all the prerequisites for the project itself. Inlcuding:
# - Linux kernel
# - Compile linux initd-server
# - Exporting of RootFS

set -eo pipefail

export DEBIAN_FRONTEND=noninteractive

apt update -y

# Installing all dependencies for a podman
apt install -y podman

# Installing rust
apt install -y curl
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y

# Installing dependencies require to compile Linux kernel
apt install -y libncurses-dev gawk flex bison openssl libssl-dev dkms libelf-dev libudev-dev libpci-dev libiberty-dev autoconf llvm bc
