//! The agent's socket, in either direction (`docs/design/debugger.md`,
//! "Launching, the start and the end"): `--debug` listens on 127.0.0.1 at
//! a port the system chooses and guards it with a token, `--debug-connect`
//! connects to a client that listens. Each connection is read on a thread
//! of its own, which hands the agent every message it reads.

use std::io::BufReader;
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, SyncSender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::agent::Input;
use super::wire;

/// The token's length: 128 bits from the operating system's random source.
const TOKEN_BYTES: usize = 16;

/// The secret a client presents in `attach` to the listening socket, which
/// any user of the machine can reach.
pub(crate) struct Token {
    bytes: [u8; TOKEN_BYTES],
}

impl Token {
    pub(crate) fn generate() -> Result<Token, ns_memory::RandomError> {
        let mut bytes = [0; TOKEN_BYTES];
        ns_memory::fill_random(&mut bytes)?;
        Ok(Token { bytes })
    }

    /// The token as the listening line prints it: 32 lowercase hex digits.
    pub(crate) fn hex(&self) -> String {
        self.bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    /// Whether `presented` is the token. Compared in constant time, so that
    /// how long a refusal takes says nothing of how much of a guess was
    /// right; only the length, which is no secret, decides at once.
    pub(crate) fn is_presented_by(&self, presented: &str) -> bool {
        let expected = self.hex();
        if presented.len() != expected.len() {
            return false;
        }
        let difference = expected
            .bytes()
            .zip(presented.bytes())
            .fold(0_u8, |difference, (expected, presented)| {
                difference | (expected ^ presented)
            });
        std::hint::black_box(difference) == 0
    }
}

/// How long the acceptor waits after a failed `accept`.
const ACCEPT_RETRY: std::time::Duration = std::time::Duration::from_millis(100);

/// The listening socket on 127.0.0.1, at a port the system chooses.
pub(crate) fn listen() -> std::io::Result<TcpListener> {
    TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
}

/// Accepts one connection at a time: each is handed to the agent, and the
/// next is accepted only once the agent says the session ended. A client
/// that connects meanwhile waits in the listening socket's backlog.
pub(crate) fn spawn_acceptor(listener: TcpListener, inputs: Sender<Input>, next: Receiver<()>) {
    std::thread::Builder::new()
        .name("ns-next debugger acceptor".to_string())
        .spawn(move || loop {
            let Ok((stream, _)) = listener.accept() else {
                // A connection that failed before it was accepted: the
                // next one is awaited, after a pause, so that an error
                // that persists costs no processor.
                std::thread::sleep(ACCEPT_RETRY);
                continue;
            };
            if inputs.send(Input::Connected(stream)).is_err() || next.recv().is_err() {
                return;
            }
        })
        .expect("spawn the debugger's acceptor");
}

/// Reads the connection numbered `connection` until it ends, handing the
/// agent each message; a frame that is no DAP message ends it too, since
/// nothing after it can be framed. Fails when no thread can be started
/// for it.
pub(crate) fn spawn_reader(
    stream: TcpStream,
    connection: u64,
    inputs: Sender<Input>,
) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("ns-next debugger reader".to_string())
        .spawn(move || {
            let mut reader = BufReader::new(stream);
            while let Ok(Some(message)) = wire::read_message(&mut reader) {
                if inputs
                    .send(Input::Request {
                        connection,
                        message,
                    })
                    .is_err()
                {
                    return;
                }
            }
            let _ = inputs.send(Input::Closed { connection });
        })
        .map(drop)
}

/// Tells the agent, once `deadline` passed, that the connection numbered
/// `connection` had that long to attach; the agent ends it unless it did.
/// Fails when no thread can be started for it.
pub(crate) fn spawn_attach_deadline(
    deadline: std::time::Duration,
    connection: u64,
    inputs: Sender<Input>,
) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("ns-next debugger attach deadline".to_string())
        .spawn(move || {
            std::thread::sleep(deadline);
            let _ = inputs.send(Input::AttachDeadline { connection });
        })
        .map(drop)
}

/// How many messages may wait for a client to read them before the agent
/// gives the client up: far more than a session's burst of events and
/// answers, so that only a client that stopped reading fills it.
const WRITE_QUEUE: usize = 1024;

/// The agent's end of a connection's writer thread, which writes every
/// message the agent queues, in order, so that the agent never waits on
/// the socket. Each write is bounded: by the time left to the attach
/// deadline while the client has not attached, by the write bound once it
/// has. A write that runs out of its bound, or fails, ends the connection
/// and tells the agent the client left. Dropping this end lets the thread
/// write what is queued and then shut the connection down.
pub(crate) struct Writer {
    queue: SyncSender<serde_json::Value>,
    attached: Arc<AtomicBool>,
    thread: std::thread::JoinHandle<()>,
}

impl Writer {
    /// Queues `message`; `false` when the queue is full -- the client has
    /// not read for that long -- or the writer gave up.
    pub(crate) fn send(&self, message: serde_json::Value) -> bool {
        self.queue.try_send(message).is_ok()
    }

    /// The client attached: its writes are bounded by the write bound from
    /// here on, not by the attach deadline.
    pub(crate) fn mark_attached(&self) {
        self.attached.store(true, Ordering::SeqCst);
    }

    /// Waits until what is queued is written, or the bound ran out, and
    /// the connection is shut down.
    pub(crate) fn finish(self) {
        drop(self.queue);
        let _ = self.thread.join();
    }
}

/// Starts the writer of the connection numbered `connection`.
pub(crate) fn spawn_writer(
    mut stream: TcpStream,
    connection: u64,
    inputs: Sender<Input>,
    attach_by: Option<Instant>,
    bound: Duration,
) -> std::io::Result<Writer> {
    let (queue, queued) = std::sync::mpsc::sync_channel::<serde_json::Value>(WRITE_QUEUE);
    let attached = Arc::new(AtomicBool::new(false));
    let attached_here = Arc::clone(&attached);
    let thread = std::thread::Builder::new()
        .name("ns-next debugger writer".to_string())
        .spawn(move || {
            let mut failed = false;
            for message in queued {
                let limit = match attach_by {
                    Some(attach_by) if !attached_here.load(Ordering::SeqCst) => {
                        attach_by.saturating_duration_since(Instant::now())
                    }
                    _ => bound,
                };
                // A zero timeout is refused by the socket, and would mean
                // none.
                if limit.is_zero()
                    || stream.set_write_timeout(Some(limit)).is_err()
                    || wire::write_message(&mut stream, &message).is_err()
                {
                    failed = true;
                    break;
                }
            }
            let _ = stream.shutdown(std::net::Shutdown::Both);
            if failed {
                let _ = inputs.send(Input::Closed { connection });
            }
        })?;
    Ok(Writer {
        queue,
        attached,
        thread,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_is_32_lowercase_hex_digits_and_only_itself_matches() {
        let token = Token {
            bytes: [
                0x00, 0x01, 0xab, 0xcd, 0xef, 0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80, 0x90,
                0xa0, 0xff,
            ],
        };
        let hex = token.hex();
        assert_eq!(hex, "0001abcdef102030405060708090a0ff");
        assert!(token.is_presented_by(&hex));
        assert!(!token.is_presented_by(&hex.to_uppercase()));
        assert!(!token.is_presented_by(&hex[1..]));
        assert!(!token.is_presented_by(""));
        let mut wrong = hex.into_bytes();
        wrong[31] = b'e';
        assert!(!token.is_presented_by(&String::from_utf8(wrong).expect("hex digits")));
    }
}
