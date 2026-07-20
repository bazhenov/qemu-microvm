# qemu-microvm

Rust project (`qemu-agent` crate) that runs commands inside a QEMU microVM. Two binaries:
`server` (runs inside/alongside the VM, executes commands) and `client` (boots the VM via
`qemu-system-aarch64` and talks to the server over a serial console).

## Running tests

Run the full suite from the project root:

```
cargo test
```

The suite has three layers, which can be run separately:

1. **Unit tests** (in `src/lib.rs`, `src/bin/client.rs`, `src/bin/server.rs`) — pure Rust, no external dependencies:
   ```
   cargo test --lib --bins
   ```
2. **CLI integration test** (`tests/cli.rs`) — spawns the `server` and `client` binaries and connects them over a local tty file. No QEMU or VM required:
   ```
   cargo test --test cli
   ```
3. **End-to-end VM tests** (`tests/qemu.rs`) — boot a real VM under QEMU (`--emulate`, i.e. TCG software emulation, so they work inside a VM/CI too) and run commands in the guest:
   ```
   cargo test --test qemu
   ```

### Prerequisites for the end-to-end tests

- `qemu-system-aarch64` on `PATH`.
- A compiled kernel at `./linux/arch/arm64/boot/Image`. Download/configure with `make` (see `Makefile`), then compile with `make -C linux -j$(nproc)` — kernel compilation must be done on Linux.
- `./initrd.gz` — built by `./build-initrd.sh` (requires the `aarch64-unknown-linux-musl` Rust target and `cpio`).
- `./rootfs.qcow2` — the base root filesystem image. Preparing it is the user's
  responsibility; the client boots the disk passed via `--root-fs` directly
  (read-write, no overlay). Each e2e test clones the base image into a temp dir
  (APFS `clonefile` on macOS, plain copy elsewhere), so the base stays pristine.

The e2e tests launch QEMU with paths relative to the project root (the tests set
`current_dir` themselves). VM boots under emulation are CPU-heavy, so the tests
serialize through a mutex — expect them to run one at a time and take a while
(each boots a full VM under emulation).

If the kernel/initrd/rootfs artifacts are missing, only the e2e tests fail; unit and
CLI tests still work.
