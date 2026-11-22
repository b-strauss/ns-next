//! The MCP server's client of the debugger's agent: DAP over the socket
//! the agent connected to, framed by `debug/wire.rs`, so that the server
//! and the agent read and write one framing. A reader thread takes each
//! message off the socket into the session's inbox, which its worker owns
//! and lends to `next`: the session reads its program's messages and the
//! calls made of it from one channel, as the DAP agent reads its inputs,
//! and handles the events itself.

use std::io::{BufReader, Write};
use std::net::TcpStream;
use std::sync::mpsc::{RecvTimeoutError, Sender};
use std::time::Instant;

use serde_json::{Value, json};

use super::worker::{Call, Inbox, Input};
use crate::debug::wire;

/// What the next input of the session's inbox is.
pub(crate) enum Next {
    Message(Value),
    /// The agent closed the connection, or sent what is no DAP frame.
    Closed,
    /// Nothing came before the deadline.
    TimedOut,
    /// A tool call of the session's, made while it waits.
    Call(Call),
    /// The client cancelled its request of this identifier.
    Cancelled(Value),
    /// The server ends.
    End,
}

/// One connection to the agent.
pub(crate) struct DapClient {
    writer: TcpStream,
    next_seq: i64,
    closed: bool,
}

impl DapClient {
    /// A client over `stream`, whose messages a reader thread takes off it
    /// into `inbox`, the session's.
    pub(crate) fn over(stream: TcpStream, inbox: Sender<Input>) -> std::io::Result<DapClient> {
        // Requests and responses answer one another, which Nagle's
        // algorithm would hold back.
        stream.set_nodelay(true)?;
        let reader = stream.try_clone()?;
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
                    if inbox.send(Input::Dap(message)).is_err() || ended {
                        break;
                    }
                }
            })?;
        Ok(DapClient {
            writer: stream,
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

    /// The next input of `inbox`, waiting until `deadline` at the latest;
    /// once the connection closed, `Closed` at once.
    pub(crate) fn next(&mut self, deadline: Instant, inbox: &Inbox) -> Next {
        if self.closed {
            return Next::Closed;
        }
        match inbox.receive(Some(deadline)) {
            Ok(Input::Dap(message)) => self.received(message),
            Ok(Input::Call(call)) => Next::Call(call),
            Ok(Input::Cancelled(identifier)) => Next::Cancelled(identifier),
            // Every sender gone: a live session holds one in its handle,
            // so only the detached inbox a dropped session leaves
            // (`Session::take_inbox`) answers this. It is the end too, and
            // every wait stops at the end, so none spins on it.
            Ok(Input::End) | Err(RecvTimeoutError::Disconnected) => Next::End,
            Err(RecvTimeoutError::Timeout) => Next::TimedOut,
        }
    }

    /// What a message the reader thread took off the connection is: the
    /// message, or, for none, the connection's close.
    pub(crate) fn received(&mut self, message: Option<Value>) -> Next {
        if self.closed {
            return Next::Closed;
        }
        match message {
            Some(message) => Next::Message(message),
            None => {
                self.closed = true;
                Next::Closed
            }
        }
    }

    /// Closes the connection both ways.
    pub(crate) fn close(&mut self) {
        let _ = self.writer.shutdown(std::net::Shutdown::Both);
    }
}
