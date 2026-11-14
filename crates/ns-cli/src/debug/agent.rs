//! The agent: the thread that speaks DAP to the client and the debugging
//! API to the VM (`docs/design/debugger.md`, decisions 1 and 6). Everything
//! reaches it as an `Input` on one channel -- a connection accepted, a
//! request read, a connection's end, an event of the VM, a word from the
//! host -- so that it alone decides what the client is sent, numbering
//! every message in the order it queues them for the connection's writer
//! thread, which writes them in that order (`connection::Writer`), so that
//! the agent never waits on a client that does not read.

use std::collections::{BTreeSet, VecDeque};
use std::net::{Shutdown, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ns_runtime::{Command, DebugControl, Event, StopReason, ValueDescription, Variable};
use serde::Serialize;
use serde_json::{json, Value};

use super::breakpoints::{self, Breakpoints, FunctionEntry, LineEntry};
use super::connection::{self, Token};
use super::host::{HostStart, HostState, TERMINATED_LINE};
use super::protocol::{self, THREAD_ID};
use super::sources::Sources;
use super::stepping::{LineStep, Phase};
use super::stopped::{self, Frames, Stop, Target};

/// What the agent's thread is handed.
pub(crate) enum Input {
    /// A client connected: the listening mode's acceptor, or the
    /// connecting mode's one connection.
    Connected(TcpStream),
    /// A message read off the connection numbered `connection`.
    Request { connection: u64, message: Value },
    /// The connection numbered `connection` ended.
    Closed { connection: u64 },
    /// The listening mode's deadline for the connection numbered
    /// `connection` to present the token passed.
    AttachDeadline { connection: u64 },
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

/// How long a listening agent gives a connection, from its acceptance, to
/// present the token in a good `attach`: a peer that connects and stays
/// silent, or trickles bytes, would otherwise hold the one connection the
/// agent serves. A total deadline rather than a read timeout, so that it
/// ends nothing once the client attached, however long it is silent then.
const ATTACH_DEADLINE: Duration = Duration::from_secs(10);

/// The variable a debug build reads `ATTACH_DEADLINE` from, in
/// milliseconds, so that the tests of the deadline take a fraction of a
/// second; a release build never reads it.
#[cfg(debug_assertions)]
const ATTACH_DEADLINE_VARIABLE: &str = "NS_NEXT_DEBUG_ATTACH_DEADLINE_MILLISECONDS";

/// The deadline a connection has to attach.
fn attach_deadline() -> Duration {
    #[cfg(debug_assertions)]
    if let Some(shortened) = debug_override(ATTACH_DEADLINE_VARIABLE) {
        return shortened;
    }
    ATTACH_DEADLINE
}

/// How long one write to an attached client may wait for the client to
/// read: a client that stops reading -- a frozen IDE -- is then dropped as
/// one that disconnected, rather than holding the agent, and with it the
/// program's stops, events and end, for as long as it does not read.
const WRITE_BOUND: Duration = Duration::from_secs(30);

/// The variable a debug build reads `WRITE_BOUND` from, in milliseconds,
/// as `ATTACH_DEADLINE_VARIABLE`.
#[cfg(debug_assertions)]
const WRITE_BOUND_VARIABLE: &str = "NS_NEXT_DEBUG_WRITE_BOUND_MILLISECONDS";

/// How long the agent waits, after `terminated`, for the client's next
/// request or its disconnect before the process ends anyway: a client
/// answers `terminated` at once, with its `disconnect`, and one that does
/// not delays the process's end by no more than this.
const ENDED_LINGER: Duration = Duration::from_secs(2);

/// The bound of one write to an attached client.
fn write_bound() -> Duration {
    #[cfg(debug_assertions)]
    if let Some(shortened) = debug_override(WRITE_BOUND_VARIABLE) {
        return shortened;
    }
    WRITE_BOUND
}

/// A duration a debug build's test sets in milliseconds through
/// `variable`.
#[cfg(debug_assertions)]
fn debug_override(variable: &str) -> Option<Duration> {
    std::env::var(variable)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_millis)
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
    writer: connection::Writer,
    next_seq: i64,
    /// Whether its `attach` succeeded: in the listening mode, whether it
    /// presented the token.
    attached: bool,
}

/// Whom the VM's answer to a command goes to.
pub(super) enum Reply {
    /// The response to a request of the connection numbered `connection`,
    /// made of the answer by `conversion`.
    Request {
        connection: u64,
        request_seq: i64,
        command: String,
        conversion: Conversion,
    },
    /// A command the agent sent of its own accord, whose answer it drops.
    Internal,
    /// A step by line's step after its first, which reports the stop it
    /// left from when the VM refuses it.
    Repeated,
    /// The stop's frames, asked at the stop `generation`, which the
    /// requests waiting for them read.
    Frames { generation: u64 },
    /// A module's current text, which the requests waiting for it read.
    Text { module: String },
}

/// How the VM's answer becomes a request's response body.
pub(super) enum Conversion {
    /// No body.
    Plain,
    /// `continue`'s.
    Continued,
    /// A frame's variables or a value's children, asked at the stop
    /// `generation`; a frame's are paged here, by `page`'s start and
    /// count, since the VM answers all of them.
    Variables {
        generation: u64,
        page: Option<(usize, usize)>,
    },
    /// `setBreakpoints`': its module, each breakpoint asked for, in the
    /// request's order, and the text the offsets count in.
    Breakpoints {
        module: String,
        entries: Vec<LineEntry>,
        text: String,
    },
    /// `setFunctionBreakpoints`': each breakpoint asked for.
    FunctionBreakpoints { entries: Vec<FunctionEntry> },
    /// `setExceptionBreakpoints`': the number of filters it turned on.
    ExceptionBreakpoints { count: usize },
}

/// A command the VM has not answered yet. The VM answers every command
/// exactly once and in the order it serves them, but a stop's answers and
/// a configuration command's may come apart, so an answer is matched to
/// the first command it answers rather than the first sent; one that
/// names a module, to the first naming that module.
struct Pending {
    command: &'static str,
    module: Option<String>,
    reply: Reply,
}

/// A response's body, or the message of an error response.
type Outcome = Result<Option<Value>, String>;

/// A request that reads what the VM has not answered yet -- the stop's
/// frames, a module's text -- and is answered once it did.
struct Waiting {
    connection: u64,
    request: protocol::Request,
    /// The stop it was asked at.
    generation: u64,
}

/// The agent's state; `run` drives it.
pub(crate) struct Agent {
    mode: Mode,
    /// Handed to each connection's reader.
    inputs: Sender<Input>,
    /// What the agent serves, in order.
    received: Receiver<Input>,
    /// Asks the pump of the VM's events to hand over every event the VM
    /// sent so far, and to end: the host's `Exited` reaches the agent on
    /// another path than the events, which may still be on their way.
    drain: Sender<Sender<()>>,
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
    /// The VM stands at a stop.
    stopped: bool,
    /// A `Continue`, a step or a run to cursor is on its way: the program
    /// counts as running for the client, but stays stopped for the agent
    /// until the VM answers it, so that a refused one leaves the stop as
    /// it was.
    pub(super) resuming: bool,
    /// A step by line in progress (`stepping.rs`).
    pub(super) line_step: Option<LineStep>,
    pending: VecDeque<Pending>,
    /// The client's breakpoints and their ids; every module among them a
    /// listening agent clears when the client leaves.
    pub(super) breakpoints: Breakpoints,
    exited: bool,
    /// The modules and their texts.
    pub(super) sources: Sources,
    /// The current stop's frames and references.
    pub(super) stop: Stop,
    waiting: Vec<Waiting>,
}

impl Agent {
    #[expect(
        clippy::too_many_arguments,
        reason = "the agent's channels and shared state, each made by `start`"
    )]
    pub(crate) fn new(
        mode: Mode,
        inputs: Sender<Input>,
        received: Receiver<Input>,
        drain: Sender<Sender<()>>,
        control: DebugControl,
        events_detached: Arc<AtomicBool>,
        host: Arc<HostState>,
        process_name: String,
        sources: Sources,
    ) -> Agent {
        Agent {
            mode,
            inputs,
            received,
            drain,
            control: Some(control),
            events_detached,
            host,
            process_name,
            client: None,
            connections: 0,
            stop_on_entry: false,
            entry_pending: false,
            stopped: false,
            resuming: false,
            line_step: None,
            pending: VecDeque::new(),
            breakpoints: Breakpoints::default(),
            exited: false,
            sources,
            stop: Stop::new(),
            waiting: Vec::new(),
        }
    }

    /// Serves every input until the process ends.
    pub(crate) fn run(mut self) {
        while let Ok(input) = self.received.recv() {
            self.serve(input);
        }
    }

    fn serve(&mut self, input: Input) {
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
            Input::AttachDeadline { connection } => {
                if self.is_current(connection) && !self.is_attached() {
                    self.leave();
                }
            }
            Input::Event(event) => self.vm_event(event),
            Input::Host(message) => self.host_message(message),
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
        // Numbered before anything can fail, so that a refused connection's
        // number is never the next one's, whose session a stale `Closed`
        // from its reader would otherwise end.
        self.connections += 1;
        let connection = self.connections;
        let reader = stream
            .try_clone()
            .and_then(|reader| connection::spawn_reader(reader, connection, self.inputs.clone()));
        let attach_by = self.listening().then(|| Instant::now() + attach_deadline());
        let deadline = if self.listening() && reader.is_ok() {
            connection::spawn_attach_deadline(attach_deadline(), connection, self.inputs.clone())
        } else {
            Ok(())
        };
        let writer = match (reader, deadline) {
            (Ok(()), Ok(())) => stream.try_clone().and_then(|stream| {
                connection::spawn_writer(
                    stream,
                    connection,
                    self.inputs.clone(),
                    attach_by,
                    write_bound(),
                )
            }),
            (Err(error), _) | (_, Err(error)) => Err(error),
        };
        let Ok(writer) = writer else {
            // A connection the agent cannot read or write is refused, never
            // a panic of the agent's that would leave the host waiting.
            let _ = stream.shutdown(Shutdown::Both);
            self.next_connection();
            return;
        };
        self.client = Some(Client {
            connection,
            writer,
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
            "continue" => self.continue_running(connection, &request),
            "next" => self.step(connection, &request, Command::StepOver),
            "stepIn" => self.step(connection, &request, Command::StepInto),
            "stepOut" => self.step_out(connection, &request),
            "nsNext/runToPosition" => self.run_to_position(connection, &request),
            "stackTrace" => self.stack_trace(connection, &request),
            "scopes" => self.scopes(connection, &request),
            "variables" => self.variables(connection, &request),
            "source" => self.source(connection, &request),
            "setBreakpoints" => self.set_breakpoints(connection, &request),
            "setFunctionBreakpoints" => self.set_function_breakpoints(connection, &request),
            "setExceptionBreakpoints" => self.set_exception_breakpoints(connection, &request),
            "nsNext/setStepFilters" => self.set_step_filters(connection, &request),
            "nsNext/statistics" => {
                let statistics = json!({ "moduleTexts": self.sources.text_answers() });
                self.respond(connection, &request, Some(statistics));
            }
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
            // Arguments that hold no token are a token not presented.
            if self.listening() && !self.is_attached() {
                self.leave();
            }
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
            client.writer.mark_attached();
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
        // next safepoint once it goes on; one asked while a `Continue` or
        // a step is on its way is asked of the program it resumes.
        if !self.stopped || self.resuming {
            if let Some(control) = &self.control {
                control.pause();
            }
        }
        // A step by line still reading where it begins has not left the
        // stop: the pause finds the program stopped, as without the step,
        // and the step's own stop is reported as the step's.
        if let Some(step) = &mut self.line_step {
            if !matches!(step.phase, Phase::Origin { .. }) {
                step.interrupted = true;
            }
        }
        self.respond(connection, request, None);
        // A step by line deciding at a stop the client has not seen
        // reports it now.
        self.advance_line_step();
    }

    /// Sends `command` to the VM, the request answered when the VM
    /// answers it.
    pub(super) fn forward(
        &mut self,
        connection: u64,
        request: &protocol::Request,
        command: Command,
    ) {
        let conversion = if command == Command::Continue {
            Conversion::Continued
        } else {
            Conversion::Plain
        };
        self.forward_converted(connection, request, command, conversion);
    }

    pub(super) fn forward_converted(
        &mut self,
        connection: u64,
        request: &protocol::Request,
        command: Command,
        conversion: Conversion,
    ) -> bool {
        let goes_on = is_going_on(command.name());
        let reply = Reply::Request {
            connection,
            request_seq: request.seq,
            command: request.command.clone(),
            conversion,
        };
        if !self.command(command, reply) {
            self.respond_error(connection, request, "the debugger is detached");
            return false;
        }
        // The stop ends when the VM answers `Event::Continued`; until then
        // the client's requests of the stop are refused, and a `pause` is
        // asked of the program it resumes.
        if goes_on {
            self.resuming = true;
        }
        true
    }

    /// The program goes on: nothing the stop handed out is valid any more,
    /// and a request still waiting for the stop is answered by an error.
    fn went_on(&mut self) {
        self.stopped = false;
        self.stop.end();
        self.answer_waiting();
    }

    // ---- The stop ----

    /// Whether the program stands at a stop the client was shown and has
    /// not asked to leave; a request only a stop serves is answered by an
    /// error otherwise, never sent to the VM.
    pub(super) fn at_stop(&mut self, connection: u64, request: &protocol::Request) -> bool {
        let at_stop = self.stopped && !self.resuming && self.line_step.is_none();
        if !at_stop {
            self.respond_error(connection, request, "the program is running");
        }
        at_stop
    }

    fn stack_trace(&mut self, connection: u64, request: &protocol::Request) {
        if self
            .arguments::<protocol::StackTraceArguments>(connection, request)
            .is_some()
            && self.at_stop(connection, request)
        {
            self.wait(connection, request);
        }
    }

    fn source(&mut self, connection: u64, request: &protocol::Request) {
        if self
            .arguments::<protocol::SourceArguments>(connection, request)
            .is_some()
        {
            self.wait(connection, request);
        }
    }

    /// Answers `request` once what it reads is there, asking the VM for it.
    fn wait(&mut self, connection: u64, request: &protocol::Request) {
        self.waiting.push(Waiting {
            connection,
            request: request.clone(),
            generation: self.stop.generation(),
        });
        self.answer_waiting();
    }

    /// Answers every waiting request whose answer is there, or can never
    /// come: the program went on since it was asked.
    fn answer_waiting(&mut self) {
        for waiting in std::mem::take(&mut self.waiting) {
            // A write that failed let the client go, and with it every
            // request it was waiting for.
            if !self.is_current(waiting.connection) {
                continue;
            }
            let outcome = if waiting.generation == self.stop.generation() {
                self.try_answer(&waiting.request)
            } else {
                Some(Err("the program went on".to_string()))
            };
            match outcome {
                Some(outcome) => self.write_response(
                    waiting.connection,
                    waiting.request.seq,
                    &waiting.request.command,
                    outcome,
                ),
                None => self.waiting.push(waiting),
            }
        }
        self.advance_line_step();
    }

    /// A waiting request's answer, `None` while the VM has not answered
    /// what it reads.
    fn try_answer(&mut self, request: &protocol::Request) -> Option<Outcome> {
        if request.command == "stackTrace" {
            self.try_stack_trace(request)
        } else {
            self.try_source(request)
        }
    }

    /// Whether the stop's frames are known, asking the VM for them when
    /// they were not asked yet; `None` while they are on their way.
    pub(super) fn frames_known(&mut self) -> Option<Result<(), String>> {
        match self.stop.frames() {
            Frames::Known(_) => Some(Ok(())),
            Frames::Asked => None,
            Frames::Refused(reason) => Some(Err(reason.clone())),
            Frames::Missing => {
                let generation = self.stop.generation();
                if !self.command(Command::StackTrace, Reply::Frames { generation }) {
                    return Some(Err("the debugger is detached".to_string()));
                }
                self.stop.frames_asked();
                None
            }
        }
    }

    fn try_stack_trace(&mut self, request: &protocol::Request) -> Option<Outcome> {
        let arguments: protocol::StackTraceArguments =
            serde_json::from_value(request.arguments.clone()).unwrap_or_default();
        if let Err(reason) = self.frames_known()? {
            return Some(Err(reason));
        }
        let (total, page) = self.stop.page(
            arguments.start_frame.unwrap_or(0),
            arguments.levels.unwrap_or(0),
        );
        // Only the texts of the frames shown: a client that asks for the
        // top frame first reads no other module's text.
        let modules: BTreeSet<String> = page
            .iter()
            .filter_map(|(_, frame)| frame.location.as_ref())
            .map(|location| location.module.clone())
            .collect();
        let mut complete = true;
        for module in modules {
            match self.read_text(&module) {
                Ok(true) => {}
                Ok(false) => complete = false,
                Err(reason) => return Some(Err(reason)),
            }
        }
        if !complete {
            return None;
        }
        let frames: Vec<protocol::StackFrame> = page
            .iter()
            .map(|(index, frame)| stopped::stack_frame(*index, frame, &mut self.sources))
            .collect();
        Some(Ok(Some(
            json!({ "stackFrames": frames, "totalFrames": total }),
        )))
    }

    /// Whether `module`'s current text is there for this stop, asking the
    /// VM for it, with the revision the agent holds, when it is not.
    pub(super) fn read_text(&mut self, module: &str) -> Result<bool, String> {
        let generation = self.stop.generation();
        if !self.sources.needs_text(module, generation) || !self.stopped {
            return Ok(true);
        }
        if let Some(known_revision) = self.sources.ask(module, generation) {
            let command = Command::ModuleText {
                module: module.to_string(),
                known_revision,
            };
            let reply = Reply::Text {
                module: module.to_string(),
            };
            if !self.command(command, reply) {
                self.sources.refused(module);
                return Err("the debugger is detached".to_string());
            }
        }
        Ok(false)
    }

    /// A `source` request's answer: the text of the module its reference
    /// names, current at a stop; while the program runs, the text read at
    /// the last stop.
    fn try_source(&mut self, request: &protocol::Request) -> Option<Outcome> {
        let arguments: protocol::SourceArguments =
            serde_json::from_value(request.arguments.clone()).unwrap_or_default();
        let reference = arguments
            .source_reference
            .filter(|reference| *reference > 0)
            .or_else(|| arguments.source.and_then(|source| source.source_reference))
            .unwrap_or_default();
        let Some(module) = self
            .sources
            .module_of_reference(reference)
            .map(str::to_string)
        else {
            return Some(Err("no source has that reference".to_string()));
        };
        match self.read_text(&module) {
            Ok(true) => {}
            Ok(false) => return None,
            Err(reason) => return Some(Err(reason)),
        }
        Some(match self.sources.text(&module) {
            Some(text) => Ok(Some(json!({ "content": text }))),
            None => Err(format!("the module `{module}` has no text")),
        })
    }

    fn scopes(&mut self, connection: u64, request: &protocol::Request) {
        let Some(arguments) = self.arguments::<protocol::ScopesArguments>(connection, request)
        else {
            return;
        };
        if !self.at_stop(connection, request) {
            return;
        }
        let index = arguments
            .frame_id
            .checked_sub(1)
            .and_then(|index| usize::try_from(index).ok())
            .filter(|index| self.stop.has_frame(*index));
        let Some(index) = index else {
            self.respond_error(connection, request, "no frame of this stop has that id");
            return;
        };
        let locals = self.stop.reference(Target::Frame(index));
        let mut scopes = vec![protocol::Scope {
            name: "Locals",
            presentation_hint: Some("locals"),
            variables_reference: locals,
            expensive: false,
        }];
        if index == 0 {
            if let Some(returned) = self.stop.returned().cloned() {
                scopes.push(protocol::Scope {
                    name: "Return value",
                    presentation_hint: Some("returnValue"),
                    variables_reference: self.stop.reference(Target::Returned(returned)),
                    expensive: false,
                });
            }
        }
        self.respond(connection, request, Some(json!({ "scopes": scopes })));
    }

    fn variables(&mut self, connection: u64, request: &protocol::Request) {
        let Some(arguments) = self.arguments::<protocol::VariablesArguments>(connection, request)
        else {
            return;
        };
        if !self.at_stop(connection, request) {
            return;
        }
        let Some(target) = self.stop.target(arguments.variables_reference) else {
            self.respond_error(
                connection,
                request,
                "no variables of this stop have that reference",
            );
            return;
        };
        let start = arguments.start.unwrap_or(0);
        let count = arguments.count.filter(|count| *count > 0);
        let generation = self.stop.generation();
        match target {
            Target::Frame(frame) => {
                let page = Some((start, count.unwrap_or(usize::MAX)));
                let conversion = Conversion::Variables { generation, page };
                let command = Command::Variables { frame };
                self.forward_converted(connection, request, command, conversion);
            }
            Target::Value {
                id,
                child_count,
                indexed,
            } => {
                let wanted = match arguments.filter.as_deref() {
                    Some("indexed") => indexed,
                    Some("named") => !indexed,
                    _ => true,
                };
                if !wanted {
                    self.respond(connection, request, Some(json!({ "variables": [] })));
                    return;
                }
                let command = Command::Children {
                    value: id,
                    start,
                    count: count.unwrap_or(child_count.saturating_sub(start)),
                };
                let conversion = Conversion::Variables {
                    generation,
                    page: None,
                };
                self.forward_converted(connection, request, command, conversion);
            }
            Target::Returned(value) => {
                let returned = Variable {
                    name: "returned".to_string(),
                    value: Some(value),
                };
                let variables = vec![self.stop.variable(&returned)];
                self.respond(connection, request, Some(json!({ "variables": variables })));
            }
        }
    }

    // ---- The session's end ----

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
        self.entry_pending = false;
        self.line_step = None;
        self.went_on();
    }

    fn disconnect(&mut self, connection: u64, request: &protocol::Request) {
        let Some(arguments) = self.arguments::<protocol::DisconnectArguments>(connection, request)
        else {
            return;
        };
        self.respond(connection, request, None);
        if arguments.terminate_debuggee {
            // At once, from this thread, for a program that reaches no
            // safepoint: no `ensure:` block runs. The response is written
            // first, within the writer's bound.
            if let Some(client) = self.client.take() {
                client.writer.finish();
            }
            eprintln!("{TERMINATED_LINE}");
            std::process::exit(1);
        }
        self.leave();
    }

    /// Parses a request's arguments, answering an error for ones it cannot.
    pub(super) fn arguments<T: serde::de::DeserializeOwned + Default>(
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

    /// The client left: by `disconnect`, by closing its connection, or
    /// refused. A client that never attached configured nothing and is
    /// only let go.
    fn leave(&mut self) {
        let Some(client) = self.client.take() else {
            return;
        };
        // The writer writes what is queued, within its bound, then shuts
        // the connection down; the agent waits for none of it.
        drop(client.writer);
        let was_attached = client.attached;
        self.waiting.clear();
        self.line_step = None;
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
        self.resuming = false;
        self.line_step = None;
        self.went_on();
    }

    /// Leaves the VM stopping nowhere, for the next client of a listening
    /// agent, since nobody would be there to go on from a stop: no line or
    /// method breakpoint and no exception stop -- not even the uncaught
    /// exceptions and halts the VM stops at before any client configures
    /// it -- the default step filters, and a stopped program going on.
    fn reset_the_vm(&mut self) {
        for module in self.breakpoints.modules() {
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
        self.breakpoints.clear();
        if self.stopped {
            self.go_on();
        }
    }

    /// Lets a stopped program go on, nobody being there to see the stop.
    fn go_on(&mut self) {
        self.command(Command::Continue, Reply::Internal);
        self.resuming = false;
        self.went_on();
    }

    // ---- The VM ----

    /// Queues `command`, its answer going to `reply`; `false` once the
    /// debugger was detached.
    pub(super) fn command(&mut self, command: Command, reply: Reply) -> bool {
        let Some(control) = &self.control else {
            return false;
        };
        let module = match &command {
            Command::SetBreakpoints { module, .. } | Command::ModuleText { module, .. } => {
                Some(module.clone())
            }
            _ => None,
        };
        let configures = matches!(
            command,
            Command::SetBreakpoints { .. }
                | Command::SetStepFilters { .. }
                | Command::SetMethodBreakpoints(_)
                | Command::SetExceptionBreakpoints { .. }
        );
        self.pending.push_back(Pending {
            command: command.name(),
            module,
            reply,
        });
        control.send(command);
        if configures {
            self.host.configuration_queued();
        }
        true
    }

    fn vm_event(&mut self, event: Event) {
        if self.control.is_none() {
            return;
        }
        match event {
            Event::Stopped { reason, returned } => self.stopped_at(&reason, returned),
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

    fn stopped_at(&mut self, reason: &StopReason, returned: Option<ValueDescription>) {
        self.stopped = true;
        self.resuming = false;
        self.stop.begin(returned);
        if !self.is_attached() {
            self.go_on();
            return;
        }
        if self.line_step_stopped(reason) {
            return;
        }
        self.report_stop(reason);
    }

    /// Shows the client the stop the program stands at, for `reason`.
    pub(super) fn report_stop(&mut self, reason: &StopReason) {
        self.line_step = None;
        let body = stopped_body(reason, self.entry_pending, &self.breakpoints);
        self.entry_pending = false;
        self.event("stopped", Some(to_value(body)));
    }

    /// Whether a command `command` names (`Command::name`), for `module`
    /// when given, is on its way to the VM: a replacement of a set, which
    /// outdates an event the VM sent unasked about that set before it.
    pub(super) fn replacing(&self, command: &str, module: Option<&str>) -> bool {
        self.pending.iter().any(|pending| {
            pending.command == command && (module.is_none() || pending.module.as_deref() == module)
        })
    }

    /// Hands the VM's answer to the command it answers.
    fn answer(&mut self, event: Event) {
        match &event {
            Event::Continued => {
                self.resuming = false;
                self.went_on();
            }
            Event::Breakpoints {
                module,
                breakpoints,
            } => {
                let ids: Vec<u32> = breakpoints.iter().map(|state| state.id.0).collect();
                let asked = self.breakpoints.is_answer(&ids);
                self.breakpoints.saw(&ids);
                if !asked {
                    self.line_breakpoints_changed(module, breakpoints);
                    return;
                }
            }
            Event::MethodBreakpoints(states) => {
                let ids: Vec<u32> = states.iter().map(|state| state.id.0).collect();
                let asked = self.breakpoints.is_answer(&ids);
                self.breakpoints.saw(&ids);
                if !asked {
                    self.function_breakpoints_changed(states);
                    return;
                }
            }
            Event::Refused { command, .. } if is_going_on(command) => {
                // The program stays at its stop, which the client keeps
                // reading.
                self.resuming = false;
            }
            _ => {}
        }
        let Some(index) = self
            .pending
            .iter()
            .position(|pending| answers(&event, pending))
        else {
            return;
        };
        let Some(pending) = self.pending.remove(index) else {
            return;
        };
        match pending.reply {
            Reply::Internal => {}
            Reply::Repeated => {
                if matches!(event, Event::Refused { .. }) && self.stopped {
                    self.report_stop(&StopReason::Step);
                }
            }
            Reply::Frames { generation } => {
                if generation == self.stop.generation() {
                    match event {
                        Event::StackTrace(frames) => self.stop.frames_answered(frames),
                        Event::Refused { reason, .. } => self.stop.frames_refused(reason),
                        _ => {}
                    }
                }
                self.answer_waiting();
            }
            Reply::Text { module } => {
                match event {
                    Event::ModuleText { revision, text, .. } => {
                        self.sources.answered(&module, revision, text);
                    }
                    _ => self.sources.refused(&module),
                }
                self.answer_waiting();
            }
            Reply::Request {
                connection,
                request_seq,
                command,
                conversion,
            } => {
                let running = self
                    .line_step
                    .as_ref()
                    .is_some_and(|step| matches!(step.phase, Phase::Running { .. }));
                if running && matches!(event, Event::Refused { .. }) {
                    // A step by line's first step, refused: no step runs.
                    self.line_step = None;
                }
                let outcome = self.convert(event, conversion);
                self.write_response(connection, request_seq, &command, outcome);
            }
        }
    }

    /// A request's response, made of the VM's answer.
    fn convert(&mut self, event: Event, conversion: Conversion) -> Outcome {
        if let Event::Refused { reason, .. } = event {
            return Err(reason);
        }
        match (event, conversion) {
            (Event::Continued, Conversion::Continued) => {
                Ok(Some(json!({ "allThreadsContinued": true })))
            }
            (Event::Variables(variables), Conversion::Variables { generation, page }) => {
                // References of a stop the program left would name nothing.
                if generation != self.stop.generation() {
                    return Err("the program went on".to_string());
                }
                let (start, count) = page.unwrap_or((0, usize::MAX));
                let variables: Vec<protocol::Variable> = variables
                    .iter()
                    .skip(start)
                    .take(count)
                    .map(|variable| self.stop.variable(variable))
                    .collect();
                Ok(Some(json!({ "variables": variables })))
            }
            (
                Event::Breakpoints { breakpoints, .. },
                Conversion::Breakpoints {
                    module,
                    entries,
                    text,
                },
            ) => {
                let (breakpoints, pairs) =
                    breakpoints::line_response(&entries, &breakpoints, &text);
                self.breakpoints.answered_lines(&module, &pairs);
                Ok(Some(json!({ "breakpoints": breakpoints })))
            }
            (Event::MethodBreakpoints(states), Conversion::FunctionBreakpoints { entries }) => {
                let (breakpoints, pairs) = breakpoints::function_response(&entries, &states);
                self.breakpoints.answered_functions(&pairs);
                Ok(Some(json!({ "breakpoints": breakpoints })))
            }
            (Event::ExceptionBreakpointsSet, Conversion::ExceptionBreakpoints { count }) => {
                let breakpoints: Vec<protocol::Breakpoint> = (0..count)
                    .map(|_| protocol::Breakpoint::verified())
                    .collect();
                Ok(Some(json!({ "breakpoints": breakpoints })))
            }
            _ => Ok(None),
        }
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
                self.line_step = None;
                self.answer_what_the_vm_sent();
                if self.is_attached() {
                    let exited = protocol::ExitedEventBody {
                        exit_code: i64::from(status),
                    };
                    self.event("exited", Some(to_value(exited)));
                    self.event("terminated", None);
                    self.answer_until_disconnected();
                }
                // Written before the host goes on to end the process,
                // within the writer's bound.
                if let Some(client) = self.client.take() {
                    client.writer.finish();
                }
                let _ = done.send(());
            }
        }
    }

    /// After `terminated`, every request the client sends is still
    /// answered -- one it sent before it heard of the end, a `terminate`
    /// racing the program's end among them, which DAP answers like any
    /// other -- until it disconnects, closes the connection, or is silent
    /// for `ENDED_LINGER`, which bounds how long the process outlives its
    /// program for a client that never disconnects. A `terminate` or a
    /// `disconnect` succeeds, the program having ended; any other request
    /// is refused, nothing being left to serve it.
    fn answer_until_disconnected(&mut self) {
        loop {
            let Some(connection) = self.client.as_ref().map(|client| client.connection) else {
                return;
            };
            let input = match self.received.recv_timeout(ENDED_LINGER) {
                Ok(input) => input,
                Err(_) => return,
            };
            match input {
                Input::Request {
                    connection: from,
                    message,
                } if from == connection => {
                    let Ok(request) = serde_json::from_value::<protocol::Request>(message) else {
                        continue;
                    };
                    if request.kind != "request" {
                        continue;
                    }
                    match request.command.as_str() {
                        "disconnect" => {
                            self.respond(connection, &request, None);
                            return;
                        }
                        "terminate" => self.respond(connection, &request, None),
                        _ => self.respond_error(connection, &request, "the program ended"),
                    }
                }
                Input::Closed { connection: from } if from == connection => return,
                Input::Connected(stream) => {
                    // The one client is served to its end; nobody else is.
                    let _ = stream.shutdown(Shutdown::Both);
                }
                _ => {}
            }
        }
    }

    /// At the program's end: every event the VM sent before it is served
    /// -- the pump hands them over and ends, and the inputs they join are
    /// served in order -- so that a request the VM answered gets its
    /// answer, and every request still waiting, whose answer can no longer
    /// come, is answered by an error, never by silence.
    fn answer_what_the_vm_sent(&mut self) {
        let (handed_over, all_handed_over) = std::sync::mpsc::channel();
        if self.control.is_some()
            && self.drain.send(handed_over).is_ok()
            && all_handed_over.recv().is_ok()
        {
            while let Ok(input) = self.received.try_recv() {
                self.serve(input);
            }
        }
        for waiting in std::mem::take(&mut self.waiting) {
            self.write_response(
                waiting.connection,
                waiting.request.seq,
                &waiting.request.command,
                Err("the program ended".to_string()),
            );
        }
        while let Some(pending) = self.pending.pop_front() {
            if let Reply::Request {
                connection,
                request_seq,
                command,
                ..
            } = pending.reply
            {
                self.write_response(
                    connection,
                    request_seq,
                    &command,
                    Err("the program ended".to_string()),
                );
            }
        }
    }

    // ---- Writing ----

    pub(super) fn respond(
        &mut self,
        connection: u64,
        request: &protocol::Request,
        body: Option<Value>,
    ) {
        self.write_response(connection, request.seq, &request.command, Ok(body));
    }

    pub(super) fn respond_error(
        &mut self,
        connection: u64,
        request: &protocol::Request,
        message: &str,
    ) {
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
    pub(super) fn event(&mut self, event: &'static str, body: Option<Value>) {
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

    /// Queues the message `build` makes of the next sequence number for
    /// the client's writer, so that this thread never waits on the socket:
    /// a client that stops reading would otherwise block it in a write,
    /// and with it every event, the host's word and the deadline. A client
    /// whose queue is full, or whose writer gave up (`connection::Writer`),
    /// has left.
    fn write(&mut self, build: impl FnOnce(i64) -> Value) {
        let Some(client) = &mut self.client else {
            return;
        };
        let message = build(client.next_seq);
        client.next_seq += 1;
        if !client.writer.send(message) {
            self.leave();
        }
    }
}

/// Whether the command named `name` (`Command::name`) makes the stopped
/// program go on.
fn is_going_on(name: &str) -> bool {
    matches!(
        name,
        "Continue" | "StepOver" | "StepInto" | "StepOut" | "RunToPosition"
    )
}

/// Whether `event` answers the command `pending` waits for.
fn answers(event: &Event, pending: &Pending) -> bool {
    let command = pending.command;
    match event {
        Event::Continued => is_going_on(command),
        Event::Refused {
            command: refused, ..
        } => refused == command,
        Event::StackTrace(_) => command == "StackTrace",
        Event::Variables(_) => matches!(command, "Variables" | "Children"),
        Event::Breakpoints { module, .. } => {
            command == "SetBreakpoints" && pending.module.as_ref() == Some(module)
        }
        Event::StepFiltersSet => command == "SetStepFilters",
        Event::MethodBreakpoints(_) => command == "SetMethodBreakpoints",
        Event::ExceptionBreakpointsSet => command == "SetExceptionBreakpoints",
        Event::ModuleText { module, .. } => {
            command == "ModuleText" && pending.module.as_ref() == Some(module)
        }
        Event::Stopped { .. } => false,
    }
}

/// The `stopped` event's body for a stop of `reason`: `entry_pending`
/// says a pause is the entry's, and a stop names the breakpoints it is for
/// by the DAP ids `known` gave them, only those the client's latest
/// request for their set holds, never one it replaced since the VM decided
/// the stop.
fn stopped_body(
    reason: &StopReason,
    entry_pending: bool,
    known: &Breakpoints,
) -> protocol::StoppedEventBody {
    let mut body = protocol::StoppedEventBody {
        reason: "pause",
        description: None,
        text: None,
        thread_id: THREAD_ID,
        all_threads_stopped: true,
        hit_breakpoint_ids: Vec::new(),
    };
    let named = |ids: &mut dyn Iterator<Item = &ns_runtime::BreakpointId>| -> Vec<u32> {
        ids.filter_map(|id| known.dap_id(id.0)).collect()
    };
    match reason {
        StopReason::Pause if entry_pending => body.reason = "entry",
        StopReason::Pause => {}
        StopReason::Breakpoint { ids } => {
            body.reason = "breakpoint";
            body.hit_breakpoint_ids = named(&mut ids.iter());
        }
        StopReason::Step => body.reason = "step",
        StopReason::MethodEntry { id, others } | StopReason::MethodExit { id, others } => {
            body.reason = "function breakpoint";
            body.hit_breakpoint_ids = named(&mut std::iter::once(id).chain(others));
        }
        StopReason::Exception {
            class_name,
            description,
            exits,
            ..
        } => {
            body.reason = "exception";
            body.text = Some(description.clone());
            body.description = Some(class_name.clone());
            body.hit_breakpoint_ids = named(&mut exits.iter());
        }
        StopReason::Halt { exits } => {
            body.reason = "exception";
            body.text = Some("halt".to_string());
            body.hit_breakpoint_ids = named(&mut exits.iter());
        }
    }
    body
}

pub(super) fn to_value(value: impl Serialize) -> Value {
    serde_json::to_value(value).expect("the agent's own types serialize")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ns_runtime::BreakpointId;

    #[test]
    fn a_breakpoint_stop_names_only_the_breakpoints_the_agent_knows() {
        let mut known = Breakpoints::default();
        let ids = known.request_lines("Loop", vec![10, 20]);
        known.answered_lines("Loop", &[(1, ids[0]), (2, ids[1])]);
        // The client removes the breakpoint at 10.
        known.request_lines("Loop", vec![20]);
        let replaced = StopReason::Breakpoint {
            ids: vec![BreakpointId(1)],
        };
        let body = to_value(stopped_body(&replaced, false, &known));
        assert_eq!(body["reason"], "breakpoint");
        assert!(body.get("hitBreakpointIds").is_none(), "{body}");

        let standing = StopReason::Breakpoint {
            ids: vec![BreakpointId(1), BreakpointId(2)],
        };
        let body = to_value(stopped_body(&standing, false, &known));
        assert_eq!(body["hitBreakpointIds"], json!([ids[1]]));
    }

    #[test]
    fn an_exception_stop_carries_its_text_and_class() {
        let exception = StopReason::Exception {
            class_name: "ZeroDivide".to_string(),
            description: "division by zero".to_string(),
            caught: false,
            exits: Vec::new(),
        };
        let body = to_value(stopped_body(&exception, false, &Breakpoints::default()));
        assert_eq!(body["reason"], "exception");
        assert_eq!(body["text"], "division by zero");
        assert_eq!(body["description"], "ZeroDivide");
        let halt = StopReason::Halt { exits: Vec::new() };
        let body = to_value(stopped_body(&halt, false, &Breakpoints::default()));
        assert_eq!(body["text"], "halt");
        let entry = to_value(stopped_body(
            &StopReason::Pause,
            true,
            &Breakpoints::default(),
        ));
        assert_eq!(entry["reason"], "entry");
    }
}
