mod common;

use common::{OutputExt, command};
use std::{ffi::OsStr, io::Write, path::Path, process::Output, thread, time::Duration};
use tempdir::TempDir;

const CLIENT: &str = env!("CARGO_BIN_EXE_client");
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

fn run_tty_test(args: &[impl AsRef<OsStr>], stdin_value: Option<&str>) -> Output {
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

    let mut child = command(CLIENT)
        .args(["run", "--serial", "./tty"])
        .current_dir(tmp_dir.path())
        .spawn()
        .unwrap();

    let mut stdin = child.stdin.take().unwrap();
    if let Some(input) = stdin_value {
        stdin.write_all(input.as_bytes()).unwrap();
    }
    // We need to close stdin, so that commands that reads it gets EOF
    drop(stdin);

    let client_out = child.wait_with_output().unwrap();
    client_out.assert_success();
    server_out.join().unwrap().unwrap().assert_success();
    client_out
}

fn wait_for_path(path: impl AsRef<Path>) {
    while !path.as_ref().exists() {
        thread::sleep(Duration::from_millis(10));
    }
}
