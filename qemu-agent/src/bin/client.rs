//! Client side of the serial multiplexer (Unix).
//!
//! Puts the local terminal in raw mode and bridges it to the server over the
//! framed channel: keystrokes ride endpoint 0, the shell's stdout/stderr come
//! back on endpoints 1/2, and SIGWINCH is forwarded as resize frames.
//!
//! Threaded, blocking I/O. The channel device path is the sole argument.
//! `Ctrl-]` then `q` disconnects locally.

use clap::Parser;
use qemu_agent::{Frame, FrameReader, FrameType, MAX_PAYLOAD, configure_raw_pty, qemu};
use signal_hook::{consts::SIGWINCH, iterator::Signals};
use std::{
    env,
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::ExitCode,
    sync::{Arc, Mutex, mpsc},
    thread,
    time::Duration,
};

/// `Ctrl-]` — begins the local escape sequence.
const ESCAPE: u8 = 0x1d;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// If specified no VM will be launched automatically. Instead client will connect to a given pty
    #[arg(long = "serial", name = "path")]
    serial: Option<PathBuf>,
}

fn main() -> ExitCode {
    let args = Args::parse();
    let (path, join_handle) = if let Some(path) = args.serial {
        (path, None)
    } else {
        let handle = thread::spawn(qemu::launch_vm);
        while !fs::exists(qemu::CONSOLE).unwrap() {
            thread::sleep(Duration::from_millis(50));
        }
        (PathBuf::from(qemu::CONSOLE), Some(handle))
    };

    let result = run(&path);
    if let Some(handle) = join_handle {
        let _ = handle.join();
    }
    match result {
        Ok(()) => ExitCode::SUCCESS,
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
    fn enable() -> io::Result<Self> {
        crossterm::terminal::enable_raw_mode()?;
        Ok(Self)
    }
}

impl Drop for RawGuard {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

fn run(path: &Path) -> io::Result<()> {
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

    let _guard = RawGuard::enable()?;

    // The server is blocked waiting for the initial size; send it first.
    let (cols, rows) = crossterm::terminal::size()?;
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
                let _ = done.send(input_loop(writer));
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
                let _ = done.send(resize_loop(writer));
            })
            .expect("Unable to spawn thread");
    }

    // Block until something ends the session.
    done_rx.recv().expect("finish signal")
}

/// Decode frames from the channel and write payloads to the local terminal.
fn output_loop<R: Read>(reader: FrameReader<R>) -> io::Result<()> {
    let mut stdout = io::stdout();
    let mut stderr = io::stderr();
    for item in reader {
        // eprintln!("Output loop fired");
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
    Ok(())
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
}
