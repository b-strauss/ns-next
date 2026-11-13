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
//! bit the safepoint stops at a push for (`stepping.rs`).

pub(crate) mod breakpoints;
pub(crate) mod metadata;
mod remote;
mod stack;
pub(crate) mod stepping;
mod values;

pub use remote::{
    BreakpointId, BreakpointState, Command, DebugRemote, Event, FrameDescription, FrameKind,
    FrameLocation, SendLocation, StopReason, ValueDescription, ValueId, Variable,
};

use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex, PoisonError};

use crate::heap::{DEBUGGER_BITS, DEBUGGER_COMMANDS, DEBUGGER_PAUSE, DEBUGGER_STEP};
use crate::interpreter::{Interpreter, Mutator, Signal};
use crate::types::NewspeakError;
use crate::{Root, Symbol, Value};
use stepping::{HeldStep, StepAction};
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
/// mode's reader, which needs no debugger.
pub(crate) enum CommandSource {
    /// The attached debugger's remote.
    Remote,
    /// The pause stress mode's reader.
    #[cfg(debug_assertions)]
    Reader(StressReader),
}

/// The pause stress mode's agent (`Interpreter::stop_for_stress`): it asks
/// for the stack, the variables of every frame and the first children of
/// every value those answered, then continues, so that every program the
/// mode runs proves that reading a stopped stack -- every frame, every
/// register the leftover analysis calls live, every value one level deep
/// -- changes nothing and reads no dead register, which a debug build
/// refuses (`docs/design/debugger.md`, "The pause stress mode").
#[cfg(debug_assertions)]
#[derive(Default)]
pub(crate) struct StressReader {
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
                panic!("internal: the pause stress mode's {command} was refused: {reason}")
            }
            Event::Stopped { .. } | Event::Continued | Event::Breakpoints { .. } => {}
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
    /// ran, then the stop a `Breakpoint` arm left for, then a pause; each
    /// stop waits until the agent continues or terminates. A remote that
    /// is gone detaches the debugger, and the program runs on.
    pub(crate) fn serve_debugger(&mut self, floor: usize) -> Served {
        let (bits, breakpoint) = self.enter(|mutator| {
            (
                mutator.heap.debugger_bits(),
                mutator.breakpoints.take_pending_stop(),
            )
        });
        if bits == 0 && breakpoint.is_none() {
            return Served::GoOn;
        }
        if self.debugger.is_none() {
            // A remote that set a bit and was dropped before this
            // safepoint, whose debugger a former one detached: nobody is
            // left to stop for.
            self.clear_debugger_bits(bits);
            return Served::GoOn;
        }
        if bits & DEBUGGER_COMMANDS != 0 && self.serve_queued_commands() == Served::Terminate {
            return self.terminate();
        }
        if let Some(breakpoint) = breakpoint {
            if self.debugger.is_some() {
                // The stop serves a pause asked meanwhile too, and ends a
                // step, whatever made it.
                self.clear_debugger_bits(DEBUGGER_PAUSE);
                let returned = self.enter(stepping::end);
                // A slot only a step's one-shot stopped at is the step's
                // stop, which reports the value of the last call; a line
                // breakpoint there makes it the breakpoint's.
                let (reason, returned) = if breakpoint.ids.is_empty() {
                    (StopReason::Step, returned)
                } else {
                    (
                        StopReason::Breakpoint {
                            ids: breakpoint.ids,
                        },
                        None,
                    )
                };
                let served = self.stop(reason, returned, floor, CommandSource::Remote);
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
            self.enter(stepping::end);
            return self.stop(StopReason::Pause, None, floor, CommandSource::Remote);
        }
        if bits & DEBUGGER_STEP != 0 && matches!(self.enter(stepping::held_step), HeldStep::Stop) {
            self.enter(stepping::end);
            // The pushed frame stands at its first statement start, which
            // it fetches as it goes on: a breakpoint there lets it past
            // once.
            let standing = self.enter(|mutator| {
                let top = mutator.heap.top_activation()?;
                let code = mutator.heap.activation_code(top);
                Some(breakpoints::PastOnce {
                    activation: top,
                    body: breakpoints::address(&code),
                    index: mutator.heap.activation_program_counter(top),
                })
            });
            let served = self.stop(StopReason::Step, None, floor, CommandSource::Remote);
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
            CommandSource::Reader(StressReader::default()),
        );
        debug_assert!(
            served == Served::GoOn,
            "internal: the pause stress mode's reader never terminates"
        );
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
                command @ Command::SetBreakpoints { .. } => {
                    self.configure(&mut CommandSource::Remote, command);
                }
                command @ (Command::Continue
                | Command::StepOver
                | Command::StepInto
                | Command::StepOut
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
            Command::Continue
            | Command::StepOver
            | Command::StepInto
            | Command::StepOut
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
    /// begins its step (`stepping::begin`) and goes on as `Continue`
    /// does.
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
                    self.enter(|mutator| stepping::begin(mutator, action));
                    self.answer(&mut source, Event::Continued);
                    break Served::GoOn;
                }
                Command::Terminate => break self.terminate(),
                Command::StackTrace => {
                    let descriptions = self.enter(|mutator| {
                        let frames = frames.get_or_insert_with(|| stack::frames(mutator, floor));
                        stack::stack_trace(mutator, frames, &mut values)
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
                command @ Command::SetBreakpoints { .. } => self.configure(&mut source, command),
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
        self.clear_debugger_bits(DEBUGGER_PAUSE);
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
                Ok(command @ Command::SetBreakpoints { .. }) => {
                    self.configure(&mut CommandSource::Remote, command);
                }
                Ok(
                    Command::Continue
                    | Command::StepOver
                    | Command::StepInto
                    | Command::StepOut
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
