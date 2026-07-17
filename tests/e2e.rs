//! End-to-end test of the whole stack: client boots a real VM under QEMU
//! (kernel + initrd + rootfs), runs a command in the guest through the
//! server and reports its output and exit code back.
//!
//! Uses `--emulate` (TCG instead of the platform hypervisor) so the test
//! itself can run inside a VM. QEMU is launched with paths relative to the
//! project root, hence `current_dir(PROJECT_ROOT)`.

use std::{
    fs::{self, File},
    path::Path,
    process::{Command, Stdio},
    sync::Mutex,
};
use tempdir::TempDir;

const CLIENT: &str = env!("CARGO_BIN_EXE_client");
const PROJECT_ROOT: &str = env!("CARGO_MANIFEST_DIR");

#[test]
fn md5sum_in_vm() {
    let tmp = TempDir::new("rootfs").unwrap();
    let root_fs = tmp.path().join("rootfs.qcow2");
    clone_rootfs(&root_fs);

    let mut child = Command::new(CLIENT)
        .arg("--emulate")
        .arg("--root-fs")
        .arg(&root_fs)
        .args(["--", "/bin/sh", "-c", "echo Hi | md5sum"])
        .current_dir(PROJECT_ROOT)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // We need to hold input until end of the test
    let _stdin = child.stdin.take().unwrap();

    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "Expected exit code 0, got {:?}\nstdout: {}\nstderr: {}",
        output.status,
        stdout,
        stderr,
    );
    assert!(
        stdout.contains("31ebdfce8b77ac49d7f5506dd1495830"),
        "Expected stdout to contain md5 of 'Hi'\nstdout: {}\nstderr: {}",
        stdout,
        stderr,
    );
}

#[test]
fn additional_disk_in_vm() {
    let tmp = TempDir::new("additional-disk").unwrap();
    let root_fs = tmp.path().join("rootfs.qcow2");
    clone_rootfs(&root_fs);
    let disk = tmp.path().join("data.img");
    create_new_disk(&disk, 4 * 1024 * 1024);

    let mut child = Command::new(CLIENT)
        .arg("--emulate")
        .arg("--root-fs")
        .arg(&root_fs)
        .arg("--disk")
        .arg(&disk)
        // Root disk is /dev/vda, so the additional disk appears as /dev/vdb.
        .args(["--", "/bin/sh", "-c", "cat /sys/block/vdb/size"])
        .current_dir(PROJECT_ROOT)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // We need to hold input until end of the test
    let _stdin = child.stdin.take().unwrap();

    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "Expected exit code 0, got {:?}\nstdout: {}\nstderr: {}",
        output.status,
        stdout,
        stderr,
    );
    assert!(
        // Disk size is reported in 512-byte sectors: 4M / 512 = 8192.
        stdout.contains("8192"),
        "Expected additional disk /dev/vdb of 8192 sectors in the guest\nstdout: {}\nstderr: {}",
        stdout,
        stderr,
    );
}

fn clone_rootfs(dst: &Path) {
    let src = Path::new(PROJECT_ROOT).join("rootfs.qcow2");
    if cfg!(target_os = "macos") {
        // Internally copy() uses clonefile on macOS APFS which is cheap COW clone
        fs::copy(&src, dst).unwrap();
    } else {
        panic!("Not implemented on non macOS systems")
    }
}

/// Create an empty (sparse) raw disk image of the given size in bytes.
fn create_new_disk(disk: &Path, size: u64) {
    let file = File::create(disk).unwrap();
    file.set_len(size).unwrap();
}
