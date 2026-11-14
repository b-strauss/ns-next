//! The agent's socket, in either direction (`docs/design/debugger.md`,
//! "Launching, the start and the end"): `--debug` listens on 127.0.0.1 at
//! a port the system chooses and guards it with a token, `--debug-connect`
//! connects to a client that listens. Each connection is read on a thread
//! of its own, which hands the agent every message it reads.

use std::io::{BufReader, Write};
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
/// nothing after it can be framed, and so does one longer than the
/// connection is read: with `attached`, the listening mode's flag of its
/// `attach`, a peer that has not attached is read short frames only
/// (`wire::UNATTACHED_MAXIMUM_LENGTH`). Fails when no thread can be
/// started for it.
pub(crate) fn spawn_reader(
    stream: TcpStream,
    connection: u64,
    inputs: Sender<Input>,
    attached: Option<Arc<AtomicBool>>,
) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("ns-next debugger reader".to_string())
        .spawn(move || {
            let mut reader = BufReader::new(stream);
            let maximum_length = || match &attached {
                Some(attached) if !attached.load(Ordering::SeqCst) => {
                    wire::UNATTACHED_MAXIMUM_LENGTH
                }
                _ => wire::MAXIMUM_LENGTH,
            };
            while let Ok(Some(message)) = wire::read_message(&mut reader, maximum_length) {
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

/// How many messages may wait for a client to read them before the agent
/// gives the client up: far more than a session's burst of events and
/// answers, so that only a client that stopped reading fills it.
const WRITE_QUEUE: usize = 1024;

/// The agent's end of a connection's writer thread, which writes every
/// message the agent queues, in order, so that the agent never waits on
/// the socket. Each message's whole write is bounded: by the attach
/// deadline while the client has not attached, by the write bound from
/// the message's start once it has, so that a client that reads a few
/// bytes at a time cannot stretch one message's write without end. A
/// write that runs out of its bound, or fails, ends the connection and
/// tells the agent the client left. Dropping this end lets the thread
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
    /// here on, not by the attach deadline, and its reader reads frames as
    /// long as an attached client's.
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

/// Starts the writer of the connection numbered `connection`; `attached`
/// is the flag `Writer::mark_attached` sets, shared with the reader.
pub(crate) fn spawn_writer(
    mut stream: TcpStream,
    connection: u64,
    inputs: Sender<Input>,
    attach_by: Option<Instant>,
    bound: Duration,
    attached: Arc<AtomicBool>,
) -> std::io::Result<Writer> {
    let (queue, queued) = std::sync::mpsc::sync_channel::<serde_json::Value>(WRITE_QUEUE);
    let attached_here = Arc::clone(&attached);
    let thread = std::thread::Builder::new()
        .name("ns-next debugger writer".to_string())
        .spawn(move || {
            let mut failed = false;
            for message in queued {
                let deadline = match attach_by {
                    Some(attach_by) if !attached_here.load(Ordering::SeqCst) => attach_by,
                    _ => Instant::now() + bound,
                };
                let written =
                    wire::frame(&message).and_then(|frame| write_by(&mut stream, &frame, deadline));
                if written.is_err() {
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

/// What a writer writes to: the socket, or a test's stand-in for one.
trait Sink {
    /// Bounds the next write's wait for the peer to read.
    fn set_timeout(&mut self, timeout: Duration) -> std::io::Result<()>;
    /// Writes some of `bytes`, answering how many.
    fn write_some(&mut self, bytes: &[u8]) -> std::io::Result<usize>;
}

impl Sink for TcpStream {
    fn set_timeout(&mut self, timeout: Duration) -> std::io::Result<()> {
        self.set_write_timeout(Some(timeout))
    }

    fn write_some(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.write(bytes)
    }
}

/// Writes all of `bytes` by `deadline`. Each write waits only for what is
/// left of it, since a socket's timeout bounds one write, and a peer that
/// reads a little now and then lets every write go on.
fn write_by(sink: &mut impl Sink, bytes: &[u8], deadline: Instant) -> std::io::Result<()> {
    let mut written = 0;
    while written < bytes.len() {
        // A zero timeout is refused by the socket, and would mean none.
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(std::io::ErrorKind::TimedOut.into());
        }
        sink.set_timeout(left)?;
        match sink.write_some(&bytes[written..]) {
            Ok(0) => return Err(std::io::ErrorKind::WriteZero.into()),
            Ok(count) => written += count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A peer that reads one byte at a time, each after `pause`, and the
    /// timeouts it was given.
    struct Trickle {
        pause: Duration,
        timeouts: Vec<Duration>,
    }

    impl Sink for Trickle {
        fn set_timeout(&mut self, timeout: Duration) -> std::io::Result<()> {
            self.timeouts.push(timeout);
            Ok(())
        }

        fn write_some(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            std::thread::sleep(self.pause);
            Ok(bytes.len().min(1))
        }
    }

    #[test]
    fn a_message_s_whole_write_is_bounded_however_the_peer_reads() {
        let mut trickle = Trickle {
            pause: Duration::from_millis(2),
            timeouts: Vec::new(),
        };
        let bound = Duration::from_millis(50);
        // 200 bytes take the trickle 400 ms at least, each write a byte.
        let written = write_by(&mut trickle, &[0; 200], Instant::now() + bound);
        assert!(
            matches!(&written, Err(error) if error.kind() == std::io::ErrorKind::TimedOut),
            "{written:?}"
        );
        assert!(trickle.timeouts.len() < 200, "{}", trickle.timeouts.len());
        assert!(trickle.timeouts.iter().all(|timeout| *timeout <= bound));
        assert!(
            trickle.timeouts.windows(2).all(|pair| pair[1] <= pair[0]),
            "each write waits for what is left: {:?}",
            trickle.timeouts
        );
    }

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
