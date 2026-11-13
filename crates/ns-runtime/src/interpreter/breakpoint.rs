//! What the run loop's `Breakpoint` arm does at a patched slot
//! (`docs/design/debugger.md`, "Stopping at and going past a
//! breakpoint"): stop, leaving the stretch for the safepoint, which stops
//! the program there, or go past, executing the slot's original through
//! its ordinary arm. Out of line and cold: only a patched slot reaches it,
//! and `vm.rs` holds no debugger logic. So are the reports of a frame the
//! run loop pops otherwise than by a return to its caller's send, which a
//! step goes on from (`frame_left`; `docs/design/debugger.md`, decision
//! 17).

use crate::bytecode::CompactInstruction;
use crate::debugger::breakpoints::{address, PastOnce, PendingStop};
use crate::debugger::stepping;
use crate::heap::ActivationReference;
use crate::interpreter::{Mutator, Signal};

/// What the `Breakpoint` arm goes on with.
pub(crate) enum BreakpointDecision {
    /// The slot's original, which the arm runs through its ordinary arm.
    Execute(CompactInstruction),
    /// A stop is recorded: the arm leaves the stretch for the safepoint,
    /// the frame standing at the slot.
    Stop,
}

/// The decision at slot `index` of the body `activation` runs, which a
/// line breakpoint or a step's one-shot patched: a stop for a line
/// breakpoint, and for a one-shot that stops in this activation. The
/// token a resumed stop set lets the frame past
/// the slot it stopped at once; this arm consumes it when it names this
/// slot, and drops it otherwise, since the activation it names may be
/// popped from here on and its offset reused.
#[cold]
#[inline(never)]
pub(crate) fn reached(
    mutator: &mut Mutator<'_>,
    activation: ActivationReference,
    index: usize,
) -> BreakpointDecision {
    let code = mutator.heap.activation_code(activation);
    let body = address(&code);
    if let Some(past) = mutator.breakpoints.take_past_once() {
        // The body and the index first: an activation is compared only
        // with one that may be the same.
        if past.body == body && past.index == index && past.activation.same(activation) {
            return BreakpointDecision::Execute(code.original(index));
        }
    }
    let (ids, one_shots) = mutator.breakpoints.kinds_at(body, index);
    // Every patched slot is in the table, which restores a slot as it
    // forgets it.
    debug_assert!(
        !ids.is_empty() || !one_shots.is_empty(),
        "internal: a patched slot no breakpoint holds"
    );
    // A step's one-shots count only in their own activation; a recording
    // one goes on, having noted the value of the call before it.
    let mut step_stops = false;
    for one_shot in one_shots {
        if !stepping::takes(mutator, one_shot.holder, activation) {
            continue;
        }
        if one_shot.records {
            stepping::record(mutator, activation, &code, index);
        }
        step_stops |= one_shot.stops;
    }
    if ids.is_empty() && !step_stops {
        return BreakpointDecision::Execute(code.original(index));
    }
    mutator.breakpoints.request_stop(PendingStop {
        slot: PastOnce {
            activation,
            body,
            index,
        },
        ids,
    });
    BreakpointDecision::Stop
}

/// The run loop popped a frame and goes on otherwise than by handing its
/// value to its caller's send: an unwind it carries on, a superclass
/// clause whose continuation pushes another step, an instruction a
/// future's `resolve` returns to that runs again, the `signal` it is
/// about to raise. Nothing has been pushed or resumed since, so the
/// popped frame is whatever lies above the top. With a step in progress
/// the step learns of it (`stepping::frame_left`), and goes on as a step
/// into from the frame now on top when it was the step's frame -- but for
/// an error about to be signalled, whose handler search decides where the
/// program goes on and reports it, as the unwind it starts does: stepping
/// into `signal` would stop in the exception's machinery, not where the
/// program goes on ("Exceptions"). Out of line and cold: only the
/// continuations other than `Deliver` reach it, and their return path
/// stays as it was.
#[cold]
#[inline(never)]
pub(crate) fn frame_left<'gc>(mutator: &mut Mutator<'gc>, signal: Option<&Signal<'gc>>) {
    if mutator.stepping.is_none() {
        return;
    }
    let now_on_top = match signal {
        Some(Signal::Error(_)) => None,
        _ => mutator.heap.top_activation(),
    };
    stepping::frame_left(mutator, now_on_top);
}

/// The run loop popped a frame whose sender is suspended into its frame
/// and about to be resumed onto the stack, where it may take the popped
/// frame's offset: with a step in progress, the step forgets every name
/// it knew the popped frame by first (`stepping::popped`), so that none
/// names the resumed sender. Its continuation then decides, as any popped
/// frame's (`frame_left`).
#[cold]
#[inline(never)]
pub(crate) fn returning_to_suspended(mutator: &mut Mutator<'_>) {
    if mutator.stepping.is_some() {
        stepping::popped(mutator);
    }
}

/// The caller `activation` runs its instruction at `index` again, which
/// met a future not resolved yet (`Continuation::Reexecute`): if a
/// breakpoint patched it, the frame went past it before, and goes past
/// it once more, so that the breakpoint does not stop twice.
pub(crate) fn reexecuting(
    mutator: &mut Mutator<'_>,
    activation: ActivationReference,
    index: usize,
) {
    let code = mutator.heap.activation_code(activation);
    if code.is_patched(index) {
        mutator.breakpoints.go_past_once(PastOnce {
            activation,
            body: address(&code),
            index,
        });
    }
}
