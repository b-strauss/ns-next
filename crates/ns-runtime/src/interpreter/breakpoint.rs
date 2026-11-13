//! What the run loop's `Breakpoint` arm does at a patched slot
//! (`docs/design/debugger.md`, "Stopping at and going past a
//! breakpoint"): stop, leaving the stretch for the safepoint, which stops
//! the program there, or go past, executing the slot's original through
//! its ordinary arm. Out of line and cold: only a patched slot reaches it,
//! and `vm.rs` holds no debugger logic. So are the reports of a frame the
//! run loop pops otherwise than by a return to its caller's send, which a
//! step goes on from (`frame_left`; `docs/design/debugger.md`, decision
//! 17), and the unwinder's of a method a method breakpoint names that it
//! leaves (`leaving_by_unwind`, `returning_by_unwind`; "Method
//! breakpoints").

use std::rc::Rc;

use crate::bytecode::{CompactInstruction, CompiledCode, Instruction};
use crate::debugger::breakpoints::{address, AfterPast, Hits, PastOnce, PendingStop, Standing};
use crate::debugger::stepping::{self, Leaving};
use crate::debugger::BreakpointId;
use crate::heap::{ActivationReference, FrameReference};
use crate::interpreter::locals::filled_own_slot;
use crate::interpreter::{Mutator, Signal};
use crate::Value;

/// What the `Breakpoint` arm goes on with.
pub(crate) enum BreakpointDecision {
    /// The slot's original, which the arm runs through its ordinary arm.
    Execute(CompactInstruction),
    /// A stop is recorded: the arm leaves the stretch for the safepoint,
    /// the frame standing at the slot.
    Stop,
}

/// The decision at slot `index` of the body `activation` runs, which a
/// breakpoint the debugger set, the run to cursor or a step's one-shot
/// patched: a stop for a line breakpoint, a method's entry, a method's
/// exit the slot really is -- a lazy slot's getter's first instruction
/// once the slot is filled, a non-local return whose home still runs --
/// the run to cursor, and a one-shot that stops in this activation; the
/// breakpoint stress mode's patch only counts its hit. The
/// token a resumed stop set lets the frame past the slot it stopped at
/// once; this arm consumes it when it names this slot, and drops it
/// otherwise, since the activation it names may be popped from here on
/// and its offset reused. Going past a method's entry that is also its
/// exit stops again for the exit first.
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
            return gone_past(mutator, activation, &code, index, past.after);
        }
    }
    let standing = mutator.breakpoints.kinds_at(body, index);
    // Every patched slot is in the table, which restores a slot as it
    // forgets it.
    debug_assert!(
        !standing.lines.is_empty()
            || !standing.entries.is_empty()
            || !standing.exits.is_empty()
            || !standing.one_shots.is_empty()
            || standing.run_to
            || standing.stress,
        "internal: a patched slot no breakpoint holds"
    );
    // The breakpoint stress mode's patch never stops: it counts, and the
    // slot is gone past as any other that does not stop.
    #[cfg(debug_assertions)]
    if standing.stress {
        mutator.breakpoints.count_stress_hit();
    }
    // A step's one-shots count only in their own activation; a recording
    // one goes on, having noted the value of the call before it.
    let mut step_stops = false;
    for one_shot in &standing.one_shots {
        if !stepping::takes(mutator, one_shot.holder, activation) {
            continue;
        }
        if one_shot.records {
            stepping::record(mutator, activation, &code, index);
        }
        step_stops |= one_shot.stops;
    }
    let (hits, after) = hits_of(mutator, activation, &code, index, standing);
    if hits.only_a_step() && !step_stops {
        return BreakpointDecision::Execute(code.original(index));
    }
    mutator.breakpoints.request_stop(PendingStop {
        slot: PastOnce {
            activation,
            body,
            index,
            after,
        },
        hits,
    });
    BreakpointDecision::Stop
}

/// The frame goes past slot `index`, where it stopped, with `after` what
/// the stop still owes: the exit of a method whose entry it stopped at
/// there, a stop of its own; and, for a stop that reported a method's
/// exit at a non-local return, the home the return arrives at, whose
/// arrival reports it no more (`returning_by_unwind`).
fn gone_past(
    mutator: &mut Mutator<'_>,
    activation: ActivationReference,
    code: &Rc<CompiledCode>,
    index: usize,
    after: AfterPast,
) -> BreakpointDecision {
    match after {
        AfterPast::Nothing => {}
        AfterPast::ExitDue => {
            let standing = mutator.breakpoints.kinds_at(address(code), index);
            let exits = if exit_is_taken(mutator, activation, code, index) {
                standing.exits
            } else {
                Vec::new()
            };
            if !exits.is_empty() {
                mutator.breakpoints.request_stop(PendingStop {
                    slot: PastOnce {
                        activation,
                        body: address(code),
                        index,
                        after: AfterPast::Nothing,
                    },
                    hits: Hits {
                        exits,
                        ..Hits::default()
                    },
                });
                return BreakpointDecision::Stop;
            }
        }
        AfterPast::ExitReported => {
            if let Some(home) = mutator.heap.activation_home(activation) {
                let object = mutator.heap.frame_object(home);
                let root = mutator.heap.stash(Value::object(object));
                mutator.breakpoints.report_home(root);
            }
        }
    }
    BreakpointDecision::Execute(code.original(index))
}

/// The breakpoints the debugger set that `standing` at slot `index` of
/// `code` makes the frame `activation` stop for, and what going past the
/// stop owes: a method's exit is one only where the slot leaves the
/// method (`exit_is_taken`), and one at the method's entry is reported by
/// a stop of its own once the entry's is gone past.
fn hits_of(
    mutator: &mut Mutator<'_>,
    activation: ActivationReference,
    code: &CompiledCode,
    index: usize,
    standing: Standing,
) -> (Hits, AfterPast) {
    let Standing {
        lines,
        entries,
        exits,
        run_to,
        ..
    } = standing;
    let exits = if !exits.is_empty() && exit_is_taken(mutator, activation, code, index) {
        exits
    } else {
        Vec::new()
    };
    let after = if exits.is_empty() {
        AfterPast::Nothing
    } else if !entries.is_empty() {
        AfterPast::ExitDue
    } else if matches!(code.instruction(index), Instruction::NonLocalReturn { .. }) {
        AfterPast::ExitReported
    } else {
        AfterPast::Nothing
    };
    let exits = if after == AfterPast::ExitDue {
        Vec::new()
    } else {
        exits
    };
    (
        Hits {
            lines,
            entries,
            exits,
            run_to,
        },
        after,
    )
}

/// Whether slot `index` of `code`, a return a method breakpoint patched,
/// leaves the method as `activation` runs it: a lazy slot's getter's
/// first instruction only once the slot is filled, since it goes on to
/// compute it otherwise; a non-local return only while its home runs,
/// since it signals `cannotReturn` otherwise; any other return always.
fn exit_is_taken(
    mutator: &mut Mutator<'_>,
    activation: ActivationReference,
    code: &CompiledCode,
    index: usize,
) -> bool {
    match code.instruction(index) {
        Instruction::ReturnSlotIfFilled { own_index } => {
            matches!(filled_own_slot(mutator, activation, own_index), Ok(Some(_)))
        }
        Instruction::NonLocalReturn { .. } => mutator
            .heap
            .activation_home(activation)
            .is_none_or(|home| mutator.heap.frame_is_running(home)),
        _ => true,
    }
}

/// The unwinder is about to pop `top`, for an unwind other than a
/// termination: the method breakpoints whose method it is a body of,
/// whose exit the pop is (`left_by_unwind`). Empty while no method
/// breakpoint is set, the one check the unwinder makes per pop.
pub(crate) fn leaving_by_unwind(
    mutator: &Mutator<'_>,
    top: ActivationReference,
) -> Vec<BreakpointId> {
    if !mutator.breakpoints.has_method_breakpoints() {
        return Vec::new();
    }
    exits_of(mutator, top)
}

/// The method breakpoints whose method `activation` runs.
#[cold]
#[inline(never)]
fn exits_of(mutator: &Mutator<'_>, activation: ActivationReference) -> Vec<BreakpointId> {
    let code = mutator.heap.activation_code(activation);
    mutator.breakpoints.exits_of(&code)
}

/// The unwinder popped a frame of the methods of the method breakpoints
/// `ids` (`leaving_by_unwind`): their exit is reported where the program
/// goes on (`stepping::method_left`).
#[cold]
#[inline(never)]
pub(crate) fn left_by_unwind(mutator: &mut Mutator<'_>, ids: Vec<BreakpointId>) {
    stepping::method_left(mutator, ids, Leaving::Popped);
}

/// A return's unwind arrived at `home`, the top frame, whose frame is
/// `frame`, and returns from it to its caller: if a method breakpoint
/// names its method, and no stop at the non-local return that began the
/// unwind reported the exit already, the exit is reported where the
/// program goes on, the caller's return point (`stepping::method_left`).
pub(crate) fn returning_by_unwind<'gc>(
    mutator: &mut Mutator<'gc>,
    home: ActivationReference,
    frame: FrameReference<'gc>,
) {
    if !mutator.breakpoints.has_method_breakpoints() {
        return;
    }
    report_return(mutator, home, frame);
}

/// `returning_by_unwind`'s report, out of line.
#[cold]
#[inline(never)]
fn report_return<'gc>(
    mutator: &mut Mutator<'gc>,
    home: ActivationReference,
    frame: FrameReference<'gc>,
) {
    if mutator
        .breakpoints
        .returned_home_reported(&mutator.heap, frame)
    {
        return;
    }
    let ids = exits_of(mutator, home);
    if !ids.is_empty() {
        stepping::method_left(mutator, ids, Leaving::Returning(home));
    }
}

/// The breakpoints the debugger set that the top frame `activation`, a
/// frame just pushed that stands at its first statement start, stops for
/// there, where a step into stops it at the safepoint without running
/// its slot; and what going past the stop owes.
pub(crate) fn standing_hits(
    mutator: &mut Mutator<'_>,
    activation: ActivationReference,
) -> (Hits, AfterPast) {
    let code = mutator.heap.activation_code(activation);
    let index = mutator.heap.activation_program_counter(activation);
    if !code.is_patched(index) {
        return (Hits::default(), AfterPast::Nothing);
    }
    let standing = mutator.breakpoints.kinds_at(address(&code), index);
    hits_of(mutator, activation, &code, index, standing)
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
            after: AfterPast::Nothing,
        });
    }
}
