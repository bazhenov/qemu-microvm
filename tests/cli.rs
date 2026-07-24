mod common;

use common::{OutputExt, command, wait_with_timeout};
use std::{ffi::OsStr, io::Write, path::Path, process::Output, thread, time::Duration};
use tempdir::TempDir;

const CLIENT: &str = env!("CARGO_BIN_EXE_client");
const SERVER: &str = env!("CARGO_BIN_EXE_server");

#[test]
fn md5sum() {
    run_tty_test(&["/bin/bash", "-c", "echo Hi | md5sum"])
        .assert_success()
        .assert_stdout_contains("31ebdfce8b77ac49d7f5506dd1495830");
}

#[test]
fn stdin_eof_reaches_command() {
    let tmp_dir = TempDir::new("example").unwrap();

    let server = command(SERVER)
        .args(["--", "/bin/bash", "-c", "md5sum"])
        .current_dir(tmp_dir.path())
        .spawn()
        .unwrap();

    wait_for_path(tmp_dir.path().join("tty"));

    let mut client = command(CLIENT)
        .args(["run", "--serial", "./tty"])
        .current_dir(tmp_dir.path())
        .spawn()
        .unwrap();

    // Feed the command's stdin and close it, signalling EOF
    let mut stdin = client.stdin.take().unwrap();
    stdin.write_all(b"Hi").unwrap();
    drop(stdin);

    let client_out = wait_with_timeout(client, "client");
    let server_out = wait_with_timeout(server, "server");

    server_out.assert_success();
    // The command must see exactly the bytes written to the client's stdin
    // ("Hi" with no trailing newline): the hash of `printf Hi | md5sum`.
    // And the client must report exactly the command's output: no echo of
    // the input, `-` is what md5sum calls its stdin.
    client_out
        .assert_success()
        .assert_stdout_match("c1a5298f939e87e8f962a5edfc206918  -\n");
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
