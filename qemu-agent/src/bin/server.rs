//! Server side of the serial multiplexer (Linux).
//!
//! Hosts an interactive shell behind a PTY and forwards it over a single
//! framed channel. stdin/stdout ride the PTY (so terminal semantics work);
//! stderr rides a separate pipe so endpoint 2 carries genuine stderr bytes.
//!
//! Threaded, blocking I/O. The channel device path is the sole argument.

use clap::Parser;
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
    path::PathBuf,
    process::ExitCode,
    ptr,
    sync::mpsc,
    thread,
};

const TTY_PATH: &str = "./tty";

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(long)]
    /// path to the serial device for communicating with the host
    pub serial: Option<PathBuf>,
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

    let exit_code = match run(client_channel, &cmd) {
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

fn run(mut client_channel: File, cmd: &[&str]) -> io::Result<()> {
    // Writing a start frame to a client allows us to bluck until the client
    // arrives. Otherwise any attempt to read from a serial device that has no connected
    // clients will fail with an error and we would need to spin.
    let start_frame = Frame::new(FrameType::Start, vec![]);
    start_frame.write_to(&mut client_channel).unwrap();
    let channel_writer = client_channel.try_clone()?;
    let mut client_channel_reader = FrameReader::new(client_channel);

    let (master, slave, _) = open_pty()?;

    // Raw fds the child must close so it doesn't inherit the channel/master.
    let master_fd = master.as_raw_fd();
    let slave_fd = slave.as_raw_fd();

    let first_frame = client_channel_reader
        .next()
        .expect("channel closed before initial resize")?;
    let Some((cols, rows)) = first_frame.as_resize() else {
        panic!("Unexpected frame: {:?}", first_frame.frame_type);
    };
    set_winsize(master_fd, cols, rows);

    // Build exec arguments and environment before forking so the child does
    // no allocation between fork() and execvpe() beyond the pointer arrays.
    let argv = cmd
        .iter()
        .copied()
        .map(str::as_bytes)
        .map(CString::new)
        .collect::<Result<Vec<_>, _>>()?;
    let envp = build_env();
    // eprintln!("Spawning: {:?}", argv);

    let child = unsafe { libc::fork() };
    if child < 0 {
        return Err(io::Error::last_os_error());
    }
    if child == 0 {
        // Child
        drop(master);
        drop(client_channel_reader);
        exec_shell(slave_fd, &argv[0], &argv, &envp);
        // exec_shell never returns.
    } else {
        // Parent: hand the slave end and the pipe's write end to the child only.
        drop(slave);

        let master_writer = File::from(master);
        let master_reader = master_writer.try_clone()?;

        let sink = channel_writer;

        let (done_tx, done_rx) = mpsc::channel();

        // Channel -> child: decode frames, drive the PTY.
        let stdin_handle = {
            let done_tx = done_tx.clone();
            thread::Builder::new()
                .name("pump_client_frames".into())
                .spawn(move || {
                    let _ = done_tx.send(pump_client_frames(client_channel_reader, master_writer));
                })
                .expect("Unable to spawn thread")
        };

        // PTY master -> channel as stdout frames.
        let stdout_handle = {
            let done_tx = done_tx.clone();
            thread::Builder::new()
                .name("pump_stdout".into())
                .spawn(move || {
                    let _ = done_tx.send(pump_stdout(master_reader, sink));
                })
                .expect("Unable to spawn thread")
        };

        drop(stdin_handle);
        // done_tx dhould be dropped here, otherwise loop will never finish
        drop(done_tx);

        // Only waiting while child process is alive. try_wait_for() is workaround for a following problem:
        //
        // In ideal world we would wait for both children threads to finish and then reaping child process.
        // Unfortunatley this is not possible because we need to signal client that child process has exited.
        //
        // The proper way to solve it is to pass stdout/stderr close() signals via communication protocol
        // (eg. [`Frame`]), so that client can understand stat stdout has been closed, therefore communication
        // channel as a whole needs to be closed.
        while try_wait_for(child).is_none()
            && let Ok(r) = done_rx.recv()
        {
            r.expect("Thread failed");
        }

        // Reap the shell. When it dies the PTY master and stderr pipe hit EOF and
        // the output threads finish; the input thread may still be blocked on the
        // channel, so we exit the process rather than joining it.
        let _ = wait_for(child);

        let _ = stdout_handle.join();
        Ok(())
    }
}

/// Decode frames from the channel and apply them to the PTY master.
fn pump_client_frames(reader: FrameReader<impl Read>, mut tty_master: File) -> io::Result<()> {
    for item in reader {
        let frame = item?;
        if frame.frame_type == FrameType::Stdin {
            tty_master.write_all(&frame.payload)?;
            tty_master.flush()?;
        } else if let Some((cols, rows)) = frame.as_resize() {
            set_winsize(tty_master.as_raw_fd(), cols, rows);
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Unexpected frame type: {:?}", frame.frame_type),
            ));
        }
    }
    Ok(())
}

/// Read bytes from `src`, chunk them into frames on `endpoint`, and write each
/// whole frame to the shared channel under the lock so frames never interleave.
fn pump_stdout(mut src: impl Read, mut sink: File) -> io::Result<()> {
    let mut buf = [0u8; MAX_PAYLOAD];

    let mut bytes_read = src.read(&mut buf)?;
    while bytes_read > 0 {
        let frame = Frame::new(FrameType::Stdout, buf[..bytes_read].to_vec());
        frame.write_to(&mut sink)?;
        bytes_read = src.read(&mut buf)?;
    }

    Ok(())
}

/// Child half of fork/exec. Replaces the process image; never returns.
///
/// Only async-signal-safe libc calls are used here. The process is
/// single-threaded at this point (threads are spawned after the fork).
fn exec_shell(slave_fd: RawFd, prog: &CString, argv: &[CString], envp: &[CString]) -> ! {
    unsafe {
        if libc::setsid() < 0 {
            libc::_exit(127);
        }
        // Make the PTY slave the controlling terminal.
        if libc::ioctl(slave_fd, libc::TIOCSCTTY as _, 0) < 0 {
            libc::_exit(127);
        }
        libc::dup2(slave_fd, libc::STDIN_FILENO);
        libc::dup2(slave_fd, libc::STDOUT_FILENO);
        libc::dup2(slave_fd, libc::STDERR_FILENO);

        if slave_fd > libc::STDERR_FILENO {
            libc::close(slave_fd);
        }

        let argv_ptrs: Vec<*const libc::c_char> = argv
            .iter()
            .map(|c| c.as_ptr())
            .chain(once(ptr::null()))
            .collect();
        let envp_ptrs: Vec<*const libc::c_char> = envp
            .iter()
            .map(|c| c.as_ptr())
            .chain(once(ptr::null()))
            .collect();

        libc::execve(prog.as_ptr(), argv_ptrs.as_ptr(), envp_ptrs.as_ptr());
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

/// Block until the given child changes state.
fn wait_for(pid: libc::pid_t) -> io::Result<()> {
    let mut status = 0;
    if unsafe { libc::waitpid(pid, &mut status, 0) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn try_wait_for(pid: libc::pid_t) -> Option<io::Result<()>> {
    let mut status = 0;
    let ret = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
    if ret < 0 {
        return Some(Err(io::Error::last_os_error()));
    } else if ret > 0 {
        return Some(Ok(()));
    }
    None
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
