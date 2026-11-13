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
//! breakpoint is decided by the handler search's leaves, which record the
//! stop and ask the next poll for it (`exceptions.rs`).

pub(crate) mod breakpoints;
pub(crate) mod exceptions;
pub(crate) mod metadata;
mod remote;
mod stack;
pub(crate) mod stepping;
mod values;

pub use remote::{
    BreakpointId, BreakpointState, ClassName, Command, DebugRemote, Event, FrameDescription,
    FrameKind, FrameLocation, MethodBreakpoint, MethodBreakpointState, SendLocation, StopReason,
    ValueDescription, ValueId, Variable,
};

use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex, PoisonError};

use crate::heap::{DEBUGGER_BITS, DEBUGGER_COMMANDS, DEBUGGER_PAUSE, DEBUGGER_STEP, DEBUGGER_STOP};
use crate::interpreter::{Interpreter, Mutator, Signal};
use crate::types::NewspeakError;
use crate::{Root, Symbol, Value};
use breakpoints::Hits;
use exceptions::ExceptionBreakpoints;
use stepping::{HeldStep, StepAction, StepFilters};
use values::StopValues;

/// The message of the error a run the debugger terminated ends with.
const TERMINATED: &str = "the debugger terminated the program";

/// Why the program cannot serve a command that only a stop serves.
const RUNNING: &str = "the program is running";

/// Why a stop refuses `Variables` for a frame index past its stack.
const NO_SUCH_FRAME: &str = "no frame at that index";

/// Why a stop refuses `Children` for a value it did not hand out.
const NO_SUCH_VALUE: &str = "no value of this stop has that id";

/// The VM's end of an attached debugger, held by its `Interpreter`.
pub(crate) struct Debugger {
    commands: Receiver<Command>,
    events: Sender<Event>,
    /// The id the next stop's first value takes, past every id the stops
    /// before handed out (`values::StopValues`).
    next_value_id: u32,
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
                                value: variable.value.as_ref()?.id,
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
            | Event::Continued
            | Event::Breakpoints { .. }
            | Event::MethodBreakpoints(_)
            | Event::StepFiltersSet
            | Event::ExceptionBreakpointsSet => {}
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
    /// as the pause stress mode is, the stack counted once more for each
    /// step of a burst (`Interpreter::step_for_stress`).
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

/// What serving the debugger at a safepoint decided.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Served {
    /// The program goes on, through the rest of the safepoint.
    GoOn,
    /// The agent terminated the run.
    Terminate,
}

impl Interpreter {
    /// Attaches a debugger and answers its remote, which may move to
    /// another thread; a debugger attached before replaces it, its queued
    /// commands dropped with its channels and its pause and commands bits
    /// cleared, so that the new remote meets no stop it never asked for.
    /// A pause the remote asks while no run executes stops the next run at
    /// its first safepoint.
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
        let (bits, breakpoint, exception) = self.enter(|mutator| {
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
        if bits & DEBUGGER_COMMANDS != 0 && self.serve_queued_commands() == Served::Terminate {
            return self.terminate();
        }
        let exception = match (exception, self.debugger.is_some()) {
            (Some(pending), true) => self.enter(|mutator| exceptions::serve(mutator, pending)),
            _ => None,
        };
        if let Some(reason) = exception {
            // Where the frame on top stands is debugger.md's "Exceptions":
            // the first instruction of what the search runs next, unless a
            // fast path answered it without a push. The stop serves a
            // pause asked meanwhile too, and ends a step, whose handler
            // report may have led it here.
            self.clear_debugger_bits(DEBUGGER_PAUSE);
            self.enter(|mutator| {
                stepping::take_exits(mutator);
                stepping::end(mutator);
            });
            return self.stop(reason, None, floor, CommandSource::Remote);
        }
        if let Some(breakpoint) = breakpoint {
            // A stop only a step's one-shot made, in a frame the step
            // filters filter, is not made: the step goes on from the first
            // unfiltered frame below, and the frame fetches the slot it
            // stands at again, past it once.
            let filtered = self.has_agent()
                && breakpoint.hits.only_a_step()
                && self.enter(|mutator| {
                    stepping::went_on_past_filtered(mutator, breakpoint.slot.activation)
                });
            if filtered {
                self.enter(|mutator| mutator.breakpoints.go_past_once(breakpoint.slot));
            } else if self.has_agent() {
                // The stop serves a pause asked meanwhile too, and ends a
                // step, whatever made it.
                self.clear_debugger_bits(DEBUGGER_PAUSE);
                let (reason, returned) = self.enter(|mutator| {
                    let unwound = stepping::take_exits(mutator);
                    let returned = stepping::end(mutator);
                    // A slot only a step's one-shot or the run to cursor
                    // stopped at is the step's stop, which reports the
                    // value of the last call; a breakpoint the debugger
                    // set there makes it the breakpoint's.
                    match breakpoint.hits.reason(unwound) {
                        Some(reason) => (reason, None),
                        None => (StopReason::Step, returned),
                    }
                });
                let source = self.step_agent();
                let served = self.stop(reason, returned, floor, source);
                // The frame fetches the patched slot again as it goes on:
                // the breakpoint it stopped at lets it past once.
                if served == Served::GoOn {
                    self.enter(|mutator| mutator.breakpoints.go_past_once(breakpoint.slot));
                }
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
        if bits & DEBUGGER_STEP != 0 && matches!(self.enter(stepping::held_step), HeldStep::Stop) {
            // The pushed frame stands at its first statement start, which
            // it fetches as it goes on: a breakpoint there makes the stop
            // the breakpoint's -- a method breakpoint's entry -- and lets
            // it past once.
            let (reason, standing) = self
                .enter(|mutator| {
                    let unwound = stepping::take_exits(mutator);
                    stepping::end(mutator);
                    let top = mutator.heap.top_activation()?;
                    let (hits, after) = crate::interpreter::breakpoint::standing_hits(mutator, top);
                    let code = mutator.heap.activation_code(top);
                    let standing = breakpoints::PastOnce {
                        activation: top,
                        body: breakpoints::address(&code),
                        index: mutator.heap.activation_program_counter(top),
                        after,
                    };
                    Some((hits.reason(unwound), standing))
                })
                .map_or((None, None), |(reason, standing)| (reason, Some(standing)));
            let reason = reason.unwrap_or(StopReason::Step);
            let source = self.step_agent();
            let served = self.stop(reason, None, floor, source);
            if let (Served::GoOn, Some(standing)) = (served, standing) {
                self.enter(|mutator| mutator.breakpoints.go_past_once(standing));
            }
            return served;
        }
        Served::GoOn
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
    /// whole stack: a deep stack's bursts come as much rarer as they cost
    /// more, and a shallow program's are paced as the pause stress mode's
    /// stops. A step still in progress -- a step out of the run's first
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
        let paced_size = traced_size.saturating_add(STEP_STRESS_BURST.saturating_mul(stack_words));
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
    fn has_agent(&self) -> bool {
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
                | Command::Variables { .. }
                | Command::Children { .. }) => {
                    self.refuse(&mut CommandSource::Remote, &command, RUNNING);
                }
            }
        }
    }

    /// Serves a configuration command, which changes what stops the
    /// program later and stops nothing itself, answering to `source`.
    fn configure(&mut self, source: &mut CommandSource, command: Command) {
        match command {
            Command::SetBreakpoints { module, offsets } => {
                let event = self
                    .enter(|mutator| breakpoints::set_line_breakpoints(mutator, module, offsets));
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
            | Command::Variables { .. }
            | Command::Children { .. } => {
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
        returned: Option<Root>,
        floor: usize,
        mut source: CommandSource,
    ) -> Served {
        self.enter(crate::jit::deoptimize_stopped_at_poll);
        let first_value_id = self
            .debugger
            .as_ref()
            .map_or(0, |debugger| debugger.next_value_id);
        let mut values = StopValues::starting_at(first_value_id);
        // An exception's stop shows first the frame below its machinery.
        let exceptional = matches!(reason, StopReason::Exception { .. } | StopReason::Halt);
        let returned = returned.map(|returned| {
            self.enter(|mutator| values.describe(mutator, mutator.heap.fetch(&returned)))
        });
        self.answer(&mut source, Event::Stopped { reason, returned });
        // The run's frames, walked at the first command that reads them:
        // nothing pushes or pops while the program is stopped.
        let mut frames = None;
        let served = loop {
            let command = match &mut source {
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
                    self.answer(&mut source, Event::Continued);
                    break Served::GoOn;
                }
                command @ (Command::StepOver | Command::StepInto | Command::StepOut) => {
                    let action = match command {
                        Command::StepOver => StepAction::Over,
                        Command::StepInto => StepAction::Into,
                        _ => StepAction::Out,
                    };
                    // An exception's stop steps from the frame it shows
                    // first, as the user sees it, never in the machinery
                    // above it the debugger chose to stop in.
                    self.enter(|mutator| {
                        let shown = exceptional.then(|| {
                            let frames =
                                frames.get_or_insert_with(|| stack::frames(mutator, floor));
                            let first = exceptions::first_shown(mutator, frames);
                            frames.get(first)
                        });
                        stepping::begin(mutator, action, shown.flatten());
                    });
                    self.answer(&mut source, Event::Continued);
                    break Served::GoOn;
                }
                Command::RunToPosition { module, offset } => {
                    match self.enter(|mutator| breakpoints::run_to(mutator, &module, offset)) {
                        Ok(()) => {
                            self.answer(&mut source, Event::Continued);
                            break Served::GoOn;
                        }
                        Err(reason) => self.refuse(
                            &mut source,
                            &Command::RunToPosition { module, offset },
                            reason,
                        ),
                    }
                }
                Command::Terminate => break self.terminate(),
                Command::StackTrace => {
                    let descriptions = self.enter(|mutator| {
                        let frames = frames.get_or_insert_with(|| stack::frames(mutator, floor));
                        let first_shown = if exceptional {
                            exceptions::first_shown(mutator, frames)
                        } else {
                            0
                        };
                        stack::stack_trace(mutator, frames, first_shown, &mut values)
                    });
                    self.answer(&mut source, Event::StackTrace(descriptions));
                }
                Command::Variables { frame } => {
                    match self.enter(|mutator| {
                        let frames = frames.get_or_insert_with(|| stack::frames(mutator, floor));
                        stack::variables(mutator, frames, frame, &mut values)
                    }) {
                        Some(variables) => self.answer(&mut source, Event::Variables(variables)),
                        None => self.refuse(&mut source, &command, NO_SUCH_FRAME),
                    }
                }
                Command::Children {
                    value,
                    start,
                    count,
                } => match self.enter(|mutator| values.children(mutator, value, start, count)) {
                    Some(children) => self.answer(&mut source, Event::Variables(children)),
                    None => self.refuse(&mut source, &command, NO_SUCH_VALUE),
                },
                command @ (Command::SetBreakpoints { .. }
                | Command::SetMethodBreakpoints(_)
                | Command::SetStepFilters { .. }
                | Command::SetExceptionBreakpoints { .. }) => {
                    self.configure(&mut source, command);
                }
            }
        };
        // The next stop's ids begin past this one's, so that none of
        // this stop's names a value there.
        if let Some(debugger) = &mut self.debugger {
            debugger.next_value_id = values.next_first();
        }
        served
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

    /// Takes every command queued for a run that is over, or that has not
    /// begun, after clearing the commands bit under the arming lock: a
    /// configuration command, which stops nothing, is served, and every
    /// other dropped. A command that acts on a run is served by the run it
    /// arrives during, and one left queued -- a `Terminate` sent after its
    /// run ended, or queued after the one that ended it -- would otherwise
    /// end the next run at its first safepoint, or be taken by its first
    /// stop, a run nobody terminated. The bit is cleared before the queue
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
                    Command::Continue
                    | Command::StepOver
                    | Command::StepInto
                    | Command::StepOut
                    | Command::RunToPosition { .. }
                    | Command::Terminate
                    | Command::StackTrace
                    | Command::Variables { .. }
                    | Command::Children { .. },
                ) => {}
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
