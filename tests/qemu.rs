//! End-to-end test of the whole stack: client boots a real VM under QEMU
//! (kernel + initrd + rootfs), runs a command in the guest through the
//! server and reports its output and exit code back.
//!
//! Uses `--emulate` (TCG instead of the platform hypervisor) so the test
//! itself can run inside a VM. QEMU is launched with paths relative to the
//! project root, hence `current_dir(PROJECT_ROOT)`.

use std::process::{Command, Stdio};

const CLIENT: &str = env!("CARGO_BIN_EXE_client");
const PROJECT_ROOT: &str = env!("CARGO_MANIFEST_DIR");

#[test]
fn md5sum_in_vm() {
    let mut child = Command::new(CLIENT)
        .args(["--emulate", "--", "/bin/sh", "-c", "echo Hi | md5sum"])
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
