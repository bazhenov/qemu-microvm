//! End-to-end test of the whole stack: client boots a real VM under QEMU
//! (kernel + initrd + rootfs), runs a command in the guest through the
//! server and reports its output and exit code back.
//!
//! Each test initializes a private VM environment with `client init` (which
//! clones the base `rootfs.qcow2` into a temp data directory) and boots it
//! with `client run`.
//!
//! Uses `--emulate` (TCG instead of the platform hypervisor) so the test
//! itself can run inside a VM. QEMU is launched with paths relative to the
//! project root, hence `current_dir(PROJECT_ROOT)`.

mod common;

use common::OutputExt;
use std::{
    fs::File,
    path::Path,
    process::{Command, Output},
};
use tempdir::TempDir;

const CLIENT: &str = env!("CARGO_BIN_EXE_client");
const PROJECT_ROOT: &str = env!("CARGO_MANIFEST_DIR");

#[test]
fn init_and_run() {
    let tmp = TempDir::new("vm-env").unwrap();
    let data_dir = tmp.path().join("vm");

    // `run` on an uninitialized data directory must fail without booting a VM
    run_in_vm(&data_dir, &[], &["/bin/sh", "-c", "true"])
        .assert_failure()
        .assert_stderr_contains("not initialized");

    // `init` clones the base rootfs into the data directory
    init_env(&data_dir);
    assert!(data_dir.join("rootfs.qcow2").is_file());

    // repeated `init` on the same data directory must fail
    client()
        .arg("init")
        .arg("--data-dir")
        .arg(&data_dir)
        .output()
        .unwrap()
        .assert_failure();

    // `run` boots a VM from the initialized environment
    run_in_vm(
        &data_dir,
        &[],
        &["/bin/sh", "-c", "echo running in $(hostname)"],
    )
    .assert_success()
    .assert_stdout_match("running in sandbox\n");
}

#[test]
fn md5sum_in_vm() {
    let tmp = TempDir::new("vm-env").unwrap();
    let data_dir = tmp.path().join("vm");
    init_env(&data_dir);

    run_in_vm(&data_dir, &[], &["/bin/sh", "-c", "echo Hi | md5sum"])
        .assert_success()
        .assert_stdout_contains("31ebdfce8b77ac49d7f5506dd1495830");
}

#[test]
fn additional_disk_in_vm() {
    let tmp = TempDir::new("vm-env").unwrap();
    let data_dir = tmp.path().join("vm");
    init_env(&data_dir);
    let disk = tmp.path().join("data.img");
    create_new_disk(&disk, 4 * 1024 * 1024);

    // Root disk is /dev/vda, so the additional disk appears as /dev/vdb.
    run_in_vm(
        &data_dir,
        &[disk.as_path()],
        &["/bin/sh", "-c", "cat /sys/block/vdb/size"],
    )
    .assert_success()
    // Disk size is reported in 512-byte sectors: 4M / 512 = 8192.
    .assert_stdout_contains("8192");
}

/// `client` command with the project root as the working directory, so the
/// default `--root-fs rootfs.qcow2` and the kernel/initrd paths resolve.
fn client() -> Command {
    let mut command = common::command(CLIENT);
    command.current_dir(PROJECT_ROOT);
    command
}

/// Initialize a VM environment in `data_dir` from the base `rootfs.qcow2`.
fn init_env(data_dir: &Path) {
    client()
        .arg("init")
        .arg("--data-dir")
        .arg(data_dir)
        .output()
        .unwrap()
        .assert_success();
}

/// Run `cmd` in a VM booted from the `data_dir` environment and return its
/// output.
fn run_in_vm(data_dir: &Path, disks: &[&Path], cmd: &[&str]) -> Output {
    let mut command = client();
    command
        .arg("run")
        .arg("--emulate")
        .arg("--data-dir")
        .arg(data_dir);
    for disk in disks {
        command.arg("--disk").arg(disk);
    }
    let mut child = command.arg("--").args(cmd).spawn().unwrap();
    // We need to hold input until end of the test
    let _stdin = child.stdin.take().unwrap();
    child.wait_with_output().unwrap()
}

/// Create an empty (sparse) raw disk image of the given size in bytes.
fn create_new_disk(disk: &Path, size: u64) {
    let file = File::create(disk).unwrap();
    file.set_len(size).unwrap();
}
