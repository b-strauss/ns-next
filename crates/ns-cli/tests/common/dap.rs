//! A Debug Adapter Protocol client for the tests of `ns-next`'s debugger
//! agent: it starts `ns-next` with `--debug-connect` against a socket it
//! listens on, or with `--debug` and reads the listening line off the
//! child's standard error, then speaks DAP to the agent. A test binary that
//! uses it declares `mod common;` beside it, since it starts `ns-next` the
//! way `common` does.
//!
//! Every wait is bounded by `TIMEOUT`, so that an agent that never answers
//! fails the test with what the child wrote, rather than hanging it.

#![allow(
    dead_code,
    reason = "a test binary that uses only some of the helpers would otherwise warn about the rest"
)]

use std::collections::VecDeque;
use std::ffi::OsStr;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// The longest any single wait takes before the test fails.
pub const TIMEOUT: Duration = Duration::from_secs(30);

/// The line `ns-next --debug` writes to its standard error before the
/// program writes anything, up to the port.
const LISTENING_PREFIX: &str = "ns-next: debugger listening on 127.0.0.1:";

/// What a finished child left: its exit status and both streams, line
/// endings normalized.
pub struct Finished {
    pub status: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// One connection to the agent: the stream it writes requests to, and the
/// messages a reader thread took off it, `None` once the agent closed it.
pub struct Connection {
    writer: TcpStream,
    messages: Receiver<Option<Value>>,
    next_seq: i64,
    /// Events that arrived while a response was awaited, in order.
    events: VecDeque<Value>,
    /// Responses that arrived while another was awaited.
    responses: Vec<Value>,
    closed: bool,
}

impl Connection {
    fn over(stream: TcpStream) -> Connection {
        let reader = stream.try_clone().expect("clone the connection");
        let (sender, messages) = mpsc::channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(reader);
            loop {
                let message = read_message(&mut reader);
                let ended = message.is_none();
                if sender.send(message).is_err() || ended {
                    break;
                }
            }
        });
        Connection {
            writer: stream,
            messages,
            next_seq: 1,
            events: VecDeque::new(),
            responses: Vec::new(),
            closed: false,
        }
    }

    /// Sends a request and answers its sequence number.
    pub fn send(&mut self, command: &str, arguments: Value) -> i64 {
        self.try_send(command, arguments)
            .unwrap_or_else(|error| panic!("sending `{command}`: {error}"))
    }

    /// Sends a request and answers its sequence number, or the error of a
    /// connection the agent may have closed.
    pub fn try_send(&mut self, command: &str, arguments: Value) -> std::io::Result<i64> {
        let seq = self.next_seq;
        self.next_seq += 1;
        let message = json!({
            "seq": seq,
            "type": "request",
            "command": command,
            "arguments": arguments,
        });
        let body = serde_json::to_vec(&message).expect("serialize a request");
        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        self.writer
            .write_all(header.as_bytes())
            .and_then(|()| self.writer.write_all(&body))
            .and_then(|()| self.writer.flush())?;
        Ok(seq)
    }

    /// Closes the connection, as a client that leaves without a word.
    pub fn close(&mut self) {
        let _ = self.writer.shutdown(std::net::Shutdown::Both);
    }

    /// The next message, or `None` once the agent closed the connection;
    /// panics after `TIMEOUT`.
    fn next_message(&mut self, awaited: &str) -> Option<Value> {
        if self.closed {
            return None;
        }
        match self.messages.recv_timeout(TIMEOUT) {
            Ok(Some(message)) => Some(message),
            Ok(None) | Err(RecvTimeoutError::Disconnected) => {
                self.closed = true;
                None
            }
            Err(RecvTimeoutError::Timeout) => panic!("no message within {TIMEOUT:?}: {awaited}"),
        }
    }

    /// The response to the request `seq`, the events before it kept.
    pub fn response_to(&mut self, seq: i64, command: &str) -> Value {
        let answers = |message: &Value| message["request_seq"] == seq;
        if let Some(index) = self.responses.iter().position(answers) {
            let message = self.responses.remove(index);
            assert_eq!(message["command"], command, "{message}");
            return message;
        }
        loop {
            let Some(message) = self.next_message(command) else {
                panic!("the connection closed before the response to `{command}`");
            };
            if message["type"] == "response" && answers(&message) {
                assert_eq!(message["command"], command, "{message}");
                return message;
            }
            if message["type"] == "event" {
                self.events.push_back(message);
            } else if message["type"] == "response" {
                self.responses.push(message);
            }
        }
    }

    /// Waits for the response to the request `seq`: the response once it
    /// came, `None` when the agent closed the connection first. Writing to
    /// a connection the agent closed may itself fail, which is that too.
    pub fn response_or_close(&mut self, seq: i64) -> Option<Value> {
        loop {
            let message = self.next_message("a response or the end")?;
            if message["type"] == "response" && message["request_seq"] == seq {
                return Some(message);
            }
            if message["type"] == "event" {
                self.events.push_back(message);
            }
        }
    }

    /// Whether the agent has closed the connection by now, without
    /// waiting; the messages that arrived meanwhile are dropped.
    pub fn closed_now(&mut self) -> bool {
        while !self.closed {
            match self.messages.try_recv() {
                Ok(Some(_)) => {}
                Ok(None) | Err(std::sync::mpsc::TryRecvError::Disconnected) => self.closed = true,
                Err(std::sync::mpsc::TryRecvError::Empty) => return false,
            }
        }
        true
    }

    /// The next event named `name`, the others before it dropped.
    pub fn event(&mut self, name: &str) -> Value {
        if let Some(index) = self.events.iter().position(|event| event["event"] == name) {
            let event = self.events.remove(index).expect("the index is in range");
            self.events.drain(..index);
            return event;
        }
        self.events.clear();
        loop {
            let Some(message) = self.next_message(name) else {
                panic!("the connection closed before the event `{name}`");
            };
            if message["type"] == "event" && message["event"] == name {
                return message;
            }
        }
    }

    /// Every event that arrives until the agent closes the connection.
    pub fn events_until_closed(&mut self) -> Vec<Value> {
        let mut events: Vec<Value> = self.events.drain(..).collect();
        while let Some(message) = self.next_message("the connection's end") {
            if message["type"] == "event" {
                events.push(message);
            }
        }
        events
    }

    /// Waits for the agent to close the connection, dropping what arrives.
    pub fn expect_closed(&mut self) {
        self.events_until_closed();
    }
}

/// The elements of `value`, which must be an array.
pub fn array(value: &Value) -> Vec<Value> {
    value
        .as_array()
        .unwrap_or_else(|| panic!("not an array: {value}"))
        .clone()
}

/// A DAP message off `reader`: `None` at the end of the stream, or at a
/// frame that is no DAP message, which a test then reports as a closed
/// connection.
fn read_message(reader: &mut impl BufRead) -> Option<Value> {
    let mut length = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(value) = line.strip_prefix("Content-Length:") {
            length = value.trim().parse::<usize>().ok();
        }
    }
    let mut body = vec![0; length?];
    reader.read_exact(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
}

/// What a child wrote to one of its streams, as a reader thread fills it.
type Written = Arc<Mutex<Vec<u8>>>;

/// A child `ns-next` under debugging and one connection to its agent.
pub struct DapClient {
    child: Child,
    stdout: Written,
    stderr: Written,
    /// The listening mode's port and token.
    listening: Option<(u16, String)>,
    pub connection: Connection,
}

impl DapClient {
    /// `ns-next run --debug-connect=<address> <program>`, connected to a
    /// socket this test listens on.
    pub fn connect_mode(program: &Path) -> DapClient {
        DapClient::connect_mode_of("run", program)
    }

    /// `ns-next <subcommand> --debug-connect=<address> <target>`, `run` or
    /// `load`.
    pub fn connect_mode_of(subcommand: &str, target: &Path) -> DapClient {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listen on a local port");
        let address = listener.local_addr().expect("the listener's address");
        let mut command = ns_next();
        command
            .arg(subcommand)
            .arg(format!("--debug-connect={address}"))
            .arg(target);
        let (mut child, stdout, stderr) = start(command);
        listener
            .set_nonblocking(true)
            .expect("a listener that does not block");
        let started = Instant::now();
        let stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => panic!("accepting the agent: {error}"),
            }
            if let Ok(Some(status)) = child.try_wait() {
                panic!(
                    "ns-next ended before it connected ({status}): {}",
                    String::from_utf8_lossy(&stderr.lock().expect("stderr"))
                );
            }
            assert!(started.elapsed() < TIMEOUT, "the agent never connected");
            std::thread::sleep(Duration::from_millis(5));
        };
        stream
            .set_nonblocking(false)
            .expect("a connection that blocks");
        DapClient {
            child,
            stdout,
            stderr,
            listening: None,
            connection: Connection::over(stream),
        }
    }

    /// `ns-next run --debug <program>`, connected to the port the
    /// listening line names.
    pub fn listen_mode(program: &Path) -> DapClient {
        DapClient::listen_mode_of("run", program)
    }

    /// `ns-next <subcommand> --debug <target>`, `run` or `load`.
    pub fn listen_mode_of(subcommand: &str, target: &Path) -> DapClient {
        DapClient::listen_mode_with(subcommand, target, &[])
    }

    /// As `listen_mode_of`, the child given the environment `variables`
    /// besides this process's.
    pub fn listen_mode_with(
        subcommand: &str,
        target: &Path,
        variables: &[(&str, &str)],
    ) -> DapClient {
        let mut command = ns_next();
        command.arg(subcommand).arg("--debug").arg(target);
        command.envs(variables.iter().copied());
        let (mut child, stdout, stderr) = start(command);
        let started = Instant::now();
        let (port, token) = loop {
            let ended = child.try_wait().ok().flatten();
            let written = String::from_utf8_lossy(&stderr.lock().expect("stderr")).to_string();
            // Only a whole line: the token may still be on its way.
            if let Some(start) = written.find(LISTENING_PREFIX) {
                if let Some(length) = written[start..].find('\n') {
                    break parse_listening_line(&written[start..start + length]);
                }
            }
            if let Some(status) = ended {
                panic!("ns-next ended ({status}) before it listened: {written}");
            }
            assert!(
                started.elapsed() < TIMEOUT,
                "no listening line within {TIMEOUT:?}: {written}"
            );
            std::thread::sleep(Duration::from_millis(5));
        };
        let stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to the agent");
        DapClient {
            child,
            stdout,
            stderr,
            listening: Some((port, token)),
            connection: Connection::over(stream),
        }
    }

    /// The listening mode's token, as the listening line printed it.
    pub fn token(&self) -> &str {
        &self.listening.as_ref().expect("a listening agent").1
    }

    /// A connection to the listening agent that no reader thread reads:
    /// the stream alone, for a peer that never reads what it is sent.
    pub fn connect_unread(&self) -> TcpStream {
        let port = self.listening.as_ref().expect("a listening agent").0;
        TcpStream::connect(("127.0.0.1", port)).expect("connect to the agent")
    }

    /// A new connection to the listening agent, replacing this one.
    pub fn reconnect(&mut self) {
        let port = self.listening.as_ref().expect("a listening agent").0;
        let stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to the agent");
        self.connection = Connection::over(stream);
    }

    pub fn process_id(&self) -> u32 {
        self.child.id()
    }

    /// Sends a request and waits for its response.
    pub fn request(&mut self, command: &str, arguments: Value) -> Value {
        let seq = self.connection.send(command, arguments);
        self.connection.response_to(seq, command)
    }

    /// The next event named `name`.
    pub fn event(&mut self, name: &str) -> Value {
        self.connection.event(name)
    }

    /// `initialize`, asserting it succeeded.
    pub fn initialize(&mut self) -> Value {
        let response = self.request(
            "initialize",
            json!({
                "clientID": "ns-next tests",
                "adapterID": "ns-next",
                "linesStartAt1": true,
                "columnsStartAt1": true,
            }),
        );
        assert_eq!(response["success"], true, "{response}");
        response
    }

    /// `attach` with the listening mode's token, if any, and `stopOnEntry`,
    /// then the `process` and `initialized` events; answers the `process`
    /// event.
    pub fn attach(&mut self, stop_on_entry: bool) -> Value {
        let mut arguments = json!({ "stopOnEntry": stop_on_entry });
        if let Some((_, token)) = &self.listening {
            arguments["token"] = json!(token);
        }
        let response = self.request("attach", arguments);
        assert_eq!(response["success"], true, "{response}");
        let process = self.event("process");
        self.event("initialized");
        process
    }

    /// `attach` with the listening mode's token as a fresh connection's
    /// first request, then the `process` and `initialized` events, on a
    /// connection made afresh as long as the agent's deadline to attach
    /// closes it first -- which a loaded machine may make it do -- at most
    /// `attempts` times.
    pub fn attach_racing_the_deadline(&mut self, attempts: u32) {
        for _ in 0..attempts {
            let arguments = json!({ "token": self.token() });
            if let Ok(seq) = self.connection.try_send("attach", arguments) {
                if let Some(response) = self.connection.response_or_close(seq) {
                    assert_eq!(response["success"], true, "{response}");
                    self.event("process");
                    self.event("initialized");
                    return;
                }
            }
            self.reconnect();
        }
        panic!("no attach within the deadline in {attempts} attempts");
    }

    /// `configurationDone`, asserting it succeeded.
    pub fn configuration_done(&mut self) {
        let response = self.request("configurationDone", json!({}));
        assert_eq!(response["success"], true, "{response}");
    }

    /// `initialize`, `attach` and `configurationDone`: the program starts.
    pub fn start_session(&mut self, stop_on_entry: bool) {
        self.initialize();
        self.attach(stop_on_entry);
        self.configuration_done();
    }

    /// A request that must succeed; answers its response's body.
    pub fn succeed(&mut self, command: &str, arguments: Value) -> Value {
        let response = self.request(command, arguments);
        assert_eq!(response["success"], true, "{response}");
        response["body"].clone()
    }

    /// `setBreakpoints` at the start of each of `lines` of `source`;
    /// answers the response's breakpoints.
    pub fn set_breakpoints(&mut self, source: Value, lines: &[u32]) -> Vec<Value> {
        let breakpoints: Vec<Value> = lines.iter().map(|line| json!({ "line": line })).collect();
        let body = self.succeed(
            "setBreakpoints",
            json!({ "source": source, "breakpoints": breakpoints }),
        );
        array(&body["breakpoints"])
    }

    /// The next `stopped` event, asserting its reason.
    pub fn stopped(&mut self, reason: &str) -> Value {
        let stopped = self.event("stopped");
        assert_eq!(stopped["body"]["reason"], reason, "{stopped}");
        stopped
    }

    /// The stopped program's frames, `levels` of them from `start_frame`,
    /// every one for `levels` 0.
    pub fn stack_frames(&mut self, start_frame: usize, levels: usize) -> Vec<Value> {
        let body = self.succeed(
            "stackTrace",
            json!({ "threadId": 1, "startFrame": start_frame, "levels": levels }),
        );
        array(&body["stackFrames"])
    }

    /// The variables of the reference `reference`, paged by `start` and
    /// `count` when given.
    pub fn variables(
        &mut self,
        reference: &Value,
        start: Option<u64>,
        count: Option<u64>,
    ) -> Vec<Value> {
        let mut arguments = json!({ "variablesReference": reference });
        if let Some(start) = start {
            arguments["start"] = json!(start);
        }
        if let Some(count) = count {
            arguments["count"] = json!(count);
        }
        let body = self.succeed("variables", arguments);
        array(&body["variables"])
    }

    /// What the program wrote to its standard output so far.
    pub fn stdout(&self) -> String {
        String::from_utf8_lossy(&self.stdout.lock().expect("stdout")).replace("\r\n", "\n")
    }

    /// Waits until the program's standard output holds `text`.
    pub fn wait_for_stdout(&mut self, text: &str) {
        let started = Instant::now();
        while !self.stdout().contains(text) {
            if let Ok(Some(status)) = self.child.try_wait() {
                panic!(
                    "ns-next ended ({status}) before writing {text:?}: {}",
                    self.stdout()
                );
            }
            assert!(
                started.elapsed() < TIMEOUT,
                "no {text:?} on stdout: {}",
                self.stdout()
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Waits at most `limit` for the child to end, and answers what it left.
    pub fn finish_within(mut self, limit: Duration) -> Finished {
        let started = Instant::now();
        let status = loop {
            if let Some(status) = self.child.try_wait().expect("the child's status") {
                break status;
            }
            if started.elapsed() > limit {
                let _ = self.child.kill();
                panic!(
                    "ns-next did not end within {limit:?}; stdout: {}; stderr: {}",
                    self.stdout(),
                    String::from_utf8_lossy(&self.stderr.lock().expect("stderr"))
                );
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        // The readers stop at the end of the child's streams, which the
        // exit closed; give them the moment it takes.
        let started = Instant::now();
        while Arc::strong_count(&self.stdout) > 1 || Arc::strong_count(&self.stderr) > 1 {
            if started.elapsed() > TIMEOUT {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        Finished {
            status: status.code(),
            stdout: self.stdout(),
            stderr: String::from_utf8_lossy(&self.stderr.lock().expect("stderr"))
                .replace("\r\n", "\n"),
        }
    }

    /// Waits for the child to end, at most `TIMEOUT`.
    pub fn finish(self) -> Finished {
        self.finish_within(TIMEOUT)
    }
}

impl Drop for DapClient {
    /// A failing test leaves no `ns-next` behind.
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn ns_next() -> std::process::Command {
    let Some(binary) = option_env!("CARGO_BIN_EXE_ns-next") else {
        panic!("cargo hands the ns-next binary only to ns-cli's own tests");
    };
    crate::common::target_command(OsStr::new(binary))
}

/// Starts `command` with both output streams read on threads of their
/// own into the buffers it answers.
fn start(mut command: std::process::Command) -> (Child, Written, Written) {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start ns-next");
    let stdout = collect(child.stdout.take().expect("a piped stdout"));
    let stderr = collect(child.stderr.take().expect("a piped stderr"));
    (child, stdout, stderr)
}

fn collect(mut stream: impl Read + Send + 'static) -> Written {
    let buffer = Arc::new(Mutex::new(Vec::new()));
    let filled = Arc::clone(&buffer);
    std::thread::spawn(move || {
        let mut chunk = [0; 4096];
        while let Ok(read) = stream.read(&mut chunk) {
            if read == 0 {
                break;
            }
            filled
                .lock()
                .expect("a buffer")
                .extend_from_slice(&chunk[..read]);
        }
    });
    buffer
}

/// The port and the token of `ns-next: debugger listening on
/// 127.0.0.1:<port>, token <32 lowercase hex digits>`.
pub fn parse_listening_line(line: &str) -> (u16, String) {
    let rest = line
        .strip_prefix(LISTENING_PREFIX)
        .unwrap_or_else(|| panic!("not the listening line: {line}"));
    let (port, token) = rest
        .split_once(", token ")
        .unwrap_or_else(|| panic!("no token in the listening line: {line}"));
    let token = token.trim_end();
    assert!(
        token.len() == 32
            && token
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "the token is not 32 lowercase hex digits: {line}"
    );
    (
        port.parse()
            .unwrap_or_else(|_| panic!("no port in the listening line: {line}")),
        token.to_string(),
    )
}
