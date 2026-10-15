//! Native-messaging framing: `u32` little-endian byte length + UTF-8 JSON.
//!
//! Used on both legs (browser ↔ host on stdin/stdout, host ↔ app on the
//! local socket). Works on any `Read`/`Write`.

use std::io::{self, Read, Write};

use serde::Serialize;

use crate::error::{Error, Result};

/// Maximum size of a message on the socket and from the browser (4 MiB).
pub const MAX_MESSAGE_SIZE: usize = 4 * 1024 * 1024;
/// Maximum size of a message from the native host to the browser (1 MiB,
/// Chrome's limit – larger messages make Chrome kill the host).
pub const MAX_BROWSER_MESSAGE_SIZE: usize = 1024 * 1024;

/// Reads one frame. Returns `Ok(None)` on a clean end of stream (EOF before
/// the first byte of a frame). A frame longer than `max_len` fails with
/// [`Error::FrameTooLarge`] *before* anything is allocated; the stream is
/// then out of sync and should be closed. EOF inside a frame is an
/// `UnexpectedEof` I/O error.
pub fn read_frame<R: Read + ?Sized>(reader: &mut R, max_len: usize) -> Result<Option<Vec<u8>>> {
    let mut header = [0u8; 4];
    let mut filled = 0;
    while filled < header.len() {
        match reader.read(&mut header[filled..]) {
            Ok(0) if filled == 0 => return Ok(None),
            Ok(0) => {
                return Err(Error::Io(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "stream ended inside a frame header",
                )))
            }
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(Error::Io(e)),
        }
    }
    let len = usize::try_from(u32::from_le_bytes(header)).unwrap_or(usize::MAX);
    if len > max_len {
        return Err(Error::FrameTooLarge { len, max: max_len });
    }
    let mut payload = vec![0u8; len];
    reader.read_exact(&mut payload).map_err(|e| {
        if e.kind() == io::ErrorKind::UnexpectedEof {
            Error::Io(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "stream ended inside a frame",
            ))
        } else {
            Error::Io(e)
        }
    })?;
    Ok(Some(payload))
}

/// Writes one frame and flushes. Fails with [`Error::FrameTooLarge`]
/// (writing nothing) if `payload` is longer than `max_len`.
pub fn write_frame<W: Write + ?Sized>(
    writer: &mut W,
    payload: &[u8],
    max_len: usize,
) -> Result<()> {
    let too_large = || Error::FrameTooLarge {
        len: payload.len(),
        max: max_len,
    };
    if payload.len() > max_len {
        return Err(too_large());
    }
    let len = u32::try_from(payload.len()).map_err(|_| too_large())?;
    // Header and payload are written separately so the (possibly secret)
    // payload is not copied into another buffer.
    writer.write_all(&len.to_le_bytes())?;
    writer.write_all(payload)?;
    writer.flush()?;
    Ok(())
}

/// Serialises `message` as JSON and writes it as one frame.
pub fn write_json<W: Write + ?Sized, T: Serialize + ?Sized>(
    writer: &mut W,
    message: &T,
    max_len: usize,
) -> Result<()> {
    let bytes = zeroize::Zeroizing::new(serde_json::to_vec(message)?);
    write_frame(writer, &bytes, max_len)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn round_trip_multiple_frames() {
        let mut buf = Vec::new();
        write_frame(&mut buf, br#"{"id":"1"}"#, MAX_MESSAGE_SIZE).unwrap();
        write_frame(&mut buf, b"", MAX_MESSAGE_SIZE).unwrap();
        write_json(&mut buf, &serde_json::json!({"a": "ä"}), MAX_MESSAGE_SIZE).unwrap();
        // Little-endian length prefix.
        assert_eq!(&buf[..4], &[10, 0, 0, 0]);

        let mut r = Cursor::new(buf);
        assert_eq!(
            read_frame(&mut r, MAX_MESSAGE_SIZE).unwrap().unwrap(),
            br#"{"id":"1"}"#
        );
        assert_eq!(read_frame(&mut r, MAX_MESSAGE_SIZE).unwrap().unwrap(), b"");
        assert_eq!(
            read_frame(&mut r, MAX_MESSAGE_SIZE).unwrap().unwrap(),
            "{\"a\":\"ä\"}".as_bytes()
        );
        assert!(read_frame(&mut r, MAX_MESSAGE_SIZE).unwrap().is_none());
    }

    #[test]
    fn little_endian_length_above_255() {
        let payload = vec![b'x'; 0x0102];
        let mut buf = Vec::new();
        write_frame(&mut buf, &payload, MAX_MESSAGE_SIZE).unwrap();
        assert_eq!(&buf[..4], &[0x02, 0x01, 0, 0]);
        assert_eq!(buf.len(), 4 + 0x0102);
    }

    #[test]
    fn limits_are_enforced_on_read_and_write() {
        // Exactly at the limit is fine.
        let payload = vec![b'a'; 16];
        let mut buf = Vec::new();
        write_frame(&mut buf, &payload, 16).unwrap();
        assert_eq!(
            read_frame(&mut Cursor::new(&buf), 16).unwrap().unwrap(),
            payload
        );
        // One byte over the limit.
        assert!(matches!(
            read_frame(&mut Cursor::new(&buf), 15),
            Err(Error::FrameTooLarge { len: 16, max: 15 })
        ));
        let mut out = Vec::new();
        assert!(matches!(
            write_frame(&mut out, &payload, 15),
            Err(Error::FrameTooLarge { len: 16, max: 15 })
        ));
        assert!(
            out.is_empty(),
            "nothing may be written for an oversized frame"
        );

        // A huge announced length is rejected without allocating it.
        let huge = u32::MAX.to_le_bytes();
        assert!(matches!(
            read_frame(&mut Cursor::new(huge), MAX_MESSAGE_SIZE),
            Err(Error::FrameTooLarge { .. })
        ));
        // The contract limits.
        let over = u32::try_from(MAX_MESSAGE_SIZE + 1).unwrap().to_le_bytes();
        assert!(matches!(
            read_frame(&mut Cursor::new(over), MAX_MESSAGE_SIZE),
            Err(Error::FrameTooLarge { .. })
        ));
        let big = vec![b' '; MAX_BROWSER_MESSAGE_SIZE + 1];
        assert!(write_frame(&mut Vec::new(), &big, MAX_BROWSER_MESSAGE_SIZE).is_err());
        assert!(write_frame(&mut Vec::new(), &big, MAX_MESSAGE_SIZE).is_ok());
    }

    #[test]
    fn truncated_input_is_an_error() {
        // Partial header.
        let err = read_frame(&mut Cursor::new([5u8, 0]), MAX_MESSAGE_SIZE).unwrap_err();
        assert!(matches!(err, Error::Io(ref e) if e.kind() == io::ErrorKind::UnexpectedEof));
        // Header announces more bytes than follow.
        let mut buf = 10u32.to_le_bytes().to_vec();
        buf.extend_from_slice(b"abc");
        let err = read_frame(&mut Cursor::new(buf), MAX_MESSAGE_SIZE).unwrap_err();
        assert!(matches!(err, Error::Io(ref e) if e.kind() == io::ErrorKind::UnexpectedEof));
    }

    /// A reader that returns one byte per call and an `Interrupted` error in
    /// between, like a slow pipe.
    struct Trickle {
        data: Vec<u8>,
        pos: usize,
        interrupt: bool,
    }

    impl Read for Trickle {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.interrupt = !self.interrupt;
            if self.interrupt {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "signal"));
            }
            if self.pos >= self.data.len() || buf.is_empty() {
                return Ok(0);
            }
            buf[0] = self.data[self.pos];
            self.pos += 1;
            Ok(1)
        }
    }

    #[test]
    fn partial_reads_are_reassembled() {
        let mut data = Vec::new();
        write_frame(&mut data, b"hello world", MAX_MESSAGE_SIZE).unwrap();
        let mut r = Trickle {
            data,
            pos: 0,
            interrupt: false,
        };
        assert_eq!(
            read_frame(&mut r, MAX_MESSAGE_SIZE).unwrap().unwrap(),
            b"hello world"
        );
        assert!(read_frame(&mut r, MAX_MESSAGE_SIZE).unwrap().is_none());
    }
}
