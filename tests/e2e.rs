//! End-to-end test of the whole stack: client boots a real VM under QEMU
//! (kernel + initrd + rootfs), runs a command in the guest through the
//! server and reports its output and exit code back.
//!
//! Each test initializes a private VM environment with `vmctl init` (which
//! clones the base `images/sysfs.qcow2` into a temp data directory) and boots it
//! with `vmctl run`.
//!
//! Uses `--emulate` (TCG instead of the platform hypervisor) so the test
//! itself can run inside a VM. QEMU is launched with paths relative to the
//! project root, hence `current_dir(PROJECT_ROOT)`.

mod common;

use common::{OutputExt, wait_with_timeout};
use std::{
    fs::File,
    io::Write,
    path::Path,
    process::{Command, Output},
};
use tempdir::TempDir;

const VMCTL: &str = env!("CARGO_BIN_EXE_vmctl");
const PROJECT_ROOT: &str = env!("CARGO_MANIFEST_DIR");

#[test]
fn init_and_run() {
    let tmp = TempDir::new("vm-env").unwrap();
    let data_dir = tmp.path().join("vm");

    // `run` on an uninitialized data directory must fail without booting a VM
    run_in_vm(&data_dir, &[], None, &["/bin/sh", "-c", "true"])
        .assert_failure()
        .assert_stderr_contains("not initialized");

    // `init` clones the base rootfs into the data directory
    init_env(&data_dir);
    assert!(data_dir.join("rootfs.qcow2").is_file());

    // repeated `init` on the same data directory must fail
    vmctl()
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
        None,
        &["/bin/sh", "-c", "echo running in $(hostname)"],
    )
    .assert_success()
    .assert_stdout_match("running in sandbox\n");
}

/// `run --root-fs` boots the given image directly, without a data directory.
#[test]
fn run_with_explicit_root_fs() {
    let tmp = TempDir::new("vm-env").unwrap();
    let data_dir = tmp.path().join("vm");
    init_env(&data_dir);

    let child = vmctl()
        .args(["run", "--emulate", "--root-fs"])
        .arg(data_dir.join("rootfs.qcow2"))
        .args(["--", "/bin/sh", "-c", "echo running in $(hostname)"])
        .spawn()
        .unwrap();
    wait_with_timeout(child, "qemu")
        .assert_success()
        .assert_stdout_match("running in sandbox\n");
}

/// If QEMU fails for whatever reason, stderr output should contain mention of --boot-log option
#[test]
fn boot_log_mention() {
    let tmp_dir = TempDir::new("vm-env").unwrap();
    let child = vmctl()
        .current_dir(tmp_dir.path())
        .args([
            "run",
            "--emulate",
            "--root-fs",
            "./not-existent-root-fs.qcow2",
        ])
        .spawn()
        .unwrap();
    wait_with_timeout(child, "qemu")
        .assert_failure()
        .assert_stderr_contains("--boot-log");
}

#[test]
fn md5sum_in_vm() {
    let tmp = TempDir::new("vm-env").unwrap();
    let data_dir = tmp.path().join("vm");
    init_env(&data_dir);

    run_in_vm(&data_dir, &[], None, &["/bin/sh", "-c", "echo Hi | md5sum"])
        .assert_success()
        .assert_stdout_contains("31ebdfce8b77ac49d7f5506dd1495830");
}

#[test]
fn using_stdin() {
    let tmp = TempDir::new("vm-env").unwrap();
    let data_dir = tmp.path().join("vm");
    init_env(&data_dir);

    run_in_vm(&data_dir, &[], Some("Hi\n"), &["/bin/sh", "-c", "md5sum"])
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
        None,
        &["/bin/sh", "-c", "cat /sys/block/vdb/size"],
    )
    .assert_success()
    // Disk size is reported in 512-byte sectors: 4M / 512 = 8192.
    .assert_stdout_contains("8192");
}

/// The server builds the session environment itself (SSH-like): identity
/// from the rootfs /etc/passwd, default PATH, and the command starts in the
/// user's home directory.
#[test]
fn session_environment() {
    let tmp = TempDir::new("vm-env").unwrap();
    let data_dir = tmp.path().join("vm");
    init_env(&data_dir);

    run_in_vm(
        &data_dir,
        &[],
        None,
        &["/bin/sh", "-c", "echo $USER,$HOME,$PATH; pwd"],
    )
    .assert_success()
    .assert_stdout_contains("root,/root,/usr/local/sbin:")
    .assert_stdout_contains("\n/root\n");
}

#[test]
fn should_respect_default_path_in_noninteractive_mode() {
    let tmp = TempDir::new("vm-env").unwrap();
    let data_dir = tmp.path().join("vm");
    init_env(&data_dir);

    // Running env (not /usr/bin/env) to check if PATH env variable is respected
    run_in_vm(&data_dir, &[], None, &["env"])
        .assert_success()
        .assert_stdout_contains("USER=root");
}

/// L3 connectivity to the gvproxy virtual switch (192.168.127.1 is the only
/// address it answers ICMP echo on). Runs in recovery mode, so only the
/// initrd network setup is exercised (no rootfs mounts). Does not need
/// internet access on the host.
#[test]
fn gateway_l3_connectivity() {
    let tmp = TempDir::new("vm-env").unwrap();
    let data_dir = tmp.path().join("vm");
    init_env(&data_dir);

    run_in_recovery_vm(&data_dir, &["ping", "-c1", "-W2", "192.168.127.1"])
        .assert_success()
        .assert_stdout_contains("1 packets received");
}

/// L3 connectivity to the outside world through gvproxy. The `internet_`
/// prefix marks tests that legitimately fail on a machine without internet
/// access.
#[test]
fn internet_l3_connectivity() {
    let tmp = TempDir::new("vm-env").unwrap();
    let data_dir = tmp.path().join("vm");
    init_env(&data_dir);

    run_in_recovery_vm(&data_dir, &["/bin/sh", "-c", "ping -c1 -W2 1.1.1.1"])
        .assert_success()
        .assert_stdout_contains("1 packets received");
}

/// DNS resolution through gvproxy's built-in resolver. The `internet_` prefix marks
/// tests that legitimately fail on a machine without internet access.
#[test]
fn internet_dns_resolution() {
    let tmp = TempDir::new("vm-env").unwrap();
    let data_dir = tmp.path().join("vm");
    init_env(&data_dir);

    run_in_vm(&data_dir, &[], None, &["nslookup", "example.com"])
        .assert_success()
        .assert_stdout_contains("Name:{..}example.com");
}

#[test]
fn propagate_exit_status() {
    let tmp = TempDir::new("vm-env").unwrap();
    let data_dir = tmp.path().join("vm");
    init_env(&data_dir);

    let out = run_in_vm(&data_dir, &[], None, &["/bin/false"]);
    assert_eq!(out.status.code(), Some(1));
}

fn vmctl() -> Command {
    let mut command = common::command(VMCTL);
    command.current_dir(PROJECT_ROOT);
    command
}

fn init_env(data_dir: &Path) {
    vmctl()
        .arg("init")
        .arg("--data-dir")
        .arg(data_dir)
        .args(["--root-fs", "images/sysfs.qcow2"])
        .output()
        .unwrap()
        .assert_success();
}

/// Run `cmd` in a VM booted from the `data_dir` environment and return its
/// output.
fn run_in_vm(data_dir: &Path, disks: &[&Path], stdin_value: Option<&str>, cmd: &[&str]) -> Output {
    run_in_vm_impl(data_dir, disks, stdin_value, false, cmd)
}

/// Same as [`run_in_vm`], but the guest init stays in recovery mode: the
/// command runs straight from the initrd, the rootfs is never mounted.
fn run_in_recovery_vm(data_dir: &Path, cmd: &[&str]) -> Output {
    run_in_vm_impl(data_dir, &[], None, true, cmd)
}

fn run_in_vm_impl(
    data_dir: &Path,
    disks: &[&Path],
    stdin_value: Option<&str>,
    recovery: bool,
    cmd: &[&str],
) -> Output {
    let mut vmctl_cmd = vmctl();
    vmctl_cmd
        .arg("run")
        .arg("--emulate")
        .arg("--data-dir")
        .arg(data_dir);
    if recovery {
        vmctl_cmd.arg("--recovery");
    }
    for disk in disks {
        vmctl_cmd.arg("--disk").arg(disk);
    }
    let mut child = vmctl_cmd.arg("--").args(cmd).spawn().unwrap();

    let mut stdin = child.stdin.take().unwrap();
    if let Some(stdin_value) = stdin_value {
        let _ = stdin.write_all(stdin_value.as_bytes());
    }
    // Close the pipe, otherwise the vmctl never sees stdin EOF
    drop(stdin);

    wait_with_timeout(child, "qemu")
}

/// Create an empty (sparse) raw disk image of the given size in bytes.
fn create_new_disk(disk: &Path, size: u64) {
    let file = File::create(disk).unwrap();
    file.set_len(size).unwrap();
}
