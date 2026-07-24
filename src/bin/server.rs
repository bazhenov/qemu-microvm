//! Server side of the serial multiplexer (Linux).
//!
//! Executes a command and forwards its I/O over a single framed channel.
//! The client's start reply tells whether it stands for a real terminal,
//! and that decides how the command's stdio is wired:
//!
//! - a terminal: the command runs behind a freshly allocated PTY (controlling
//!   terminal, line discipline, resize), stdout and stderr share the PTY;
//! - no terminal (the client's stdin is piped): the command gets three plain
//!   pipes, bytes pass through byte-exact and stderr rides its own endpoint.
//!
//! Threaded, blocking I/O. The channel device path is the sole argument.

use clap::Parser;
use libc::c_char;
use nix::errno::Errno;
use qemu_agent::{Frame, FrameReader, FrameType, MAX_PAYLOAD, configure_raw_pty};
use std::{
    env,
    ffi::{CStr, CString},
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    iter::once,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
        unix::{
            self,
            ffi::{OsStrExt, OsStringExt},
        },
    },
    path::{Path, PathBuf},
    process::ExitCode,
    ptr,
    sync::{Arc, Mutex},
    thread,
};

const TTY_PATH: &str = "./tty";

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(long)]
    /// path to the serial device for communicating with the host
    pub serial: Option<PathBuf>,
    #[arg(long)]
    /// chroot into this directory before executing the command. The PTY is
    /// allocated first, then /dev/pts is move-mounted to <DIR>/dev/pts and
    /// moved back once the command exits. Linux only.
    pub chroot: Option<PathBuf>,
    pub command: Vec<String>,
}

fn main() -> ExitCode {
    let args = Args::parse();

    let (client_channel, _slave) = match args.serial.as_ref() {
        Some(p) => {
            let fd = OpenOptions::new().write(true).read(true).open(p).unwrap();
            (fd, None)
        }
        None => {
            // The channel is read and written from different threads. Open once and
            // dup the fd so the reader can own its half while the writers share theirs.
            // _slave needs to be bounded, otherwise pty will be closed
            let (client_channel, slave, name) = open_pty().unwrap();
            configure_raw_pty(&client_channel).unwrap();
            unix::fs::symlink(name, TTY_PATH).unwrap();
            (File::from(client_channel), Some(slave))
        }
    };

    let cmd = args.command.iter().map(String::as_str).collect::<Vec<_>>();

    let exit_code = match run(client_channel, &cmd, args.chroot.as_deref()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("server: {e}");
            ExitCode::FAILURE
        }
    };
    if args.serial.is_none() && fs::exists(TTY_PATH).unwrap_or(false) {
        let _ = fs::remove_file(TTY_PATH);
    }
    exit_code
}

/// How the command's stdio is wired, decided by the client's start reply.
enum Stdio {
    /// A PTY pair: the command gets the slave as its controlling terminal.
    Tty { master: OwnedFd, slave: OwnedFd },
    /// One plain pipe per stream, each as (read, write) ends.
    Pipes {
        stdin: (OwnedFd, OwnedFd),
        stdout: (OwnedFd, OwnedFd),
        stderr: (OwnedFd, OwnedFd),
    },
}

fn run(mut client_channel: File, cmd: &[&str], new_root: Option<&Path>) -> io::Result<()> {
    // Writing a start frame to a client allows us to bluck until the client
    // arrives. Otherwise any attempt to read from a serial device that has no connected
    // clients will fail with an error and we would need to spin.
    let start_frame = Frame::new(FrameType::Start, vec![]);
    start_frame.write_to(&mut client_channel).unwrap();
    let channel_writer = Arc::new(Mutex::new(client_channel.try_clone()?));
    let mut client_channel_reader = FrameReader::new(client_channel);

    // The client opens with a start reply telling whether it stands for a
    // real terminal. When it does, the command runs behind a PTY; when it
    // does not (stdin is piped), the command gets plain pipes instead.
    let start_reply_frame = client_channel_reader
        .next()
        .expect("channel closed before start reply")?;
    let Some(tty_allocate) = start_reply_frame.as_start_reply() else {
        panic!(
            "Unexpected frame ({:?} expected): {:?}",
            FrameType::StartReply,
            start_reply_frame
        );
    };

    // The initial terminal size always follows; without a tty it is ignored.
    let resize_frame = client_channel_reader
        .next()
        .expect("channel closed before initial resize")?;
    let Some((cols, rows)) = resize_frame.as_resize() else {
        panic!("Unexpected frame: {:?}", resize_frame.frame_type);
    };

    let stdio = if tty_allocate {
        let (master, slave, _) = open_pty()?;
        set_winsize(master.as_raw_fd(), cols, rows);
        Stdio::Tty { master, slave }
    } else {
        Stdio::Pipes {
            stdin: pipe()?,
            stdout: pipe()?,
            stderr: pipe()?,
        }
    };

    // Build exec arguments and environment before forking so the child does
    // no allocation between fork() and execvpe() beyond the pointer arrays.
    let argv = cmd
        .iter()
        .copied()
        .map(str::as_bytes)
        .map(CString::new)
        .collect::<Result<Vec<_>, _>>()?;
    let envp = build_env();

    let new_root_c = new_root
        .map(|p| CString::new(p.as_os_str().as_bytes()))
        .transpose()?;

    // Usually mount points are moved to a new rootfs by an init process, /dev/pts is an exception, because
    // it can not be moved before server started. Server spawns a new pty, so it needs /dev/pts
    // in place when it started, but new spawned process (in a rootfs context) probably also needs
    // /dev/pts in the propes place (moved to a rootfs). This leaves only one opportinity –
    // move /dev/pts here, after we allocated pty, but before we forked.
    let devpts_target = match new_root {
        Some(root) => {
            let target = root.join("dev/pts");
            move_mount(Path::new("/dev/pts"), &target)?;
            Some(target)
        }
        None => None,
    };

    let argv_ptrs: Vec<*const c_char> = argv
        .iter()
        .map(|c| c.as_ptr())
        .chain(once(ptr::null()))
        .collect();
    let envp_ptrs: Vec<*const c_char> = envp
        .iter()
        .map(|c| c.as_ptr())
        .chain(once(ptr::null()))
        .collect();

    let status = match stdio {
        Stdio::Tty { master, slave } => run_with_tty(
            client_channel_reader,
            &channel_writer,
            master,
            slave,
            &argv_ptrs,
            &envp_ptrs,
            new_root_c.as_deref(),
        ),
        Stdio::Pipes {
            stdin,
            stdout,
            stderr,
        } => run_with_pipes(
            client_channel_reader,
            &channel_writer,
            stdin,
            stdout,
            stderr,
            &argv_ptrs,
            &envp_ptrs,
            new_root_c.as_deref(),
        ),
    };

    // Move devpts back even if pumping or waiting failed, so the old root
    // is left the way we found it.
    if let Some(target) = &devpts_target
        && let Err(e) = move_mount(target, Path::new("/dev/pts"))
    {
        eprintln!("server: unable to move devpts back: {e}");
    }

    send_frame(&channel_writer, &Frame::exit(exit_code(status?)))
}

/// Fork the command behind the given PTY and pump its I/O until it exits.
/// Returns the raw `waitpid` status.
fn run_with_tty(
    reader: FrameReader<File>,
    channel: &Arc<Mutex<File>>,
    pty_master: OwnedFd,
    pty_slave: OwnedFd,
    argv: &[*const c_char],
    envp: &[*const c_char],
    new_root: Option<&CStr>,
) -> io::Result<libc::c_int> {
    let child = unsafe { libc::fork() };
    // NOTE: There must be no allocations or any ther async-signal-unsafe calls
    // past this point in the children
    if child < 0 {
        return Err(io::Error::last_os_error());
    }
    if child == 0 {
        drop(pty_master);
        drop(reader);

        unsafe {
            // Creating process group and make the PTY slave the controlling terminal.
            if libc::setsid() < 0 || libc::ioctl(pty_slave.as_raw_fd(), libc::TIOCSCTTY as _, 0) < 0
            {
                libc::_exit(127);
            }
        }

        exec_child(&pty_slave, &pty_slave, &pty_slave, argv, envp, new_root);
        // exec_child never returns.
    } else {
        // Parent: the slave end travels with the child only.
        drop(pty_slave);
        let pty_master = File::from(pty_master);

        // Channel -> child: decode frames, drive the PTY.
        {
            let pty_master = pty_master.try_clone()?;
            thread::Builder::new()
                .name("pump_client_frames".into())
                .spawn(move || pump_client_frames(reader, pty_master))
                .expect("Unable to spawn thread");
        }

        // PTY master -> channel as stdout frames (stderr shares the PTY).
        let pump_result = pump_output(pty_master, FrameType::Stdout, channel);
        let status = wait_for(child);
        pump_result?;
        status
    }
}

/// Fork the command with plain pipes for stdin/stdout/stderr and pump its
/// I/O until it exits. Returns the raw `waitpid` status.
#[allow(clippy::too_many_arguments)]
fn run_with_pipes(
    reader: FrameReader<File>,
    channel: &Arc<Mutex<File>>,
    (stdin_read, stdin_write): (OwnedFd, OwnedFd),
    (stdout_read, stdout_write): (OwnedFd, OwnedFd),
    (stderr_read, stderr_write): (OwnedFd, OwnedFd),
    argv: &[*const c_char],
    envp: &[*const c_char],
    new_root: Option<&CStr>,
) -> io::Result<libc::c_int> {
    let child = unsafe { libc::fork() };
    if child < 0 {
        return Err(io::Error::last_os_error());
    }
    if child == 0 {
        // Close the parent's ends so the command sees EOF on stdin when the
        // parent drops its write end, and the parent sees EOF on stdout and
        // stderr when the command exits.
        drop(stdin_write);
        drop(stdout_read);
        drop(stderr_read);
        drop(reader);
        exec_child(
            &stdin_read,
            &stdout_write,
            &stderr_write,
            argv,
            envp,
            new_root,
        );
        // exec_child never returns.
    } else {
        // Parent: the child's ends travel with the child only.
        drop(stdin_read);
        drop(stdout_write);
        drop(stderr_write);

        // Channel -> child stdin.
        thread::Builder::new()
            .name("pump_client_frames".into())
            .spawn(move || pump_client_frames_to_pipe(reader, File::from(stdin_write)))
            .expect("Unable to spawn thread");

        // Child stderr -> channel as stderr frames, concurrently with stdout.
        let stderr_pump = {
            let channel = Arc::clone(channel);
            thread::Builder::new()
                .name("pump_stderr".into())
                .spawn(move || pump_output(File::from(stderr_read), FrameType::Stderr, &channel))
                .expect("Unable to spawn \thread")
        };

        // Child stdout -> channel as stdout frames.
        let stdout_result = pump_output(File::from(stdout_read), FrameType::Stdout, channel);
        let stderr_result = stderr_pump.join().expect("stderr pump panicked");
        let status = wait_for(child);
        stdout_result?;
        stderr_result?;
        status
    }
}

/// The default `VEOF` control character (`Ctrl-D`).
const EOF_CHAR: u8 = 0x04;

/// Decode frames from the channel and apply them to the PTY master.
fn pump_client_frames(reader: FrameReader<impl Read>, mut pty_master: File) -> io::Result<()> {
    // In canonical mode VEOF produces EOF (a zero-length read) only at the
    // start of a line; written mid-line it just flushes the partial line to
    // the reader. Track where we are to know how many VEOFs EOF takes.
    let mut at_line_start = true;
    for item in reader {
        let frame = item?;
        if frame.frame_type == FrameType::Stdin {
            if frame.payload.is_empty() {
                // The client's stdin reached EOF. The PTY master can not be
                // closed (it also carries the command's output), so emit the
                // EOF character instead; a pending partial line takes an
                // extra one to flush it first.
                if !at_line_start {
                    pty_master.write_all(&[EOF_CHAR])?;
                }
                pty_master.write_all(&[EOF_CHAR])?;
            } else {
                pty_master.write_all(&frame.payload)?;
                // ICRNL is on by default, so `\r` terminates a line too.
                at_line_start = matches!(frame.payload.last(), Some(b'\n') | Some(b'\r'));
            }
            pty_master.flush()?;
        } else if let Some((cols, rows)) = frame.as_resize() {
            set_winsize(pty_master.as_raw_fd(), cols, rows);
        }
    }
    Ok(())
}

/// Decode frames from the channel and feed stdin bytes to the command's
/// stdin pipe. An empty stdin frame (the client's stdin reached EOF) closes
/// the write end so the command sees EOF. Resize frames are ignored: there
/// is no terminal to resize.
fn pump_client_frames_to_pipe(
    reader: FrameReader<impl Read>,
    mut stdin_pipe: File,
) -> io::Result<()> {
    for item in reader {
        let frame = item?;
        if frame.frame_type == FrameType::Stdin {
            if frame.payload.is_empty() {
                break;
            } else {
                stdin_pipe.write_all(&frame.payload)?;
                stdin_pipe.flush()?;
            }
        }
    }
    Ok(())
}

/// Read bytes from `src` and forward them to the channel as frames of the
/// given type.
fn pump_output(mut src: impl Read, frame_type: FrameType, channel: &Mutex<File>) -> io::Result<()> {
    let mut buf = [0u8; MAX_PAYLOAD];

    loop {
        match src.read(&mut buf) {
            Ok(0) => break Ok(()),
            Ok(n) => send_frame(channel, &Frame::new(frame_type, buf[..n].to_vec()))?,
            // On Unix systems when child process drops slave part of pty, read doesn't return EOF,
            // it generate EIO error instead.
            // Unfortunatley this error code is not stabilized by Rust, so we need to use `Errno` here.
            // see: https://unix.stackexchange.com/questions/538198/why-blocking-read-on-a-pty-returns-when-process-on-the-other-end-dies
            Err(_) if Errno::last() == Errno::EIO => break Ok(()),
            Err(e) => break Err(e),
        }
    }
}

/// Write a whole frame to the shared channel under the lock so frames from
/// concurrent pumps never interleave.
fn send_frame(channel: &Mutex<File>, frame: &Frame) -> io::Result<()> {
    let mut w = channel
        .lock()
        .map_err(|_| io::Error::other("channel lock poisoned"))?;
    frame.write_to(&mut *w)
}

/// Child half of fork/exec. Replaces the process image; never returns.
///
/// Only async-signal-safe libc calls are used here. The process is
/// single-threaded at this point (threads are spawned after the fork).
fn exec_child(
    stdin: &impl AsRawFd,
    stdout: &impl AsRawFd,
    stderr: &impl AsRawFd,
    argv: &[*const c_char],
    envp: &[*const c_char],
    new_root: Option<&CStr>,
) -> ! {
    let stdin = stdin.as_raw_fd();
    let stdout = stdout.as_raw_fd();
    let stderr = stderr.as_raw_fd();
    unsafe {
        libc::dup2(stdin, libc::STDIN_FILENO);
        libc::dup2(stdout, libc::STDOUT_FILENO);
        libc::dup2(stderr, libc::STDERR_FILENO);

        for fd in [stdin, stdout, stderr] {
            if fd > libc::STDERR_FILENO {
                libc::close(fd);
            }
        }

        if let Some(root) = new_root
            && (libc::chroot(root.as_ptr()) < 0 || libc::chdir(c"/".as_ptr()) < 0)
        {
            libc::_exit(127);
        }

        libc::execve(argv[0], argv.as_ptr(), envp.as_ptr());
        // Only reached if exec failed.
        libc::_exit(127);
    }
}

/// Current environment with `TERM` forced to `xterm-256color`.
fn build_env() -> Vec<CString> {
    let mut env: Vec<CString> = env::vars_os()
        .filter(|(k, _)| k != "TERM")
        .filter_map(|(k, v)| {
            let mut entry = k.into_vec();
            entry.push(b'=');
            entry.extend_from_slice(v.as_bytes());
            CString::new(entry).ok()
        })
        .collect();
    env.push(CString::new("TERM=xterm-256color").unwrap());
    env
}

/// Move a mounted filesystem onto a new mountpoint (`mount -o move`),
/// creating the target directory if needed.
#[cfg(target_os = "linux")]
fn move_mount(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    let from = CString::new(from.as_os_str().as_bytes())?;
    let to = CString::new(to.as_os_str().as_bytes())?;
    let rc = unsafe {
        libc::mount(
            from.as_ptr(),
            to.as_ptr(),
            ptr::null(),
            libc::MS_MOVE,
            ptr::null(),
        )
    };
    if rc < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(target_os = "linux"))]
fn move_mount(_from: &Path, _to: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "--chroot is only supported on Linux",
    ))
}

/// `openpty(3)` wrapper returning owned master/slave fds.
fn open_pty() -> io::Result<(OwnedFd, OwnedFd, PathBuf)> {
    let mut master: libc::c_int = 0;
    let mut slave: libc::c_int = 0;
    #[cfg(target_os = "macos")]
    let mut name = vec![0i8; 256];
    #[cfg(target_os = "linux")]
    let mut name = vec![0u8; 256];
    let rc = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            name.as_mut_ptr(),
            ptr::null_mut(),
            ptr::null_mut(),
        )
    };
    let pty_name = unsafe { CStr::from_ptr(name.as_ptr()) }
        .to_string_lossy()
        .to_string();
    if rc < 0 {
        return Err(io::Error::last_os_error());
    }

    let (master, slave) = unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };

    Ok((master, slave, PathBuf::from(pty_name)))
}

/// `pipe(2)` wrapper returning owned (read, write) fds.
fn pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds: [libc::c_int; 2] = [0; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

/// Apply a terminal size to a PTY via `TIOCSWINSZ`.
fn set_winsize(fd: RawFd, cols: u16, rows: u16) {
    let ws = libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    unsafe {
        libc::ioctl(fd, libc::TIOCSWINSZ as _, &ws);
    }
}

/// Block until the given child changes state. Returns the raw `waitpid` status.
fn wait_for(pid: libc::pid_t) -> io::Result<libc::c_int> {
    let mut status = 0;
    if unsafe { libc::waitpid(pid, &mut status, 0) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(status)
}

/// Map a raw `waitpid` status to a shell-style exit code:
/// the code itself for a normal exit, `128 + signal` for a killed process.
fn exit_code(status: libc::c_int) -> i32 {
    if libc::WIFEXITED(status) {
        libc::WEXITSTATUS(status)
    } else if libc::WIFSIGNALED(status) {
        128 + libc::WTERMSIG(status)
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{Rng, RngExt, make_rng, rngs::SmallRng};

    #[test]
    fn openpty_mirroring() {
        let (master, slave, _) = open_pty().unwrap();
        configure_raw_pty(&master).unwrap();

        let mut master = File::from(master);
        let mut slave = File::from(slave);

        for _ in 0..100 {
            let mut rng: SmallRng = make_rng();
            let size = rng.random_range(1..16);
            let mut bytes = vec![0; size];
            rng.fill_bytes(&mut bytes[..]);

            let join_handle = thread::spawn(move || {
                let mut bytes_copy = vec![0; size];
                slave.read_exact(&mut bytes_copy[..]).unwrap();
                (bytes_copy, slave)
            });
            master.write_all(&bytes[..]).unwrap();
            master.flush().unwrap();

            let (bytes_copy, s) = join_handle.join().unwrap();
            slave = s;
            assert_eq!(bytes, bytes_copy);
        }
    }
}
