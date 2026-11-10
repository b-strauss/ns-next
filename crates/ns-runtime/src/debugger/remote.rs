//! The debugger's remote: the half of the debugging API that a host hands
//! to another thread -- the agent that speaks to an IDE -- and the
//! commands and events that pass between it and the VM
//! (`docs/design/debugger.md`, decisions 1, 7 and 9). Nothing here holds a
//! `Value`, a `Root`, a handle or a `Symbol`: each is meaningful only on
//! the VM's thread, so everything that crosses is text and numbers, and a
//! value is named by a `ValueId` the stop that handed it out resolves.

use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use super::Attachments;
use crate::heap::machine_words::{MachineWord, MachineWords};
use crate::heap::{DEBUGGER_COMMANDS, DEBUGGER_PAUSE};

/// What the agent asks of the VM. `Continue`, `StackTrace`, `Variables`
/// and `Children` are served only while the program is stopped; one that
/// arrives while it runs is refused (`Event::Refused`). `Terminate` is
/// served either way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// The stopped program goes on.
    Continue,
    /// The run ends as an uncaught termination does: every pending
    /// `ensure:` block runs, and the host's send answers the error
    /// "the debugger terminated the program".
    Terminate,
    /// The stopped program's activations, innermost first.
    StackTrace,
    /// The variables of the stopped program's activation at index `frame`
    /// of the stack trace.
    Variables { frame: usize },
    /// `count` of the children of a value the stop handed out, from the
    /// `start`th.
    Children {
        value: ValueId,
        start: usize,
        count: usize,
    },
}

impl Command {
    /// The command's name, as a refusal names it.
    pub fn name(&self) -> &'static str {
        match self {
            Command::Continue => "Continue",
            Command::Terminate => "Terminate",
            Command::StackTrace => "StackTrace",
            Command::Variables { .. } => "Variables",
            Command::Children { .. } => "Children",
        }
    }
}

/// What the VM tells the agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The program stopped at a safepoint, and serves commands until a
    /// `Continue` or a `Terminate`.
    Stopped { reason: StopReason },
    /// The answer to `Command::StackTrace`.
    StackTrace(Vec<FrameDescription>),
    /// The answer to `Command::Variables` and `Command::Children`.
    Variables(Vec<Variable>),
    /// The command `command` names (`Command::name`) was not served, for
    /// `reason`.
    Refused { command: String, reason: String },
    /// The answer to `Command::Continue`: the program runs again.
    Continued,
}

/// Why the program stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// The agent asked for it (`DebugRemote::pause`).
    Pause,
}

/// A value a stop handed out, by its index in the stop's table; it names
/// nothing once the program goes on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValueId(pub u32);

/// One activation of a stopped program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameDescription {
    pub kind: FrameKind,
    pub program_counter: usize,
    pub source: ns_syntax::Span,
    pub receiver: ValueDescription,
}

/// What an activation runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameKind {
    Method { selector: String },
    Block,
    Initializer,
    SuperclassClause,
    Factory,
    TopLevel,
}

/// A named value of an activation, or a child of a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variable {
    pub name: String,
    pub value: ValueDescription,
}

/// A value as the agent shows it: its id for `Command::Children`, its
/// label, its class's name and how many children it has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueDescription {
    pub id: ValueId,
    pub label: String,
    pub class_name: String,
    pub child_count: usize,
}

/// The agent's end of an attached debugger (`Interpreter::attach_debugger`),
/// which may move to another thread. Dropping it detaches the debugger: the
/// program runs on, a stopped one included.
pub struct DebugRemote {
    commands: Sender<Command>,
    events: Receiver<Event>,
    words: MachineWords,
    /// The interpreter's arming lock: held across the two writes that arm
    /// the poll, so that the VM's clearing of a bit cannot fall between
    /// them (`Arming`).
    arming: Arc<Mutex<Attachments>>,
    /// The number of the attachment that handed this remote out, which
    /// arms only while it is current.
    attachment: u64,
}

impl DebugRemote {
    pub(crate) fn new(
        commands: Sender<Command>,
        events: Receiver<Event>,
        words: MachineWords,
        arming: Arc<Mutex<Attachments>>,
        attachment: u64,
    ) -> DebugRemote {
        DebugRemote {
            commands,
            events,
            words,
            arming,
            attachment,
        }
    }

    /// Asks the program to stop at its next safepoint, interpreted or in
    /// machine code, a push or a loop's head; the VM answers
    /// `Event::Stopped`. A pause asked while no run executes stops the
    /// next run at its first safepoint.
    pub fn pause(&self) {
        self.arm(DEBUGGER_PAUSE);
    }

    /// Queues `command` for the VM: the next safepoint serves it if the
    /// program runs, the stop does if it is stopped. Queued before the bit
    /// is set, so that the safepoint that finds the bit finds the command;
    /// a remote whose VM is gone queues it nowhere.
    pub fn send(&self, command: Command) {
        if self.commands.send(command).is_ok() {
            self.arm(DEBUGGER_COMMANDS);
        }
    }

    /// The VM's next event, waiting for at most `timeout`; `None` once it
    /// passed, or once the VM is gone.
    pub fn next_event(&self, timeout: Duration) -> Option<Event> {
        self.events.recv_timeout(timeout).ok()
    }

    /// Sets `bit` in the request word and lowers the depth limit, so that
    /// the next poll anywhere takes the safepoint -- a compiled callee's
    /// prologue compares the limit alone. Both writes sequentially
    /// consistent, against the VM's fenced re-read
    /// (`Heap::synchronize_depth_limit`); the agent never clears a bit. A
    /// remote whose debugger is gone, detached or replaced by a later
    /// attachment, arms nothing: its bit would stop the run for another
    /// remote's debugger, or for none.
    fn arm(&self, bit: u64) {
        let armed = self.arming.lock().unwrap_or_else(PoisonError::into_inner);
        if !armed.is_current(self.attachment) {
            return;
        }
        self.words.fetch_or(MachineWord::SafepointRequested, bit);
        self.words
            .set_sequentially_consistent(MachineWord::DepthLimit, 0);
    }
}
