//! Server side of the serial multiplexer (Linux).
//!
//! Hosts an interactive shell behind a PTY and forwards it over a single
//! framed channel. stdin/stdout ride the PTY (so terminal semantics work);
//! stderr rides a separate pipe so endpoint 2 carries genuine stderr bytes.
//!
//! Threaded, blocking I/O. The channel device path is the sole argument.

use qemu_agent::{configure_raw_pty, Endpoint, Frame, FrameReader, MAX_PAYLOAD};
use std::ffi::{CStr, CString, OsStr};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::PathBuf;
use std::process::ExitCode;
use std::ptr;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use std::{env, io};

fn main() -> ExitCode {
    let (client_channel, _slave) = match env::var_os("TTY_PATH") {
        Some(p) => {
            let fd = OpenOptions::new().write(true).read(true).open(p).unwrap();
            // configure_raw_pty(&fd).unwrap();
            (fd, None)
        }
        None => {
            // The channel is read and written from different threads. Open once and
            // dup the fd so the reader can own its half while the writers share theirs.
            // _slave needs to be bounded, otherwise pty will be closed
            let (client_channel, slave, name) = open_pty().unwrap();
            configure_raw_pty(&client_channel).unwrap();
            std::os::unix::fs::symlink(name, "./tty").unwrap();
            (File::from(client_channel), Some(slave))
        }
    };

    let args = env::args().skip(1).collect::<Vec<_>>();
    let args = args.iter().map(String::as_str).collect::<Vec<_>>();

    match run(client_channel, &args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("server: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(mut client_channel: File, cmd: &[&str]) -> io::Result<()> {
    // There is some kind of race, that I was unable to diagnose yet.
    // If we try to write to communication_channel immediatley write will succeed,
    // but the data will be lost. We need to wait a little bit. Probably until
    // serial device will be connected to the host pty, idk.
    thread::sleep(Duration::from_millis(100));
    let start_frame = Frame::new(Endpoint::Start as u8, vec![]);
    start_frame.write_to(&mut client_channel).unwrap();
    let channel_writer = client_channel.try_clone()?;
    let channel_fd = client_channel.as_raw_fd();
    let mut client_channel_reader = FrameReader::new(client_channel);

    let (master, slave, _) = open_pty()?;

    //if let Ok(current_tty) = File::open("/dev/tty") {
    //    // If there is current terminal, copying setattr from it
    //    let tio = termios::tcgetattr(&current_tty)?;
    //    eprintln!("Settings termio flags: {tio:?}");
    //    termios::tcsetattr(&master, termios::SetArg::TCSANOW, &tio)?;
    //}

    // Raw fds the child must close so it doesn't inherit the channel/master.
    let master_fd = master.as_raw_fd();
    let slave_fd = slave.as_raw_fd();

    // The server blocks until the client announces its terminal size. Any
    // stdin bytes that arrive before the first resize are buffered and
    // flushed once the shell is running.
    let mut pending = Vec::new();
    eprintln!("Waiting for size...");
    let (cols, rows) = loop {
        match client_channel_reader.next() {
            Some(Ok(frame)) => {
                if let Some(size) = frame.as_resize() {
                    break size;
                } else if frame.endpoint == Endpoint::Stdin as u8 {
                    pending.extend_from_slice(&frame.payload);
                } else {
                    panic!("Unexpected frame: {}", frame.endpoint);
                }
                // Other endpoints before the shell exists: drop.
            }
            Some(Err(e)) => return Err(io::Error::other(format!("decode: {e}"))),
            None => {
                return Err(io::Error::other("channel closed before initial resize"));
            }
        }
    };
    eprintln!("Size received {cols}x{rows}...");
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
    eprintln!("Spawning: {:?}", argv);

    let child = unsafe { libc::fork() };
    if child < 0 {
        return Err(io::Error::last_os_error());
    }
    if child == 0 {
        // ---- child ----
        let close_in_child = [channel_fd, master_fd];
        exec_shell(slave_fd, &close_in_child, &argv[0], &argv, &envp);
        // exec_shell never returns.
    } else {
        // ---- parent ----
        // Hand the slave end and the pipe's write end to the child only.
        drop(slave);

        let mut master_writer = File::from(master);
        if !pending.is_empty() {
            master_writer.write_all(&pending)?;
            master_writer.flush()?;
        }
        let master_reader = master_writer.try_clone()?;

        let sink = Arc::new(Mutex::new(channel_writer));

        // eprintln!("Spawning tasks...");
        // Channel -> child: decode frames, drive the PTY.
        let t_in = thread::spawn(move || client_read_worker(client_channel_reader, master_writer));

        // PTY master -> channel as stdout frames.
        let t_out = {
            let sink = Arc::clone(&sink);
            thread::spawn(move || pump_to_channel(master_reader, Endpoint::Stdout as u8, sink))
        };

        // Reap the shell. When it dies the PTY master and stderr pipe hit EOF and
        // the output threads finish; the input thread may still be blocked on the
        // channel, so we exit the process rather than joining it.
        // eprintln!("Waiting for exit");
        let _ = wait_for(child);
        let _ = t_out.join();
        drop(t_in);
        Ok(())
    }
}

/// Decode frames from the channel and apply them to the PTY master.
fn client_read_worker(reader: FrameReader<File>, mut master: File) {
    for item in reader {
        match item {
            Ok(frame) => {
                // eprintln!("Processing frame!");
                if frame.endpoint == Endpoint::Stdin as u8 {
                    if master.write_all(&frame.payload).is_err() || master.flush().is_err() {
                        break;
                    }
                } else if let Some((cols, rows)) = frame.as_resize() {
                    set_winsize(master.as_raw_fd(), cols, rows);
                } else {
                    // eprintln!("channel_to_child() unknown frame {}", frame.endpoint);
                }
                // Unknown endpoints: drop (the frame is already consumed).
            }
            Err(e) => {
                // eprintln!("channel_to_child() error {e}");
                break;
            }
        }
    }
    // eprintln!("channel_to_child() finished")
}

/// Read bytes from `src`, chunk them into frames on `endpoint`, and write each
/// whole frame to the shared channel under the lock so frames never interleave.
fn pump_to_channel(mut src: File, endpoint: u8, sink: Arc<Mutex<File>>) {
    let mut buf = [0u8; MAX_PAYLOAD];
    loop {
        match src.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                let frame = Frame::new(endpoint, buf[..n].to_vec());
                let mut w = match sink.lock() {
                    Ok(w) => w,
                    Err(_) => break,
                };
                frame.write_to(&mut *w).unwrap();
            }
            // Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => {
                eprintln!("pump_to_channel() = {e}");
                break;
            }
        }
    }
    // eprintln!("pump_to_channel() finished")
}

/// Child half of fork/exec. Replaces the process image; never returns.
///
/// Only async-signal-safe libc calls are used here. The process is
/// single-threaded at this point (threads are spawned after the fork).
fn exec_shell(
    slave_fd: RawFd,
    close_fds: &[RawFd],
    prog: &CString,
    argv: &[CString],
    envp: &[CString],
) -> ! {
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

        // Drop the parent-only fds and the now-duplicated originals.
        for &fd in close_fds {
            if fd > libc::STDERR_FILENO {
                libc::close(fd);
            }
        }
        if slave_fd > libc::STDERR_FILENO {
            libc::close(slave_fd);
        }

        let mut argv_ptrs: Vec<*const libc::c_char> = argv.iter().map(|c| c.as_ptr()).collect();
        argv_ptrs.push(ptr::null());
        let mut envp_ptrs: Vec<*const libc::c_char> = envp.iter().map(|c| c.as_ptr()).collect();
        envp_ptrs.push(ptr::null());

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

/// Turn an `OsStr` into a `CString` (NUL-terminated), discarding anything
/// after an embedded NUL — paths/shells never legitimately contain one.
fn cstr(s: &OsStr) -> CString {
    CString::new(s.as_bytes()).unwrap_or_else(|e| {
        let valid = &s.as_bytes()[..e.nul_position()];
        CString::new(valid).unwrap()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{make_rng, rngs::SmallRng, Rng, RngExt};

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
