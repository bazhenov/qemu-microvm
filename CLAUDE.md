# qemu-microvm

Rust project (`qemu-agent` crate) that runs commands inside a QEMU microVM. Two binaries:
`server` (runs inside/alongside the VM, executes commands) and `vmctl` (boots the VM via
`qemu-system-aarch64` and talks to the server over a serial console).

The vmctl has four subcommands:

- `vmctl init` — initialize a new VM environment: clone the source root filesystem
  image (`--root-fs`) into the data directory (`--data-dir`, default `./.vm`).
- `vmctl run` — spawn QEMU VM and spawns a new shell as child
  processes and report the guest command's exit code. Boots the rootfs from an
  initialized data directory (`--data-dir`, default `./.vm`) or a given image
  directly (`--root-fs`);

## Running tests

Run the full suite from the project root:

```
cargo test
```

The suite has three layers, which can be run separately:

1. **Unit tests** (in `src/lib.rs`, `src/bin/vmctl.rs`, `src/bin/server.rs`) — pure Rust, no external dependencies:
   ```
   cargo test --lib --bins
   ```
2. **CLI integration test** (`tests/cli.rs`) — spawns the `server` and `vmctl` binaries and connects them over a local tty file. No QEMU or VM required:
   ```
   cargo test --test cli
   ```
3. **End-to-end VM tests** (`tests/e2e.rs`) — boot a real VM under QEMU (`--emulate`, i.e. TCG software emulation, so they work inside a VM/CI too) and run commands in the guest:
   ```
   cargo test --test e2e
   ```

### Prerequisites for the end-to-end tests

- `qemu-system-aarch64` on `PATH`.
- A compiled kernel at `./linux/arch/arm64/boot/Image`. Download/configure with `make` (see `Makefile`), then compile with `make -C linux -j$(nproc)` — kernel compilation must be done on Linux.
- `./target/initrd.gz` — built by `make ./target/initrd.gz` (requires the `aarch64-unknown-linux-musl` Rust target and `cpio`).
- `./images/sysfs.qcow2` — the base root filesystem image. Prepare it using `./prepare-scripts/prepare-rootfs.sh`

If the kernel/initrd/rootfs artifacts are missing, only the e2e tests fail; unit and
CLI tests still work.
