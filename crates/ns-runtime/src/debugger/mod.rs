//! The debugging API's VM side: an attached debugger
//! (`Interpreter::attach_debugger`), served at the run loop's safepoint
//! (`docs/design/debugger.md`, decisions 1, 7 and 9). The agent arms the
//! poll words from its own thread (`DebugRemote`); the safepoint serves
//! the commands it queued while the program runs and, for a pause, stops:
//! it blocks on the command channel, between two stretches, where no
//! handle is live and nothing pushes or pops, until the agent continues or
//! terminates the program. Meanwhile it answers the run's stack
//! (`stack.rs`) -- its activations on the activation stack and the
//! senders suspended into their frame objects -- and the values it hands
//! out, held for the stop (`values.rs`). A line breakpoint patches the
//! slot its statement starts at (`breakpoints.rs`); the run loop's arm
//! there leaves for the safepoint, which stops. A step patches one-shot
//! breakpoints of the same table and, for a step into, holds a request
//! bit the safepoint stops at a push for (`stepping.rs`). An exception
//! breakpoint is decided by the handler search's mirror rows, a leaf and
//! an exit, which record the stop and ask the next poll for it
//! (`exceptions.rs`).

pub(crate) mod breakpoints;
pub(crate) mod conditions;
pub(crate) mod evaluation;
pub(crate) mod exceptions;
pub(crate) mod metadata;
mod remote;
mod stack;
pub(crate) mod stepping;
mod values;

pub use metadata::breakable::BreakableStatement;
pub use remote::{
    BreakpointId, BreakpointState, ClassName, Command, DebugControl, DebugEvents, DebugRemote,
    Evaluation, Event, FrameDescription, FrameKind, FrameLocation, HitCondition, LineBreakpoint,
    LogPart, MethodBreakpoint, MethodBreakpointState, ModuleName, ModuleTextAnswer, ReturnedValue,
    SendLocation, StopReason, ValueDescription, ValueId, Variable, VariableValue,
};

use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex, PoisonError};

use crate::heap::{DEBUGGER_BITS, DEBUGGER_COMMANDS, DEBUGGER_PAUSE, DEBUGGER_STEP, DEBUGGER_STOP};
use crate::interpreter::{Interpreter, Mutator, Signal};
use crate::types::NewspeakError;
use crate::{Symbol, Value};
use breakpoints::Hits;
use exceptions::ExceptionBreakpoints;
use stepping::{HeldStep, Returned, StepAction, StepFilters};
use values::StopValues;

/// The message of the error a run the debugger terminated ends with.
const TERMINATED: &str = "the debugger terminated the program";

/// Why the program cannot serve a command that only a stop serves.
const RUNNING: &str = "the program is running";

/// Why a stop refuses `Variables` or `Evaluate` for a frame index past its
/// stack.
const NO_SUCH_FRAME: &str = "no frame at that index";

/// Why a stop a stress mode's reader serves refuses `Evaluate`: only an
/// attached debugger's stop is kept while an evaluation runs.
const NOT_KEPT: &str = "only a debugger's stop evaluates";

/// Why a stop refuses `Children` for a value it did not hand out.
const NO_SUCH_VALUE: &str = "no value of this stop has that id";

/// Why an expression with a `^` is refused: the evaluation's block has no
/// home among the program's frames that a return could leave for, and a
/// return to the stopped frame's home would unwind the program's own frames
/// (`docs/design/debugger.md`, decision 8).
pub(crate) const RETURN_FROM_AN_EVALUATION: &str = "a ^ cannot leave an evaluation";

/// What follows a variable's name in the refusal of an assignment to a
/// parameter, an immutable local or a marked label:
/// `"<name> is not assignable"`.
pub(crate) const NOT_ASSIGNABLE: &str = "is not assignable";

/// What follows a variable's name in the refusal of an assignment a block
/// may already hold a copy of the variable before:
/// `"<name> is captured by a block made before this point"`
/// (`docs/design/debugger.md`, "Assigning a captured variable").
pub(crate) const CAPTURED: &str = "is captured by a block made before this point";

/// The VM's end of an attached debugger, held by its `Interpreter`.
pub(crate) struct Debugger {
    commands: Receiver<Command>,
    events: Sender<Event>,
    /// The id the next stop's first value takes, past every id the stops
    /// before handed out (`values::StopValues`).
    next_value_id: u32,
    /// The stop an evaluation left to run, kept until the safepoint the
    /// evaluation's end asks for re-enters it (`Served::Evaluate`). Dropped
    /// with the debugger: an evaluation whose debugger is gone ends
    /// unanswered, and the program goes on.
    stop: Option<Stop>,
    /// The reach of a breakpoint's slot whose condition or log expression
    /// an evaluation runs for, kept until the safepoint the evaluation's
    /// end asks for goes on deciding it (`Interpreter::decide`). Dropped
    /// with the debugger, as the stop is.
    deciding: Option<conditions::Deciding>,
}

/// A stop, as the commands it serves read it: what `stop_standing` decided
/// of where it stands, the values it handed out, the frames it walked, the
/// agent it serves and what it owes the program once it goes on. Kept by
/// the `Debugger` while an evaluation it began runs, and served again when
/// the evaluation ends, so that the stop and its value ids outlast every
/// evaluation made at it.
pub(crate) struct Stop {
    /// Whether the stop is an exception's, or a `Halt`'s, which hides the
    /// exception's machinery.
    exceptional: bool,
    /// Whether it is a caught exception's, standing in its handler.
    caught: bool,
    /// See `Standing::at_header`.
    at_header: bool,
    /// The run's base (`stack.rs`).
    floor: usize,
    source: CommandSource,
    values: StopValues,
    /// The run's frames, walked at the first command that reads them --
    /// nothing pushes or pops while the program is stopped -- and walked
    /// again after an evaluation, which may have moved them (the suspend
    /// stress mode suspends every activation at a safepoint).
    frames: Option<Vec<stack::StackFrame>>,
    /// What the stop owes once the program goes on.
    after: AfterStop,
    /// Whether an evaluation ran at the stop.
    evaluated: bool,
    /// The name of the variable the running evaluation sets
    /// (`Command::SetVariable`), whose value it answers as the variable.
    setting: Option<String>,
}

impl Stop {
    /// The run's frames, walked now if no command read them yet.
    pub(crate) fn frames(&mut self, mutator: &Mutator<'_>) -> &[stack::StackFrame] {
        self.frames
            .get_or_insert_with(|| stack::frames(mutator, self.floor))
    }
}

/// What a stop owes the program once it goes on.
#[derive(Clone, Copy, Default)]
struct AfterStop {
    /// The slot the stopped frame stands at, which it fetches again as it
    /// goes on and goes past once (`Breakpoints::go_past_once`).
    past: Option<breakpoints::PastOnce>,
    /// Whether the run loop reads the frame on top again: a step stopped
    /// in a factory by undoing its initializer's push.
    rebuilt: bool,
}

/// The lock under which the poll is armed and the debugger's bits are
/// cleared, one per `Interpreter`, shared by every remote it ever handed
/// out. Held by the agent across its two writes that arm the poll -- the
/// request bit's `fetch_or` and the store of 0 into the depth limit --
/// and by the VM across its clearing of a debugger bit, which
/// resynchronizes the limit. Without it the VM could serve and clear the
/// bit between the agent's two writes, and the agent's late 0 would leave
/// the limit at 0 with no bit set: every compiled push refused and
/// counted as an exit until the next resynchronization. One lock for
/// every attachment, since a remote of an earlier one holding a lock of
/// its own would reopen that race; what it guards says which remote may
/// arm at all, so that one whose debugger is gone, detached or replaced,
/// arms nothing. Only the cold paths take it, never a poll. V8 arms its
/// interrupts the same way, under a lock (`StackGuard::RequestInterrupt`
/// under `ExecutionAccess`, `v8/src/execution/stack-guard.cc:169-183`).
#[derive(Default)]
pub(crate) struct Arming {
    attachments: Arc<Mutex<Attachments>>,
}

/// What the arming lock guards: which attachment is current.
#[derive(Default)]
pub(crate) struct Attachments {
    /// The number of the attached debugger, `None` while none is.
    current: Option<u64>,
    /// The number the last attachment took.
    last: u64,
}

impl Attachments {
    /// Whether the remote of attachment `attachment` may arm the poll:
    /// only that of the debugger attached now.
    pub(crate) fn is_current(&self, attachment: u64) -> bool {
        self.current == Some(attachment)
    }
}

/// Where a stop takes its commands from and sends its answers to: the
/// attached debugger's channels, or, in a debug build, the pause stress
/// mode's reader or the step stress mode's, which need no debugger.
pub(crate) enum CommandSource {
    /// The attached debugger's remote.
    Remote,
    /// The pause stress mode's reader.
    #[cfg(debug_assertions)]
    Reader(StressReader),
    /// The step stress mode's reader.
    #[cfg(debug_assertions)]
    Stepper(StepReader),
}

/// The pause stress mode's agent (`Interpreter::stop_for_stress`): it asks
/// for the stack, the variables of every frame and the first children of
/// every value those answered, then continues, so that every program the
/// mode runs proves that reading a stopped stack -- every frame, every
/// register the leftover analysis calls live, every value one level deep
/// -- changes nothing and reads no dead register, which a debug build
/// refuses (`docs/design/debugger.md`, "The pause stress mode").
#[cfg(debug_assertions)]
pub(crate) struct StressReader {
    /// The stress mode the reader reads for, which a refusal names.
    mode: &'static str,
    /// Whether the stack trace was asked for yet.
    started: bool,
    /// The commands the answers so far call for, in order.
    pending: std::collections::VecDeque<Command>,
    /// Whether the command last issued asks for a value's children, whose
    /// answer is one level deep and calls for nothing more.
    reading_children: bool,
}

#[cfg(debug_assertions)]
impl StressReader {
    /// How many children of each value the reader asks for.
    const CHILDREN: usize = 8;

    /// A reader for the stress mode `mode`, which has asked for nothing
    /// yet.
    fn for_mode(mode: &'static str) -> StressReader {
        StressReader {
            mode,
            started: false,
            pending: std::collections::VecDeque::new(),
            reading_children: false,
        }
    }

    /// The reader's next command: the stack trace first, then what the
    /// answers called for, then `Continue`.
    fn next_command(&mut self) -> Command {
        let command = if self.started {
            self.pending.pop_front().unwrap_or(Command::Continue)
        } else {
            self.started = true;
            Command::StackTrace
        };
        self.reading_children = matches!(command, Command::Children { .. });
        command
    }

    /// Takes the stop's answer to the command last issued.
    fn take(&mut self, event: Event) {
        match event {
            Event::StackTrace(frames) => {
                self.pending
                    .extend((0..frames.len()).map(|frame| Command::Variables { frame }));
            }
            Event::Variables(variables) => {
                if !self.reading_children {
                    self.pending
                        // A variable that is not available has no value
                        // to ask children of.
                        .extend(variables.iter().filter_map(|variable| {
                            Some(Command::Children {
                                value: variable.value.shown()?.id,
                                start: 0,
                                count: StressReader::CHILDREN,
                            })
                        }));
                }
            }
            // The reader asks only for frames and values the stop handed
            // out: a refusal is the stop misreading its own stack.
            Event::Refused { command, reason } => {
                panic!(
                    "internal: the {} stress mode's {command} was refused: {reason}",
                    self.mode
                )
            }
            Event::Stopped { .. }
            | Event::Evaluated(_)
            | Event::VariableSet(_)
            | Event::Continued
            | Event::Breakpoints { .. }
            | Event::MethodBreakpoints(_)
            | Event::StepFiltersSet
            | Event::ExceptionBreakpointsSet
            | Event::ModuleText { .. }
            | Event::Logged { .. } => {}
        }
    }
}

/// The step stress mode's agent (`Interpreter::step_for_stress` and the
/// stops its steps make): it sets the step filters the burst's
/// rotation calls for, reads the stack as the pause stress mode's reader
/// does, and goes on with the step, or `Continue` once the burst is over
/// (`docs/design/debugger.md`, "The step stress mode"). The stop refuses
/// no step command -- a step with no frame to step in goes on as a
/// `Continue` does (`stepping::begin`) -- so every refusal is the stop
/// misreading its own stack, and panics.
#[cfg(debug_assertions)]
pub(crate) struct StepReader {
    reader: StressReader,
    /// The shipped filter to set first, if the rotation switches it here.
    shipped_filter: Option<bool>,
    /// What the stop goes on with once the stack is read.
    then: Command,
}

#[cfg(debug_assertions)]
impl StepReader {
    fn next_command(&mut self) -> Command {
        if let Some(shipped) = self.shipped_filter.take() {
            return Command::SetStepFilters {
                shipped,
                classes: Vec::new(),
            };
        }
        match self.reader.next_command() {
            Command::Continue => std::mem::replace(&mut self.then, Command::Continue),
            command => command,
        }
    }

    fn take(&mut self, event: Event) {
        self.reader.take(event);
    }
}

/// How many steps a burst of the step stress mode takes from each stop
/// its schedule picks. Stepping every instruction of a program to its end
/// would make the `stress` gate's run far too slow; a burst steps through
/// a callee's first statements, out of it and over its sends, with and
/// without the shipped filter, each action meeting both filters within a
/// burst, while the schedule spreads the bursts over the whole run.
#[cfg(debug_assertions)]
pub(crate) const STEP_STRESS_BURST: usize = 16;

/// How many times the step stress mode's schedule may fall due while a
/// step is in progress before the mode ends the step itself, and how many
/// safepoints the step must have run through first. A step that
/// legitimately outlasts both is a step out of a long loop -- a shipped
/// `to:do:` stopped in with the shipped filter off -- or of the run's
/// first frame, which returns to the host; four periods let most steps
/// over a call stop, while a long one costs the mode few bursts. The
/// floor of safepoints keeps the grace from shrinking with the heap: the
/// schedule's period is paced by the traced size, which the collect and
/// scavenge stress modes keep small by reclaiming at their safepoints, so
/// that four dues alone came in under half the safepoints there, and a
/// step was ended about one time in three rather than one in five.
#[cfg(debug_assertions)]
const STEP_STRESS_GRACE: usize = 4;

/// The safepoints a step runs through before the step stress mode may end
/// it (`STEP_STRESS_GRACE`).
#[cfg(debug_assertions)]
const STEP_STRESS_GRACE_SAFEPOINTS: usize = 64;

/// The step stress mode's state (`NS_NEXT_STRESS_STEP`, or
/// `Interpreter::set_stress_step`): its schedule, the burst under way and
/// the rotation of its steps.
#[cfg(debug_assertions)]
#[derive(Default)]
pub(crate) struct StepStress {
    /// Picks the safepoints a burst begins at, paced by the traced size
    /// and the stack as the pause stress mode is, the stack counted once
    /// more for each step of a burst rather than the pause's eight times
    /// (`Interpreter::step_for_stress`).
    schedule: crate::interpreter::safepoint::StressSchedule,
    /// The steps the burst under way has still to take.
    left: usize,
    /// The steps begun since the mode was switched on, which picks the
    /// next one's action and filter.
    begun: usize,
    /// The steps that stopped since the mode was switched on.
    pub(crate) stopped: usize,
    /// The steps the mode ended itself since it was switched on, still in
    /// progress when the schedule fell due `STEP_STRESS_GRACE` times more.
    pub(crate) ended: usize,
    /// How many times the schedule fell due while the step in progress
    /// ran; at `STEP_STRESS_GRACE` the mode ends it.
    overdue: usize,
    /// The safepoints the step in progress has run through.
    safepoints_in_step: usize,
}

#[cfg(debug_assertions)]
impl StepStress {
    /// Whether `NS_NEXT_STRESS_STEP` asks for the mode: set, and neither
    /// empty nor `0`.
    pub(crate) fn from_env() -> Option<StepStress> {
        let requested = std::env::var_os("NS_NEXT_STRESS_STEP")
            .is_some_and(|value| !value.is_empty() && value != "0");
        requested.then(StepStress::default)
    }

    /// The reader of the mode's next stop: a step's, when `stepped`, or
    /// the stop that begins a burst. Steps into, over and out in turn, the
    /// shipped filter switched at every third, so that each action steps
    /// with the filter on and off.
    fn reader(&mut self, stepped: bool) -> StepReader {
        self.overdue = 0;
        self.safepoints_in_step = 0;
        if stepped {
            self.stopped += 1;
        } else {
            self.left = STEP_STRESS_BURST;
        }
        let reader = StressReader::for_mode("step");
        if self.left == 0 {
            return StepReader {
                reader,
                shipped_filter: None,
                then: Command::Continue,
            };
        }
        self.left -= 1;
        let (then, shipped_filter) = match self.begun % 3 {
            0 => (Command::StepInto, Some(self.begun.is_multiple_of(6))),
            1 => (Command::StepOver, None),
            _ => (Command::StepOut, None),
        };
        self.begun += 1;
        StepReader {
            reader,
            shipped_filter,
            then,
        }
    }
}

/// What deciding a reach's conditions makes of its stop beyond its
/// breakpoints (`Interpreter::serve_breakpoint`).
enum Decided {
    /// The conditions or log expressions of `failed` failed, as `error`
    /// says first: the stop is their failure.
    Failed {
        failed: Vec<BreakpointId>,
        error: String,
    },
    /// A pause aborted a condition: the stop is the breakpoints' that
    /// held, or else the pause's.
    Aborted,
}

/// Where a stop stands, beyond its frames.
#[derive(Clone, Copy)]
struct Standing {
    /// Whether a caught exception's stop stands in a frame its handler
    /// pushed (`exceptions::serve`): one that does not stands in the
    /// kernel's machinery, which it hides, and steps from the frame below
    /// it, as an uncaught exception's stop does.
    in_handler: bool,
    /// Whether the stop is an initializer's header stop, at its entry
    /// before its first statement (`stepping::HeldStep::StopAtHeader`):
    /// its frame is placed at its declaration's header, and a step from
    /// it goes on to its first statement (`stepping::begin_at_header`).
    at_header: bool,
    /// Whether the step the stop ends went on from another frame than the
    /// one it was taken in (`stepping::went_on_elsewhere`), which the
    /// stop's event reports.
    elsewhere: bool,
}

impl Standing {
    /// A stop in the frame on top, at no header, for no step that went on
    /// elsewhere.
    const ON_TOP: Standing = Standing {
        in_handler: true,
        at_header: false,
        elsewhere: false,
    };
}

/// What serving the debugger at a safepoint decided.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Served {
    /// The program goes on, through the rest of the safepoint.
    GoOn,
    /// The agent terminated the run.
    Terminate,
    /// The program goes on, through the rest of the safepoint, in another
    /// frame than the one the run loop left, which it reads again: a step
    /// stopped in a factory by undoing its initializer's push
    /// (`stepping::HeldStep::StopInFactory`).
    Rebuilt,
    /// The stop pushed an evaluation and is kept (`Debugger::stop`): the
    /// run loop reads the frame on top again, as for `Rebuilt`, and runs
    /// the evaluation, whose end asks for the stop that serves the kept
    /// one again.
    Evaluate,
    /// The evaluation the kept stop waits for is aborted: the run loop
    /// unwinds from the top activation with the termination's signal, as
    /// for `Terminate`, and the evaluation's base ends the unwind
    /// (`evaluation::abort`), whose end asks for the stop that serves the
    /// kept one again -- or, the remote gone, lets the program run on.
    Abort,
}

impl Interpreter {
    /// Attaches a debugger and answers its remote, which may move to
    /// another thread; a debugger attached before replaces it, its queued
    /// commands dropped with its channels and its pause and commands bits
    /// cleared, so that the new remote meets no stop it never asked for.
    /// A pause the remote asks while no run executes stops the next run's
    /// first activation at its first statement start.
    pub fn attach_debugger(&mut self) -> DebugRemote {
        let (command_sender, command_receiver) = mpsc::channel();
        let (event_sender, event_receiver) = mpsc::channel();
        let words = self.enter(|mutator| mutator.heap.machine_words_handle());
        let attachments = Arc::clone(&self.debugger_arming.attachments);
        // The earlier attachment's bits are cleared under the hold that
        // makes the new one current, so that no remote of the earlier one
        // arms in between and leaves a bit the new one inherits.
        let mut guarded = attachments.lock().unwrap_or_else(PoisonError::into_inner);
        guarded.last += 1;
        guarded.current = Some(guarded.last);
        let attachment = guarded.last;
        self.enter(|mutator| mutator.heap.clear_debugger_bits(DEBUGGER_BITS));
        drop(guarded);
        // The earlier attachment's breakpoints and step go with it, as its
        // pause does.
        let reports = event_sender.clone();
        let arming = Arc::clone(&attachments);
        self.enter(|mutator| {
            breakpoints::clear(mutator);
            mutator.breakpoints.report_to(Some(reports));
            mutator.breakpoints.arm_under(Some(arming));
        });
        self.debugger = Some(Debugger {
            commands: command_receiver,
            events: event_sender,
            next_value_id: 0,
            stop: None,
            deciding: None,
        });
        DebugRemote::new(
            command_sender,
            event_receiver,
            words,
            attachments,
            attachment,
        )
    }

    /// Serves the debugger's bits at the run loop's safepoint, before
    /// anything else there: first the commands queued while the program
    /// ran, then the stop an exception breakpoint asked for, then the stop
    /// a `Breakpoint` arm left for, then a pause; each
    /// stop waits until the agent continues or terminates. A stop reports
    /// every breakpoint it is for, and a method's exit an unwind made
    /// since the step that reports it began (`stepping::method_left`),
    /// whatever made the stop (`Hits::reason`). A remote that is gone
    /// detaches the debugger, and the program runs on.
    pub(crate) fn serve_debugger(&mut self, floor: usize) -> Served {
        if let Some(served) = self.serve_evaluation(floor) {
            return served;
        }
        let (bits, mut breakpoint, exception) = self.enter(|mutator| {
            (
                mutator.heap.debugger_bits(),
                mutator.breakpoints.take_pending_stop(),
                mutator.breakpoints.take_pending_exception(),
            )
        });
        if bits == 0 && breakpoint.is_none() {
            return Served::GoOn;
        }
        if bits & DEBUGGER_STOP != 0 {
            self.clear_debugger_bits(DEBUGGER_STOP);
        }
        if self.debugger.is_none() {
            // A remote that set a bit and was dropped before this
            // safepoint, whose debugger a former one detached: nobody is
            // left to stop for -- but for the step stress mode's steps,
            // whose held request stays.
            let stale = if self.has_agent() {
                bits & !DEBUGGER_STEP
            } else {
                bits
            };
            if stale != 0 {
                self.clear_debugger_bits(stale);
            }
            if !self.has_agent() {
                return Served::GoOn;
            }
        }
        if bits & DEBUGGER_COMMANDS != 0 {
            if self.serve_queued_commands() == Served::Terminate {
                return self.terminate();
            }
            // A `SetBreakpoints` just served may have removed a breakpoint
            // the arm's stop is for, whose id the client no longer knows.
            breakpoint = breakpoint.and_then(|stop| {
                self.enter(|mutator| stop.still_standing(&mut mutator.breakpoints))
            });
        }
        let exception = match (exception, self.debugger.is_some()) {
            (Some(pending), true) => self.enter(|mutator| exceptions::serve(mutator, pending)),
            _ => None,
        };
        if let Some((mut reason, in_handler)) = exception {
            // The exception's stop is made instead of the arm's, whose
            // frame fetches its slot again as it goes on: that fetch
            // counts none of the reach's breakpoints once more.
            if let Some(stop) = breakpoint {
                self.enter(|mutator| stop.not_made(&mut mutator.breakpoints));
            }
            // Where the frame on top stands is debugger.md's "Exceptions":
            // the first instruction of what the search runs next, unless a
            // slot's setter or a fast path answered it without a push. The stop serves a
            // pause asked meanwhile too, and ends a step, whose handler
            // report may have led it here -- and the step that waited to
            // report a method's exit by an unwind, which this stop, the
            // next place the program reached, reports beside the
            // exception, so that the exit is not lost.
            self.clear_debugger_bits(DEBUGGER_PAUSE);
            let unwound = self.enter(|mutator| {
                let unwound = stepping::take_exits(mutator);
                stepping::end(mutator);
                unwound
            });
            if let StopReason::Exception { exits, .. } | StopReason::Halt { exits } = &mut reason {
                exits.extend(unwound);
            }
            let standing = Standing {
                in_handler,
                ..Standing::ON_TOP
            };
            return self.stop_standing(reason, None, floor, CommandSource::Remote, standing);
        }
        if let Some(mut breakpoint) = breakpoint {
            if !breakpoint.conditions.is_empty() {
                if self.debugger.is_some() {
                    let deciding = conditions::Deciding::new(breakpoint, floor);
                    return self.decide(deciding);
                }
                // The debugger whose breakpoints they are is gone, and its
                // breakpoints with it.
                breakpoint.conditions.clear();
            }
            if let Some(served) = self.serve_breakpoint(breakpoint, floor, None, None) {
                return served;
            }
        }
        let bits = self.enter(|mutator| mutator.heap.debugger_bits());
        if bits & DEBUGGER_PAUSE != 0 && self.debugger.is_some() {
            self.clear_debugger_bits(DEBUGGER_PAUSE);
            // A method's exit an unwind made before the pause is reported
            // by the stop that serves it.
            let unwound = self.enter(|mutator| {
                let unwound = stepping::take_exits(mutator);
                stepping::end(mutator);
                unwound
            });
            let reason = Hits::default().reason(unwound).unwrap_or(StopReason::Pause);
            return self.stop(reason, None, floor, CommandSource::Remote);
        }
        let held = if bits & DEBUGGER_STEP != 0 {
            self.enter(stepping::held_step)
        } else {
            HeldStep::GoOn
        };
        if let HeldStep::StopAtHeader = held {
            // The initializer stands before its first statement: a
            // breakpoint at the slot it fetches next is the next stop's,
            // which the step from the header, or a `Continue`, reaches
            // there, so the header stop lets nothing past.
            let (unwound, elsewhere) = self.enter(|mutator| {
                let unwound = stepping::take_exits(mutator);
                let elsewhere = stepping::went_on_elsewhere(mutator);
                stepping::end(mutator);
                (unwound, elsewhere)
            });
            let reason = Hits::default().reason(unwound);
            let source = self.step_agent();
            let standing = Standing {
                at_header: true,
                elsewhere: reason.is_none() && elsewhere,
                ..Standing::ON_TOP
            };
            let reason = reason.unwrap_or(StopReason::Step);
            return self.stop_standing(reason, None, floor, source, standing);
        }
        if let HeldStep::Stop | HeldStep::StopInFactory = held {
            // The pushed frame stands at its first statement start, which
            // it fetches as it goes on: a breakpoint there makes the stop
            // the breakpoint's -- a method breakpoint's entry -- and lets
            // it past once, a condition or a log standing there decided in
            // the frame first. A factory stands at its
            // `InitializeInstance`.
            let rebuilt = matches!(held, HeldStep::StopInFactory);
            let mut reached = self.enter(|mutator| {
                let top = mutator.heap.top_activation()?;
                let (hits, after, passed, conditions) =
                    crate::interpreter::breakpoint::standing_hits(mutator, top);
                let code = mutator.heap.activation_code(top);
                let slot = breakpoints::PastOnce {
                    activation: top,
                    body: breakpoints::address(&code),
                    index: mutator.heap.activation_program_counter(top),
                    after,
                };
                Some(breakpoints::PendingStop {
                    slot,
                    hits,
                    step_stops: true,
                    passed,
                    conditions,
                })
            });
            let decides = reached
                .as_ref()
                .is_some_and(|reached| !reached.conditions.is_empty());
            if decides
                && self.debugger.is_some()
                && let Some(reached) = reached.take()
            {
                return self.decide(conditions::Deciding::at_push(reached, floor, rebuilt));
            }
            let (reason, standing, elsewhere) = match reached {
                Some(reached) => self.enter(|mutator| {
                    let unwound = stepping::take_exits(mutator);
                    let elsewhere = stepping::went_on_elsewhere(mutator);
                    stepping::end(mutator);
                    (reached.hits.reason(unwound), Some(reached.slot), elsewhere)
                }),
                None => self.enter(|mutator| {
                    let unwound = stepping::take_exits(mutator);
                    let elsewhere = stepping::went_on_elsewhere(mutator);
                    stepping::end(mutator);
                    (Hits::default().reason(unwound), None, elsewhere)
                }),
            };
            let stood = Standing {
                elsewhere: reason.is_none() && elsewhere,
                ..Standing::ON_TOP
            };
            let reason = reason.unwrap_or(StopReason::Step);
            let source = self.step_agent();
            let after = AfterStop {
                past: standing,
                rebuilt,
            };
            return self.stop_owing(reason, None, floor, source, stood, after);
        }
        Served::GoOn
    }

    /// Serves the safepoints of an evaluation a stop began, which the stop
    /// is kept for (`Debugger::stop`): once the evaluation ended -- its
    /// end asked for this safepoint -- the kept stop answers its result
    /// and serves the agent again, without a second `Event::Stopped`, a
    /// pause asked meanwhile moot, since the program is stopped; while it
    /// runs, a safepoint serves the commands queued meanwhile -- a
    /// terminate ends the run -- and a pause aborts it, as does the
    /// remote's departure, which detaches the debugger and lets the
    /// program run on (`Served::Abort`). `None` when no stop is kept: the
    /// answer of an evaluation whose debugger is gone waits for nobody,
    /// and goes with the run (`evaluation::run_ended`).
    fn serve_evaluation(&mut self, floor: usize) -> Option<Served> {
        let (kept, deciding) = self.debugger.as_ref().map_or((false, false), |debugger| {
            (debugger.stop.is_some(), debugger.deciding.is_some())
        });
        // An evaluation at a method's exit by an unwind is served as one a
        // stop is kept for while it runs; it ends without asking for the
        // safepoint, its unwind going on.
        let exiting =
            self.debugger.is_some() && self.enter(|mutator| conditions::exit_evaluating(mutator));
        if !kept && !deciding && !exiting {
            return None;
        }
        if !self.enter(|mutator| evaluation::has_ended(mutator)) {
            let bits = self.enter(|mutator| mutator.heap.debugger_bits());
            if bits & DEBUGGER_COMMANDS != 0 && self.serve_queued_commands() == Served::Terminate {
                return Some(self.terminate());
            }
            let detached = self.debugger.is_none();
            if detached || bits & DEBUGGER_PAUSE != 0 {
                self.clear_debugger_bits(DEBUGGER_PAUSE);
                return Some(self.abort_evaluation());
            }
            return Some(Served::GoOn);
        }
        if deciding {
            // The program is not stopped: a pause asked meanwhile stops it
            // at a later safepoint, once the decision went on.
            self.clear_debugger_bits(DEBUGGER_STOP);
            let mut deciding = self
                .debugger
                .as_mut()
                .and_then(|debugger| debugger.deciding.take())
                .expect("internal: the decision is kept");
            debug_assert_eq!(
                deciding.floor, floor,
                "internal: a decision outlived its run"
            );
            self.enter(|mutator| {
                let outcome = evaluation::take_outcome(mutator)
                    .expect("internal: an evaluation that ended holds its answer");
                deciding.evaluated(mutator, outcome);
            });
            return Some(self.decide(deciding));
        }
        self.clear_debugger_bits(DEBUGGER_STOP | DEBUGGER_PAUSE);
        // A stop is kept only while its evaluation runs or its answer
        // waits, and only the debugger that keeps it takes the answer.
        let mut stop = self
            .debugger
            .as_mut()
            .and_then(|debugger| debugger.stop.take())
            .expect("internal: the stop is kept");
        let setting = stop.setting.take();
        let answer = self.enter(|mutator| {
            let outcome = evaluation::take_outcome(mutator)
                .expect("internal: an evaluation that ended holds its answer");
            let answer = match outcome {
                evaluation::Outcome::Value(value) => {
                    Evaluation::Value(stop.values.describe(mutator, mutator.heap.fetch(&value)))
                }
                evaluation::Outcome::Error {
                    description,
                    exception,
                } => {
                    let exception = exception
                        .map_or_else(Value::nil, |exception| mutator.heap.fetch(&exception));
                    Evaluation::Error {
                        description,
                        exception: stop.values.describe(mutator, exception),
                    }
                }
                evaluation::Outcome::Aborted => Evaluation::Aborted,
            };
            match (setting, answer) {
                // The assignment answers the value it wrote, which the
                // variable now holds and `Variables` shows.
                (Some(name), Evaluation::Value(value)) => Event::VariableSet(Variable {
                    name,
                    value: VariableValue::Shown(value),
                }),
                (_, answer) => Event::Evaluated(answer),
            }
        });
        self.answer(&mut stop.source, answer);
        Some(self.serve_stop(stop))
    }

    /// Goes on deciding the conditions of a breakpoint's slot
    /// (`conditions::Deciding`): the next condition or log expression's
    /// evaluation begun, the decision kept for the safepoint its end asks
    /// for (`Served::Evaluate`) -- a frame of optimized code its poll left
    /// going on in baseline code first, as before a stop -- or, all of
    /// them decided, the stop made or the slot gone past
    /// (`decided`).
    fn decide(&mut self, mut deciding: conditions::Deciding) -> Served {
        let begun = self.enter(|mutator| {
            crate::jit::deoptimize_stopped_at_poll(mutator);
            deciding.advance(mutator)
        });
        if begun {
            if let Some(debugger) = &mut self.debugger {
                debugger.deciding = Some(deciding);
            }
            return Served::Evaluate;
        }
        let floor = deciding.floor;
        self.decided(deciding.decided(), floor)
    }

    /// Makes the stop `decision` says, or lets the frame past its slot: a
    /// reach where no breakpoint holds, no condition failed, no pause
    /// aborted one and no step stops goes past it as a resumed stop's
    /// token lets it, so that its conditions are not evaluated again; a
    /// failure stops for every breakpoint that failed
    /// (`StopReason::ConditionFailed`), a pause that aborted a condition
    /// where nothing else stops as a pause, and otherwise the stop is the
    /// breakpoints', as the arm's would be (`serve_breakpoint`). The commands served while the conditions
    /// ran may have removed the breakpoints it would be for: then it is
    /// not made, and the frame's fetch of the slot again decides nothing
    /// this reach decided (`PendingStop::decided_standing`).
    fn decided(&mut self, decision: conditions::Decision, floor: usize) -> Served {
        let conditions::Decision {
            stop,
            failed,
            error,
            aborted,
            pushed,
        } = decision;
        if stop.hits.only_a_step() && !stop.step_stops && failed.is_empty() && !aborted {
            self.enter(|mutator| mutator.breakpoints.go_past_once(stop.slot));
            return Served::GoOn;
        }
        let Some((stop, failed)) =
            self.enter(|mutator| stop.decided_standing(&mut mutator.breakpoints, failed, aborted))
        else {
            return Served::GoOn;
        };
        let decided = if !failed.is_empty() {
            Some(Decided::Failed {
                failed,
                error: error.unwrap_or_default(),
            })
        } else if aborted {
            Some(Decided::Aborted)
        } else {
            None
        };
        self.serve_breakpoint(stop, floor, decided, pushed)
            .unwrap_or(Served::GoOn)
    }

    /// Makes the stop a `Breakpoint` arm decided, its conditions decided
    /// if it had any, for `reason` if given and else for what made it --
    /// answering how the program goes on -- but for one only a step's
    /// one-shot made in a frame the step filters filter, which is not
    /// made: the step goes on from the first unfiltered frame below, and
    /// the frame fetches the slot it stands at again, past it once. `None`
    /// when no stop is made.
    fn serve_breakpoint(
        &mut self,
        breakpoint: breakpoints::PendingStop,
        floor: usize,
        decided: Option<Decided>,
        pushed: Option<bool>,
    ) -> Option<Served> {
        let filtered = self.has_agent()
            && decided.is_none()
            && pushed.is_none()
            && breakpoint.hits.only_a_step()
            && self.enter(|mutator| {
                stepping::went_on_past_filtered(mutator, breakpoint.slot.activation)
            });
        if filtered {
            self.enter(|mutator| mutator.breakpoints.go_past_once(breakpoint.slot));
            return None;
        }
        if !self.has_agent() {
            return None;
        }
        // The stop serves a pause asked meanwhile too, and ends a step,
        // whatever made it.
        self.clear_debugger_bits(DEBUGGER_PAUSE);
        let (reason, returned, elsewhere) = self.enter(|mutator| {
            let unwound = stepping::take_exits(mutator);
            let at_entry = stepping::is_at_entry(mutator);
            let elsewhere = stepping::went_on_elsewhere(mutator);
            let returned = stepping::end(mutator);
            // A slot only a step's one-shot or the run to cursor stopped
            // at is the step's stop, which reports the value of the last
            // call -- or the pause's, for the one-shot a pause pending at
            // a run's start set; a breakpoint the debugger set there makes
            // it the breakpoint's.
            // A step into's stop at a push reports no value of a call.
            let returned = returned.filter(|_| pushed.is_none());
            let reason = breakpoint.hits.reason(unwound);
            match (decided, reason) {
                // Every breakpoint the stop is for and every exit it
                // reports is named after the failed ones, so that a
                // temporary breakpoint hit there is removed all the same.
                (Some(Decided::Failed { mut failed, error }), reason) => {
                    for id in reason.map(conditions::named_by).unwrap_or_default() {
                        if !failed.contains(&id) {
                            failed.push(id);
                        }
                    }
                    let ids = failed;
                    (StopReason::ConditionFailed { ids, error }, None, false)
                }
                (Some(Decided::Aborted), reason) => {
                    (reason.unwrap_or(StopReason::Pause), None, false)
                }
                (None, Some(reason)) => (reason, None, false),
                (None, None) if at_entry && pushed.is_none() => (StopReason::Pause, None, false),
                (None, None) => (StopReason::Step, returned, elsewhere),
            }
        });
        let source = self.step_agent();
        let standing = Standing {
            elsewhere,
            ..Standing::ON_TOP
        };
        // The frame fetches the patched slot again as it goes on: the
        // breakpoint it stopped at lets it past once.
        let after = AfterStop {
            past: Some(breakpoint.slot),
            rebuilt: pushed.unwrap_or(false),
        };
        Some(self.stop_owing(reason, returned, floor, source, standing, after))
    }

    /// Aborts the running evaluation (`evaluation::abort`): the run loop
    /// unwinds with the termination's signal from the top activation, a
    /// frame of optimized code its poll left going on in baseline code
    /// first, as for a termination (`terminate`).
    fn abort_evaluation(&mut self) -> Served {
        self.enter(|mutator| {
            evaluation::abort(mutator);
            crate::jit::deoptimize_stopped_at_poll(mutator);
        });
        Served::Abort
    }

    /// Stops the program as a pause does, for the pause stress mode, with
    /// its reader as the agent; the program goes on.
    #[cfg(debug_assertions)]
    pub(crate) fn stop_for_stress(&mut self, floor: usize) {
        let served = self.stop(
            StopReason::Pause,
            None,
            floor,
            CommandSource::Reader(StressReader::for_mode("pause")),
        );
        debug_assert!(
            served == Served::GoOn,
            "internal: the pause stress mode's reader never terminates"
        );
    }

    /// The step stress mode at a safepoint: when its schedule says a
    /// burst is due, the program stops as a pause does, with the mode's
    /// reader as the agent, which begins the burst's first step. The
    /// schedule is fed the traced size with the stack counted once more
    /// for each step of a burst, since every stop of a burst reads the
    /// whole stack (`safepoint::size_with_stack`): a deep stack's bursts
    /// come as much rarer as they cost more, and a shallow program's are
    /// paced about as the pause stress mode's stops. A step still in progress -- a step out of the run's first
    /// frame, which returns to the host, or a step over a long call -- is
    /// given `STEP_STRESS_GRACE` periods of the schedule more, and at least
    /// `STEP_STRESS_GRACE_SAFEPOINTS` safepoints, then ended
    /// as a debugger's pause ends it, so that no step keeps the mode from its
    /// next burst; its frame must still be there then, or it left without
    /// the step learning of it, which a debug build refuses. The mode
    /// stands aside while a debugger is attached, so that it never steps
    /// in the debugger's place.
    #[cfg(debug_assertions)]
    pub(crate) fn step_for_stress(&mut self, floor: usize, traced_size: usize) {
        if self.debugger.is_some() {
            return;
        }
        let (stepping, about_to_stop, stack_words) = self.enter(|mutator| {
            (
                mutator.stepping.is_some(),
                stepping::at_return_point(mutator),
                mutator.heap.activation_stack_top(),
            )
        });
        let Some(stress) = &mut self.stress_step else {
            return;
        };
        if stepping {
            stress.safepoints_in_step += 1;
        }
        let paced_size = crate::interpreter::safepoint::size_with_stack(
            traced_size,
            stack_words,
            STEP_STRESS_BURST,
        );
        if !stress.schedule.due(paced_size) {
            return;
        }
        // A step whose frame has just returned stops at its caller's next
        // fetch: it is left to make that stop, its grace still running.
        let in_grace = stress.overdue < STEP_STRESS_GRACE
            || stress.safepoints_in_step < STEP_STRESS_GRACE_SAFEPOINTS;
        if stepping && (in_grace || about_to_stop) {
            stress.overdue += 1;
            return;
        }
        if stepping {
            stress.ended += 1;
        }
        let source = CommandSource::Stepper(stress.reader(false));
        self.enter(|mutator| {
            // A step ended here only outlasted its grace: its frame is
            // still there. One that left -- popped, its pop reported to
            // nobody -- is a step that could never stop, which ending it
            // would hide.
            debug_assert!(
                stepping::frame_still_there(mutator),
                "internal: the step stress mode ended a step whose frame left unreported"
            );
            debug_assert!(
                stepping::still_waiting_for_a_handler(mutator),
                "internal: the step stress mode ended a step that waits for a handler report that never came"
            );
            stepping::take_exits(mutator);
            stepping::end(mutator);
        });
        let served = self.stop(StopReason::Pause, None, floor, source);
        debug_assert!(
            served == Served::GoOn,
            "internal: the step stress mode's reader never terminates"
        );
    }

    /// Whether anything takes a step's stop: an attached debugger, or, in
    /// a debug build, the step stress mode.
    pub(crate) fn has_agent(&self) -> bool {
        #[cfg(debug_assertions)]
        if self.stress_step.is_some() {
            return true;
        }
        self.debugger.is_some()
    }

    /// The agent a step's stop serves: the attached debugger, else the
    /// step stress mode's reader. The mode stands aside while a debugger
    /// is attached, so that it never steps in the debugger's place.
    fn step_agent(&mut self) -> CommandSource {
        #[cfg(debug_assertions)]
        if let (None, Some(stress)) = (&self.debugger, &mut self.stress_step) {
            return CommandSource::Stepper(stress.reader(true));
        }
        CommandSource::Remote
    }

    /// Drains the commands queued while the program ran. The bit is
    /// cleared before the queue is drained, so that a command queued
    /// meanwhile keeps its bit for the next safepoint
    /// (`docs/design/debugger.md`, "Commands while the program runs"). A
    /// `Terminate` ends the run, and the commands queued after it go with
    /// the run (`serve_between_runs`), never to the next; a configuration
    /// command among them is served all the same.
    fn serve_queued_commands(&mut self) -> Served {
        self.clear_debugger_bits(DEBUGGER_COMMANDS);
        loop {
            let Some(debugger) = &self.debugger else {
                return Served::GoOn;
            };
            let command = match debugger.commands.try_recv() {
                Ok(command) => command,
                Err(TryRecvError::Empty) => return Served::GoOn,
                Err(TryRecvError::Disconnected) => {
                    self.detach_debugger();
                    return Served::GoOn;
                }
            };
            match command {
                Command::Terminate => {
                    self.serve_between_runs();
                    return Served::Terminate;
                }
                command @ (Command::SetBreakpoints { .. }
                | Command::SetMethodBreakpoints(_)
                | Command::SetStepFilters { .. }
                | Command::SetExceptionBreakpoints { .. }) => {
                    self.configure(&mut CommandSource::Remote, command);
                }
                command @ (Command::Continue
                | Command::StepOver
                | Command::StepInto
                | Command::StepOut
                | Command::RunToPosition { .. }
                | Command::StackTrace
                | Command::ModuleText { .. }
                | Command::Variables { .. }
                | Command::Children { .. }
                | Command::Evaluate { .. }
                | Command::SetVariable { .. }) => {
                    self.refuse(&mut CommandSource::Remote, &command, RUNNING);
                }
            }
        }
    }

    /// Serves a configuration command, which changes what stops the
    /// program later and stops nothing itself, answering to `source`.
    fn configure(&mut self, source: &mut CommandSource, command: Command) {
        match command {
            Command::SetBreakpoints {
                module,
                breakpoints: requested,
            } => {
                let event = self
                    .enter(|mutator| breakpoints::set_line_breakpoints(mutator, module, requested));
                self.answer(source, event);
            }
            Command::SetStepFilters { shipped, classes } => {
                self.enter(|mutator| {
                    mutator
                        .breakpoints
                        .set_step_filters(StepFilters { shipped, classes });
                });
                self.answer(source, Event::StepFiltersSet);
            }
            Command::SetMethodBreakpoints(requested) => {
                let event =
                    self.enter(|mutator| breakpoints::set_method_breakpoints(mutator, requested));
                self.answer(source, event);
            }
            Command::SetExceptionBreakpoints {
                uncaught,
                caught,
                halt,
                classes,
            } => {
                self.enter(|mutator| {
                    mutator
                        .breakpoints
                        .set_exception_breakpoints(ExceptionBreakpoints {
                            uncaught,
                            caught,
                            halt,
                            classes,
                        });
                });
                self.answer(source, Event::ExceptionBreakpointsSet);
            }
            Command::Continue
            | Command::StepOver
            | Command::StepInto
            | Command::StepOut
            | Command::RunToPosition { .. }
            | Command::Terminate
            | Command::StackTrace
            | Command::ModuleText { .. }
            | Command::Variables { .. }
            | Command::Children { .. }
            | Command::Evaluate { .. }
            | Command::SetVariable { .. } => {
                unreachable!("internal: only a configuration command configures")
            }
        }
    }

    /// Stops the program for `reason`: tells the agent, then serves the
    /// commands `source` gives until it continues or terminates the
    /// program, answering to `source`. A frame of
    /// optimized code that its poll left goes on in baseline code first,
    /// its eliminated blocks made, so that every frame the agent reads is
    /// the interpreter's word for word (`docs/design/debugger.md`,
    /// "The stop"). The values the stop hands out are
    /// held in its table, `floor` the run's base (`stack.rs`), and go
    /// with it when the program goes on; `returned`, the value of the
    /// last call a step recorded, is the first of them. A step command
    /// begins its step (`stepping::begin`), and a run to cursor sets its
    /// target (`breakpoints::run_to`), each going on as `Continue` does;
    /// a run to cursor whose position resolves to no statement is
    /// refused, the program still stopped.
    fn stop(
        &mut self,
        reason: StopReason,
        returned: Option<Returned>,
        floor: usize,
        source: CommandSource,
    ) -> Served {
        self.stop_standing(reason, returned, floor, source, Standing::ON_TOP)
    }

    /// `stop`, standing where `standing` says.
    fn stop_standing(
        &mut self,
        reason: StopReason,
        returned: Option<Returned>,
        floor: usize,
        source: CommandSource,
        standing: Standing,
    ) -> Served {
        self.stop_owing(
            reason,
            returned,
            floor,
            source,
            standing,
            AfterStop::default(),
        )
    }

    /// `stop_standing`, owing the program `after` once it goes on.
    fn stop_owing(
        &mut self,
        reason: StopReason,
        returned: Option<Returned>,
        floor: usize,
        source: CommandSource,
        standing: Standing,
        after: AfterStop,
    ) -> Served {
        let Standing {
            in_handler,
            at_header,
            elsewhere,
        } = standing;
        // A condition that failed at a method's exit by an unwind since
        // the last stop is named by this one, which reports the exit.
        let reason = self.enter(|mutator| conditions::failure_named(mutator, reason));
        self.enter(crate::jit::deoptimize_stopped_at_poll);
        let first_value_id = self
            .debugger
            .as_ref()
            .map_or(0, |debugger| debugger.next_value_id);
        let mut values = StopValues::starting_at(first_value_id);
        // An exception's stop hides its machinery: an uncaught one's and a
        // `Halt`'s show first the frame below it, a caught one's the
        // handler about to run, the frame below the machinery next -- but
        // for a handler that pushed no frame, whose stop stands in the
        // machinery and so shows it as an uncaught one's does.
        let exceptional = matches!(
            reason,
            StopReason::Exception { .. } | StopReason::Halt { .. }
        );
        let caught = in_handler && matches!(reason, StopReason::Exception { caught: true, .. });
        let returned = returned.map(|returned| {
            self.enter(|mutator| {
                let value = values.describe(mutator, mutator.heap.fetch(&returned.value));
                let call = stack::call_name(mutator, returned.call);
                ReturnedValue { call, value }
            })
        });
        let mut stop = Stop {
            exceptional,
            caught,
            at_header,
            floor,
            source,
            values,
            frames: None,
            after,
            evaluated: false,
            setting: None,
        };
        self.answer(
            &mut stop.source,
            Event::Stopped {
                reason,
                returned,
                elsewhere,
            },
        );
        self.serve_stop(stop)
    }

    /// Serves `stop`'s commands until the agent continues, steps or
    /// terminates the program, or begins an evaluation, for which the
    /// stop is kept (`Served::Evaluate`) and served again once the
    /// evaluation ended (`serve_evaluation`). Once the program goes on,
    /// the stop pays what it owes (`AfterStop`), and the next stop's ids
    /// begin past this one's.
    fn serve_stop(&mut self, mut stop: Stop) -> Served {
        let served = loop {
            let command = match &mut stop.source {
                CommandSource::Remote => {
                    let Some(debugger) = &self.debugger else {
                        // The remote is gone: as a `Continue`.
                        break Served::GoOn;
                    };
                    let Ok(command) = debugger.commands.recv() else {
                        self.detach_debugger();
                        break Served::GoOn;
                    };
                    command
                }
                #[cfg(debug_assertions)]
                CommandSource::Reader(reader) => reader.next_command(),
                #[cfg(debug_assertions)]
                CommandSource::Stepper(stepper) => stepper.next_command(),
            };
            match command {
                Command::Continue => {
                    self.answer(&mut stop.source, Event::Continued);
                    break Served::GoOn;
                }
                command @ (Command::StepOver | Command::StepInto | Command::StepOut) => {
                    let action = match command {
                        Command::StepOver => StepAction::Over,
                        Command::StepInto => StepAction::Into,
                        _ => StepAction::Out,
                    };
                    // An uncaught exception's and a `Halt`'s stop steps
                    // from the frame it shows first, as the user sees it,
                    // never in the machinery above it the debugger chose
                    // to stop in; a caught one's steps in the handler it
                    // stands in, which it shows first.
                    let shows_below = stop.exceptional && !stop.caught;
                    let at_header = stop.at_header;
                    self.enter(|mutator| {
                        let shown = shows_below.then(|| {
                            let frames = stop.frames(mutator);
                            let first = exceptions::first_shown(mutator, frames);
                            frames.get(first)
                        });
                        if at_header {
                            stepping::begin_at_header(mutator, action);
                        } else {
                            stepping::begin(mutator, action, shown.flatten());
                        }
                    });
                    self.answer(&mut stop.source, Event::Continued);
                    break Served::GoOn;
                }
                Command::RunToPosition { module, offset } => {
                    match self.enter(|mutator| breakpoints::run_to(mutator, &module, offset)) {
                        Ok(()) => {
                            self.answer(&mut stop.source, Event::Continued);
                            break Served::GoOn;
                        }
                        Err(reason) => self.refuse(
                            &mut stop.source,
                            &Command::RunToPosition { module, offset },
                            reason,
                        ),
                    }
                }
                Command::Terminate => break self.terminate(),
                Command::StackTrace => {
                    let (exceptional, caught, at_header) =
                        (stop.exceptional, stop.caught, stop.at_header);
                    let descriptions = self.enter(|mutator| {
                        let Stop {
                            frames,
                            values,
                            floor,
                            ..
                        } = &mut stop;
                        let frames = frames.get_or_insert_with(|| stack::frames(mutator, *floor));
                        let hidden = if exceptional {
                            exceptions::hidden(mutator, frames, caught)
                        } else {
                            0..0
                        };
                        stack::stack_trace(mutator, frames, hidden, at_header, values)
                    });
                    self.answer(&mut stop.source, Event::StackTrace(descriptions));
                }
                Command::ModuleText {
                    module,
                    known_revision,
                } => {
                    let event = self.enter(|mutator| {
                        let revision = mutator.breakpoints.text_revision();
                        let text = if known_revision == Some(revision) {
                            ModuleTextAnswer::Unchanged
                        } else {
                            breakpoints::module_text(mutator, &module)
                                .map_or(ModuleTextAnswer::None, ModuleTextAnswer::Text)
                        };
                        Event::ModuleText {
                            module,
                            revision,
                            text,
                        }
                    });
                    self.answer(&mut stop.source, event);
                }
                Command::Variables { frame } => {
                    match self.enter(|mutator| {
                        let Stop {
                            frames,
                            values,
                            floor,
                            ..
                        } = &mut stop;
                        let frames = frames.get_or_insert_with(|| stack::frames(mutator, *floor));
                        stack::variables(mutator, frames, frame, values)
                    }) {
                        Some(variables) => {
                            self.answer(&mut stop.source, Event::Variables(variables));
                        }
                        None => self.refuse(&mut stop.source, &command, NO_SUCH_FRAME),
                    }
                }
                Command::Children {
                    value,
                    start,
                    count,
                } => match self.enter(|mutator| stop.values.children(mutator, value, start, count))
                {
                    Some(children) => self.answer(&mut stop.source, Event::Variables(children)),
                    None => self.refuse(&mut stop.source, &command, NO_SUCH_VALUE),
                },
                command @ (Command::Evaluate { .. } | Command::SetVariable { .. }) => {
                    match self.begin_evaluation(&mut stop, &command) {
                        Ok(()) => {
                            // Kept for the evaluation, whose run may move
                            // the frames before the stop serves again.
                            stop.evaluated = true;
                            stop.frames = None;
                            stop.setting = match command {
                                Command::SetVariable { name, .. } => Some(name),
                                _ => None,
                            };
                            if let Some(debugger) = &mut self.debugger {
                                debugger.stop = Some(stop);
                            }
                            return Served::Evaluate;
                        }
                        Err(reason) => self.refuse(&mut stop.source, &command, &reason),
                    }
                }
                command @ (Command::SetBreakpoints { .. }
                | Command::SetMethodBreakpoints(_)
                | Command::SetStepFilters { .. }
                | Command::SetExceptionBreakpoints { .. }) => {
                    self.configure(&mut stop.source, command);
                }
            }
        };
        // The next stop's ids begin past this one's, so that none of
        // this stop's names a value there.
        if let Some(debugger) = &mut self.debugger {
            debugger.next_value_id = stop.values.next_first();
        }
        if served != Served::GoOn {
            return served;
        }
        let AfterStop { past, rebuilt } = stop.after;
        if let Some(mut past) = past {
            let evaluated = stop.evaluated;
            self.enter(|mutator| {
                // The frame the stop stands in is the top one, which an
                // evaluation leaves on top, but may have moved: the
                // suspend stress mode resumes it elsewhere.
                if evaluated && let Some(top) = mutator.heap.top_activation() {
                    past.activation = top;
                }
                mutator.breakpoints.go_past_once(past);
            });
        }
        if rebuilt { Served::Rebuilt } else { served }
    }

    /// Compiles what `command` -- an `Evaluate` or a `SetVariable` --
    /// evaluates in the frame it names of `stop`'s stack and pushes it
    /// (`evaluation::compile`, `evaluation::compile_assignment`,
    /// `evaluation::begin`); the refusal's reason otherwise. Only a stop
    /// the attached debugger serves can be kept for an evaluation.
    fn begin_evaluation(&mut self, stop: &mut Stop, command: &Command) -> Result<(), String> {
        if self.debugger.is_none() || !matches!(stop.source, CommandSource::Remote) {
            return Err(String::from(NOT_KEPT));
        }
        let (frame, code) = self.enter(|mutator| match command {
            Command::Evaluate { frame, expression } => {
                evaluation::compile(mutator, stop, *frame, expression).map(|code| (*frame, code))
            }
            Command::SetVariable {
                frame,
                name,
                expression,
            } => evaluation::compile_assignment(mutator, stop, *frame, name, expression)
                .map(|code| (*frame, code)),
            _ => unreachable!("internal: only an `Evaluate` and a `SetVariable` evaluate"),
        })?;
        self.enter(|mutator| evaluation::begin(mutator, stop, frame, code, &[]))
            .map_err(String::from)
    }

    /// The run ends: a pause still pending is moot, so its bit goes, and
    /// a frame of optimized code its poll left goes on in baseline code,
    /// its eliminated blocks made, before the unwind pops it.
    fn terminate(&mut self) -> Served {
        // So is an exception's stop not served yet, whose frame the
        // termination pops.
        self.clear_debugger_bits(DEBUGGER_PAUSE | DEBUGGER_STOP);
        // An exception an `ensure:` block of the termination's unwind
        // raises stops nothing either, until the run ends
        // (`exceptions::drop_pending`).
        self.enter(|mutator| {
            mutator.breakpoints.take_pending_exception();
            mutator.breakpoints.set_terminating(true);
        });
        // A step in progress ends with the run: its request would lower
        // the depth limit for the `ensure:` blocks the unwind runs, and
        // stop in one of them after the termination.
        self.enter(stepping::end);
        // So does an evaluation, unanswered, whose base the unwind pops on
        // its way to the floor, and the stop kept for it; the depth limit
        // keeps the evaluation's raise until the run ended.
        self.enter(evaluation::terminated);
        // So does a decision of conditions an evaluation ran for.
        if let Some(debugger) = &mut self.debugger {
            debugger.deciding = None;
        }
        if let Some(debugger) = &mut self.debugger
            && let Some(stop) = debugger.stop.take()
        {
            debugger.next_value_id = stop.values.next_first();
        }
        self.enter(crate::jit::deoptimize_stopped_at_poll);
        Served::Terminate
    }

    fn refuse(&mut self, source: &mut CommandSource, command: &Command, reason: &str) {
        self.answer(
            source,
            Event::Refused {
                command: String::from(command.name()),
                reason: String::from(reason),
            },
        );
    }

    /// Sends `event` to whoever gave the command it answers.
    fn answer(&mut self, source: &mut CommandSource, event: Event) {
        match source {
            CommandSource::Remote => self.tell(event),
            #[cfg(debug_assertions)]
            CommandSource::Reader(reader) => reader.take(event),
            #[cfg(debug_assertions)]
            CommandSource::Stepper(stepper) => stepper.take(event),
        }
    }

    /// Sends `event` to the agent; a remote that is gone detaches the
    /// debugger.
    fn tell(&mut self, event: Event) {
        let delivered = self
            .debugger
            .as_ref()
            .is_some_and(|debugger| debugger.events.send(event).is_ok());
        if !delivered {
            self.detach_debugger();
        }
    }

    /// Serves the commands an agent queued while no run executes, for a
    /// host that waits between runs -- before its first, for the client's
    /// configuration -- so that a configuration command is answered while
    /// the host waits rather than when the next run starts: every
    /// configuration command is served, and every other is refused as one
    /// queued between runs is (`serve_between_runs`). A host calls it on
    /// its own thread, never during a run. Cold: only a debugged host
    /// calls it.
    #[cold]
    pub fn serve_debugger_between_runs(&mut self) {
        self.serve_between_runs();
    }

    /// What a run's start serves, its first activation pushed and about
    /// to run: the commands queued between runs (`serve_between_runs`),
    /// then a pause asked meanwhile, which stops the first activation at
    /// its first statement start (`stepping::stop_at_entry`) rather than at
    /// the run's first safepoint, or else a step the run before left in
    /// progress, which goes on from the first activation
    /// (`stepping::carry_into`) and stops it there too, a step's stop. A
    /// pause's stop reports that step's method exits, which it ends.
    /// Cold: a run without an agent tests one option.
    #[cold]
    pub(crate) fn serve_before_run(&mut self) {
        self.serve_between_runs();
        if !self.has_agent() {
            return;
        }
        let debugged = self.debugger.is_some();
        self.enter(|mutator| {
            let paused = debugged && mutator.heap.debugger_bits() & DEBUGGER_PAUSE != 0;
            let Some(entry) = mutator.heap.top_activation() else {
                return;
            };
            if paused {
                let exits = mutator.breakpoints.take_carried_step();
                stepping::stop_at_entry(mutator, entry, exits.unwrap_or_default());
            } else if let Some(exits) = mutator.breakpoints.take_carried_step() {
                stepping::carry_into(mutator, entry, exits);
            }
        });
    }

    /// Takes every command queued for a run that is over, or that has not
    /// begun, after clearing the commands bit under the arming lock: a
    /// configuration command, which stops nothing, is served, and every
    /// other command is dropped and answered by `Event::Refused`. A command
    /// that acts on a run is served by the run it arrives during, and one
    /// left queued -- a `Terminate` sent after its run ended, or queued
    /// after the one that ended it -- would otherwise end the next run at
    /// its first safepoint, or be taken by its first stop, a run nobody
    /// terminated. The bit is cleared before the queue
    /// is drained, as `serve_queued_commands` clears it: the agent queues
    /// a command before it sets the bit, so a command the drain misses
    /// keeps its bit and is served, or dropped, at the next safepoint; the
    /// other order would leave it queued with its bit cleared, for a later
    /// stop to take. A pause asked between runs is no command and stays,
    /// to stop the next run. A remote that is gone detaches the debugger.
    pub(crate) fn serve_between_runs(&mut self) {
        if self.debugger.is_none() {
            return;
        }
        self.clear_debugger_bits(DEBUGGER_COMMANDS);
        loop {
            let Some(debugger) = &self.debugger else {
                return;
            };
            match debugger.commands.try_recv() {
                Ok(
                    command @ (Command::SetBreakpoints { .. }
                    | Command::SetMethodBreakpoints(_)
                    | Command::SetStepFilters { .. }
                    | Command::SetExceptionBreakpoints { .. }),
                ) => {
                    self.configure(&mut CommandSource::Remote, command);
                }
                Ok(
                    command @ (Command::Continue
                    | Command::StepOver
                    | Command::StepInto
                    | Command::StepOut
                    | Command::RunToPosition { .. }
                    | Command::Terminate
                    | Command::StackTrace
                    | Command::ModuleText { .. }
                    | Command::Variables { .. }
                    | Command::Children { .. }
                    | Command::Evaluate { .. }
                    | Command::SetVariable { .. }),
                ) => {
                    // A `Terminate` ends the step the run before left in
                    // progress, as it ends one in a run, though no run
                    // executes for it to end.
                    if command == Command::Terminate {
                        self.enter(stepping::end);
                    }
                    // The drop stays, but never silent: every command has
                    // exactly one answer, as every DAP request has one.
                    self.refuse(&mut CommandSource::Remote, &command, "no run executes");
                }
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    self.detach_debugger();
                    return;
                }
            }
        }
    }

    /// The remote is gone: the debugger goes, and with it every bit it
    /// left, which nobody is left to serve.
    /// No attachment is current from here on, under the same lock, so
    /// that a remote of this one still held arms nothing after the bits
    /// are cleared.
    fn detach_debugger(&mut self) {
        let attachments = Arc::clone(&self.debugger_arming.attachments);
        let mut guarded = attachments.lock().unwrap_or_else(PoisonError::into_inner);
        guarded.current = None;
        self.enter(|mutator| mutator.heap.clear_debugger_bits(DEBUGGER_BITS));
        drop(guarded);
        // Nobody is left to stop for: every breakpoint goes, each slot
        // given its original back, and any step with them.
        self.enter(|mutator| {
            breakpoints::clear(mutator);
            mutator.breakpoints.report_to(None);
            mutator.breakpoints.arm_under(None);
        });
        self.debugger = None;
    }

    /// Clears the debugger's `bits` under the arming lock, which keeps the
    /// clearing's resynchronization of the depth limit from falling
    /// between the agent's two writes (`Arming`).
    fn clear_debugger_bits(&mut self, bits: u64) {
        let attachments = Arc::clone(&self.debugger_arming.attachments);
        let _armed = attachments.lock().unwrap_or_else(PoisonError::into_inner);
        self.enter(|mutator| mutator.heap.clear_debugger_bits(bits));
    }
}

/// The signal a termination by the debugger unwinds with, raised in the
/// top activation: as the termination a leaf raises, it runs every pending
/// `ensure:` block on its way to the run's floor, and the host's send
/// answers its error.
pub(crate) fn termination<'gc>(mutator: &Mutator<'gc>) -> Signal<'gc> {
    let receiver = mutator
        .heap
        .top_activation()
        .map_or_else(Value::nil, |top| mutator.heap.activation_receiver(top));
    Signal::Terminate(Box::new(NewspeakError::error(
        Symbol::intern("terminate"),
        receiver,
        TERMINATED,
    )))
}
