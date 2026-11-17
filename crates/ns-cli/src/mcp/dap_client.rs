//! The MCP server's client of the debugger's agent: DAP over the socket
//! the agent connected to, framed by `debug/wire.rs`, so that the server
//! and the agent read and write one framing. A reader thread takes each
//! message off the socket; the session pulls them one at a time, bounded
//! by a deadline, and handles the events itself.

use std::io::{BufReader, Write};
use std::net::TcpStream;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::Instant;

use serde_json::{Value, json};

use crate::debug::wire;

/// What the next message off the connection is.
pub(crate) enum Next {
    Message(Value),
    /// The agent closed the connection, or sent what is no DAP frame.
    Closed,
    /// Nothing came before the deadline.
    TimedOut,
}

/// One connection to the agent.
pub(crate) struct DapClient {
    writer: TcpStream,
    messages: Receiver<Option<Value>>,
    next_seq: i64,
    closed: bool,
}

impl DapClient {
    /// A client over `stream`, whose messages a reader thread takes off it.
    pub(crate) fn over(stream: TcpStream) -> std::io::Result<DapClient> {
        // Requests and responses answer one another, which Nagle's
        // algorithm would hold back.
        stream.set_nodelay(true)?;
        let reader = stream.try_clone()?;
        let (sender, messages) = mpsc::channel();
        std::thread::Builder::new()
            .name("ns-next mcp dap reader".to_string())
            .spawn(move || {
                let mut reader = BufReader::new(reader);
                loop {
                    // The agent is the server's own child, so a message is
                    // bounded as an attached client's is.
                    let message =
                        wire::read_message(&mut reader, || wire::MAXIMUM_LENGTH).unwrap_or(None);
                    let ended = message.is_none();
                    if sender.send(message).is_err() || ended {
                        break;
                    }
                }
            })?;
        Ok(DapClient {
            writer: stream,
            messages,
            next_seq: 1,
            closed: false,
        })
    }

    /// Sends the request `command` and answers its sequence number.
    pub(crate) fn send(&mut self, command: &str, arguments: Value) -> std::io::Result<i64> {
        let seq = self.next_seq;
        self.next_seq += 1;
        let message = json!({
            "seq": seq,
            "type": "request",
            "command": command,
            "arguments": arguments,
        });
        self.writer.write_all(&wire::frame(&message)?)?;
        self.writer.flush()?;
        Ok(seq)
    }

    /// The next message, waiting until `deadline` at the latest.
    pub(crate) fn next(&mut self, deadline: Instant) -> Next {
        if self.closed {
            return Next::Closed;
        }
        let timeout = deadline.saturating_duration_since(Instant::now());
        match self.messages.recv_timeout(timeout) {
            Ok(Some(message)) => Next::Message(message),
            Ok(None) | Err(RecvTimeoutError::Disconnected) => {
                self.closed = true;
                Next::Closed
            }
            Err(RecvTimeoutError::Timeout) => Next::TimedOut,
        }
    }

    /// Closes the connection both ways.
    pub(crate) fn close(&mut self) {
        let _ = self.writer.shutdown(std::net::Shutdown::Both);
    }
}
