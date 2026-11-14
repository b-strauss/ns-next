//! DAP's framing: each message a JSON value after a `Content-Length`
//! header and a blank line (`docs/design/debugger.md`, decision 6). Written
//! by hand on `serde_json`, so that no protocol crate reaches `ns-next`.
//!
//! The listening socket answers whoever connects before any token is
//! checked, so a frame is bounded before it is read: a header line, the
//! number of header lines and a message's length.

use std::io::{BufRead, Read, Write};

/// The longest message read: far above any request a client sends, and a
/// bound on what one connection can make the agent allocate.
const MAXIMUM_LENGTH: usize = 64 * 1024 * 1024;

/// The longest header line read.
const MAXIMUM_HEADER_LINE: u64 = 1024;

/// The most header lines one message may have; DAP defines two.
const MAXIMUM_HEADER_LINES: usize = 16;

const CONTENT_LENGTH: &str = "content-length:";

/// Why a frame could not be read.
#[derive(Debug)]
pub(crate) enum WireError {
    Read(std::io::Error),
    /// The stream ended inside a frame.
    TruncatedFrame,
    /// A header line was too long, or there were too many of them.
    OversizedHeader,
    /// The header held no `Content-Length`, or one that is no number.
    MissingLength,
    /// The length named is above the bound.
    TooLong(usize),
    MalformedJson(serde_json::Error),
}

impl std::fmt::Display for WireError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WireError::Read(error) => write!(formatter, "{error}"),
            WireError::TruncatedFrame => write!(formatter, "the stream ended inside a message"),
            WireError::OversizedHeader => write!(formatter, "a message's header is too long"),
            WireError::MissingLength => {
                write!(formatter, "a message's header has no Content-Length")
            }
            WireError::TooLong(length) => write!(
                formatter,
                "a message of {length} bytes is longer than the {MAXIMUM_LENGTH} the agent reads"
            ),
            WireError::MalformedJson(error) => write!(formatter, "a message is no JSON: {error}"),
        }
    }
}

/// The next message off `reader`, or `None` at a clean end of the stream,
/// one before a frame begins.
pub(crate) fn read_message(
    reader: &mut impl BufRead,
) -> Result<Option<serde_json::Value>, WireError> {
    let mut length = None;
    let mut lines = 0;
    loop {
        let mut line = Vec::new();
        let read = reader
            .by_ref()
            .take(MAXIMUM_HEADER_LINE)
            .read_until(b'\n', &mut line)
            .map_err(WireError::Read)?;
        if read == 0 {
            return if lines == 0 {
                Ok(None)
            } else {
                Err(WireError::TruncatedFrame)
            };
        }
        if line.last() != Some(&b'\n') {
            return Err(if read as u64 == MAXIMUM_HEADER_LINE {
                WireError::OversizedHeader
            } else {
                WireError::TruncatedFrame
            });
        }
        lines += 1;
        if lines > MAXIMUM_HEADER_LINES {
            return Err(WireError::OversizedHeader);
        }
        let line = String::from_utf8_lossy(&line);
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        // Header names are case-insensitive, as HTTP's are.
        if line.len() >= CONTENT_LENGTH.len()
            && line.is_char_boundary(CONTENT_LENGTH.len())
            && line[..CONTENT_LENGTH.len()].eq_ignore_ascii_case(CONTENT_LENGTH)
        {
            let value = line[CONTENT_LENGTH.len()..].trim();
            length = Some(
                value
                    .parse::<usize>()
                    .map_err(|_| WireError::MissingLength)?,
            );
        }
    }
    let length = length.ok_or(WireError::MissingLength)?;
    if length > MAXIMUM_LENGTH {
        return Err(WireError::TooLong(length));
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).map_err(|error| {
        if error.kind() == std::io::ErrorKind::UnexpectedEof {
            WireError::TruncatedFrame
        } else {
            WireError::Read(error)
        }
    })?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(WireError::MalformedJson)
}

/// Writes `message` as one frame and flushes it.
pub(crate) fn write_message(
    writer: &mut impl Write,
    message: &serde_json::Value,
) -> std::io::Result<()> {
    let body = serde_json::to_vec(message).map_err(std::io::Error::other)?;
    let mut frame = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    frame.extend_from_slice(&body);
    writer.write_all(&frame)?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn read_all(bytes: &[u8]) -> Result<Option<serde_json::Value>, WireError> {
        read_message(&mut std::io::BufReader::new(bytes))
    }

    #[test]
    fn wire_round_trips_a_message() {
        let messages = [
            json!({ "seq": 1, "type": "request", "command": "initialize" }),
            json!({ "seq": 2, "type": "event", "event": "output", "body": { "output": "é\r\n" } }),
        ];
        let mut bytes = Vec::new();
        for message in &messages {
            write_message(&mut bytes, message).expect("write to a vector");
        }
        let mut reader = std::io::BufReader::new(bytes.as_slice());
        for message in &messages {
            let read = read_message(&mut reader).expect("a frame");
            assert_eq!(read.as_ref(), Some(message));
        }
        assert!(matches!(read_message(&mut reader), Ok(None)));
    }

    #[test]
    fn wire_refuses_a_missing_length() {
        assert!(matches!(
            read_all(b"Content-Type: application/json\r\n\r\n{}"),
            Err(WireError::MissingLength)
        ));
        assert!(matches!(
            read_all(b"Content-Length: many\r\n\r\n{}"),
            Err(WireError::MissingLength)
        ));
    }

    #[test]
    fn wire_reads_a_header_in_any_case_and_with_other_fields() {
        let read = read_all(b"content-length: 2\r\nContent-Type: x\r\n\r\n{}").expect("a frame");
        assert_eq!(read, Some(json!({})));
    }

    #[test]
    fn wire_refuses_a_length_above_the_bound_before_reading_it() {
        let header = format!("Content-Length: {}\r\n\r\n", MAXIMUM_LENGTH + 1);
        assert!(matches!(
            read_all(header.as_bytes()),
            Err(WireError::TooLong(length)) if length == MAXIMUM_LENGTH + 1
        ));
    }

    #[test]
    fn wire_refuses_malformed_json_and_a_truncated_frame() {
        assert!(matches!(
            read_all(b"Content-Length: 3\r\n\r\n{x}"),
            Err(WireError::MalformedJson(_))
        ));
        assert!(matches!(
            read_all(b"Content-Length: 10\r\n\r\n{}"),
            Err(WireError::TruncatedFrame)
        ));
        assert!(matches!(
            read_all(b"Content-Length: 2\r\n"),
            Err(WireError::TruncatedFrame)
        ));
    }

    #[test]
    fn wire_refuses_an_endless_header_line() {
        let line = vec![b'x'; 4096];
        assert!(matches!(read_all(&line), Err(WireError::OversizedHeader)));
    }
}
