//! What the run loop's `Breakpoint` arm does at a patched slot
//! (`docs/design/debugger.md`, "Stopping at and going past a
//! breakpoint"): stop, leaving the stretch for the safepoint, which stops
//! the program there, or go past, executing the slot's original through
//! its ordinary arm. Out of line and cold: only a patched slot reaches it,
//! and `vm.rs` holds no debugger logic.

use crate::bytecode::CompactInstruction;
use crate::debugger::breakpoints::{address, PastOnce, PendingStop};
use crate::debugger::stepping;
use crate::heap::ActivationReference;
use crate::interpreter::Mutator;

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
