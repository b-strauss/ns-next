//! The agent: the thread that speaks DAP to the client and the debugging
//! API to the VM (`docs/design/debugger.md`, decisions 1 and 6). Everything
//! reaches it as an `Input` on one channel -- a connection accepted, a
//! request read, a connection's end, an event of the VM, a word from the
//! host -- so that it is the only writer to the socket and numbers every
//! message it writes in the order it writes them.

use std::collections::{BTreeSet, VecDeque};
use std::net::{Shutdown, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;

use ns_runtime::{Command, DebugControl, Event, FrameDescription, StopReason};
use serde::Serialize;
use serde_json::{json, Value};

use super::connection::{self, Token};
use super::host::{HostStart, HostState, TERMINATED_LINE};
use super::protocol::{self, THREAD_ID};
use super::wire;

/// What the agent's thread is handed.
pub(crate) enum Input {
    /// A client connected: the listening mode's acceptor, or the
    /// connecting mode's one connection.
    Connected(TcpStream),
    /// A message read off the connection numbered `connection`.
    Request { connection: u64, message: Value },
    /// The connection numbered `connection` ended.
    Closed { connection: u64 },
    /// An event of the VM.
    Event(Event),
    /// A word from the host, which waits for the agent to act on it.
    Host(HostMessage),
}

/// What the host tells the agent; each carries the sender the agent
/// answers on once it acted.
pub(crate) enum HostMessage {
    AboutToRunMain(Sender<()>),
    Exited { status: u8, done: Sender<()> },
}

/// Which way the connection was opened.
pub(crate) enum Mode {
    /// `--debug`: a client presents the token; when it leaves, the VM is
    /// reset and the next client is accepted.
    Listening {
        token: Token,
        next_connection: Sender<()>,
    },
    /// `--debug-connect`: when the client leaves, the debugger detaches
    /// and the program runs on.
    Connecting,
}

/// The connected client.
struct Client {
    connection: u64,
    writer: TcpStream,
    next_seq: i64,
    /// Whether its `attach` succeeded: in the listening mode, whether it
    /// presented the token.
    attached: bool,
}

/// Whom the VM's answer to a command goes to.
enum Reply {
    /// The response to a request of the connection numbered `connection`.
    Request {
        connection: u64,
        request_seq: i64,
        command: String,
    },
    /// A command the agent sent of its own accord, whose answer it drops.
    Internal,
}

/// A command the VM has not answered yet. The VM answers every command
/// exactly once and in the order it serves them, but a stop's answers and
/// a configuration command's may come apart, so an answer is matched to
/// the first command it answers rather than the first sent.
struct Pending {
    command: &'static str,
    reply: Reply,
}

/// The agent's state; `run` drives it.
pub(crate) struct Agent {
    mode: Mode,
    /// Handed to each connection's reader.
    inputs: Sender<Input>,
    /// `None` once the debugger was detached.
    control: Option<DebugControl>,
    /// Tells the pump of the VM's events to drop its half of the remote.
    events_detached: Arc<AtomicBool>,
    host: Arc<HostState>,
    /// The program, as the `process` event names it.
    process_name: String,
    client: Option<Client>,
    connections: u64,
    /// The current client's `attach` asked to stop at `main:args:`.
    stop_on_entry: bool,
    /// The next pause stop is the entry's.
    entry_pending: bool,
    stopped: bool,
    pending: VecDeque<Pending>,
    /// Every module a client set line breakpoints in, which a listening
    /// agent clears when the client leaves.
    breakpoint_modules: BTreeSet<String>,
    exited: bool,
}

impl Agent {
    pub(crate) fn new(
        mode: Mode,
        inputs: Sender<Input>,
        control: DebugControl,
        events_detached: Arc<AtomicBool>,
        host: Arc<HostState>,
        process_name: String,
    ) -> Agent {
        Agent {
            mode,
            inputs,
            control: Some(control),
            events_detached,
            host,
            process_name,
            client: None,
            connections: 0,
            stop_on_entry: false,
            entry_pending: false,
            stopped: false,
            pending: VecDeque::new(),
            breakpoint_modules: BTreeSet::new(),
            exited: false,
        }
    }

    /// Serves every input until the process ends.
    pub(crate) fn run(mut self, inputs: Receiver<Input>) {
        for input in inputs {
            match input {
                Input::Connected(stream) => self.connected(stream),
                Input::Request {
                    connection,
                    message,
                } => self.request(connection, message),
                Input::Closed { connection } => {
                    if self.is_current(connection) {
                        self.leave();
                    }
                }
                Input::Event(event) => self.vm_event(event),
                Input::Host(message) => self.host_message(message),
            }
        }
    }

    fn connected(&mut self, stream: TcpStream) {
        if self.client.is_some() {
            // One client at a time; the acceptor waits for the end of a
            // session before it accepts again, so this is never reached.
            let _ = stream.shutdown(Shutdown::Both);
            return;
        }
        // Requests and responses are small and answer one another, which
        // Nagle's algorithm would hold back.
        let _ = stream.set_nodelay(true);
        let Ok(reader) = stream.try_clone() else {
            self.next_connection();
            return;
        };
        self.connections += 1;
        connection::spawn_reader(reader, self.connections, self.inputs.clone());
        self.client = Some(Client {
            connection: self.connections,
            writer: stream,
            next_seq: 1,
            attached: false,
        });
    }

    fn is_current(&self, connection: u64) -> bool {
        self.client
            .as_ref()
            .is_some_and(|client| client.connection == connection)
    }

    fn is_attached(&self) -> bool {
        self.client.as_ref().is_some_and(|client| client.attached)
    }

    fn listening(&self) -> bool {
        matches!(self.mode, Mode::Listening { .. })
    }

    // ---- Requests ----

    fn request(&mut self, connection: u64, message: Value) {
        if !self.is_current(connection) {
            return;
        }
        // A message that is no request has nothing to answer.
        let Ok(request) = serde_json::from_value::<protocol::Request>(message) else {
            return;
        };
        if request.kind != "request" {
            return;
        }
        if self.listening()
            && !self.is_attached()
            && !matches!(request.command.as_str(), "initialize" | "attach")
        {
            self.respond_error(
                connection,
                &request,
                "attach first, presenting the token the listening line printed",
            );
            self.leave();
            return;
        }
        match request.command.as_str() {
            "initialize" => self.initialize(connection, &request),
            "attach" => self.attach(connection, &request),
            "configurationDone" => self.configuration_done(connection, &request),
            "threads" => self.respond(
                connection,
                &request,
                Some(json!({
                    "threads": [protocol::Thread { id: THREAD_ID, name: "main" }]
                })),
            ),
            "pause" => self.pause(connection, &request),
            "continue" => self.forward(connection, &request, Command::Continue),
            "stackTrace" => self.forward(connection, &request, Command::StackTrace),
            "terminate" => self.terminate(connection, &request),
            "disconnect" => self.disconnect(connection, &request),
            command => {
                let message = format!("`{command}` is not supported yet");
                self.respond_error(connection, &request, &message);
            }
        }
    }

    fn initialize(&mut self, connection: u64, request: &protocol::Request) {
        let Some(arguments) = self.arguments::<protocol::InitializeArguments>(connection, request)
        else {
            return;
        };
        // Lines and columns count from 1, the only form the agent speaks.
        if arguments.lines_start_at1 == Some(false) || arguments.columns_start_at1 == Some(false) {
            self.respond_error(
                connection,
                request,
                "ns-next counts lines and columns from 1 only",
            );
            return;
        }
        let capabilities = to_value(protocol::Capabilities::of_the_agent());
        self.respond(connection, request, Some(capabilities));
    }

    fn attach(&mut self, connection: u64, request: &protocol::Request) {
        let Some(arguments) = self.arguments::<protocol::AttachArguments>(connection, request)
        else {
            return;
        };
        if self.is_attached() {
            self.respond_error(connection, request, "the client is attached already");
            return;
        }
        if let Mode::Listening { token, .. } = &self.mode {
            let presented = arguments.token.as_deref().unwrap_or_default();
            if !token.is_presented_by(presented) {
                self.respond_error(
                    connection,
                    request,
                    "the token is missing or wrong: present the one the listening line printed",
                );
                self.leave();
                return;
            }
        }
        if let Some(client) = &mut self.client {
            client.attached = true;
        }
        self.stop_on_entry = arguments.stop_on_entry;
        self.respond(connection, request, None);
        let process = protocol::ProcessEventBody {
            name: self.process_name.clone(),
            system_process_id: std::process::id(),
            is_local_process: true,
            start_method: "attach",
        };
        self.event("process", Some(to_value(process)));
        self.event("initialized", None);
    }

    fn configuration_done(&mut self, connection: u64, request: &protocol::Request) {
        if !self.is_attached() {
            self.respond_error(connection, request, "attach first");
            return;
        }
        self.respond(connection, request, None);
        self.host.decide(HostStart::Run {
            stop_on_entry: self.stop_on_entry,
        });
    }

    fn pause(&mut self, connection: u64, request: &protocol::Request) {
        // A pause asked while stopped would stop the program again at the
        // next safepoint once it goes on.
        if !self.stopped {
            if let Some(control) = &self.control {
                control.pause();
            }
        }
        self.respond(connection, request, None);
    }

    /// Sends `command` to the VM, the request answered when the VM
    /// answers it.
    fn forward(&mut self, connection: u64, request: &protocol::Request, command: Command) {
        let reply = Reply::Request {
            connection,
            request_seq: request.seq,
            command: request.command.clone(),
        };
        if !self.command(command, reply) {
            self.respond_error(connection, request, "the debugger is detached");
        }
    }

    fn terminate(&mut self, connection: u64, request: &protocol::Request) {
        self.host.request_terminate();
        self.send_terminate();
        self.respond(connection, request, None);
    }

    /// Queues a `Terminate` for the VM. Its only answer is a refusal, when
    /// no run executes; the host's record ends the program then.
    fn send_terminate(&mut self) {
        if let Some(control) = &self.control {
            control.send(Command::Terminate);
        }
        self.stopped = false;
        self.entry_pending = false;
    }

    fn disconnect(&mut self, connection: u64, request: &protocol::Request) {
        let Some(arguments) = self.arguments::<protocol::DisconnectArguments>(connection, request)
        else {
            return;
        };
        self.respond(connection, request, None);
        if arguments.terminate_debuggee {
            // At once, from this thread, for a program that reaches no
            // safepoint: no `ensure:` block runs.
            eprintln!("{TERMINATED_LINE}");
            std::process::exit(1);
        }
        self.leave();
    }

    /// Parses a request's arguments, answering an error for ones it cannot.
    fn arguments<T: serde::de::DeserializeOwned + Default>(
        &mut self,
        connection: u64,
        request: &protocol::Request,
    ) -> Option<T> {
        if request.arguments.is_null() {
            return Some(T::default());
        }
        match serde_json::from_value(request.arguments.clone()) {
            Ok(arguments) => Some(arguments),
            Err(error) => {
                let message = format!("malformed arguments: {error}");
                self.respond_error(connection, request, &message);
                None
            }
        }
    }

    // ---- The session's end ----

    /// The client left: by `disconnect`, by closing its connection, or
    /// refused. A client that never attached configured nothing and is
    /// only let go.
    fn leave(&mut self) {
        let Some(client) = self.client.take() else {
            return;
        };
        let _ = client.writer.shutdown(Shutdown::Write);
        let was_attached = client.attached;
        self.stop_on_entry = false;
        self.entry_pending = false;
        match &self.mode {
            Mode::Connecting => {
                // The program runs on without its debugger, and without
                // waiting for a configuration nobody will finish.
                self.host.decide(HostStart::Run {
                    stop_on_entry: false,
                });
                self.detach();
            }
            Mode::Listening { .. } => {
                if was_attached {
                    self.reset_the_vm();
                }
                self.next_connection();
            }
        }
    }

    fn next_connection(&self) {
        if let Mode::Listening {
            next_connection, ..
        } = &self.mode
        {
            let _ = next_connection.send(());
        }
    }

    /// Drops both halves of the remote: the VM detaches the debugger --
    /// every breakpoint removed, a stopped program going on -- where it
    /// next talks to it.
    fn detach(&mut self) {
        self.control = None;
        self.events_detached.store(true, Ordering::SeqCst);
        self.pending.clear();
        self.stopped = false;
    }

    /// Puts the VM back as no client had configured it, for the next
    /// client of a listening agent: no breakpoint, no exception stops
    /// -- nobody would be there to go on from one -- the default step
    /// filters, and a stopped program going on.
    fn reset_the_vm(&mut self) {
        for module in std::mem::take(&mut self.breakpoint_modules) {
            self.command(
                Command::SetBreakpoints {
                    module,
                    breakpoints: Vec::new(),
                },
                Reply::Internal,
            );
        }
        self.command(Command::SetMethodBreakpoints(Vec::new()), Reply::Internal);
        self.command(
            Command::SetExceptionBreakpoints {
                uncaught: false,
                caught: false,
                halt: false,
                classes: Vec::new(),
            },
            Reply::Internal,
        );
        self.command(
            Command::SetStepFilters {
                shipped: true,
                classes: Vec::new(),
            },
            Reply::Internal,
        );
        if self.stopped {
            self.go_on();
        }
    }

    /// Lets a stopped program go on, nobody being there to see the stop.
    fn go_on(&mut self) {
        self.command(Command::Continue, Reply::Internal);
        self.stopped = false;
    }

    // ---- The VM ----

    /// Queues `command`, its answer going to `reply`; `false` once the
    /// debugger was detached.
    fn command(&mut self, command: Command, reply: Reply) -> bool {
        let Some(control) = &self.control else {
            return false;
        };
        self.pending.push_back(Pending {
            command: command.name(),
            reply,
        });
        control.send(command);
        true
    }

    fn vm_event(&mut self, event: Event) {
        if self.control.is_none() {
            return;
        }
        match event {
            Event::Stopped { reason, .. } => self.stopped_at(&reason),
            Event::Refused { command, .. } if command == Command::Terminate.name() => {
                // A `Terminate` that met no run -- sent between two, or
                // just before the next started -- is sent again, so that a
                // run the host started before it saw the record still
                // ends; once no run follows, it waits in the queue.
                if self.host.terminate_requested() && !self.exited {
                    self.send_terminate();
                }
            }
            event => self.answer(event),
        }
    }

    fn stopped_at(&mut self, reason: &StopReason) {
        self.stopped = true;
        if !self.is_attached() {
            self.go_on();
            return;
        }
        let (reason, hit_breakpoint_ids, description) = match reason {
            StopReason::Pause if self.entry_pending => ("entry", Vec::new(), None),
            StopReason::Pause => ("pause", Vec::new(), None),
            StopReason::Breakpoint { ids } => {
                ("breakpoint", ids.iter().map(|id| id.0).collect(), None)
            }
            StopReason::Step => ("step", Vec::new(), None),
            StopReason::MethodEntry { id, others } | StopReason::MethodExit { id, others } => (
                "function breakpoint",
                std::iter::once(id).chain(others).map(|id| id.0).collect(),
                None,
            ),
            StopReason::Exception { description, .. } => {
                ("exception", Vec::new(), Some(description.clone()))
            }
            StopReason::Halt { .. } => ("exception", Vec::new(), Some("halt".to_string())),
        };
        self.entry_pending = false;
        let body = protocol::StoppedEventBody {
            reason,
            description,
            thread_id: THREAD_ID,
            all_threads_stopped: true,
            hit_breakpoint_ids,
        };
        self.event("stopped", Some(to_value(body)));
    }

    /// Hands the VM's answer to the command it answers.
    fn answer(&mut self, event: Event) {
        if matches!(event, Event::Continued) {
            self.stopped = false;
        }
        let Some(index) = self
            .pending
            .iter()
            .position(|pending| answers(&event, pending.command))
        else {
            // Unasked: a pending breakpoint resolved, an install moved one.
            return;
        };
        let Some(pending) = self.pending.remove(index) else {
            return;
        };
        let Reply::Request {
            connection,
            request_seq,
            command,
        } = pending.reply
        else {
            return;
        };
        let outcome = match event {
            Event::Refused { reason, .. } => Err(reason),
            Event::Continued => Ok(Some(json!({ "allThreadsContinued": true }))),
            Event::StackTrace(frames) => Ok(Some(stack_trace(&frames))),
            _ => Ok(None),
        };
        self.write_response(connection, request_seq, &command, outcome);
    }

    // ---- The host ----

    fn host_message(&mut self, message: HostMessage) {
        match message {
            HostMessage::AboutToRunMain(done) => {
                if self.stop_on_entry && self.is_attached() {
                    if let Some(control) = &self.control {
                        control.pause();
                        self.entry_pending = true;
                    }
                }
                let _ = done.send(());
            }
            HostMessage::Exited { status, done } => {
                self.exited = true;
                if self.is_attached() {
                    let exited = protocol::ExitedEventBody {
                        exit_code: i64::from(status),
                    };
                    self.event("exited", Some(to_value(exited)));
                    self.event("terminated", None);
                }
                if let Some(client) = self.client.take() {
                    let _ = client.writer.shutdown(Shutdown::Write);
                }
                let _ = done.send(());
            }
        }
    }

    // ---- Writing ----

    fn respond(&mut self, connection: u64, request: &protocol::Request, body: Option<Value>) {
        self.write_response(connection, request.seq, &request.command, Ok(body));
    }

    fn respond_error(&mut self, connection: u64, request: &protocol::Request, message: &str) {
        self.write_response(
            connection,
            request.seq,
            &request.command,
            Err(message.to_string()),
        );
    }

    fn write_response(
        &mut self,
        connection: u64,
        request_seq: i64,
        command: &str,
        outcome: Result<Option<Value>, String>,
    ) {
        if !self.is_current(connection) {
            return;
        }
        let (success, message, body) = match outcome {
            Ok(body) => (true, None, body),
            Err(message) => (false, Some(message), None),
        };
        self.write(|seq| {
            to_value(protocol::Response {
                seq,
                kind: "response",
                request_seq,
                success,
                command: command.to_string(),
                message,
                body,
            })
        });
    }

    /// Sends an event to the attached client.
    fn event(&mut self, event: &'static str, body: Option<Value>) {
        if !self.is_attached() {
            return;
        }
        self.write(|seq| {
            to_value(protocol::Event {
                seq,
                kind: "event",
                event,
                body,
            })
        });
    }

    /// Writes the message `build` makes of the next sequence number; a
    /// client that cannot be written to has left.
    fn write(&mut self, build: impl FnOnce(i64) -> Value) {
        let Some(client) = &mut self.client else {
            return;
        };
        let message = build(client.next_seq);
        client.next_seq += 1;
        if wire::write_message(&mut client.writer, &message).is_err() {
            self.leave();
        }
    }
}

/// Whether `event` answers the command named `command`.
fn answers(event: &Event, command: &str) -> bool {
    match event {
        Event::Continued => matches!(
            command,
            "Continue" | "StepOver" | "StepInto" | "StepOut" | "RunToPosition"
        ),
        Event::Refused {
            command: refused, ..
        } => refused == command,
        Event::StackTrace(_) => command == "StackTrace",
        Event::Variables(_) => matches!(command, "Variables" | "Children"),
        Event::Breakpoints { .. } => command == "SetBreakpoints",
        Event::StepFiltersSet => command == "SetStepFilters",
        Event::MethodBreakpoints(_) => command == "SetMethodBreakpoints",
        Event::ExceptionBreakpointsSet => command == "SetExceptionBreakpoints",
        Event::ModuleText { .. } => command == "ModuleText",
        Event::Stopped { .. } => false,
    }
}

/// A `stackTrace` response's body: each frame by its name.
fn stack_trace(frames: &[FrameDescription]) -> Value {
    let frames: Vec<protocol::StackFrame> = frames
        .iter()
        .zip(0_i64..)
        .map(|(frame, index)| protocol::StackFrame {
            id: index,
            name: frame.name.clone(),
            line: 0,
            column: 0,
        })
        .collect();
    json!({ "stackFrames": frames, "totalFrames": frames.len() })
}

fn to_value(value: impl Serialize) -> Value {
    serde_json::to_value(value).expect("the agent's own types serialize")
}
