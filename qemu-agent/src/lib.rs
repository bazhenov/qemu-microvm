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
use thiserror::Error;

pub mod qemu;

/// Maximum size of a whole frame on the wire (header + payload).
pub const MAX_FRAME_SIZE: usize = 16384;
/// Maximum payload length: a frame minus its 3-byte header.
pub const MAX_PAYLOAD: usize = MAX_FRAME_SIZE - 3; // 16381

/// Logical channels carried over the single byte stream.
///
/// The numeric values are the on-wire endpoint IDs. Endpoint 3 is reserved.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endpoint {
    /// stdin bytes for the shell (listener: server)
    Stdin = 0,
    /// stdout bytes from the shell (listener: client)
    Stdout = 1,
    /// stderr bytes from the shell (listener: client)
    Stderr = 2,
    /// terminal resize `cols:u16, rows:u16` (listener: server)
    Resize = 4,
    Start = 5,
}

/// Errors produced while encoding or decoding frames.
#[derive(Debug, Error)]
pub enum Error {
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("payload too large: {len} bytes (max {MAX_PAYLOAD})")]
    PayloadTooLarge { len: usize },
    #[error("unexpected eof in the middle of a frame")]
    UnexpectedEof,
}

/// A decoded message: an endpoint id plus its raw payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub endpoint: u8,
    pub payload: Vec<u8>,
}

impl Frame {
    /// Build a frame for an arbitrary endpoint id and payload.
    pub fn new(endpoint: u8, payload: Vec<u8>) -> Self {
        Self { endpoint, payload }
    }

    /// stdin bytes for the shell.
    pub fn stdin(bytes: impl Into<Vec<u8>>) -> Self {
        Self::new(Endpoint::Stdin as u8, bytes.into())
    }

    /// stdout bytes from the shell.
    pub fn stdout(bytes: impl Into<Vec<u8>>) -> Self {
        Self::new(Endpoint::Stdout as u8, bytes.into())
    }

    /// stderr bytes from the shell.
    pub fn stderr(bytes: impl Into<Vec<u8>>) -> Self {
        Self::new(Endpoint::Stderr as u8, bytes.into())
    }

    /// A terminal resize event.
    pub fn resize(cols: u16, rows: u16) -> Self {
        let mut payload = Vec::with_capacity(4);
        payload.extend_from_slice(&cols.to_le_bytes());
        payload.extend_from_slice(&rows.to_le_bytes());
        Self::new(Endpoint::Resize as u8, payload)
    }

    /// Decode a resize payload, if this frame is a well-formed resize.
    pub fn as_resize(&self) -> Option<(u16, u16)> {
        if self.endpoint == Endpoint::Resize as u8 && self.payload.len() == 4 {
            let cols = u16::from_le_bytes([self.payload[0], self.payload[1]]);
            let rows = u16::from_le_bytes([self.payload[2], self.payload[3]]);
            Some((cols, rows))
        } else {
            None
        }
    }

    /// Serialize the frame to `w`.
    pub fn write_to<W: Write>(&self, w: &mut W) -> Result<(), Error> {
        if self.payload.len() > MAX_PAYLOAD {
            return Err(Error::PayloadTooLarge {
                len: self.payload.len(),
            });
        }
        let len = self.payload.len() as u16;
        let mut header = [0u8; 3];
        header[0] = self.endpoint;
        header[1..3].copy_from_slice(&len.to_le_bytes());
        // eprintln!("Sending header: {header:?}");
        w.write_all(&header)?;
        w.write_all(&self.payload)?;
        w.flush()?;
        Ok(())
    }
}

/// Reads frames from an underlying byte stream.
///
/// Iterating yields `Result<Frame, Error>`. A clean EOF at a frame boundary
/// ends the iterator (`None`); an EOF in the middle of a frame surfaces as
/// `Error::UnexpectedEof`.
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
    type Item = Result<Frame, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut header = [0u8; 3];
        let n = match read_full(&mut self.inner, &mut header) {
            Ok(n) => n,
            Err(e) => return Some(Err(e.into())),
        };
        if n == 0 {
            return None; // clean EOF at a frame boundary
        }
        if n < header.len() {
            return Some(Err(Error::UnexpectedEof));
        }

        let endpoint = header[0];
        let len = u16::from_le_bytes([header[1], header[2]]) as usize;
        // eprintln!("Received header:Len: {header:?}, len: {len}");
        if len > MAX_PAYLOAD {
            return Some(Err(Error::PayloadTooLarge { len }));
        }

        let mut payload = vec![0u8; len];
        match read_full(&mut self.inner, &mut payload) {
            Ok(m) if m == len => Some(Ok(Frame { endpoint, payload })),
            Ok(_) => Some(Err(Error::UnexpectedEof)),
            Err(e) => Some(Err(e.into())),
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
    fn max_payload_round_trips() {
        let frame = Frame::stdout(vec![0xab; MAX_PAYLOAD]);
        assert_eq!(round_trip(&frame), frame);
    }

    #[test]
    fn oversize_payload_rejected_on_write() {
        let frame = Frame::stdout(vec![0u8; MAX_PAYLOAD + 1]);
        let mut buf = Vec::new();
        match frame.write_to(&mut buf) {
            Err(Error::PayloadTooLarge { len }) => assert_eq!(len, MAX_PAYLOAD + 1),
            other => panic!("expected PayloadTooLarge, got {other:?}"),
        }
    }

    #[test]
    fn oversize_payload_rejected_on_read() {
        // Hand-craft a header advertising a payload bigger than the ceiling.
        let bogus_len = (MAX_PAYLOAD + 1) as u16;
        let mut bytes = vec![Endpoint::Stdout as u8];
        bytes.extend_from_slice(&bogus_len.to_le_bytes());
        let mut reader = FrameReader::new(io::Cursor::new(bytes));
        match reader.next() {
            Some(Err(Error::PayloadTooLarge { len })) => assert_eq!(len, MAX_PAYLOAD + 1),
            other => panic!("expected PayloadTooLarge, got {other:?}"),
        }
    }

    #[test]
    fn truncated_header_is_unexpected_eof() {
        let reader = FrameReader::new(io::Cursor::new(vec![Endpoint::Stdin as u8]));
        match reader.into_iter().next() {
            Some(Err(Error::UnexpectedEof)) => {}
            other => panic!("expected UnexpectedEof, got {other:?}"),
        }
    }

    #[test]
    fn truncated_payload_is_unexpected_eof() {
        // Header claims 5 bytes of payload, but only 2 follow.
        let mut bytes = vec![Endpoint::Stdin as u8, 5, 0];
        bytes.extend_from_slice(b"ab");
        let mut reader = FrameReader::new(io::Cursor::new(bytes));
        match reader.next() {
            Some(Err(Error::UnexpectedEof)) => {}
            other => panic!("expected UnexpectedEof, got {other:?}"),
        }
    }

    #[test]
    fn unknown_endpoint_still_decodes() {
        // Endpoint 99 is unknown, but the frame must still decode so the
        // stream stays in sync; the caller decides to drop it.
        let frame = Frame::new(99, b"mystery".to_vec());
        let decoded = round_trip(&frame);
        assert_eq!(decoded.endpoint, 99);
        assert_eq!(decoded.payload, b"mystery");
        assert!(decoded.as_resize().is_none());
    }

    #[test]
    fn clean_eof_terminates_iteration() {
        let reader = FrameReader::new(io::Cursor::new(Vec::new()));
        assert_eq!(reader.into_iter().count(), 0);
    }
}
