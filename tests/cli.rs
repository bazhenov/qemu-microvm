mod common;

use common::{command, OutputExt};
use std::{ffi::OsStr, path::Path, process::Output, thread, time::Duration};
use tempdir::TempDir;

const CLIENT: &str = env!("CARGO_BIN_EXE_client");
const SERVER: &str = env!("CARGO_BIN_EXE_server");

#[test]
fn md5sum() {
    run_tty_test(&["/bin/bash", "-c", "echo Hi | md5sum"])
        .assert_success()
        .assert_stdout_contains("31ebdfce8b77ac49d7f5506dd1495830");
}

fn run_tty_test(args: &[impl AsRef<OsStr>]) -> Output {
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

    // Waiting until server creates an tty
    wait_for_path(tmp_dir.path().join("tty"));

    let mut child = command(CLIENT)
        .args(["run", "--serial", "./tty"])
        .current_dir(tmp_dir.path())
        .spawn()
        .unwrap();
    // We need to hold input until end of the test
    let _stdin = child.stdin.take().unwrap();

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
