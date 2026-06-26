use regex::Regex;
use std::{
    env,
    ffi::OsStr,
    path::Path,
    process::{Command, ExitStatus, Output, Stdio},
    thread,
    time::Duration,
};
use tempdir::TempDir;

const CLIENT: &str = env!("CARGO_BIN_EXE_client");
const SERVER: &str = env!("CARGO_BIN_EXE_server");

#[test]
fn md5sum() {
    run_tty_test(&["/bin/bash", "c", "echo Hi | md5sum"])
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

    let server_out = thread::spawn(|| command(SERVER).args(args).current_dir(path).output());

    // Waiting until server creates an tty
    wait_for_path(tmp_dir.path().join("tty"));

    let mut child = command(CLIENT)
        .args(["tty"])
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

fn command(cmd: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(cmd);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

trait OutputExt {
    fn status(&self) -> ExitStatus;
    fn stdout(&self) -> &[u8];
    fn stderr(&self) -> &[u8];

    #[track_caller]
    fn assert_success(&self) -> &Self {
        assert!(
            self.status().success(),
            "Expected exit code 0, got {:?}\nstdout: {}\nstderr: {}",
            self.status(),
            String::from_utf8_lossy(self.stdout()),
            String::from_utf8_lossy(self.stderr()),
        );
        self
    }

    fn assert_failure(&self) -> &Self {
        assert!(
            !self.status().success(),
            "Expected non-zero exit code\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(self.stdout()),
            String::from_utf8_lossy(self.stderr()),
        );
        self
    }

    fn assert_stdout_match(&self, pattern: &str) -> &Self {
        let re_pattern = format!("(?s)^{}$", compile_pattern(pattern));
        let re = Regex::new(&re_pattern).expect("Invalid regex");
        let stdout = String::from_utf8_lossy(self.stdout());

        assert!(
            re.is_match(&stdout),
            "Expected stdout to match: {}\nstdout: {}",
            pattern.trim(),
            stdout,
        );
        self
    }

    fn assert_stdout_contains(&self, pattern: &str) -> &Self {
        let re_pattern = format!("(?s){}", compile_pattern(pattern));
        let re = Regex::new(&re_pattern).expect("Invalid regex");
        let stdout = String::from_utf8_lossy(self.stdout());
        let stderr = String::from_utf8_lossy(self.stderr());

        assert!(
            re.find(&stdout).is_some(),
            "Expected stdout to contain: {}\n--- STDOUT ---\n{}--- STDERR ---\n{}--------------",
            pattern,
            stdout,
            stderr,
        );
        self
    }

    fn assert_stderr_contains(&self, pattern: &str) -> &Self {
        let re_pattern = format!("(?si){}", compile_pattern(pattern));
        let re = Regex::new(&re_pattern).expect("Invalid regex");
        let stderr = String::from_utf8_lossy(self.stderr());

        assert!(
            re.find(&stderr).is_some(),
            "Expected stderr to contain: {}\nstderr: {}",
            pattern,
            String::from_utf8_lossy(self.stderr()),
        );
        self
    }
}

impl OutputExt for Output {
    fn status(&self) -> ExitStatus {
        self.status
    }

    fn stdout(&self) -> &[u8] {
        &self.stdout
    }

    fn stderr(&self) -> &[u8] {
        &self.stderr
    }
}

fn compile_pattern(pattern: &str) -> String {
    let parts = pattern.split("{..}").collect::<Vec<_>>();
    let escaped = parts.into_iter().map(regex::escape).collect::<Vec<_>>();
    escaped.join(".+")
}
