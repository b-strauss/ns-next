//! Restarting a frame and forcing it to return at a stop
//! (`docs/design/debugger.md`, "Restart frame and force return"). The
//! stop names the frame -- and, for a forced return, the value -- and
//! begins the step that makes the next stop; it is then left, and the run
//! loop raises the unwind it started from the top activation through the
//! one unwinder (`Served::Unwind`), which runs every `ensure:` and
//! `ifCurtailed:` block above the frame and arrives at it whatever it
//! runs (`interpreter::unwind`): no Rust frame waits for the unwind, and
//! nothing but the unwinder pops.

use super::evaluation::SHIPPED_FRAME;
use super::stack;
use super::stepping::{self, StepAction};
use super::{Command, NO_SUCH_FRAME, NO_SUCH_VALUE, Stop};
use crate::interpreter::{Mutator, Signal};
use crate::types::{PendingUnwind, UnwindAction};
use crate::{Root, Value};

/// Why a value given for an initializer's forced return is refused: an
/// initializer answers its instance, as its end does.
pub(crate) const INITIALIZER_RETURNS_SELF: &str = "an initializer returns self";

/// An unwind a stop started, held by the debugger from the command until
/// the run loop raises it (`Interpreter::take_started_unwind`): its frame
/// and what it does there, rooted, since the stop's handles do not outlive
/// the stop.
pub(crate) struct StartedUnwind {
    /// The frame object of the frame the unwind ends at.
    frame: Root,
    action: StartedAction,
}

/// What a `StartedUnwind` does at its frame.
enum StartedAction {
    Restart,
    ForceReturn(Root),
}

impl StartedUnwind {
    /// The signal that carries the unwind, raised in the top activation.
    pub(crate) fn signal<'gc>(&self, mutator: &Mutator<'gc>) -> Signal<'gc> {
        let heap = &mutator.heap;
        let target = heap
            .fetch(&self.frame)
            .as_object()
            .and_then(|object| heap.object_frame(object))
            .expect("internal: a started unwind names a frame object");
        let action = match &self.action {
            StartedAction::Restart => UnwindAction::RestartFrame,
            StartedAction::ForceReturn(value) => UnwindAction::ForceReturn(heap.fetch(value)),
        };
        Signal::Unwind(Box::new(PendingUnwind {
            target: Some(target),
            action,
        }))
    }
}

/// Starts what `command` -- a `RestartFrame` or a `ForceReturn` -- asks of
/// the frame it names of `stop`'s stack: the step that makes the next stop
/// is begun -- a restarted frame flooded for its first statement start
/// (`stepping::stop_at_restart`), a frame forced to return stepped out of,
/// so that the stop is where its sender goes on -- and the unwind is
/// answered for the run loop to raise. Refused, with the reason and
/// nothing changed, for a frame index past the stack, a frame of the core
/// library -- forcing `signal` or an `ensure:` marker to return would
/// break the kernel's own invariants -- a value the stop did not hand
/// out, a withheld one, which the frame's sender would hand the program
/// (`docs/design/debugger.md`, "Marking objects"), and a value given for
/// an initializer's frame.
pub(crate) fn start(
    mutator: &mut Mutator<'_>,
    stop: &mut Stop,
    command: &Command,
) -> Result<StartedUnwind, String> {
    let (frame, forced) = match command {
        Command::RestartFrame { frame } => (*frame, None),
        Command::ForceReturn { frame, value } => (*frame, Some(*value)),
        _ => unreachable!("internal: only a `RestartFrame` and a `ForceReturn` unwind"),
    };
    let frames = stop.frames(mutator);
    let stack_frame = frames.get(frame).ok_or(NO_SUCH_FRAME)?;
    if stack::is_shipped(mutator, stack_frame) {
        return Err(String::from(SHIPPED_FRAME));
    }
    let action = match forced {
        None => StartedAction::Restart,
        Some(None) => StartedAction::ForceReturn(mutator.heap.stash(Value::nil())),
        Some(Some(_)) if stack::is_initializer(&mutator.heap, stack_frame) => {
            return Err(String::from(INITIALIZER_RETURNS_SELF));
        }
        Some(Some(id)) => {
            let (value, withheld) = stop.values.fetch(mutator, id).ok_or(NO_SUCH_VALUE)?;
            if withheld {
                return Err(String::from(super::WITHHELD));
            }
            StartedAction::ForceReturn(mutator.heap.stash(value))
        }
    };
    // Read again: the value's lookup borrowed the stop.
    let frames = stop.frames(mutator);
    let stack_frame = frames.get(frame).ok_or(NO_SUCH_FRAME)?;
    match action {
        StartedAction::Restart => stepping::stop_at_restart(mutator, stack_frame),
        StartedAction::ForceReturn(_) => {
            stepping::begin(mutator, StepAction::Out, Some(stack_frame));
            // The frame the stop stands in leaves with the unwind: the slot
            // it stands at is never fetched again for the step to go past.
            mutator.breakpoints.take_past_once();
        }
    }
    let frame = stack::reified(&mut mutator.heap, stack_frame);
    let frame = mutator
        .heap
        .stash(Value::object(mutator.heap.frame_object(frame)));
    Ok(StartedUnwind { frame, action })
}
