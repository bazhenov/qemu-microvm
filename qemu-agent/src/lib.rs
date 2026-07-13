//! Wire protocol for the VM serial multiplexer.
//!
//! A single byte stream carries multiple logical channels ("endpoints").
//! Every message names a `u8` endpoint and carries a payload; the endpoint
//! determines what the payload means.
//!
//! Frame layout (little-endian):
//!
//! ```text
//! +------------+----------------+----------+
//! | endpoint:1 | payload_len:2  | payload  |
//! +------------+----------------+----------+
//! ```
//!
//! `payload_len` does not include the 3-byte header. The channel is assumed
//! reliable: no CRC, no magic byte, no resync.

use nix::sys::termios;
use std::{
    io::{self, Read, Write},
    os::fd::AsFd,
};

pub mod qemu;

/// Maximum size of a whole frame on the wire (header + payload).
pub const MAX_FRAME_SIZE: usize = 16384;
pub const HEADER_SIZE: usize = 3;
/// Maximum payload length: a frame minus its 3-byte header.
pub const MAX_PAYLOAD: usize = MAX_FRAME_SIZE - HEADER_SIZE; // 16381

/// Logical frame types carried over the single byte stream.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameType {
    Unknown,
    Start = 1,
    /// stdin bytes for the shell (listener: server)
    Stdin = 2,
    /// stdout bytes from the shell (listener: client)
    Stdout = 3,
    /// stderr bytes from the shell (listener: client)
    Stderr = 4,
    /// terminal resize `cols:u16, rows:u16` (listener: server)
    Resize = 5,
    /// exit code of the shell process `code:i32` (listener: client)
    Exit = 6,
}

impl From<u8> for FrameType {
    fn from(value: u8) -> Self {
        if value == FrameType::Stdin as u8 {
            FrameType::Stdin
        } else if value == FrameType::Start as u8 {
            FrameType::Start
        } else if value == FrameType::Stdout as u8 {
            FrameType::Stdout
        } else if value == FrameType::Stderr as u8 {
            FrameType::Stderr
        } else if value == FrameType::Resize as u8 {
            FrameType::Resize
        } else if value == FrameType::Exit as u8 {
            FrameType::Exit
        } else {
            FrameType::Unknown
        }
    }
}

/// Error for a payload that exceeds [`MAX_PAYLOAD`], reported as
/// [`io::ErrorKind::InvalidInput`].
fn payload_too_large(len: usize) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("payload too large: {len} bytes (max {MAX_PAYLOAD})"),
    )
}

/// Error for an EOF reached in the middle of a frame.
fn unexpected_eof() -> io::Error {
    io::Error::new(
        io::ErrorKind::UnexpectedEof,
        "unexpected eof in the middle of a frame",
    )
}

/// A decoded message: an endpoint id plus its raw payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub frame_type: FrameType,
    pub payload: Vec<u8>,
}

impl Frame {
    /// Build a frame for an arbitrary endpoint id and payload.
    pub fn new(frame_type: FrameType, payload: Vec<u8>) -> Self {
        Self {
            frame_type,
            payload,
        }
    }

    /// stdin bytes for the shell.
    pub fn stdin(bytes: impl Into<Vec<u8>>) -> Self {
        Self::new(FrameType::Stdin, bytes.into())
    }

    /// stdout bytes from the shell.
    pub fn stdout(bytes: impl Into<Vec<u8>>) -> Self {
        Self::new(FrameType::Stdout, bytes.into())
    }

    /// stderr bytes from the shell.
    pub fn stderr(bytes: impl Into<Vec<u8>>) -> Self {
        Self::new(FrameType::Stderr, bytes.into())
    }

    /// A terminal resize event.
    pub fn resize(cols: u16, rows: u16) -> Self {
        let mut payload = Vec::with_capacity(4);
        payload.extend_from_slice(&cols.to_le_bytes());
        payload.extend_from_slice(&rows.to_le_bytes());
        Self::new(FrameType::Resize, payload)
    }

    /// The exit code of the shell process.
    pub fn exit(code: i32) -> Self {
        Self::new(FrameType::Exit, code.to_le_bytes().to_vec())
    }

    /// Decode an exit payload, if this frame is a well-formed exit.
    pub fn as_exit(&self) -> Option<i32> {
        if self.frame_type == FrameType::Exit && self.payload.len() == 4 {
            Some(i32::from_le_bytes([
                self.payload[0],
                self.payload[1],
                self.payload[2],
                self.payload[3],
            ]))
        } else {
            None
        }
    }

    /// Decode a resize payload, if this frame is a well-formed resize.
    pub fn as_resize(&self) -> Option<(u16, u16)> {
        if self.frame_type == FrameType::Resize && self.payload.len() == 4 {
            let cols = u16::from_le_bytes([self.payload[0], self.payload[1]]);
            let rows = u16::from_le_bytes([self.payload[2], self.payload[3]]);
            Some((cols, rows))
        } else {
            None
        }
    }

    /// Serialize the frame to `w`.
    pub fn write_to<W: Write>(&self, w: &mut W) -> io::Result<()> {
        if self.payload.len() > MAX_PAYLOAD {
            return Err(payload_too_large(self.payload.len()));
        }
        let len = self.payload.len() as u16;
        let mut header = [0u8; HEADER_SIZE];
        header[0] = self.frame_type as u8;
        header[1..3].copy_from_slice(&len.to_le_bytes());
        w.write_all(&header)?;
        w.write_all(&self.payload)?;
        w.flush()?;
        Ok(())
    }
}

/// Reads frames from an underlying byte stream.
///
/// Iterating yields `io::Result<Frame>`. A clean EOF at a frame boundary
/// ends the iterator (`None`); an EOF in the middle of a frame surfaces as
/// an [`io::ErrorKind::UnexpectedEof`] error.
pub struct FrameReader<R: Read> {
    inner: R,
}

impl<R: Read> FrameReader<R> {
    pub fn new(inner: R) -> Self {
        Self { inner }
    }

    /// Consume the reader and hand back the underlying stream.
    pub fn into_inner(self) -> R {
        self.inner
    }
}

/// Read until `buf` is full or EOF. Returns the number of bytes actually read;
/// a value smaller than `buf.len()` means EOF was reached early.
fn read_full<R: Read>(r: &mut R, buf: &mut [u8]) -> io::Result<usize> {
    let mut total = 0;
    while total < buf.len() {
        match r.read(&mut buf[total..]) {
            Ok(0) => break,
            Ok(n) => total += n,
            Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(total)
}

impl<R: Read> Iterator for FrameReader<R> {
    type Item = io::Result<Frame>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut header = [0u8; 3];
        let n = match read_full(&mut self.inner, &mut header) {
            Ok(n) => n,
            Err(e) => return Some(Err(e)),
        };
        if n == 0 {
            return None; // clean EOF at a frame boundary
        }
        if n < header.len() {
            return Some(Err(unexpected_eof()));
        }

        let len = u16::from_le_bytes([header[1], header[2]]) as usize;
        if len > MAX_PAYLOAD {
            return Some(Err(payload_too_large(len)));
        }

        let mut payload = vec![0u8; len];
        match read_full(&mut self.inner, &mut payload) {
            Ok(m) if m == len => Some(Ok(Frame {
                frame_type: FrameType::from(header[0]),
                payload,
            })),
            Ok(_) => Some(Err(unexpected_eof())),
            Err(e) => Some(Err(e)),
        }
    }
}

/// Enabling raw mode for master and slave to make it behave like a serial port/pipe
///
/// PTY's are not pipes or serial ports. There is a thing called Line Discipline that it applied
/// to every PTY in the kernel. It does several things:
///
/// - buffers input, so that it's available on counterparty only after newline is recieved
/// - treats some characters in a special way (Ctrl+C sends SIGINT to a process group session leader, etc)
///
/// Because here we just want to use PTY as a full duplex pipe (otherwise we would need to create 2 pipes)
/// we want to disable all this machinery.
pub fn configure_raw_pty(tty: &impl AsFd) -> io::Result<()> {
    let mut tio = termios::tcgetattr(tty)?;
    termios::cfmakeraw(&mut tio);
    termios::tcsetattr(tty, termios::SetArg::TCSANOW, &tio)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encode a frame and decode it back through a `FrameReader`.
    fn round_trip(frame: &Frame) -> Frame {
        let mut buf = Vec::new();
        frame.write_to(&mut buf).expect("encode");
        let mut reader = FrameReader::new(io::Cursor::new(buf));
        let decoded = reader.next().expect("a frame").expect("no error");
        assert!(reader.next().is_none(), "exactly one frame");
        decoded
    }

    #[test]
    fn round_trip_every_endpoint() {
        let frames = [
            Frame::stdin(b"hello stdin".to_vec()),
            Frame::stdout(b"hello stdout".to_vec()),
            Frame::stderr(b"hello stderr".to_vec()),
            Frame::resize(80, 24),
            Frame::exit(42),
        ];
        for frame in &frames {
            assert_eq!(&round_trip(frame), frame);
        }
    }

    #[test]
    fn empty_payload_round_trips() {
        let frame = Frame::stdin(Vec::new());
        assert_eq!(round_trip(&frame), frame);
    }

    #[test]
    fn resize_payload_decodes() {
        let frame = Frame::resize(120, 40);
        assert_eq!(frame.as_resize(), Some((120, 40)));
    }

    #[test]
    fn exit_payload_decodes() {
        let frame = Frame::exit(137);
        assert_eq!(frame.as_exit(), Some(137));
        assert_eq!(Frame::exit(0).as_exit(), Some(0));
        assert!(Frame::stdout(b"not an exit".to_vec()).as_exit().is_none());
    }

    #[test]
    fn max_payload_round_trips() {
        let frame = Frame::stdout(vec![0xab; MAX_PAYLOAD]);
        assert_eq!(round_trip(&frame), frame);
    }

    #[test]
    fn oversize_payload_rejected_on_write() {
        let frame = Frame::stdout(vec![0u8; MAX_PAYLOAD + 1]);
        let mut buf = Vec::new();
        let err = frame.write_to(&mut buf).expect_err("expected an error");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn oversize_payload_rejected_on_read() {
        // Hand-craft a header advertising a payload bigger than the ceiling.
        let bogus_len = (MAX_PAYLOAD + 1) as u16;
        let mut bytes = vec![FrameType::Stdout as u8];
        bytes.extend_from_slice(&bogus_len.to_le_bytes());
        let mut reader = FrameReader::new(io::Cursor::new(bytes));
        let err = reader
            .next()
            .expect("a result")
            .expect_err("expected an error");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn truncated_header_is_unexpected_eof() {
        let reader = FrameReader::new(io::Cursor::new(vec![FrameType::Stdin as u8]));
        let err = reader
            .into_iter()
            .next()
            .expect("a result")
            .expect_err("expected an error");
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn truncated_payload_is_unexpected_eof() {
        // Header claims 5 bytes of payload, but only 2 follow.
        let mut bytes = vec![FrameType::Stdin as u8, 5, 0];
        bytes.extend_from_slice(b"ab");
        let mut reader = FrameReader::new(io::Cursor::new(bytes));
        let err = reader
            .next()
            .expect("a result")
            .expect_err("expected an error");
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn unknown_endpoint_still_decodes() {
        // Endpoint 99 is unknown, but the frame must still decode so the
        // stream stays in sync; the caller decides to drop it.
        let frame = Frame::new(FrameType::Unknown, b"mystery".to_vec());
        let decoded = round_trip(&frame);
        assert_eq!(decoded.frame_type, FrameType::Unknown);
        assert_eq!(decoded.payload, b"mystery");
        assert!(decoded.as_resize().is_none());
    }

    #[test]
    fn clean_eof_terminates_iteration() {
        let reader = FrameReader::new(io::Cursor::new(Vec::new()));
        assert_eq!(reader.into_iter().count(), 0);
    }
}
