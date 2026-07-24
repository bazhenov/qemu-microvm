//! Helpers shared between the integration test binaries (`cli.rs`, `e2e.rs`).
//!
//! Each test file is compiled as a separate crate and only uses a subset of
//! these helpers, hence `allow(dead_code)`.

#![allow(dead_code)]

use regex::Regex;
use std::{
    ffi::OsStr,
    process::{Child, Command, ExitStatus, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

/// How long a process is allowed to run in tests before it is considered hung.
const TIMEOUT: Duration = Duration::from_secs(5);

#[track_caller]
pub fn wait_with_timeout(mut child: Child, name: &str) -> Output {
    let deadline = Instant::now() + TIMEOUT;
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("{name} did not exit within {TIMEOUT:?}");
        }
        thread::sleep(Duration::from_millis(10));
    }
    child.wait_with_output().unwrap()
}

/// `Command` with stdin/stdout/stderr piped.
pub fn command(cmd: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(cmd);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

pub trait OutputExt {
    fn status(&self) -> ExitStatus;
    fn stdout(&self) -> &[u8];
    fn stderr(&self) -> &[u8];

    #[track_caller]
    fn assert_success(&self) -> &Self {
        assert!(
            self.status().success(),
            "Expected exit code 0, got {:?}\nstdout: {}\nstderr: {}",
            self.status(),
            normalize_line_endings(self.stdout()),
            normalize_line_endings(self.stderr()),
        );
        self
    }

    #[track_caller]
    fn assert_failure(&self) -> &Self {
        assert!(
            !self.status().success(),
            "Expected non-zero exit code\nstdout: {}\nstderr: {}",
            normalize_line_endings(self.stdout()),
            normalize_line_endings(self.stderr()),
        );
        self
    }

    #[track_caller]
    fn assert_stdout_match(&self, pattern: &str) -> &Self {
        let re_pattern = format!("(?s)^{}$", compile_pattern(pattern));
        let re = Regex::new(&re_pattern).expect("Invalid regex");
        let stdout = normalize_line_endings(self.stdout());

        assert!(
            re.is_match(&stdout),
            "Expected stdout to match: {}\nstdout: {}",
            pattern.trim(),
            stdout,
        );
        self
    }

    #[track_caller]
    fn assert_stdout_contains(&self, pattern: &str) -> &Self {
        let re_pattern = format!("(?s){}", compile_pattern(pattern));
        let re = Regex::new(&re_pattern).expect("Invalid regex");
        let stdout = normalize_line_endings(self.stdout());
        let stderr = normalize_line_endings(self.stderr());

        assert!(
            re.find(&stdout).is_some(),
            "Expected stdout to contain: {}\n--- STDOUT ---\n{}--- STDERR ---\n{}--------------",
            pattern,
            stdout,
            stderr,
        );
        self
    }

    #[track_caller]
    fn assert_stderr_contains(&self, pattern: &str) -> &Self {
        let re_pattern = format!("(?si){}", compile_pattern(pattern));
        let re = Regex::new(&re_pattern).expect("Invalid regex");
        let stderr = normalize_line_endings(self.stderr());

        assert!(
            re.find(&stderr).is_some(),
            "Expected stderr to contain: {}\nstderr: {}",
            pattern,
            stderr,
        );
        self
    }
}

/// Guest commands run on a PTY, whose ONLCR post-processing turns
/// every `\n` into `\r\n`, so normalize line endings before matching.
/// TODO: run noninteractive commands without a PTY; then the output
/// arrives byte-exact and this normalization can go away.
fn normalize_line_endings(v: &[u8]) -> String {
    String::from_utf8_lossy(v).replace("\r\n", "\n")
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

/// Translate a plain-text pattern into a regex: everything is matched
/// literally except `{..}`, which matches any (non-empty) text.
fn compile_pattern(pattern: &str) -> String {
    let escaped = pattern.split("{..}").map(regex::escape).collect::<Vec<_>>();
    escaped.join(".+")
}
