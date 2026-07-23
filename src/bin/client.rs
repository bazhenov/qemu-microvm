//! Client side of the serial multiplexer (Unix).
//!
//! Two subcommands:
//!
//! - `init` — set up a new VM environment: clone the root filesystem image
//!   into the data directory (`./.vm` by default).
//! - `run` — boot a VM from an initialized data directory, put the local
//!   terminal in raw mode and bridge it to the server over the framed channel:
//!   keystrokes ride endpoint 0, the shell's stdout/stderr come back on
//!   endpoints 1/2, and SIGWINCH is forwarded as resize frames.
//!
//! Threaded, blocking I/O. `Ctrl-]` then `q` disconnects locally.

use clap::{Args, Parser, Subcommand};
use qemu_agent::{
    Frame, FrameReader, FrameType, MAX_PAYLOAD, configure_raw_pty,
    qemu::{self, VmLaunchOpts},
};
use signal_hook::{consts::SIGWINCH, iterator::Signals};
use std::{
    fs::{self, File},
    io::{self, IsTerminal, Read, Write, stdin},
    path::{Path, PathBuf},
    process::ExitCode,
    sync::{Arc, Mutex, mpsc},
    thread,
    time::Duration,
};
use tempdir::TempDir;

/// `Ctrl-]` — begins the local escape sequence.
const ESCAPE: u8 = 0x1d;

/// Default VM data directory.
const DEFAULT_DATA_DIR: &str = ".vm";

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Initialize a new VM environment in the data directory
    Init(InitArgs),
    /// Run a VM from an already initialized data directory
    Run(RunArgs),
}

#[derive(Args, Debug)]
struct InitArgs {
    /// VM data directory where the rootfs and related VM data are stored
    #[arg(long = "data-dir", value_name = "dir", default_value = DEFAULT_DATA_DIR)]
    data_dir: PathBuf,

    /// Source root filesystem disk image cloned into the data directory
    /// (format inferred from the extension: .qcow2 — qcow2, anything else — raw).
    #[arg(long = "root-fs", value_name = "disk", default_value = "rootfs.qcow2")]
    root_fs: PathBuf,
}

#[derive(Args, Debug)]
struct RunArgs {
    /// VM data directory previously initialized with `init`
    #[arg(long = "data-dir", value_name = "dir", default_value = DEFAULT_DATA_DIR)]
    data_dir: PathBuf,

    /// If specified no VM will be launched automatically. Instead client will connect to a given pty
    #[arg(long = "serial", value_name = "path")]
    serial: Option<PathBuf>,

    /// Dump VM boot logs to the stdout
    #[arg(long = "boot-log")]
    dump_boot_log: bool,

    /// Run VM init in a recovery mode
    #[arg(long = "recovery")]
    recovery: bool,

    /// Run in emulation mode (without using hypervisor)
    #[arg(long = "emulate")]
    emulate: bool,

    /// Attach an additional disk image to the VM (may be given multiple times).
    /// Disks appear in the guest as /dev/vdb, /dev/vdc, ... in the given order
    #[arg(long = "disk", name = "disk")]
    additional_disks: Vec<PathBuf>,

    /// Command to run in the VM instead of the default login shell
    /// (e.g. `client run -- /bin/sh -c 'uname -a'`)
    #[arg(last = true, name = "command")]
    command: Vec<String>,
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Cmd::Init(args) => init_cmd(args),
        Cmd::Run(args) => run_cmd(args),
    }
}

fn init_cmd(args: InitArgs) -> ExitCode {
    match init_env(&args.data_dir, &args.root_fs) {
        Ok(root_fs) => {
            println!(
                "Initialized VM environment in {} (root fs: {})",
                args.data_dir.display(),
                root_fs.display()
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("init: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Create the data directory and clone the source root filesystem image into
/// it as `rootfs.qcow2` (for a `.qcow2` source) or `rootfs.raw` (anything
/// else), mirroring how the disk format is inferred from the extension.
fn init_env(data_dir: &Path, src_root_fs: &Path) -> io::Result<PathBuf> {
    if !src_root_fs.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("root fs image not found: {}", src_root_fs.display()),
        ));
    }
    if let Some(existing) = find_root_fs(data_dir) {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("VM environment already initialized: {}", existing.display()),
        ));
    }
    fs::create_dir_all(data_dir)?;
    let file_name = match src_root_fs.extension() {
        Some(ext) if ext.eq_ignore_ascii_case("qcow2") => "rootfs.qcow2",
        _ => "rootfs.raw",
    };
    let dst = data_dir.join(file_name);
    // On macOS/APFS `fs::copy` uses `clonefile`, so the clone is a cheap COW copy.
    fs::copy(src_root_fs, &dst)?;
    Ok(dst)
}

/// Locate the root filesystem image in the data directory: `rootfs.qcow2`
/// takes priority over `rootfs.raw`. `None` means the environment is not
/// initialized.
fn find_root_fs(data_dir: &Path) -> Option<PathBuf> {
    ["rootfs.qcow2", "rootfs.raw"]
        .iter()
        .map(|name| data_dir.join(name))
        .find(|path| path.is_file())
}

fn run_cmd(args: RunArgs) -> ExitCode {
    let (path, join_handle) = if let Some(path) = args.serial {
        (path, None)
    } else {
        let Some(root_fs) = find_root_fs(&args.data_dir) else {
            eprintln!(
                "run: VM environment is not initialized in {}, run `client init` first",
                args.data_dir.display()
            );
            return ExitCode::FAILURE;
        };
        // Private temporary VM directory to hold intermediate VM artifacts
        let private_dir = TempDir::new("vm").unwrap();
        let serial_path = private_dir.path().join("vm-server");
        let opts = VmLaunchOpts {
            dump_boot_log: args.dump_boot_log,
            serial_path: serial_path.clone(),
            recovery: args.recovery,
            root_fs,
            emulate: args.emulate,
            command: args.command,
            additional_disks: args.additional_disks,
        };
        let handle = thread::spawn(move || {
            let result = qemu::launch_vm(opts);
            // We want to drop private dir only after VM has finished
            drop(private_dir);
            result
        });
        // Waiting util VM has started
        while !fs::exists(&serial_path).unwrap() && !handle.is_finished() {
            thread::sleep(Duration::from_millis(50));
        }
        if handle.is_finished() {
            // VM failed, returning error
            eprintln!("vm: {:?}", handle.join().unwrap());
            return ExitCode::FAILURE;
        }
        (serial_path, Some(handle))
    };

    let result = run(&path);
    if let Some(handle) = join_handle {
        let _ = handle.join();
    }
    match result {
        // Exit code reported by the VM process; absent when the session ended
        // without one (local escape, channel EOF).
        Ok(Some(code)) => ExitCode::from(u8::try_from(code).unwrap_or(u8::MAX)),
        Ok(None) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("client: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Restores cooked terminal mode on drop, so a panic still leaves the terminal
/// usable.
struct RawGuard;

impl RawGuard {
    fn enable() -> Option<Self> {
        if stdin().is_terminal() {
            // if stdin is not terminal, crossterm will try to operate on /dev/tty
            // which is still linked to users terminal even if stdin/stdout was piped.
            // It breaks tests, so we only enabling raw mode if stdin is a tty
            let _ = crossterm::terminal::enable_raw_mode();
            Some(Self)
        } else {
            None
        }
    }
}

impl Drop for RawGuard {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

/// Bridge the local terminal to the server over the framed channel at `path`.
fn run(path: &Path) -> io::Result<Option<i32>> {
    let channel = File::options().read(true).write(true).open(path)?;
    configure_raw_pty(&channel).unwrap();
    let reader_file = channel.try_clone()?;
    let mut reader = FrameReader::new(reader_file);
    match reader.next() {
        Some(Ok(f)) => {
            if f.frame_type != FrameType::Start {
                panic!("Expected start frame. Got: {:?}", f.frame_type);
            }
        }
        Some(Err(e)) => panic!("{e}"),
        None => panic!("Unexpected EOF"),
    }
    let writer = Arc::new(Mutex::new(channel));

    let _guard = RawGuard::enable();

    // The server is blocked waiting for the initial size; send it first.
    // Without a controlling terminal (headless run, CI) the size can not be
    // queried — fall back to a conventional 80x24, non-interactive commands
    // don't care about it anyway.
    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
    send_frame(&writer, &Frame::resize(cols, rows))?;

    // Any thread that finishes (clean disconnect, EOF, or error) signals here;
    // main then returns and drops the RawGuard, restoring the terminal.
    let (done_tx, done_rx) = mpsc::channel();

    // Channel -> local stdout/stderr.
    {
        let done = done_tx.clone();
        thread::Builder::new()
            .name("output_loop".into())
            .spawn(move || {
                let _ = done.send(output_loop(reader));
            })
            .expect("Unable to spawn thread");
    }

    // Local stdin -> channel.
    {
        let writer = Arc::clone(&writer);
        let done = done_tx.clone();
        thread::Builder::new()
            .name("input_loop".into())
            .spawn(move || {
                let _ = done.send(input_loop(writer).map(|()| None));
            })
            .expect("Unable to spawn thread");
    }

    // SIGWINCH -> resize frames.
    {
        let writer = Arc::clone(&writer);
        let done = done_tx.clone();
        thread::spawn(move || {});
        thread::Builder::new()
            .name("resize_loop".into())
            .spawn(move || {
                let _ = done.send(resize_loop(writer).map(|()| None));
            })
            .expect("Unable to spawn thread");
    }

    // Block until something ends the session.
    done_rx.recv().expect("finish signal")
}

/// Decode frames from the channel and write payloads to the local terminal.
///
/// Returns the exit code of the VM process if an [`FrameType::Exit`] frame
/// arrived before the stream ended.
fn output_loop<R: Read>(reader: FrameReader<R>) -> io::Result<Option<i32>> {
    let mut stdout = io::stdout();
    let mut stderr = io::stderr();
    for item in reader {
        match item {
            Ok(frame) if frame.frame_type == FrameType::Stdout => {
                if stdout.write_all(&frame.payload).is_err() || stdout.flush().is_err() {
                    break;
                }
            }
            Ok(frame) if frame.frame_type == FrameType::Stderr => {
                if stderr.write_all(&frame.payload).is_err() || stderr.flush().is_err() {
                    break;
                }
            }
            Ok(frame) if frame.frame_type == FrameType::Exit => {
                let Some(code) = frame.as_exit() else {
                    eprintln!("Malformed exit frame: {} bytes", frame.payload.len());
                    break;
                };
                // The exit frame is the last one the server sends.
                return Ok(Some(code));
            }
            Ok(frame) => {
                eprintln!("Unknown frame endpoint = {:?}", frame.frame_type);
                break;
            }

            Err(e) => {
                eprintln!("output_loop() = {e:?}");
                break;
            }
        }
    }
    Ok(None)
}

/// Read raw stdin, filter the local escape sequence, forward the rest.
fn input_loop(writer: Arc<Mutex<File>>) -> io::Result<()> {
    let mut stdin = io::stdin();
    let mut buf = [0u8; MAX_PAYLOAD];
    let mut escaped = false;
    loop {
        match stdin.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                let (payload, quit) = filter_escape(&buf[..n], &mut escaped);
                if !payload.is_empty() && send_frame(&writer, &Frame::stdin(payload)).is_err() {
                    break;
                }
                if quit {
                    break;
                }
            }
            Err(e)
                if e.kind() == io::ErrorKind::WouldBlock
                    || e.kind() == io::ErrorKind::Interrupted =>
            {
                thread::yield_now();
                continue;
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Re-query the terminal size on every SIGWINCH and forward it.
fn resize_loop(writer: Arc<Mutex<File>>) -> io::Result<()> {
    for _ in Signals::new([SIGWINCH])?.forever() {
        if let Ok((cols, rows)) = crossterm::terminal::size() {
            send_frame(&writer, &Frame::resize(cols, rows))?
        }
    }
    Ok(())
}

/// Strip the `Ctrl-]` `q` escape sequence from `input`, returning the bytes to
/// forward and whether the user asked to quit. A lone `Ctrl-]` is held across
/// calls via `escaped`; if not followed by `q` it is forwarded verbatim.
fn filter_escape(input: &[u8], escaped: &mut bool) -> (Vec<u8>, bool) {
    let mut out = Vec::with_capacity(input.len());
    for &b in input {
        if *escaped {
            if b == b'q' {
                *escaped = false;
                return (out, true);
            }
            // The held Ctrl-] was not an escape; emit it now.
            out.push(ESCAPE);
            if b == ESCAPE {
                // This one starts a fresh escape; keep waiting.
                continue;
            }
            *escaped = false;
            out.push(b);
        } else if b == ESCAPE {
            *escaped = true;
        } else {
            out.push(b);
        }
    }
    (out, false)
}

/// Write a whole frame to the channel under the lock so frames never interleave.
fn send_frame(writer: &Arc<Mutex<File>>, frame: &Frame) -> io::Result<()> {
    let mut w = writer
        .lock()
        .map_err(|_| io::Error::other("channel lock poisoned"))?;
    frame.write_to(&mut *w)?;
    w.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_input_passes_through() {
        let mut escaped = false;
        let (out, quit) = filter_escape(b"hello", &mut escaped);
        assert_eq!(out, b"hello");
        assert!(!quit);
        assert!(!escaped);
    }

    #[test]
    fn escape_then_q_quits() {
        let mut escaped = false;
        let (out, quit) = filter_escape(&[ESCAPE, b'q'], &mut escaped);
        assert!(quit);
        assert!(out.is_empty());
    }

    #[test]
    fn escape_split_across_reads() {
        let mut escaped = false;
        let (out, quit) = filter_escape(&[b'a', ESCAPE], &mut escaped);
        assert_eq!(out, b"a");
        assert!(!quit);
        assert!(escaped);
        let (out, quit) = filter_escape(b"q", &mut escaped);
        assert!(quit);
        assert!(out.is_empty());
    }

    #[test]
    fn escape_not_followed_by_q_is_forwarded() {
        let mut escaped = false;
        let (out, quit) = filter_escape(&[ESCAPE, b'x'], &mut escaped);
        assert_eq!(out, &[ESCAPE, b'x']);
        assert!(!quit);
        assert!(!escaped);
    }

    #[test]
    fn double_escape_holds_second() {
        let mut escaped = false;
        let (out, quit) = filter_escape(&[ESCAPE, ESCAPE], &mut escaped);
        assert_eq!(out, &[ESCAPE]);
        assert!(!quit);
        assert!(escaped);
    }

    /// Make sure init_env() will not overwrite already existing env, if user call `init` by accident
    #[test]
    fn init_env_fails_if_already_initialized() {
        let tmp = TempDir::new("init-env").unwrap();
        let src = tmp.path().join("base.qcow2");
        fs::write(&src, b"disk image").unwrap();
        let data_dir = tmp.path().join("vm");

        init_env(&data_dir, &src).unwrap();
        let err = init_env(&data_dir, &src).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
    }

    #[test]
    fn find_root_fs_on_uninitialized_dir() {
        let tmp = TempDir::new("find-root-fs").unwrap();
        // Missing directory
        assert_eq!(find_root_fs(&tmp.path().join("vm")), None);
        // Existing but empty directory
        assert_eq!(find_root_fs(tmp.path()), None);
    }
}
