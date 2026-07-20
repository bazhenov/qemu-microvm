#!/usr/bin/env bash

set -eo pipefail

cp /workspace/kernel-config.aarch64 .
cp /workspace/Makefile .

make linux linux/.config
make -C linux -j$(nproc)
