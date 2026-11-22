//! One debug session of the MCP server: the child `ns-next run
//! --debug-connect` it started against a socket it listens on, the DAP
//! connection the child's agent made, the program's output, and the
//! waiting for the next stop (`docs/design/debugger.md`, "The MCP
//! server"). The program runs in the child, so that its output never
//! reaches the stream the protocol owns and a crash ends the child, never
//! the server. The child is killed when the session is dropped: at
//! `debug_stop` and when the server ends. The session belongs to its
//! worker (`worker.rs`), and reads its program's messages from the
//! worker's inbox, where the calls made of it while it waits are queued.

use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::dap_client::{DapClient, Next};
use super::registry::{SessionHandle, SessionState};
use super::shipped_sources;
use super::summary::{self, Frame, Shown, Situation, Stop};
use super::tools;
use super::trace_files::TraceFile;
use super::trace_index::{self, TraceIndex};
use super::worker::{Call, Inbox, Input};
use crate::debug::{SECRET_VARIABLE, Token};
use crate::target_runner;

/// The longest the server waits for the agent to answer a request that
/// runs nothing of the program's: far above any answer, a bound on a
/// child that stopped answering.
const REQUEST_BOUND: Duration = Duration::from_secs(30);

/// The longest the started child takes to connect to the server.
const CONNECT_BOUND: Duration = Duration::from_secs(30);

/// The longest an ended program's process takes to exit, and its streams
/// to close, before it is killed.
pub(crate) const END_BOUND: Duration = Duration::from_secs(10);

/// The longest an aborted evaluation takes to answer.
const ABORT_BOUND: Duration = Duration::from_secs(10);

/// How long the program's output must be quiet at a stop before the
/// summary takes it, and the longest the summary waits for that: what the
/// program wrote before it stopped is in its pipe, on its way to the
/// reader thread.
const OUTPUT_QUIET: Duration = Duration::from_millis(20);
const OUTPUT_SETTLE: Duration = Duration::from_millis(200);

/// The most lines `debug_stack`, `debug_variables` and `debug_memory`
/// answer unless asked for a page.
pub(crate) const LISTING_LIMIT: usize = 100;

/// The agent's message for an evaluation the abort ended.
const ABORTED: &str = "evaluation aborted";

/// What an evaluation answers that `debug_stop`, or its cancellation,
/// aborted.
const INTERRUPTED_EVALUATION: &str =
    "the evaluation was aborted: debug_stop ends the program, or the call was cancelled";

/// The longest text of an evaluation's value an answer shows.
const EVALUATION_LIMIT: usize = 10_000;

/// The breakpoint a tool asks for (`Breakpoint` in the tools' schema).
#[derive(serde::Deserialize, Clone)]
pub(crate) struct Breakpoint {
    pub(crate) location: String,
    #[serde(default)]
    pub(crate) condition: Option<String>,
    #[serde(default)]
    pub(crate) log: Option<String>,
    #[serde(default)]
    pub(crate) hit_condition: Option<String>,
}

/// Where a location string points.
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Location {
    /// `<absolute path>:<line>`; `path` is the file's path as the server
    /// names it, the same for every spelling of one file.
    Line { path: String, line: u32 },
    /// `<id>[@<version>][/<Class>(.<Class>)*]>>selector`, or with
    /// ` class>>selector` for the class side, which the DAP agent reads
    /// (`debug::breakpoints::parse_method_name`).
    Method { name: String },
}

impl Location {
    /// Parses `text`, refusing what is neither form.
    pub(crate) fn parse(text: &str) -> Result<Location, String> {
        let text = text.trim();
        if text.contains(">>") {
            return Ok(Location::Method {
                name: text.to_string(),
            });
        }
        let refused = || {
            format!(
                "{text:?} is no location: <absolute path>:<line>, or \
                 <id>[@<version>][/<Class>(.<Class>)*]>>selector"
            )
        };
        let (path, line) = text.rsplit_once(':').ok_or_else(refused)?;
        let line: u32 = line.trim().parse().map_err(|_| refused())?;
        if line == 0 {
            return Err(format!("{text:?}: lines count from 1"));
        }
        if !Path::new(path).is_absolute() {
            return Err(format!("{text:?}: a location's path is absolute"));
        }
        Ok(Location::Line {
            path: file_key(Path::new(path)),
            line,
        })
    }
}

/// A file's path as the server names it: its canonical path, every `..`
/// and link resolved, when it exists, without Windows' verbatim prefix,
/// so that two spellings of one file name one set of breakpoints, which
/// DAP sets per file.
fn file_key(path: &Path) -> String {
    let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let text = resolved.to_string_lossy().to_string();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = text.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        text
    }
}

/// A breakpoint the session holds: what was asked, and what the agent
/// said of it last.
struct Held {
    asked: Breakpoint,
    location: Location,
    status: String,
}

/// What the program does, as the session last heard.
enum State {
    Running { since: Instant },
    Stopped(StopEvent),
    Ended { status: Option<i64> },
}

/// The `stopped` event's body.
struct StopEvent {
    reason: String,
    description: Option<String>,
    text: Option<String>,
}

/// A value a stop's answers handed out by its reference: the DAP
/// `variablesReference` that lists its children, 0 for none, and its
/// `nsNext/valueReference`, by which it is marked and its referrers asked.
#[derive(Clone, Copy)]
struct Reference {
    variables: i64,
    value: Option<i64>,
}

/// Why a request got no response.
enum Unanswered {
    TimedOut,
    Ended,
    /// The server ends.
    ServerEnding,
    /// An evaluation's wait gave way: its call was cancelled, or a
    /// `debug_stop` came.
    Interrupted,
}

/// What a response is awaited for: a request that runs nothing of the
/// program's, which is answered within its bound and waited for whatever
/// comes meanwhile, or an evaluation, which runs the program's code and
/// gives way to its cancellation and to a `debug_stop`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Awaited {
    Request,
    Evaluation,
}

/// What a call the server's end cut off is answered.
pub(crate) const SERVER_ENDING: &str = "the server is ending";

/// What `debug_start` asks for.
pub(crate) struct StartOptions {
    pub(crate) program: PathBuf,
    pub(crate) runtime: PathBuf,
    pub(crate) arguments: Vec<String>,
    pub(crate) stop_on_entry: bool,
    /// `--trace`'s value, when the program is to trace.
    pub(crate) trace: Option<String>,
    /// One `--trace-filter` pattern each.
    pub(crate) trace_filter: Vec<String>,
    /// The listings, each `--print-<kind>`.
    pub(crate) listings: Vec<String>,
    /// One `--print-filter` pattern each.
    pub(crate) listing_filter: Vec<String>,
    /// The file the child writes the trace and the listings into, its
    /// `--trace-file`, when it traces or lists.
    pub(crate) trace_file: Option<TraceFile>,
    pub(crate) breakpoints: Vec<Breakpoint>,
    pub(crate) wait: Duration,
}

/// What a child wrote to its standard streams, and the agent's own output
/// events, in the order they arrived.
type Written = Arc<Mutex<Vec<u8>>>;

pub(crate) struct Session {
    child: Child,
    client: DapClient,
    output: Written,
    /// How much of `output` an answer has shown.
    shown: usize,
    state: State,
    /// The `exited` event's exit code.
    exit_code: Option<i64>,
    /// The `terminated` event came, or the connection closed.
    ending: bool,
    /// A `pause` was asked and no stop has come since, so that a second
    /// pause asks nothing more of the program.
    pause_requested: bool,
    lines: BTreeMap<String, Vec<Held>>,
    methods: Vec<Held>,
    references: HashMap<i64, Reference>,
    /// The `runtime:` line of `debug_start`'s answer.
    runtime: String,
    /// The worker's inbox, which the connection's messages arrive in.
    inbox: Inbox,
    /// Where the session publishes its state.
    handle: Arc<SessionHandle>,
    /// The index of the file the child writes its trace into, which the
    /// session's handle shares with the trace tools; the file is closed
    /// and deleted with the session once the child is gone.
    trace: Option<Arc<Mutex<TraceIndex>>>,
}

/// What a start made before the session is: the child, connected.
struct Launched {
    child: Child,
    client: DapClient,
    output: Written,
    secret: String,
    core_library: PathBuf,
    fingerprint: String,
}

impl Drop for Session {
    fn drop(&mut self) {
        self.client.close();
        let _ = self.child.kill();
        let _ = self.child.wait();
        // Closed here rather than when the last holder of the index lets
        // it go -- a trace tool's thread may still hold it -- so that the
        // file is gone when `debug_stop` answers.
        close_trace(self.trace.as_ref());
    }
}

/// Closes the trace file `trace` indexes, which deletes it.
fn close_trace(trace: Option<&Arc<Mutex<TraceIndex>>>) {
    if let Some(trace) = trace {
        trace_index::lock(trace).close();
    }
}

/// The index of `file`, published in `handle` and followed from now on,
/// so that the trace tools read the trace of a start still waiting.
fn index_trace(file: TraceFile, handle: &SessionHandle) -> Arc<Mutex<TraceIndex>> {
    let index = Arc::new(Mutex::new(TraceIndex::over(file)));
    trace_index::spawn_follower(&index);
    // Set by this start alone: a session starts once.
    let _ = handle.trace.set(Arc::clone(&index));
    index
}

impl Session {
    /// Starts `options.program` under the debugger as the session
    /// `handle`, its connection's messages arriving in `inbox`, and waits
    /// for its first stop at most `options.wait`; answers the session and
    /// the start's answer, or why it could not start and the inbox, with
    /// the calls queued meanwhile.
    pub(crate) fn start(
        mut options: StartOptions,
        handle: Arc<SessionHandle>,
        inbox: Inbox,
    ) -> Result<(Session, String), (String, Inbox)> {
        let trace_path = options
            .trace_file
            .as_ref()
            .map(|file| file.path().to_path_buf());
        let trace = options
            .trace_file
            .take()
            .map(|file| index_trace(file, &handle));
        let launched = match launch(&options, trace_path.as_deref(), &handle) {
            Ok(launched) => launched,
            Err(reason) => {
                close_trace(trace.as_ref());
                return Err((reason, inbox));
            }
        };
        let mut session = Session {
            child: launched.child,
            client: launched.client,
            output: launched.output,
            shown: 0,
            state: State::Running {
                since: Instant::now(),
            },
            exit_code: None,
            ending: false,
            pause_requested: false,
            lines: BTreeMap::new(),
            methods: Vec::new(),
            references: HashMap::new(),
            runtime: format!("{} {}", options.runtime.display(), launched.fingerprint),
            inbox,
            handle,
            trace,
        };
        match session.configure(&options, &launched.secret, &launched.core_library) {
            Ok(answer) => Ok((session, answer)),
            Err(reason) => Err((reason, session.take_inbox())),
        }
    }

    /// Attaches to the started child, sets the breakpoints and lets the
    /// program run; answers the start's answer.
    fn configure(
        &mut self,
        options: &StartOptions,
        secret: &str,
        core_library: &Path,
    ) -> Result<String, String> {
        self.attach(secret, core_library, options.stop_on_entry)?;
        let breakpoints = (!options.breakpoints.is_empty())
            .then(|| self.change_breakpoints(&options.breakpoints, &[]).0);
        self.request(
            "setExceptionBreakpoints",
            json!({ "filters": ["uncaught", "halt"] }),
        )?;
        // The program runs from the configuration's end: a stop it makes
        // at once may come before the response, and is kept.
        self.state = State::Running {
            since: Instant::now(),
        };
        self.request("configurationDone", json!({}))?;
        self.wait_for_stop(options.wait);
        self.publish();
        let mut answer = self.summary(true);
        if let Some(breakpoints) = breakpoints {
            answer.push_str(&format!("\nbreakpoints:\n{breakpoints}"));
        }
        Ok(answer)
    }

    /// The worker's inbox, taken out of a session about to be dropped.
    pub(crate) fn take_inbox(&mut self) -> Inbox {
        let (_, detached) = mpsc::channel();
        std::mem::replace(&mut self.inbox, Inbox::new(detached))
    }

    /// The next call to serve: the one queued first, or the next to
    /// arrive. What the connection says meanwhile is taken as it comes,
    /// the program's end finished at once, so that the state the session
    /// publishes is current. `None` once the server ends.
    pub(crate) fn next_call(&mut self) -> Option<Call> {
        self.inbox.serve(None);
        loop {
            // The server's end comes first: a call queued behind it is
            // refused, never served -- a resumed program and a wait
            // nobody reads.
            if self.inbox.ended() {
                return None;
            }
            if let Some(call) = self.inbox.next_pending() {
                self.inbox.serve(Some(&call));
                return Some(call);
            }
            match self.inbox.receive(None) {
                Ok(Input::Call(call)) => {
                    self.inbox.serve(Some(&call));
                    return Some(call);
                }
                Ok(Input::Cancelled(identifier)) => self.inbox.cancel(&identifier),
                Ok(Input::Dap(message)) => {
                    match self.client.received(message) {
                        Next::Message(message) if message["type"] == "event" => {
                            self.handle_event(&message);
                        }
                        Next::Closed => self.ending = true,
                        _ => {}
                    }
                    if self.ending {
                        self.finish();
                    }
                }
                Ok(Input::End) | Err(_) => self.inbox.end(),
            }
        }
    }

    /// How much of the program's output answers have shown.
    pub(crate) fn shown_output(&self) -> usize {
        self.shown
    }

    /// Shows the output from `shown` on again in the next answer: the
    /// answer that took it was cancelled, and nobody saw it.
    pub(crate) fn show_again_from(&mut self, shown: usize) {
        self.shown = shown;
    }

    /// Publishes the program's state in the session's handle.
    fn publish(&self) {
        self.handle.publish(match self.state {
            State::Running { .. } => SessionState::Running,
            State::Stopped(_) => SessionState::Stopped,
            State::Ended { .. } => SessionState::Ended,
        });
    }

    /// `initialize` and `attach`, then the `process` event, which must name
    /// the child this session started -- its process id and the secret
    /// only it was given -- and `initialized`.
    fn attach(
        &mut self,
        secret: &str,
        core_library: &Path,
        stop_on_entry: bool,
    ) -> Result<(), String> {
        self.request(
            "initialize",
            json!({
                "clientID": "ns-next mcp",
                "adapterID": "ns-next",
                "linesStartAt1": true,
                "columnsStartAt1": true,
            }),
        )?;
        let core_library = core_library.to_string_lossy().to_string();
        self.request(
            "attach",
            json!({ "stopOnEntry": stop_on_entry, "coreLibrary": core_library }),
        )?;
        let process = self.event("process")?;
        let process_id = process["body"]["systemProcessId"].as_u64();
        if process_id != Some(u64::from(self.child.id()))
            || process["body"]["nsNextSecret"].as_str() != Some(secret)
        {
            return Err("another process connected to the session's socket".to_string());
        }
        self.event("initialized")?;
        Ok(())
    }

    // ---- The connection ----

    /// Sends a request and answers its response's body, or its message.
    fn request(&mut self, command: &str, arguments: Value) -> Result<Value, String> {
        let seq = self.send(command, arguments)?;
        match self.response(seq, Instant::now() + REQUEST_BOUND, Awaited::Request) {
            Ok(response) => body_of(&response),
            Err(Unanswered::TimedOut | Unanswered::Interrupted) => Err(format!(
                "the debugger did not answer `{command}` within {} s",
                REQUEST_BOUND.as_secs()
            )),
            Err(Unanswered::Ended) => Err(self.ended_refusal()),
            Err(Unanswered::ServerEnding) => Err(SERVER_ENDING.to_string()),
        }
    }

    fn send(&mut self, command: &str, arguments: Value) -> Result<i64, String> {
        // After the server's end the inbox is read no more, so a request
        // would wait out its whole bound.
        if self.inbox.ended() {
            return Err(SERVER_ENDING.to_string());
        }
        if self.ending || matches!(self.state, State::Ended { .. }) {
            return Err(self.ended_refusal());
        }
        self.client
            .send(command, arguments)
            .map_err(|error| format!("the debugger's connection: {error}"))
    }

    /// The response to `seq`, the events before it handled; waits until
    /// `deadline` at the latest, and for an evaluation until its call is
    /// cancelled or a `debug_stop` comes, which is then served first.
    fn response(
        &mut self,
        seq: i64,
        deadline: Instant,
        awaited: Awaited,
    ) -> Result<Value, Unanswered> {
        loop {
            if awaited == Awaited::Evaluation && (self.inbox.cancelled() || self.inbox.stop_first())
            {
                return Err(Unanswered::Interrupted);
            }
            match self.client.next(deadline, &self.inbox) {
                Next::Message(message) => {
                    if message["type"] == "response" && message["request_seq"] == seq {
                        return Ok(message);
                    }
                    if message["type"] == "event" {
                        self.handle_event(&message);
                    }
                }
                Next::Closed => {
                    self.ending = true;
                    return Err(Unanswered::Ended);
                }
                Next::TimedOut => return Err(Unanswered::TimedOut),
                Next::Call(call) => self.inbox.queue(call),
                Next::Cancelled(identifier) => self.inbox.cancel(&identifier),
                Next::End => {
                    self.inbox.end();
                    return Err(Unanswered::ServerEnding);
                }
            }
        }
    }

    /// The next event named `name`, the others before it handled.
    fn event(&mut self, name: &str) -> Result<Value, String> {
        let deadline = Instant::now() + REQUEST_BOUND;
        loop {
            match self.client.next(deadline, &self.inbox) {
                Next::Message(message) => {
                    if message["type"] == "event" {
                        if message["event"] == name {
                            return Ok(message);
                        }
                        self.handle_event(&message);
                    }
                }
                Next::Closed => {
                    self.ending = true;
                    return Err(self.ended_refusal());
                }
                Next::TimedOut => {
                    return Err(format!("the debugger sent no `{name}` event"));
                }
                Next::Call(call) => self.inbox.queue(call),
                Next::Cancelled(identifier) => self.inbox.cancel(&identifier),
                Next::End => {
                    self.inbox.end();
                    return Err(SERVER_ENDING.to_string());
                }
            }
        }
    }

    fn handle_event(&mut self, event: &Value) {
        let body = &event["body"];
        match event["event"].as_str() {
            Some("stopped") => {
                self.references.clear();
                self.pause_requested = false;
                self.state = State::Stopped(StopEvent {
                    reason: body["reason"].as_str().unwrap_or("pause").to_string(),
                    description: body["description"].as_str().map(str::to_string),
                    text: body["text"].as_str().map(str::to_string),
                });
                self.publish();
            }
            Some("output") => {
                if let Some(text) = body["output"].as_str() {
                    self.output
                        .lock()
                        .expect("the output")
                        .extend_from_slice(text.as_bytes());
                }
            }
            Some("exited") => self.exit_code = body["exitCode"].as_i64(),
            Some("terminated") => self.ending = true,
            _ => {}
        }
    }

    /// Ends the session once the program ended: the agent is let go, the
    /// child's exit and the end of its streams awaited, the child killed
    /// past `END_BOUND`.
    fn finish(&mut self) {
        if matches!(self.state, State::Ended { .. }) {
            return;
        }
        let _ = self.client.send("disconnect", json!({}));
        let started = Instant::now();
        let status = loop {
            match self.child.try_wait() {
                Ok(Some(status)) => break status.code(),
                Ok(None) if started.elapsed() < END_BOUND => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                _ => {
                    let _ = self.child.kill();
                    break self.child.wait().ok().and_then(|status| status.code());
                }
            }
        };
        self.client.close();
        wait_for_readers(&self.output);
        self.references.clear();
        self.state = State::Ended {
            status: self.exit_code.or(status.map(i64::from)),
        };
        self.publish();
    }

    fn ended_refusal(&self) -> String {
        "the program has ended; debug_start starts it again".to_string()
    }

    // ---- Waiting ----

    /// Waits at most `wait` for the running program to stop or end; the
    /// server's end and the cancellation of the call served end the wait
    /// at once. A `debug_pause` or a `debug_stop` made meanwhile acts at
    /// once -- pauses the program, or ends it -- and is served next, so
    /// that it answers the stop or the end the wait answers.
    fn wait_for_stop(&mut self, wait: Duration) {
        let deadline = Instant::now() + wait;
        let mut interrupting = Vec::new();
        while matches!(self.state, State::Running { .. })
            && !self.ending
            && !self.inbox.ended()
            && !self.inbox.cancelled()
        {
            if let Some(call) = self.inbox.take_interrupting() {
                self.interrupt(call, &mut interrupting);
                continue;
            }
            match self.client.next(deadline, &self.inbox) {
                Next::Message(message) => {
                    if message["type"] == "event" {
                        self.handle_event(&message);
                    }
                }
                Next::Closed => self.ending = true,
                Next::TimedOut => break,
                Next::Call(call) if Inbox::interrupts(&call) => {
                    self.interrupt(call, &mut interrupting);
                }
                Next::Call(call) => self.inbox.queue(call),
                Next::Cancelled(identifier) => self.inbox.cancel(&identifier),
                Next::End => self.inbox.end(),
            }
        }
        // Before every call that waits its turn, in the order they came.
        for call in interrupting.into_iter().rev() {
            self.inbox.queue_first(call);
        }
        if self.ending {
            self.finish();
        } else if matches!(self.state, State::Stopped(_)) {
            self.settle_output();
        }
    }

    /// Acts on `call`, a `debug_pause` or a `debug_stop` made while the
    /// program runs and a call waits for it: asks the pause, or ends the
    /// program, and keeps the call in `interrupting`, to be served once
    /// the wait answered.
    fn interrupt(&mut self, call: Call, interrupting: &mut Vec<Call>) {
        if call.tool == "debug_stop" {
            self.end_program(tools::STOP_WAIT);
        } else {
            // A refusal leaves the pause to its own call, which asks again.
            let _ = self.request_pause();
        }
        interrupting.push(call);
    }

    /// Asks the running program to pause, unless a pause was asked and no
    /// stop came since.
    fn request_pause(&mut self) -> Result<(), String> {
        if self.pause_requested {
            return Ok(());
        }
        // Noted before the request: the stop may come before its response.
        self.pause_requested = true;
        let asked = self.request("pause", json!({ "threadId": 1 }));
        if asked.is_err() {
            self.pause_requested = false;
        }
        asked.map(|_| ())
    }

    /// Waits until the program's output has been quiet for `OUTPUT_QUIET`,
    /// at most `OUTPUT_SETTLE`.
    fn settle_output(&self) {
        let started = Instant::now();
        let mut length = self.output.lock().expect("the output").len();
        let mut quiet_since = Instant::now();
        while started.elapsed() < OUTPUT_SETTLE && quiet_since.elapsed() < OUTPUT_QUIET {
            std::thread::sleep(Duration::from_millis(2));
            let now = self.output.lock().expect("the output").len();
            if now != length {
                length = now;
                quiet_since = Instant::now();
            }
        }
    }

    /// Sends `command`, which lets the stopped program go on, and waits at
    /// most `wait` for its next stop; answers the summary.
    pub(crate) fn resume(
        &mut self,
        command: &str,
        arguments: Value,
        wait: Duration,
    ) -> Result<String, String> {
        self.require_stopped()?;
        // Running before the request is sent: the stop the program makes
        // next may arrive right after the response, or, for a forced
        // return, before it.
        let stopped = std::mem::replace(
            &mut self.state,
            State::Running {
                since: Instant::now(),
            },
        );
        self.publish();
        if let Err(reason) = self.request(command, arguments) {
            // Refused, the program is still at its stop.
            if matches!(self.state, State::Running { .. }) && !self.ending {
                self.state = stopped;
                self.publish();
            }
            return Err(reason);
        }
        self.wait_for_stop(wait);
        Ok(self.summary(false))
    }

    /// `debug_pause`: stops the running program where it is.
    pub(crate) fn pause(&mut self, wait: Duration) -> Result<String, String> {
        if matches!(self.state, State::Running { .. }) && !self.ending {
            self.request_pause()?;
            self.wait_for_stop(wait);
        }
        Ok(self.summary(false))
    }

    /// `debug_wait`: waits on, at most `wait`.
    pub(crate) fn wait(&mut self, wait: Duration) -> String {
        self.wait_for_stop(wait);
        self.summary(false)
    }

    /// `debug_stop`: ends the program as its own `terminate…` would, its
    /// `ensure:` blocks run, and kills it when it does not end within
    /// `wait`.
    pub(crate) fn stop(&mut self, wait: Duration) -> String {
        self.end_program(wait);
        self.summary(false)
    }

    /// Ends the program as `debug_stop` does, and the session's view of it.
    /// A cancellation of the call served lets the end go on: it is bounded
    /// by `wait`, and a program half ended is no state to leave.
    fn end_program(&mut self, wait: Duration) {
        if !self.ending && !matches!(self.state, State::Ended { .. }) {
            // A refusal leaves the end to the kill below.
            let _ = self.request("terminate", json!({}));
            if matches!(self.state, State::Stopped(_)) && !self.ending {
                self.state = State::Running {
                    since: Instant::now(),
                };
                self.publish();
            }
            let deadline = Instant::now() + wait;
            // The server's end kills at once: the server waits for no
            // program's `ensure:` blocks.
            while !self.ending && !self.inbox.ended() && Instant::now() < deadline {
                match self.client.next(deadline, &self.inbox) {
                    Next::Message(message) => {
                        if message["type"] == "event" {
                            self.handle_event(&message);
                        }
                    }
                    Next::Closed => self.ending = true,
                    Next::TimedOut => break,
                    Next::Call(call) => self.inbox.queue(call),
                    Next::Cancelled(identifier) => self.inbox.cancel(&identifier),
                    Next::End => self.inbox.end(),
                }
            }
            if !self.ending {
                let _ = self.child.kill();
            }
        }
        self.finish();
    }

    fn require_stopped(&mut self) -> Result<(), String> {
        // Events that arrived since the last answer -- the program's end
        // above all -- are taken first.
        self.wait_for_stop(Duration::ZERO);
        match self.state {
            State::Stopped(_) => Ok(()),
            State::Running { .. } => {
                Err("the program is running; debug_pause stops it, debug_wait waits".to_string())
            }
            State::Ended { .. } => Err(self.ended_refusal()),
        }
    }

    // ---- The summary ----

    /// The answer that says where the program is, with its output since
    /// the last answer; with `runtime`, the binary and its fingerprint.
    pub(crate) fn summary(&mut self, runtime: bool) -> String {
        let situation = match &self.state {
            State::Running { since } => Situation::Running {
                seconds: since.elapsed().as_secs_f64(),
            },
            State::Ended { status } => Situation::Ended { status: *status },
            State::Stopped(_) => Situation::Stopped(Box::new(self.stop_view())),
        };
        let output = self.new_output();
        let runtime = runtime.then(|| self.runtime.clone());
        summary::summary(&situation, &output, runtime.as_deref())
    }

    /// The output since the last answer that showed output.
    fn new_output(&mut self) -> String {
        let output = self.output.lock().expect("the output");
        let start = self.shown.min(output.len());
        let text = String::from_utf8_lossy(&output[start..]).replace("\r\n", "\n");
        self.shown = output.len();
        text
    }

    /// The stop as its summary shows it, read from the agent.
    fn stop_view(&mut self) -> Stop {
        let State::Stopped(event) = &self.state else {
            unreachable!("asked only at a stop");
        };
        let reason = event.reason.clone();
        let exception = (reason == "exception").then(|| {
            let text = event.text.clone().unwrap_or_default();
            match &event.description {
                Some(class) => format!("{class}: {text}"),
                None => text,
            }
        });
        let condition_error = (reason == "breakpoint")
            .then(|| event.text.clone())
            .flatten();
        let mut stop = Stop {
            reason,
            exception,
            condition_error,
            top: None,
            text: None,
            locals: Vec::new(),
            more_locals: 0,
            stack: Vec::new(),
            total_frames: 0,
        };
        let Ok((frames, total)) = self.frames(summary::STACK_LIMIT) else {
            return stop;
        };
        stop.total_frames = total;
        stop.stack = frames.iter().map(frame_of).collect();
        stop.top = frames.first().map(frame_of);
        if let Some(path) = stop.top.as_ref().and_then(|top| top.path.clone()) {
            stop.text = std::fs::read_to_string(path).ok();
        }
        if !frames.is_empty()
            && let Ok(variables) = self.frame_variables(0, 0, None)
        {
            let total = variables.len();
            stop.locals = variables.into_iter().take(summary::LOCALS_LIMIT).collect();
            stop.more_locals = total - stop.locals.len();
        }
        stop
    }

    /// The first `levels` frames of the stop, and how many there are.
    fn frames(&mut self, levels: usize) -> Result<(Vec<Value>, usize), String> {
        let body = self.request(
            "stackTrace",
            json!({ "threadId": 1, "startFrame": 0, "levels": levels }),
        )?;
        let frames = body["stackFrames"].as_array().cloned().unwrap_or_default();
        let total = body["totalFrames"]
            .as_u64()
            .and_then(|total| usize::try_from(total).ok())
            .unwrap_or(frames.len());
        Ok((frames, total))
    }

    /// The variables of the frame at `index`, from `start`, `count` of
    /// them or every one.
    fn frame_variables(
        &mut self,
        index: usize,
        start: usize,
        count: Option<usize>,
    ) -> Result<Vec<Shown>, String> {
        let scopes = self.request("scopes", json!({ "frameId": index + 1 }))?;
        let locals = scopes["scopes"]
            .as_array()
            .and_then(|scopes| scopes.iter().find(|scope| scope["name"] == "Locals"))
            .and_then(|scope| scope["variablesReference"].as_i64())
            .ok_or_else(|| "the frame shows no variables".to_string())?;
        self.variables_of(locals, start, count)
    }

    /// The children `variablesReference` names, from `start`.
    fn variables_of(
        &mut self,
        variables_reference: i64,
        start: usize,
        count: Option<usize>,
    ) -> Result<Vec<Shown>, String> {
        let mut arguments = json!({ "variablesReference": variables_reference, "start": start });
        if let Some(count) = count {
            arguments["count"] = json!(count);
        }
        let body = self.request("variables", arguments)?;
        let variables = body["variables"].as_array().cloned().unwrap_or_default();
        Ok(variables
            .iter()
            .map(|variable| Shown {
                name: variable["name"].as_str().unwrap_or_default().to_string(),
                value: variable["value"].as_str().unwrap_or_default().to_string(),
                reference: self.remember(variable),
            })
            .collect())
    }

    /// Records the references of a value an answer shows, and answers the
    /// one it shows: the value's own, for a value with children.
    fn remember(&mut self, shown: &Value) -> Option<i64> {
        let variables = shown["variablesReference"].as_i64().unwrap_or(0);
        let value = shown["nsNext/valueReference"].as_i64();
        let reference = Reference { variables, value };
        // The agent numbers both from one count, so neither names
        // another value's.
        let number = value.or((variables != 0).then_some(variables))?;
        self.references.insert(number, reference);
        (variables != 0).then_some(number)
    }

    fn reference(&self, number: i64) -> Result<Reference, String> {
        self.references.get(&number).copied().ok_or_else(|| {
            format!(
                "no value of this stop has the reference {number}; references last \
                 until the program goes on"
            )
        })
    }

    // ---- The tools at a stop ----

    /// `debug_stack`: every frame, one a line, at most `LISTING_LIMIT`.
    pub(crate) fn stack(&mut self) -> Result<String, String> {
        self.require_stopped()?;
        let (frames, total) = self.frames(LISTING_LIMIT)?;
        let mut lines: Vec<String> = frames
            .iter()
            .enumerate()
            .map(|(index, frame)| summary::frame_text(index, &frame_of(frame)))
            .collect();
        if total > frames.len() {
            lines.push(format!("… {} more frames", total - frames.len()));
        }
        Ok(lines.join("\n"))
    }

    /// `debug_variables`: the variables of a frame, or a value's children,
    /// one a line, from `start`, `count` of them or `LISTING_LIMIT`.
    pub(crate) fn variables(
        &mut self,
        frame: Option<usize>,
        reference: Option<i64>,
        start: usize,
        count: Option<usize>,
    ) -> Result<String, String> {
        self.require_stopped()?;
        let shown = count.unwrap_or(LISTING_LIMIT);
        // One more than shown, to tell whether there are more.
        let asked = Some(shown.saturating_add(1));
        let (variables, more_hint) = match (frame, reference) {
            (Some(_), Some(_)) => {
                return Err("debug_variables takes a frame or a reference, not both".to_string());
            }
            (_, Some(number)) => {
                let target = self.reference(number)?;
                if target.variables == 0 {
                    return Err(format!("the value [ref {number}] has no children"));
                }
                let variables = self.variables_of(target.variables, start, asked)?;
                (variables, format!("reference {number}"))
            }
            (frame, None) => {
                let frame = frame.unwrap_or(0);
                let variables = self.frame_variables(frame, start, asked)?;
                (variables, format!("frame {frame}"))
            }
        };
        let mut lines: Vec<String> = variables
            .iter()
            .take(shown)
            .map(summary::variable_text)
            .collect();
        if lines.is_empty() {
            lines.push("no variables".to_string());
        }
        if variables.len() > shown {
            lines.push(format!(
                "… more: debug_variables {more_hint} start {}",
                start + shown
            ));
        }
        Ok(lines.join("\n"))
    }

    /// `debug_evaluate`: the expression in frame `frame`, the top one
    /// without; aborted when it does not end within `wait`.
    pub(crate) fn evaluate(
        &mut self,
        expression: &str,
        frame: Option<usize>,
        wait: Duration,
    ) -> Result<String, String> {
        self.require_stopped()?;
        let mut arguments = json!({ "expression": expression, "context": "repl" });
        if let Some(frame) = frame {
            arguments["frameId"] = json!(frame + 1);
        }
        let seq = self.send("evaluate", arguments)?;
        let awaited = self.response(seq, Instant::now() + wait, Awaited::Evaluation);
        let response = match awaited {
            Ok(response) => response,
            Err(cut @ (Unanswered::TimedOut | Unanswered::Interrupted)) => {
                let _ = self.send("cancel", json!({ "requestId": seq }));
                // The evaluation's answer, which the agent sends however it
                // ended: aborted, or answered just before the abort.
                match self.response(seq, Instant::now() + ABORT_BOUND, Awaited::Request) {
                    // Answered, or ended otherwise than by the abort: its
                    // own word.
                    Ok(response) if response["message"] != ABORTED => response,
                    Err(Unanswered::Ended) => return Err(self.ended_refusal()),
                    Err(Unanswered::ServerEnding) => return Err(SERVER_ENDING.to_string()),
                    _ if matches!(cut, Unanswered::Interrupted) => {
                        return Err(INTERRUPTED_EVALUATION.to_string());
                    }
                    _ => {
                        return Err(format!(
                            "the evaluation did not end within {:.1} s and was aborted; \
                             the program is still stopped",
                            wait.as_secs_f64()
                        ));
                    }
                }
            }
            Err(Unanswered::Ended) => return Err(self.ended_refusal()),
            Err(Unanswered::ServerEnding) => return Err(SERVER_ENDING.to_string()),
        };
        let body = body_of(&response)?;
        let mut text = summary::cut(
            body["result"].as_str().unwrap_or_default(),
            EVALUATION_LIMIT,
        );
        if let Some(reference) = self.remember(&body) {
            text.push_str(&format!(" [ref {reference}]"));
        }
        Ok(self.with_output(text))
    }

    /// `debug_set_variable`: the variable `name` of frame `frame`, the top
    /// one without, set to what `value` answers.
    pub(crate) fn set_variable(
        &mut self,
        name: &str,
        value: &str,
        frame: Option<usize>,
    ) -> Result<String, String> {
        self.require_stopped()?;
        let index = frame.unwrap_or(0);
        let scopes = self.request("scopes", json!({ "frameId": index + 1 }))?;
        let locals = scopes["scopes"]
            .as_array()
            .and_then(|scopes| scopes.iter().find(|scope| scope["name"] == "Locals"))
            .and_then(|scope| scope["variablesReference"].as_i64())
            .ok_or_else(|| "the frame shows no variables".to_string())?;
        let body = self.request(
            "setVariable",
            json!({ "variablesReference": locals, "name": name, "value": value }),
        )?;
        let shown = Shown {
            name: name.to_string(),
            value: body["value"].as_str().unwrap_or_default().to_string(),
            reference: self.remember(&body),
        };
        Ok(self.with_output(summary::variable_text(&shown)))
    }

    /// `debug_step_targets`: the sends and blocks a step into the top
    /// frame's statement can stop in.
    pub(crate) fn step_targets(&mut self) -> Result<String, String> {
        self.require_stopped()?;
        let body = self.request("stepInTargets", json!({ "frameId": 1 }))?;
        let targets = body["targets"].as_array().cloned().unwrap_or_default();
        if targets.is_empty() {
            return Ok("no targets: the statement sends nothing a step can enter".to_string());
        }
        Ok(targets
            .iter()
            .map(|target| {
                let mut line = format!(
                    "{} {}",
                    target["id"],
                    target["label"].as_str().unwrap_or_default()
                );
                if let (Some(row), Some(column)) =
                    (target["line"].as_u64(), target["column"].as_u64())
                {
                    line.push_str(&format!(" at {row}:{column}"));
                }
                line
            })
            .collect::<Vec<_>>()
            .join("\n"))
    }

    /// `debug_code`: the bytecode of frame `frame`, then the machine code
    /// it goes on in, or the line saying it goes on interpreted.
    pub(crate) fn code(&mut self, frame: usize) -> Result<String, String> {
        self.require_stopped()?;
        let body = self.request(
            "nsNext/frameCode",
            json!({ "frameId": frame.saturating_add(1) }),
        )?;
        let bytecode = body["bytecode"].as_str().unwrap_or_default();
        let text = match body["machineCode"].as_str() {
            // Every listing ends its lines; the answer ends with its last.
            Some(machine_code) => format!("{bytecode}{}", machine_code.trim_end_matches('\n')),
            None => format!("{bytecode}the frame goes on in the interpreter"),
        };
        Ok(self.with_output(text))
    }

    /// `debug_frame`: restarts frame `frame` or makes it return `value`,
    /// then waits for the stop that follows.
    pub(crate) fn frame_action(
        &mut self,
        restart: bool,
        frame: usize,
        value: Option<&str>,
        wait: Duration,
    ) -> Result<String, String> {
        if restart {
            self.resume("restartFrame", json!({ "frameId": frame + 1 }), wait)
        } else {
            let mut arguments = json!({ "frameId": frame + 1 });
            if let Some(value) = value {
                arguments["expression"] = json!(value);
            }
            self.resume("nsNext/forceReturn", arguments, wait)
        }
    }

    /// `debug_memory`: the instance counts, a class's instances, or what
    /// refers to a value.
    pub(crate) fn memory(
        &mut self,
        class: Option<&str>,
        referrers_of: Option<i64>,
    ) -> Result<String, String> {
        self.require_stopped()?;
        let asked = LISTING_LIMIT + 1;
        let (body, what) = match (class, referrers_of) {
            (Some(_), Some(_)) => {
                return Err("debug_memory takes a class or referrers_of, not both".to_string());
            }
            (None, None) => {
                let body = self.request("nsNext/instanceCounts", json!({}))?;
                let classes = body["classes"].as_array().cloned().unwrap_or_default();
                let mut lines: Vec<String> = classes
                    .iter()
                    .take(LISTING_LIMIT)
                    .map(|class| {
                        let shipped = if class["nsNext/shipped"] == true {
                            " (core library)"
                        } else {
                            ""
                        };
                        format!(
                            "{} {}{shipped}",
                            class["count"],
                            class["name"].as_str().unwrap_or_default()
                        )
                    })
                    .collect();
                if classes.len() > LISTING_LIMIT {
                    lines.push(format!("… {} more classes", classes.len() - LISTING_LIMIT));
                }
                return Ok(lines.join("\n"));
            }
            (Some(class), None) => {
                let body = self.request(
                    "nsNext/instances",
                    json!({ "name": class, "start": 0, "count": asked }),
                )?;
                (body, "instances")
            }
            (None, Some(number)) => {
                let value = self.reference(number)?.value.ok_or_else(|| {
                    format!("the value [ref {number}] is withheld; nothing is asked of it")
                })?;
                let body = self.request(
                    "nsNext/referrers",
                    json!({ "valueReference": value, "start": 0, "count": asked }),
                )?;
                (body, "referrers")
            }
        };
        let listed = body["variables"].as_array().cloned().unwrap_or_default();
        let mut lines: Vec<String> = listed
            .iter()
            .take(LISTING_LIMIT)
            .map(|variable| {
                let shown = Shown {
                    name: variable["name"].as_str().unwrap_or_default().to_string(),
                    value: variable["value"].as_str().unwrap_or_default().to_string(),
                    reference: self.remember(variable),
                };
                summary::variable_text(&shown)
            })
            .collect();
        if lines.is_empty() {
            lines.push(format!("no {what}"));
        }
        if listed.len() > LISTING_LIMIT {
            lines.push(format!("… more {what} than {LISTING_LIMIT}"));
        }
        Ok(lines.join("\n"))
    }

    /// `debug_mark`: marks the value `reference` names with `label`, or
    /// takes the mark `label` off without one.
    pub(crate) fn mark(&mut self, reference: Option<i64>, label: &str) -> Result<String, String> {
        self.require_stopped()?;
        let Some(number) = reference else {
            self.request("nsNext/unmark", json!({ "label": label }))?;
            return Ok(format!("the mark {label} is taken off"));
        };
        let value = self
            .reference(number)?
            .value
            .ok_or_else(|| format!("the value [ref {number}] is withheld and cannot be marked"))?;
        self.request(
            "nsNext/mark",
            json!({ "valueReference": value, "label": label }),
        )?;
        Ok(format!(
            "[ref {number}] is marked {label}: an evaluation names it {label}_DebugLabel"
        ))
    }

    /// `text`, and the output since the last answer on a line after it.
    fn with_output(&mut self, text: String) -> String {
        let output = self.new_output();
        match summary::output_line(&output) {
            Some(line) => format!("{text}\n{line}"),
            None => text,
        }
    }

    // ---- Breakpoints ----

    /// Sets the breakpoints `set` asks for and clears those `clear`
    /// names, then answers every breakpoint held, one a line, and whether
    /// any was refused -- a location naming no module among them, which is
    /// not held.
    pub(crate) fn change_breakpoints(
        &mut self,
        set: &[Breakpoint],
        clear: &[String],
    ) -> (String, bool) {
        let mut refusals = Vec::new();
        let mut files = std::collections::BTreeSet::new();
        let mut methods_changed = false;
        for text in clear {
            match Location::parse(text) {
                Ok(location) => {
                    let removed = match &location {
                        Location::Line { path, .. } => {
                            files.insert(path.clone());
                            let held = self.lines.entry(path.clone()).or_default();
                            let before = held.len();
                            held.retain(|held| held.location != location);
                            before != held.len()
                        }
                        Location::Method { .. } => {
                            methods_changed = true;
                            let before = self.methods.len();
                            self.methods.retain(|held| held.location != location);
                            before != self.methods.len()
                        }
                    };
                    if !removed {
                        refusals.push(format!("{text}: no breakpoint is set there"));
                    }
                }
                Err(reason) => refusals.push(reason),
            }
        }
        for asked in set {
            match Location::parse(&asked.location) {
                Ok(location) => {
                    let held = Held {
                        asked: asked.clone(),
                        location: location.clone(),
                        status: String::new(),
                    };
                    let list = match &location {
                        Location::Line { path, .. } => {
                            files.insert(path.clone());
                            self.lines.entry(path.clone()).or_default()
                        }
                        Location::Method { .. } => {
                            methods_changed = true;
                            &mut self.methods
                        }
                    };
                    // One breakpoint a place: a second asks the first anew.
                    list.retain(|held| held.location != location);
                    list.push(held);
                }
                Err(reason) => refusals.push(reason),
            }
        }
        for path in files {
            refusals.extend(self.send_line_breakpoints(&path));
        }
        if methods_changed {
            refusals.extend(self.send_method_breakpoints());
        }
        self.lines.retain(|_, held| !held.is_empty());
        let mut lines: Vec<String> = self
            .lines
            .values()
            .flatten()
            .chain(&self.methods)
            .map(held_text)
            .collect();
        let refused = !refusals.is_empty();
        lines.extend(refusals.iter().map(|reason| format!("refused: {reason}")));
        if lines.is_empty() {
            lines.push("no breakpoints".to_string());
        }
        (lines.join("\n"), refused)
    }

    /// `setBreakpoints` for every breakpoint held in the file `path`;
    /// answers the refusals, whose breakpoints are dropped.
    fn send_line_breakpoints(&mut self, path: &str) -> Vec<String> {
        let held = self.lines.remove(path).unwrap_or_default();
        let breakpoints: Vec<Value> = held
            .iter()
            .map(|held| {
                let Location::Line { line, .. } = held.location else {
                    unreachable!("a file holds line breakpoints");
                };
                let mut breakpoint = json!({ "line": line });
                set_options(&mut breakpoint, &held.asked, true);
                breakpoint
            })
            .collect();
        let answer = self.request(
            "setBreakpoints",
            json!({ "source": { "path": path }, "breakpoints": breakpoints }),
        );
        let (kept, refusals) = statuses(held, answer);
        if !kept.is_empty() {
            self.lines.insert(path.to_string(), kept);
        }
        refusals
    }

    /// `setFunctionBreakpoints` for every method breakpoint held; answers
    /// the refusals, whose breakpoints are dropped.
    fn send_method_breakpoints(&mut self) -> Vec<String> {
        let held = std::mem::take(&mut self.methods);
        let breakpoints: Vec<Value> = held
            .iter()
            .map(|held| {
                let Location::Method { name } = &held.location else {
                    unreachable!("the methods hold method breakpoints");
                };
                let mut breakpoint = json!({ "name": name });
                set_options(&mut breakpoint, &held.asked, false);
                breakpoint
            })
            .collect();
        let answer = self.request(
            "setFunctionBreakpoints",
            json!({ "breakpoints": breakpoints }),
        );
        let (kept, refusals) = statuses(held, answer);
        self.methods = kept;
        refusals
    }
}

/// Writes a breakpoint's condition, log message and hit condition into its
/// DAP form; a method breakpoint takes no log message.
fn set_options(breakpoint: &mut Value, asked: &Breakpoint, logs: bool) {
    if let Some(condition) = &asked.condition {
        breakpoint["condition"] = json!(condition);
    }
    if let Some(hit_condition) = &asked.hit_condition {
        breakpoint["hitCondition"] = json!(hit_condition);
    }
    if logs && let Some(log) = &asked.log {
        breakpoint["logMessage"] = json!(log);
    }
}

/// The breakpoints of one `setBreakpoints` or `setFunctionBreakpoints`
/// kept, each with the agent's word on it, and the refusals: a breakpoint
/// the agent did not set -- no id -- is refused, one it set and could not
/// place yet is pending.
fn statuses(held: Vec<Held>, answer: Result<Value, String>) -> (Vec<Held>, Vec<String>) {
    let answers = match answer {
        Ok(body) => body["breakpoints"].as_array().cloned().unwrap_or_default(),
        Err(reason) => {
            let refusals = held
                .iter()
                .map(|held| format!("{}: {reason}", held.asked.location))
                .collect();
            return (Vec::new(), refusals);
        }
    };
    let mut kept = Vec::new();
    let mut refusals = Vec::new();
    for (index, mut held) in held.into_iter().enumerate() {
        let answer = answers.get(index).cloned().unwrap_or(Value::Null);
        let message = answer["message"]
            .as_str()
            .unwrap_or("the debugger did not answer it");
        if answer["verified"] == true {
            held.status = match answer["line"].as_u64() {
                Some(line) => format!("set at line {line}"),
                None => "set".to_string(),
            };
            kept.push(held);
        } else if answer["id"].is_number() {
            held.status = message.to_string();
            kept.push(held);
        } else {
            refusals.push(format!("{}: {message}", held.asked.location));
        }
    }
    (kept, refusals)
}

/// A held breakpoint as `debug_breakpoints` lists it.
fn held_text(held: &Held) -> String {
    let mut text = format!("{} {}", held.asked.location.trim(), held.status);
    if let Some(condition) = &held.asked.condition {
        text.push_str(&format!(", condition {condition}"));
    }
    if let Some(log) = &held.asked.log {
        text.push_str(&format!(", log {log:?}"));
    }
    if let Some(hit_condition) = &held.asked.hit_condition {
        text.push_str(&format!(", hit condition {hit_condition}"));
    }
    text
}

/// A response's body, or the message of an error response, which is said
/// on the standard error too, so that a refusal nobody expected shows
/// the request it answers.
fn body_of(response: &Value) -> Result<Value, String> {
    if response["success"] == true {
        Ok(response["body"].clone())
    } else {
        let message = response["message"]
            .as_str()
            .unwrap_or("the debugger refused it")
            .to_string();
        super::say(&format!(
            "ns-next mcp: the agent refused `{}` (request {}): {message}",
            response["command"].as_str().unwrap_or_default(),
            response["request_seq"]
        ));
        Err(message)
    }
}

/// A DAP stack frame as the summary shows it.
fn frame_of(frame: &Value) -> Frame {
    let number = |key: &str| {
        frame[key]
            .as_u64()
            .and_then(|number| u32::try_from(number).ok())
            .unwrap_or(0)
    };
    Frame {
        name: frame["name"].as_str().unwrap_or_default().to_string(),
        path: frame["source"]["path"].as_str().map(str::to_string),
        line: number("line"),
        column: number("column"),
        shipped: frame["nsNext/shipped"] == true,
    }
}

/// Starts the child `options` asks for, its trace written into
/// `trace_path`, and waits for its agent to
/// connect; answers it connected, its messages arriving in the inbox of
/// `handle`, or why it did not start.
fn launch(
    options: &StartOptions,
    trace_path: Option<&Path>,
    handle: &SessionHandle,
) -> Result<Launched, String> {
    let (core_library, fingerprint) =
        shipped_sources::core_library(&options.runtime, handle.number)?;
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|error| format!("listening on 127.0.0.1: {error}"))?;
    let address = listener
        .local_addr()
        .map_err(|error| format!("listening on 127.0.0.1: {error}"))?;
    let secret = Token::generate()
        .map_err(|error| format!("making the session's secret: {error}"))?
        .hex();
    let mut command = target_runner::target_command(&options.runtime);
    command.arg("run").arg(format!("--debug-connect={address}"));
    // Before the program, as `run` reads its options there. The trace
    // and the listings go into the server's file, which replaces the
    // child's standard error for them, so that the output stays the
    // program's.
    if let Some(trace) = &options.trace {
        command.arg(format!("--trace={trace}"));
    }
    for pattern in &options.trace_filter {
        command.arg(format!("--trace-filter={pattern}"));
    }
    for kind in &options.listings {
        command.arg(format!("--print-{kind}"));
    }
    for pattern in &options.listing_filter {
        command.arg(format!("--print-filter={pattern}"));
    }
    if let Some(trace_path) = trace_path {
        command.arg(format!("--trace-file={}", trace_path.display()));
    }
    command.arg(&options.program);
    if !options.arguments.is_empty() {
        command.arg("--").args(&options.arguments);
    }
    command
        .env(SECRET_VARIABLE, &secret)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("starting {}: {error}", options.runtime.display()))?;
    // Said for whoever watches the server: a test, or a person looking
    // for the program's process.
    super::say(&format!(
        "ns-next mcp: {} runs as process {}",
        options.program.display(),
        child.id()
    ));
    let output: Written = Arc::new(Mutex::new(Vec::new()));
    if let Some(stdout) = child.stdout.take() {
        collect(stdout, Arc::clone(&output));
    }
    if let Some(stderr) = child.stderr.take() {
        collect(stderr, Arc::clone(&output));
    }
    let stream = match accept(&listener, &mut child) {
        Ok(stream) => stream,
        Err(reason) => {
            let _ = child.kill();
            let _ = child.wait();
            wait_for_readers(&output);
            let written = String::from_utf8_lossy(&output.lock().expect("the output")).to_string();
            return Err(summary::output_line(&written)
                .map_or(reason.clone(), |line| format!("{reason}\n{line}")));
        }
    };
    let client = match DapClient::over(stream, handle.inbox.clone()) {
        Ok(client) => client,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("the connection: {error}"));
        }
    };
    Ok(Launched {
        child,
        client,
        output,
        secret,
        core_library,
        fingerprint,
    })
}

/// Waits for the child's agent to connect to `listener`, at most
/// `CONNECT_BOUND`; refuses a child that ended first.
fn accept(listener: &TcpListener, child: &mut Child) -> Result<std::net::TcpStream, String> {
    listener
        .set_nonblocking(true)
        .map_err(|error| format!("listening: {error}"))?;
    let started = Instant::now();
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream
                    .set_nonblocking(false)
                    .map_err(|error| format!("the connection: {error}"))?;
                return Ok(stream);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(format!("accepting the debugger: {error}")),
        }
        if let Ok(Some(status)) = child.try_wait() {
            let status = status
                .code()
                .map_or_else(|| "unknown".to_string(), |code| code.to_string());
            return Err(format!(
                "the program did not start: ended: exit status {status}"
            ));
        }
        if started.elapsed() > CONNECT_BOUND {
            return Err(format!(
                "the program's debugger did not connect within {} s",
                CONNECT_BOUND.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Reads `stream` into `written` on a thread of its own until it ends.
fn collect(mut stream: impl Read + Send + 'static, written: Written) {
    let _ = std::thread::Builder::new()
        .name("ns-next mcp output".to_string())
        .spawn(move || {
            let mut chunk = [0; 4096];
            while let Ok(read) = stream.read(&mut chunk) {
                if read == 0 {
                    break;
                }
                written
                    .lock()
                    .expect("the output")
                    .extend_from_slice(&chunk[..read]);
            }
        });
}

/// Waits, at most `END_BOUND`, for the readers of an ended child's streams
/// to reach their end, each holding the buffer until it does.
fn wait_for_readers(output: &Written) {
    let started = Instant::now();
    while Arc::strong_count(output) > 1 && started.elapsed() < END_BOUND {
        std::thread::sleep(Duration::from_millis(1));
    }
}
