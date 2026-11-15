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

/// What the agent asks of the VM. `Continue`, the steps, `RunToPosition`,
/// `StackTrace`, `Variables`, `Children` and `ModuleText` are served only
/// while the program is stopped; one that arrives while it runs is refused
/// (`Event::Refused`). `Terminate` is served either way. An agent waits
/// for `Event::Stopped` before it sends a command only a stop serves: one
/// sent right after `DebugRemote::pause` is refused or served depending
/// on whether a safepoint drains the queue before the stop or the stop
/// serves it. A command is served by the run it arrives during; one
/// queued while no run executes, or still queued when a run ends, is
/// dropped and answered by `Event::Refused`, its reason that no run
/// executes. A configuration command -- `SetBreakpoints`,
/// `SetMethodBreakpoints`, `SetStepFilters`, `SetExceptionBreakpoints` --
/// is served wherever commands are taken instead: at a safepoint while
/// the program runs, at a stop, and when a run starts or ends, for one
/// queued between runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// The stopped program goes on.
    Continue,
    /// The stopped program goes on to the next statement start of the top
    /// frame -- not of a recursive call of its body, which runs past --
    /// or, once the frame returns, to the place its caller goes on at:
    /// the index after the instruction the caller stands at, mid-statement.
    /// Answered by `Event::Continued`, then `Event::Stopped` with
    /// `StopReason::Step`, whose `returned` is the value the frame's last
    /// call answered; any other stop before -- a breakpoint, a pause --
    /// ends the step and reports its own reason.
    StepOver,
    /// As `StepOver`, or the first statement start of a frame the top
    /// frame pushes meanwhile, the callee of a send no primitive or leaf
    /// answered.
    StepInto,
    /// The stopped program goes on to the place the top frame's caller
    /// goes on at once the frame returns, as `StepOver` does past its
    /// last statement.
    StepOut,
    /// The stopped program goes on to the statement `offset` names in the
    /// module `module` names, resolved as a line breakpoint is
    /// (`SetBreakpoints`) and taken in any activation, or to the next
    /// stop of any other reason before it: either ends it. Answered by
    /// `Event::Continued`, then `Event::Stopped` with `StopReason::Step`,
    /// or, for a position that resolves to no statement -- its module not
    /// built, no statement at or after the offset -- by `Event::Refused`,
    /// the program still stopped.
    RunToPosition { module: String, offset: u32 },
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
    /// Replaces every line breakpoint of the module `module` names -- its
    /// root class's name, as `FrameLocation::module` names it, a shipped
    /// module's too -- by `breakpoints`, each at a byte offset into the
    /// module's current text. Each resolves to the first statement start
    /// at or after its offset in any body of the module, and every body
    /// with a statement of that same span stops there: an inlined block's
    /// copy in its home and its fallback closure alike. Each is set
    /// afresh, under a new id and its hits counted from none, whether or
    /// not the module's set held it before. Answered by
    /// `Event::Breakpoints`.
    SetBreakpoints {
        module: String,
        breakpoints: Vec<LineBreakpoint>,
    },
    /// Replaces the step filters: the frames no step stops in
    /// (`docs/design/debugger.md`, decision 16). `shipped` filters every
    /// body of the shipped modules, and `classes` the bodies of each class
    /// declaration it names and of every declaration nested in one. A
    /// step that would stop in a filtered frame goes on as a step into
    /// until it stops in an unfiltered one; a breakpoint in filtered code
    /// stops all the same, and a step begun in a filtered frame steps in
    /// that frame. Until the first, the shipped modules are filtered and
    /// no class is. Answered by `Event::StepFiltersSet`.
    SetStepFilters {
        shipped: bool,
        classes: Vec<ClassName>,
    },
    /// Replaces every method breakpoint by these. Each stops every
    /// activation of its method at the method's entry -- its first
    /// statement start -- and at each of its exits: before each of its
    /// returns -- the body's own, and the non-local returns of the blocks
    /// the compiler lowered inline in it, so that both lowerings stop
    /// there alike -- and, for a method left otherwise, by an unwind or
    /// by the non-local return of another block of its own, at the next
    /// place the program reaches. A method of a module not built yet, or one its class does
    /// not declare, is not verified until a module's build or an install
    /// declares it; an install sets each again on the method's new body.
    /// Answered by `Event::MethodBreakpoints`.
    SetMethodBreakpoints(Vec<MethodBreakpoint>),
    /// Replaces the exception breakpoints (`docs/design/debugger.md`,
    /// "Exceptions"). `uncaught` stops an exception no handler takes, where
    /// the search, having found none, is about to send `defaultAction`;
    /// `caught` stops one a handler takes, where the search is about to
    /// run the handler; either only for an exception whose class is, or
    /// inherits from, a class declaration `classes` names, or for every
    /// exception when `classes` is empty. `halt` stops a `Halt` no handler
    /// takes, which going on resumes with nil; with `halt` off, a `Halt` is
    /// an exception as any other. Until the first, `uncaught` and `halt`
    /// are on, `caught` is off and no class is named. Answered by
    /// `Event::ExceptionBreakpointsSet`.
    SetExceptionBreakpoints {
        uncaught: bool,
        caught: bool,
        halt: bool,
        classes: Vec<ClassName>,
    },
    /// The current text of the module `module` names -- its root class's
    /// name, as `FrameLocation::module` names it, a shipped module's too,
    /// built or not -- unless `known_revision` is the revision the answer
    /// would carry. `known_revision` is the revision of a
    /// `ModuleTextAnswer::Text` the agent holds for this very module; an
    /// agent keeps a revision only beside a `Text`, since the revision
    /// counts every install and every build of a module's root class, and
    /// a `None` answered at one revision says nothing of the next. Served
    /// only while the program is stopped, as `StackTrace` is, so that the
    /// agent reads a text only where it reads positions in it. Answered by
    /// `Event::ModuleText`.
    ModuleText {
        module: String,
        known_revision: Option<u64>,
    },
}

/// A method a method breakpoint names: by its selector, the class
/// declaration that declares it, and its side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MethodBreakpoint {
    /// The class declaration, as a step filter names it.
    pub class: ClassName,
    /// Whether the method is the class side's.
    pub class_side: bool,
    pub selector: String,
    /// Which of its hits stop: each entry and each exit of an activation
    /// of the method is a hit, an exit by an unwind included, counted
    /// while the debugger is attached from the moment the breakpoint is
    /// set; `None` stops at every one.
    pub hit_condition: Option<HitCondition>,
}

/// A line breakpoint as `Command::SetBreakpoints` asks for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineBreakpoint {
    /// A byte offset into its module's current text.
    pub offset: u32,
    /// Which of its hits stop: each time an activation reaches its
    /// statement is a hit, counted while the debugger is attached from the
    /// moment the breakpoint is set; `None` stops at every one.
    pub hit_condition: Option<HitCondition>,
}

impl LineBreakpoint {
    /// A breakpoint at `offset` that stops at every hit.
    pub fn at(offset: u32) -> LineBreakpoint {
        LineBreakpoint {
            offset,
            hit_condition: None,
        }
    }
}

/// Which hits of a breakpoint stop, by the hit's number, the first being
/// 1: DAP's `hitCondition`, counted by the VM, so that a hit that does not
/// stop costs a counter rather than a stop and a round trip to the agent
/// (`docs/design/debugger.md`, "Hit counts and temporary breakpoints").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitCondition {
    /// The hit numbered so, and no other.
    Equal(u32),
    /// Every hit after the one numbered so.
    Greater(u32),
    /// The hit numbered so and every one after it.
    GreaterOrEqual(u32),
    /// Every hit whose number is a multiple of this one; none for 0.
    Multiple(u32),
}

impl HitCondition {
    /// Whether the hit numbered `hits` stops.
    pub fn holds(self, hits: u32) -> bool {
        match self {
            HitCondition::Equal(count) => hits == count,
            HitCondition::Greater(count) => hits > count,
            HitCondition::GreaterOrEqual(count) => hits >= count,
            HitCondition::Multiple(count) => count != 0 && hits.is_multiple_of(count),
        }
    }
}

/// A class declaration of a module, as a step filter names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassName {
    /// The name of the module's root class, as `FrameLocation::module`
    /// names it.
    pub module: String,
    /// The names of the declarations from the module's root down to the
    /// class, the root's own excluded: empty for the root itself, `["A",
    /// "B"]` for `B` nested in `A`.
    pub path: Vec<String>,
}

impl Command {
    /// The command's name, as a refusal names it.
    pub fn name(&self) -> &'static str {
        match self {
            Command::Continue => "Continue",
            Command::StepOver => "StepOver",
            Command::StepInto => "StepInto",
            Command::StepOut => "StepOut",
            Command::RunToPosition { .. } => "RunToPosition",
            Command::Terminate => "Terminate",
            Command::StackTrace => "StackTrace",
            Command::Variables { .. } => "Variables",
            Command::Children { .. } => "Children",
            Command::SetBreakpoints { .. } => "SetBreakpoints",
            Command::SetStepFilters { .. } => "SetStepFilters",
            Command::SetMethodBreakpoints(_) => "SetMethodBreakpoints",
            Command::SetExceptionBreakpoints { .. } => "SetExceptionBreakpoints",
            Command::ModuleText { .. } => "ModuleText",
        }
    }
}

/// What the VM tells the agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The program stopped at a safepoint, and serves commands until a
    /// `Continue`, a step or a `Terminate`. `returned` is, at a step's
    /// stop, the value the stepping frame's last call answered -- the
    /// first value of the stop -- and `None` otherwise.
    Stopped {
        reason: StopReason,
        returned: Option<ValueDescription>,
    },
    /// The answer to `Command::StackTrace`.
    StackTrace(Vec<FrameDescription>),
    /// The answer to `Command::Variables` and `Command::Children`.
    Variables(Vec<Variable>),
    /// The command `command` names (`Command::name`) was not served, for
    /// `reason`.
    Refused { command: String, reason: String },
    /// The answer to `Command::Continue`: the program runs again.
    Continued,
    /// The line breakpoints of the module `module` names, in the order of
    /// their offsets: the answer to `Command::SetBreakpoints`, and sent
    /// unasked when a pending one resolves -- its module was built -- or
    /// an install moved one.
    Breakpoints {
        module: String,
        breakpoints: Vec<BreakpointState>,
    },
    /// The answer to `Command::SetStepFilters`: the filters are in effect.
    StepFiltersSet,
    /// The method breakpoints, in the order they were set: the answer to
    /// `Command::SetMethodBreakpoints`, and sent unasked when a module's
    /// build or an install verifies one or no longer declares its method.
    MethodBreakpoints(Vec<MethodBreakpointState>),
    /// The answer to `Command::SetExceptionBreakpoints`: the exception
    /// breakpoints are in effect.
    ExceptionBreakpointsSet,
    /// The answer to `Command::ModuleText`. `revision` is the interpreter's
    /// count of installs and of builds of a module's root class, the same
    /// for every module and 0 before the first: a text read at a revision
    /// is current until it is raised.
    ModuleText {
        module: String,
        revision: u64,
        text: ModuleTextAnswer,
    },
}

/// What `Event::ModuleText` says of a module's text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModuleTextAnswer {
    /// The agent's `known_revision` is the current one.
    Unchanged,
    /// The module's current text.
    Text(String),
    /// No module of that name is built (a shipped module needs no build),
    /// or it has no text (an image built without its source).
    None,
}

/// Why the program stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    /// The agent asked for it (`DebugRemote::pause`).
    Pause,
    /// The program reached the statement the breakpoints `ids` stand at.
    Breakpoint { ids: Vec<BreakpointId> },
    /// A step the agent asked for (`Command::StepOver`, `StepInto`,
    /// `StepOut`, `RunToPosition`) arrived.
    Step,
    /// The program entered the method of the method breakpoint `id`, and
    /// stands at its first statement start; `others` are the other
    /// breakpoints standing there -- line breakpoints, and method
    /// breakpoints of the same method -- which this stop is for too.
    MethodEntry {
        id: BreakpointId,
        others: Vec<BreakpointId>,
    },
    /// The program leaves the method of the method breakpoint `id`: it
    /// stands at one of the method's returns, or, for a method an unwind
    /// left, at the next place the program reaches. `others` as for
    /// `MethodEntry`.
    MethodExit {
        id: BreakpointId,
        others: Vec<BreakpointId>,
    },
    /// An exception breakpoint took an exception of the class named
    /// `class_name`: one no handler takes, which going on lets
    /// `defaultAction` handle -- `caught` false -- or one a handler takes,
    /// which going on runs -- `caught` true. `description` is its
    /// `messageText` as text, or its class's name when it has none, read
    /// without running the program's code. The stack trace hides the
    /// exception's own machinery (`FrameDescription::hidden`): it shows
    /// first the frame below it, or, for a caught one, the handler about
    /// to run on top and then that frame. `exits` are the method
    /// breakpoints whose method an unwind left since the last stop, whose
    /// exit waited for the next place the program reaches -- this stop's,
    /// made before any other -- and which it reports too.
    Exception {
        class_name: String,
        description: String,
        caught: bool,
        exits: Vec<BreakpointId>,
    },
    /// A `Halt` no handler takes stopped under the `Halt` breakpoint; going
    /// on resumes it with nil, which its `halt` answers. `exits` as for
    /// `Exception`.
    Halt { exits: Vec<BreakpointId> },
}

/// A breakpoint, by the number the VM gave it when it was set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BreakpointId(pub u32);

/// Where a line breakpoint stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BreakpointState {
    pub id: BreakpointId,
    /// The offset it was set at.
    pub offset: u32,
    /// The statement it stops at, in its module's current text; `None`
    /// while it is pending: its module is not built, or no statement
    /// starts at or after its offset.
    pub statement: Option<ns_syntax::Span>,
}

/// Where a method breakpoint stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MethodBreakpointState {
    pub id: BreakpointId,
    /// Whether its method is declared: `false` while its module is not
    /// built, or its class declares no such method.
    pub verified: bool,
}

/// A value a stop handed out, by its index in the stop's table; it names
/// nothing once the program goes on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValueId(pub u32);

/// One activation of a stopped program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameDescription {
    pub kind: FrameKind,
    /// What the frame runs, as a reader names it: `Foo>>bar`,
    /// `Foo class>>bar`, `[] in Foo>>bar`, `Foo initializer`,
    /// `Foo superclass clause`, `Foo class>>new:` for a factory and
    /// `top level`.
    pub name: String,
    pub program_counter: usize,
    pub source: ns_syntax::Span,
    /// Where the frame stands in its module's current text; `None` for a
    /// frame whose body has no metadata -- a module without text, or a
    /// body its current text no longer compiles to (decision 5).
    pub location: Option<FrameLocation>,
    pub receiver: ValueDescription,
    /// Whether a client shows the frame only on demand: at an exception's
    /// stop, every frame above the first one below the exception's own
    /// machinery -- `signal`, `signal:` on either side, `pass` and
    /// `Object>>doesNotUnderstand:` -- and, with step filters, above the
    /// first frame they do not filter; at a caught exception's, the handler
    /// on top, where the program stands, is shown all the same. Hidden
    /// frames stay in the trace, so that a frame's index is the same at
    /// every stop.
    pub hidden: bool,
}

/// Where a frame stands, in the current text of its module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameLocation {
    /// The name of the module's root class; the spans count in its
    /// current text.
    pub module: String,
    /// The innermost statement whose instructions hold the frame's
    /// program counter; for a factory, the class header's factory
    /// pattern (decision 12).
    pub statement: ns_syntax::Span,
    /// The send at the frame's program counter, for a frame below the
    /// top, which waits for that send's answer.
    pub send: Option<SendLocation>,
}

/// The source of one send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendLocation {
    /// `None` where nothing in the source spells the receiver: an
    /// implicit-receiver send and a setter assignment.
    pub receiver: Option<ns_syntax::Span>,
    /// The selector's tokens, one per keyword.
    pub selector: Vec<ns_syntax::Span>,
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
    pub value: VariableValue,
}

/// What a variable holds as the agent shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VariableValue {
    /// Its value.
    Shown(ValueDescription),
    /// Nothing, for a variable that lives nowhere any more: a block's
    /// outer variable whose activation returned without keeping its
    /// registers, and that nothing captured (decision 10).
    Unavailable,
    /// Nothing yet, for a field that holds no value yet: a lazy slot
    /// whose getter has not filled it (§6.3.3), or a nested class's
    /// hidden slot before its accessor built the class. Its word is
    /// `nil`, but its value is not: it does not exist until the program
    /// asks for it, and no Newspeak code runs at a stop to compute it.
    NotComputed,
}

impl VariableValue {
    /// The value shown, if there is one.
    pub fn shown(&self) -> Option<&ValueDescription> {
        match self {
            VariableValue::Shown(value) => Some(value),
            VariableValue::Unavailable | VariableValue::NotComputed => None,
        }
    }
}

/// A value as the agent shows it: its id for `Command::Children`, its
/// label, its class's name, how many children it has and whether they are
/// elements.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueDescription {
    pub id: ValueId,
    pub label: String,
    pub class_name: String,
    pub child_count: usize,
    /// Whether its children are elements, named `[1]` up, rather than
    /// fields: an `Array`'s, by its shape.
    pub indexed: bool,
}

/// The agent's end of an attached debugger (`Interpreter::attach_debugger`),
/// which may move to another thread. Dropping it detaches the debugger: the
/// program runs on, a stopped one included.
///
/// It is served from another thread than the one running the interpreter:
/// a stop blocks the running thread until a `Continue` or a `Terminate`
/// arrives, so a host that pauses and then runs the program on one thread
/// waits in the stop forever. An agent that pumps events on a thread of its
/// own while another sends commands splits it (`DebugRemote::into_parts`).
pub struct DebugRemote {
    control: DebugControl,
    events: DebugEvents,
}

/// The half of a remote that speaks to the VM: it pauses the program and
/// queues commands. It is `Clone`, `Send` and `Sync`, so that several
/// threads of an agent command the VM at once, each through its own handle.
///
/// The VM notices that every handle is gone where it next takes commands:
/// at a safepoint that finds the commands bit, or at a stop, whose wait for
/// a command ends. Dropping the control alone therefore detaches the
/// debugger at the next stop or drain, and dropping `DebugEvents` alone at
/// the next event the VM cannot send.
#[derive(Clone)]
pub struct DebugControl {
    commands: Sender<Command>,
    words: MachineWords,
    /// The interpreter's arming lock: held across the two writes that arm
    /// the poll, so that the VM's clearing of a bit cannot fall between
    /// them (`Arming`).
    arming: Arc<Mutex<Attachments>>,
    /// The number of the attachment that handed this remote out, which
    /// arms only while it is current.
    attachment: u64,
}

/// The half of a remote that listens to the VM: its events, in order. It
/// moves to a thread of its own, so that a stop is heard while another
/// thread waits to send.
pub struct DebugEvents {
    events: Receiver<Event>,
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
            control: DebugControl {
                commands,
                words,
                arming,
                attachment,
            },
            events: DebugEvents { events },
        }
    }

    /// Splits the remote into the half that commands the VM and the half
    /// that hears it.
    pub fn into_parts(self) -> (DebugControl, DebugEvents) {
        (self.control, self.events)
    }

    /// As `DebugControl::pause`.
    pub fn pause(&self) {
        self.control.pause();
    }

    /// As `DebugControl::send`.
    pub fn send(&self, command: Command) {
        self.control.send(command);
    }

    /// As `DebugEvents::next_event`.
    pub fn next_event(&self, timeout: Duration) -> Option<Event> {
        self.events.next_event(timeout)
    }
}

impl DebugControl {
    /// Asks the program to stop at its next safepoint, interpreted or in
    /// machine code, a push or a loop's head; the VM answers
    /// `Event::Stopped`. A pause asked while no run executes stops the
    /// next run's first activation -- the method or block the host sent --
    /// at its first statement start.
    pub fn pause(&self) {
        self.arm(DEBUGGER_PAUSE);
    }

    /// Queues `command` for the VM: the next safepoint serves it if the
    /// program runs, the stop does if it is stopped, and the start of the
    /// next run if no run executes, which drops it unless it is a
    /// configuration command (`Command`) and answers it with a refusal.
    /// Queued before the bit is set, so that the safepoint that finds the
    /// bit finds the command; a remote whose VM is gone queues it nowhere.
    pub fn send(&self, command: Command) {
        if self.commands.send(command).is_ok() {
            self.arm(DEBUGGER_COMMANDS);
        }
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

impl DebugEvents {
    /// The VM's next event, waiting for at most `timeout`; `None` once it
    /// passed, or once the VM is gone.
    pub fn next_event(&self, timeout: Duration) -> Option<Event> {
        self.events.recv_timeout(timeout).ok()
    }

    /// The VM's next event, waiting as long as it takes; `None` once the VM
    /// is gone, which ends an agent's pumping loop.
    pub fn recv(&self) -> Option<Event> {
        self.events.recv().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::HitCondition;

    #[test]
    fn hit_condition_holds() {
        let table = [
            (HitCondition::Equal(2), [false, false, true, false, false]),
            (HitCondition::Equal(0), [true, false, false, false, false]),
            (HitCondition::Greater(2), [false, false, false, true, true]),
            (
                HitCondition::GreaterOrEqual(2),
                [false, false, true, true, true],
            ),
            (HitCondition::Multiple(2), [true, false, true, false, true]),
            (HitCondition::Multiple(1), [true, true, true, true, true]),
            (
                HitCondition::Multiple(0),
                [false, false, false, false, false],
            ),
        ];
        for (condition, expected) in table {
            let held: Vec<bool> = (0..=4).map(|hits| condition.holds(hits)).collect();
            assert_eq!(held, expected, "{condition:?}");
        }
    }
}
