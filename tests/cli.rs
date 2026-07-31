mod common;

use common::{OutputExt, command};
use std::{ffi::OsStr, io::Write, path::Path, process::Output, thread, time::Duration};
use tempdir::TempDir;

const VMCTL: &str = env!("CARGO_BIN_EXE_vmctl");
const SERVER: &str = env!("CARGO_BIN_EXE_server");

#[test]
fn md5sum() {
    run_tty_test(&["/bin/bash", "-c", "echo Hi | md5sum"], None)
        .assert_success()
        .assert_stdout_contains("31ebdfce8b77ac49d7f5506dd1495830");
}

#[test]
fn redirecting_stderr() {
    run_tty_test(&["/bin/bash", "-c", "echo Hi | md5sum >&2"], None)
        .assert_success()
        .assert_stderr_contains("31ebdfce8b77ac49d7f5506dd1495830");
}

#[test]
fn redirecting_stdin() {
    run_tty_test(&["/bin/bash", "-c", "md5sum"], Some("Hi\n"))
        .assert_success()
        .assert_stdout_contains("31ebdfce8b77ac49d7f5506dd1495830");
}

/// `--data-dir` and `--root-fs` are two mutually exclusive ways to point
/// `run` at a root filesystem.
#[test]
fn run_rejects_data_dir_with_root_fs() {
    command(VMCTL)
        .args(["run", "--data-dir", "vm", "--root-fs", "rootfs.qcow2"])
        .output()
        .unwrap()
        .assert_failure()
        .assert_stderr_contains("cannot be used with");
}

/// `shell` attaches to the tty directly, without the `run` orchestration.
#[test]
fn shell_subcommand() {
    run_tty_test_via(
        &["shell", "--serial", "./tty"],
        &["/bin/bash", "-c", "echo Hi | md5sum"],
        None,
    )
    .assert_success()
    .assert_stdout_contains("31ebdfce8b77ac49d7f5506dd1495830");
}

fn run_tty_test(args: &[impl AsRef<OsStr>], stdin_value: Option<&str>) -> Output {
    run_tty_test_via(&["shell", "--serial", "./tty"], args, stdin_value)
}

/// Spawn the server on a tty in a temp directory and connect the client to it
/// with the given client arguments.
fn run_tty_test_via(
    vmctl_args: &[&str],
    args: &[impl AsRef<OsStr>],
    stdin_value: Option<&str>,
) -> Output {
    let tmp_dir = TempDir::new("example").unwrap();
    let path = tmp_dir.path().to_path_buf();

    let args = args
        .iter()
        .map(|s| s.as_ref().to_os_string())
        .collect::<Vec<_>>();

    let server_out = thread::spawn(|| {
        command(SERVER)
            .arg("--")
            .args(args)
            .current_dir(path)
            .output()
    });

    wait_for_path(tmp_dir.path().join("tty"));

    let mut child = command(VMCTL)
        .args(vmctl_args)
        .current_dir(tmp_dir.path())
        .spawn()
        .unwrap();

    let mut stdin = child.stdin.take().unwrap();
    if let Some(input) = stdin_value {
        stdin.write_all(input.as_bytes()).unwrap();
    }
    // We need to close stdin, so that commands that reads it gets EOF
    drop(stdin);

    let vmctl_out = child.wait_with_output().unwrap();
    vmctl_out.assert_success();
    server_out.join().unwrap().unwrap().assert_success();
    vmctl_out
}

fn wait_for_path(path: impl AsRef<Path>) {
    while !path.as_ref().exists() {
        thread::sleep(Duration::from_millis(10));
    }
}
