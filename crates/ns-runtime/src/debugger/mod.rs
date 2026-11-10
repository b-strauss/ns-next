//! The debugging API's VM side: an attached debugger
//! (`Interpreter::attach_debugger`), served at the run loop's safepoint
//! (`docs/design/debugger.md`, decisions 1, 7 and 9). The agent arms the
//! poll words from its own thread (`DebugRemote`); the safepoint serves
//! the commands it queued while the program runs and, for a pause, stops:
//! it blocks on the command channel, between two stretches, where no
//! handle is live and the whole stack is on the activation stack, until
//! the agent continues or terminates the program. Meanwhile it answers the
//! run's stack (`stack.rs`) and the values it hands out, held for the stop
//! (`values.rs`).

mod remote;
mod stack;
mod values;

pub use remote::{
    Command, DebugRemote, Event, FrameDescription, FrameKind, StopReason, ValueDescription,
    ValueId, Variable,
};

use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex, PoisonError};

use crate::heap::{DEBUGGER_COMMANDS, DEBUGGER_PAUSE};
use crate::interpreter::{Interpreter, Mutator, Signal};
use crate::types::NewspeakError;
use crate::{Symbol, Value};
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
    /// another thread; a debugger attached before replaces it. A pause
    /// the remote asks while no run executes stops the next run at its
    /// first safepoint.
    pub fn attach_debugger(&mut self) -> DebugRemote {
        let (command_sender, command_receiver) = mpsc::channel();
        let (event_sender, event_receiver) = mpsc::channel();
        let words = self.enter(|mutator| mutator.heap.machine_words_handle());
        let attachments = Arc::clone(&self.debugger_arming.attachments);
        let attachment = {
            let mut guarded = attachments.lock().unwrap_or_else(PoisonError::into_inner);
            guarded.last += 1;
            guarded.current = Some(guarded.last);
            guarded.last
        };
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
    /// ran, then a pause, which stops until the agent continues or
    /// terminates. A remote that is gone detaches the debugger, and the
    /// program runs on.
    pub(crate) fn serve_debugger(&mut self, floor: usize) -> Served {
        let bits = self.enter(|mutator| mutator.heap.debugger_bits());
        if bits == 0 {
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
        let pause_requested = self.enter(|mutator| mutator.heap.debugger_bits()) & DEBUGGER_PAUSE;
        if pause_requested != 0 && self.debugger.is_some() {
            self.clear_debugger_bits(DEBUGGER_PAUSE);
            return self.stop(StopReason::Pause, floor);
        }
        Served::GoOn
    }

    /// Drains the commands queued while the program ran. The bit is
    /// cleared before the queue is drained, so that a command queued
    /// meanwhile keeps its bit for the next safepoint
    /// (`docs/design/debugger.md`, "Commands while the program runs").
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
                Command::Terminate => return Served::Terminate,
                command @ (Command::Continue
                | Command::StackTrace
                | Command::Variables { .. }
                | Command::Children { .. }) => self.refuse(&command, RUNNING),
            }
        }
    }

    /// Stops the program for `reason`: tells the agent, then serves its
    /// commands until it continues or terminates the program. A frame of
    /// optimized code that its poll left goes on in baseline code first,
    /// its eliminated blocks made, so that every frame the agent reads is
    /// the interpreter's word for word (`docs/design/debugger.md`,
    /// "Reading the stack at a stop"). The values the stop hands out are
    /// held in its table, `floor` the run's base (`stack.rs`), and go
    /// with it when the program goes on.
    fn stop(&mut self, reason: StopReason, floor: usize) -> Served {
        self.enter(crate::jit::deoptimize_stopped_at_poll);
        self.tell(Event::Stopped { reason });
        let first_value_id = self
            .debugger
            .as_ref()
            .map_or(0, |debugger| debugger.next_value_id);
        let mut values = StopValues::starting_at(first_value_id);
        let served = loop {
            let Some(debugger) = &self.debugger else {
                // The remote is gone: as a `Continue`.
                break Served::GoOn;
            };
            let Ok(command) = debugger.commands.recv() else {
                self.detach_debugger();
                break Served::GoOn;
            };
            match command {
                Command::Continue => {
                    self.tell(Event::Continued);
                    break Served::GoOn;
                }
                Command::Terminate => break self.terminate(),
                Command::StackTrace => {
                    let frames =
                        self.enter(|mutator| stack::stack_trace(mutator, floor, &mut values));
                    self.tell(Event::StackTrace(frames));
                }
                Command::Variables { frame } => {
                    match self.enter(|mutator| stack::variables(mutator, floor, frame, &mut values))
                    {
                        Some(variables) => self.tell(Event::Variables(variables)),
                        None => self.refuse(&command, NO_SUCH_FRAME),
                    }
                }
                Command::Children {
                    value,
                    start,
                    count,
                } => match self.enter(|mutator| values.children(mutator, value, start, count)) {
                    Some(children) => self.tell(Event::Variables(children)),
                    None => self.refuse(&command, NO_SUCH_VALUE),
                },
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

    fn refuse(&mut self, command: &Command, reason: &str) {
        self.tell(Event::Refused {
            command: String::from(command.name()),
            reason: String::from(reason),
        });
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

    /// The remote is gone: the debugger goes, and with it every bit it
    /// left, which nobody is left to serve.
    /// No attachment is current from here on, under the same lock, so
    /// that a remote of this one still held arms nothing after the bits
    /// are cleared.
    fn detach_debugger(&mut self) {
        let attachments = Arc::clone(&self.debugger_arming.attachments);
        let mut guarded = attachments.lock().unwrap_or_else(PoisonError::into_inner);
        guarded.current = None;
        self.enter(|mutator| {
            mutator
                .heap
                .clear_debugger_bits(DEBUGGER_PAUSE | DEBUGGER_COMMANDS);
        });
        drop(guarded);
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
