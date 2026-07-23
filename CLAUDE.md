# qemu-microvm

Rust project (`qemu-agent` crate) that runs commands inside a QEMU microVM. Two binaries:
`server` (runs inside/alongside the VM, executes commands) and `client` (boots the VM via
`qemu-system-aarch64` and talks to the server over a serial console).

The client has two subcommands:

- `client init` — initialize a new VM environment: clone the source root filesystem
  image (`--root-fs`, default `rootfs.qcow2`) into the data directory (`--data-dir`,
  default `./.vm`).
- `client run` — boot a VM from an already initialized data directory (`--data-dir`,
  default `./.vm`) and run a command in the guest.

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
3. **End-to-end VM tests** (`tests/e2e.rs`) — boot a real VM under QEMU (`--emulate`, i.e. TCG software emulation, so they work inside a VM/CI too) and run commands in the guest:
   ```
   cargo test --test e2e
   ```

### Prerequisites for the end-to-end tests

- `qemu-system-aarch64` on `PATH`.
- A compiled kernel at `./linux/arch/arm64/boot/Image`. Download/configure with `make` (see `Makefile`), then compile with `make -C linux -j$(nproc)` — kernel compilation must be done on Linux.
- `./initrd.gz` — built by `./build-initrd.sh` (requires the `aarch64-unknown-linux-musl` Rust target and `cpio`).
- `./rootfs.qcow2` — the base root filesystem image. Preparing it is the user's
  responsibility; `client init` clones it into the data directory (`fs::copy`, which
  is an APFS `clonefile` on macOS and a plain copy elsewhere) and `client run` boots
  the clone directly (read-write, no overlay). Each e2e test initializes its own VM
  environment in a temp dir via `client init`, so the base image stays pristine.

The e2e tests launch QEMU with paths relative to the project root (the tests set
`current_dir` themselves). VM boots under emulation are CPU-heavy, so the tests
serialize through a mutex — expect them to run one at a time and take a while
(each boots a full VM under emulation).

If the kernel/initrd/rootfs artifacts are missing, only the e2e tests fail; unit and
CLI tests still work.
