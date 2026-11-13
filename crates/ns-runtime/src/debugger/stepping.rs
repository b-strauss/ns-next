//! Stepping by one-shot breakpoints and the safepoint
//! (`docs/design/debugger.md`, decision 3, "Step over in recursion and
//! blocks", "Step into's held request", "The return value of the last
//! call"). A step over floods the stepping frame's body -- every
//! statement start of it and of the blocks the compiler lowered inline in
//! it -- with one-shot breakpoints for that frame, and sets one at its
//! return point, the index after the instruction its caller stands at; a
//! step out sets only the return point's; a step into is a step over
//! whose request bit (`DEBUGGER_STEP`) every poll takes until a frame is
//! pushed above the stepping one, which stops at its first statement
//! start. The one-shots are a kind of the breakpoint table's, patched and
//! restored as a line breakpoint is, so that their bodies lose their
//! machine code for the step; the next stop, whatever made it, ends the
//! step. No check enters the run loop: what the step does not patch runs
//! at full speed. A step never lands in a frame the step filters filter
//! -- by default the shipped modules' (decision 16): a stop it would make
//! there goes on as a step into from the first unfiltered frame below,
//! and a step into's request outlasts the pushes of filtered bodies. A
//! method an unwind leaves while a method breakpoint names it is reported
//! by a step of the same kind (`method_left`), whose stop reports the
//! method's exit where the program goes on.

use std::rc::Rc;
use std::sync::PoisonError;

use super::breakpoints::{address, settle, AfterPast, OneShot, PastOnce};
use super::metadata::again::{declaration_path, BodyOrigin};
use super::metadata::{BodyMetadata, Cache};
use super::remote::{BreakpointId, ClassName};
use super::stack::{self, StackFrame};
use crate::bytecode::{register_written, CompiledCode, Instruction};
use crate::heap::{ActivationReference, FrameReference, DEBUGGER_STEP};
use crate::interpreter::Mutator;
use crate::{Root, Value};

/// The frames no step stops in (`docs/design/debugger.md`, decision 16):
/// the shipped modules' bodies while `shipped` holds, and the bodies of
/// each class declaration `classes` names and of the declarations nested
/// in it. The default, until the agent sets them, filters the shipped
/// modules and no class.
#[derive(Clone)]
pub(crate) struct StepFilters {
    pub(crate) shipped: bool,
    pub(crate) classes: Vec<ClassName>,
}

impl Default for StepFilters {
    fn default() -> StepFilters {
        StepFilters {
            shipped: true,
            classes: Vec::new(),
        }
    }
}

/// What the agent asked a step to do.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum StepAction {
    /// To the next statement start of the stepping frame, or its
    /// caller's return point.
    Over,
    /// As `Over`, or to the first statement start of a frame pushed
    /// meanwhile.
    Into,
    /// To the stepping frame's return point.
    Out,
}

/// A frame the step names: a running activation, or one suspended into
/// its frame object -- by the suspend stress mode, or a sender an image
/// decoded -- by its key among the step's suspended frames, until it is
/// resumed as an activation of its own (`resumed`); or a frame popped
/// otherwise than by a return to its caller, which no running activation
/// is (`popped`).
#[derive(Clone, Copy)]
pub(crate) enum Holder {
    Running(ActivationReference),
    Suspended(u32),
    Left,
}

impl Holder {
    /// Whether the two name the same frame.
    pub(crate) fn is(self, other: Holder) -> bool {
        match (self, other) {
            (Holder::Running(held), Holder::Running(other)) => held.names(other),
            (Holder::Suspended(held), Holder::Suspended(other)) => held == other,
            (Holder::Left, Holder::Left) => true,
            _ => false,
        }
    }
}

/// The step in progress, on the `Mutator` until the next stop ends it.
/// Its handles name activations of the run, which it holds no longer
/// than the step: a position on the activation stack is reused only
/// after its frame's pop, which the return point's one-shot stops at
/// first, and the step ends there.
pub(crate) struct Stepping {
    action: StepAction,
    /// The frame the step steps in.
    frame: Holder,
    /// The frames between the one the step was taken in and the stepping
    /// frame, its creator, when the step was taken in a block the
    /// compiler lowered inline: the receiver's `ifTrue:` that runs a
    /// fallback block, with their positions while they run. They are on
    /// the stack already, so none of them is a push of the step's; each
    /// goes from the list once the top lies below it, so that a frame the
    /// stepping frame pushes later at its offset is one.
    between: Vec<(Holder, Option<usize>)>,
    /// Every body whose statement starts the step patched for the
    /// stepping frame: its own and its inlined blocks'.
    flooded: Vec<Rc<CompiledCode>>,
    /// The caller and the index after the instruction it stands at, where
    /// the stepping frame's return goes on.
    return_point: Option<(Holder, usize)>,
    /// The value the stepping frame's last call answered, recorded at the
    /// return point after it (`record`).
    returned: Option<Root>,
    /// The frame object of each suspended frame the step names, by its
    /// key (`Holder::Suspended`).
    suspended: Vec<(u32, Root)>,
    /// The key the next suspended frame takes.
    next_key: u32,
    /// Whether the step was begun in a frame the step filters filter,
    /// which it then steps in as in any other (`is_filtered`); decided as
    /// the step begins, whatever filters are set while it runs.
    started_filtered: bool,
    /// The method breakpoints whose method an unwind left since the step
    /// began (`method_left`), which its stop reports as the method's exit.
    exits: Vec<BreakpointId>,
    /// The frame on top when the step's frame left and an error was about
    /// to be signalled (`frame_left` with no frame to go on in): the
    /// exception's machinery runs above it until the handler search
    /// reports, which replaces the step. For the step stress mode's check.
    #[cfg(debug_assertions)]
    waiting_on: Option<ActivationReference>,
}

/// A frame of the run: an activation on the stack, or one suspended into
/// its frame object.
#[derive(Clone, Copy)]
enum Place<'gc> {
    Running(ActivationReference),
    Suspended(FrameReference<'gc>),
}

/// What the safepoint goes on with while a step into holds its request.
pub(crate) enum HeldStep {
    /// Nothing was pushed, or the push stops at a one-shot.
    GoOn,
    /// A push: the program stops in the pushed frame, where it stands.
    Stop,
}

/// Begins a step of `action` in the frame a stop stands in: the top
/// activation, or `shown`, the first frame an exception's stop shows
/// below the exception's machinery (`exceptions::first_shown`), which the
/// step steps from as if it had stopped there, the machinery's frames above
/// it frames the step was taken above. Floods that frame, sets its return
/// point and, for a step into, holds the request bit. The program then
/// goes on as after a `Continue`. Whether the step starts in a frame the
/// step filters filter is decided here, once: a `SetStepFilters` served
/// while the step runs changes where it lands, not that.
pub(crate) fn begin(mutator: &mut Mutator<'_>, action: StepAction, shown: Option<&StackFrame>) {
    end(mutator);
    let Some(top) = mutator.heap.top_activation() else {
        return;
    };
    let taken_in = shown
        .and_then(|frame| place_of(mutator, frame))
        .unwrap_or(Place::Running(top));
    let started_filtered = is_filtered(mutator, shown.unwrap_or(&StackFrame::Running(top)));
    start(mutator, action, Place::Running(top), taken_in, None);
    if let Some(stepping) = &mut mutator.stepping {
        stepping.started_filtered = started_filtered;
    }
    settle(mutator);
    // The top frame fetches the slot it stands at again as it goes on,
    // which the flood may just have patched -- a pause at a push leaves a
    // callee at its first statement start, one at a backward jump at its
    // loop's head: the step lets it past once, so that it stops at the
    // next place, not where it stands.
    let code = mutator.heap.activation_code(top);
    let standing = PastOnce {
        activation: top,
        body: address(&code),
        index: mutator.heap.activation_program_counter(top),
        after: AfterPast::Nothing,
    };
    mutator.breakpoints.go_past_once(standing);
}

/// The frame of the run `frame` names at a stop.
fn place_of<'gc>(mutator: &Mutator<'gc>, frame: &StackFrame) -> Option<Place<'gc>> {
    match frame {
        StackFrame::Running(activation) => Some(Place::Running(*activation)),
        StackFrame::Suspended(root) => {
            let heap = &mutator.heap;
            heap.fetch(root)
                .as_object()
                .and_then(|object| heap.object_frame(object))
                .map(Place::Suspended)
        }
    }
}

/// Ends the step, if one is in progress: every one-shot is removed, each
/// slot no line breakpoint holds given its original back, and the
/// request bit dropped. Answers the value the step recorded last.
pub(crate) fn end(mutator: &mut Mutator<'_>) -> Option<Root> {
    let stepping = mutator.stepping.take();
    mutator.breakpoints.remove_one_shots();
    drop_request(mutator);
    settle(mutator);
    stepping.and_then(|stepping| stepping.returned)
}

/// A frame was just popped otherwise than by a return to its caller's
/// send (`Continuation::Deliver`), and nothing has been pushed or resumed
/// since: every frame the step names that lies above the top of the
/// stack is gone, and every name the step knew it by is `Holder::Left`
/// from here on, which no activation is; a frame between the one the step
/// was taken in and the stepping frame goes from that list. Done before
/// anything takes its place on the stack, since an activation pushed or
/// resumed there may take its offset, which is all a release build
/// compares. The popped frame is found by its offset rather than handed
/// in, so that the return path keeps no handle alive across its pop.
pub(crate) fn popped(mutator: &mut Mutator<'_>) {
    let Some(stepping) = &mut mutator.stepping else {
        return;
    };
    let heap = &mutator.heap;
    let gone = |holder: Holder| matches!(holder, Holder::Running(activation) if heap.activation_lies_above_the_top(activation));
    if gone(stepping.frame) {
        stepping.frame = Holder::Left;
    }
    if let Some((caller, index)) = stepping.return_point {
        if gone(caller) {
            stepping.return_point = Some((Holder::Left, index));
        }
    }
    stepping.between.retain(|(held, _)| !gone(*held));
    mutator.breakpoints.forget_one_shot_holders(gone);
}

/// A frame popped otherwise than by a return to its caller's send, and
/// `now_on_top`, the frame the program goes on in (decision 17): the
/// popped frame's names are forgotten first (`popped`). When it was the
/// stepping frame, or the caller its return point is in, the step goes on
/// as a step into from `now_on_top`, so that it stops at the next
/// statement start the program reaches: in the frame an unwind goes on
/// in, a block it runs there, the caller an instruction runs again in, or
/// what that pushes. Any other frame the step named above the stepping
/// one -- one the step was taken in, a pushed frame's one-shot -- is only
/// forgotten. `now_on_top` is `None` when no frame goes on yet -- the run
/// ends, whose end ends the step, or an error is about to be signalled,
/// whose handler search, or the unwind it starts, says where the program
/// goes on; the step's frame stays `Holder::Left` until then.
pub(crate) fn frame_left(mutator: &mut Mutator<'_>, now_on_top: Option<ActivationReference>) {
    popped(mutator);
    let Some(stepping) = &mutator.stepping else {
        return;
    };
    let left = stepping.frame.is(Holder::Left)
        || stepping
            .return_point
            .is_some_and(|(caller, _)| caller.is(Holder::Left));
    match (left, now_on_top) {
        (true, Some(frame)) => continue_as_step_into(mutator, frame),
        #[cfg(debug_assertions)]
        (true, None) => {
            let top = mutator.heap.top_activation();
            if let Some(stepping) = &mut mutator.stepping {
                stepping.waiting_on = top;
            }
        }
        _ => {}
    }
}

/// Goes on with the step as a step into from `frame`, the frame the
/// program goes on in, or from the first frame below it the step filters
/// do not filter (decision 16): its one-shots so far removed, that frame
/// flooded and its return point set, and the request bit held, so that
/// the step stops at the next statement start of that frame or of an
/// unfiltered frame it pushes, itself or through filtered ones -- a
/// block's next activation by the shipped `do:` above it. With no
/// unfiltered frame below, the step goes on from `frame`, and every stop
/// it would make in a filtered one goes on again. The value recorded so
/// far is kept.
pub(crate) fn continue_as_step_into(mutator: &mut Mutator<'_>, frame: ActivationReference) {
    let (returned, exits) = carried(mutator);
    mutator.breakpoints.remove_one_shots();
    let place = first_unfiltered(mutator, frame).unwrap_or(Place::Running(frame));
    start(mutator, StepAction::Into, place, place, returned);
    keep_exits(mutator, exits);
    settle(mutator);
}

/// The handler search found the handler of the `on:do:` activation
/// `handler`, which is about to run on top of the stack, before anything
/// unwinds ("Exceptions"): a step whose frame stands at or above
/// `handler` -- the handler takes an exception raised in it or in what it
/// called -- or whose frame, or its return point's caller, has left
/// already, waiting for where the error's handling takes the program
/// (decision 17), goes on as a step into from the frame on top, through
/// the exception's machinery, which the filters pass, so that it stops at
/// the handler block's first statement, as Java's step over stops in the
/// `catch` that takes an exception thrown below. A step whose frame lies
/// below `handler` -- the `on:do:` was sent above it -- goes on as it was.
pub(crate) fn handler_found<'gc>(mutator: &mut Mutator<'gc>, handler: FrameReference<'gc>) {
    let Some(stepping) = &mutator.stepping else {
        return;
    };
    let left = stepping.frame.is(Holder::Left)
        || stepping
            .return_point
            .is_some_and(|(caller, _)| caller.is(Holder::Left));
    let Some(top) = mutator.heap.top_activation() else {
        return;
    };
    if left || steps_at_or_above(mutator, top, handler) {
        continue_as_step_into(mutator, top);
    }
}

/// Whether the stepping frame is met walking from `top` down its senders,
/// through the suspended ones, before `handler`'s frame is passed:
/// `handler` itself counts as met.
fn steps_at_or_above<'gc>(
    mutator: &Mutator<'gc>,
    top: ActivationReference,
    handler: FrameReference<'gc>,
) -> bool {
    let Some(stepping) = &mutator.stepping else {
        return false;
    };
    let heap = &mutator.heap;
    let handler_object = heap.frame_object(handler);
    let mut place = Some(Place::Running(top));
    while let Some(next) = place {
        let (held, is_handler) = match next {
            Place::Running(activation) => (
                Some(Holder::Running(activation)),
                heap.frame_activation(handler)
                    .is_some_and(|running| running.names(activation)),
            ),
            Place::Suspended(frame) => (
                suspended_key(mutator, frame).map(Holder::Suspended),
                heap.frame_object(frame).same(handler_object),
            ),
        };
        if held.is_some_and(|held| stepping.frame.is(held)) {
            return true;
        }
        if is_handler {
            return false;
        }
        place = match next {
            Place::Running(activation) => sender_of(mutator, activation),
            Place::Suspended(frame) => heap.frame_sender(frame).map(Place::Suspended),
        };
    }
    false
}

/// The step's value of the last call and the exits it reports, taken with
/// the step, which a step that replaces it carries on.
fn carried(mutator: &mut Mutator<'_>) -> (Option<Root>, Vec<BreakpointId>) {
    mutator
        .stepping
        .take()
        .map_or((None, Vec::new()), |stepping| {
            (stepping.returned, stepping.exits)
        })
}

/// The step in progress reports `exits` too.
fn keep_exits(mutator: &mut Mutator<'_>, exits: Vec<BreakpointId>) {
    if let Some(stepping) = &mut mutator.stepping {
        stepping.exits.extend(exits);
    }
}

/// How an unwind leaves a method a method breakpoint names.
pub(crate) enum Leaving {
    /// The unwind popped its frame, and the program goes on in the frame
    /// now on top, if any.
    Popped,
    /// The unwind arrived at it, the home of a non-local return or the
    /// target of another, and it returns to its caller from `home`, the
    /// top frame.
    Returning(ActivationReference),
}

/// An unwind left a method of the method breakpoints `ids`, which it
/// passed no return of (`docs/design/debugger.md`, "Method breakpoints"):
/// the exit is reported at the next place the program reaches, by a step
/// of the same kind as a step whose frame an unwind pops (decision 17),
/// which replaces any step in progress, its value of the last call kept
/// -- a step into from the frame now on top for a popped frame, a step out
/// from the method's frame for one that returns -- and whose stop reports
/// the exits (`take_exits`), or an exception's stop served first, beside
/// its exception.
pub(crate) fn method_left(mutator: &mut Mutator<'_>, ids: Vec<BreakpointId>, leaving: Leaving) {
    match leaving {
        Leaving::Popped => {
            let Some(now_on_top) = mutator.heap.top_activation() else {
                return;
            };
            continue_as_step_into(mutator, now_on_top);
        }
        Leaving::Returning(home) => {
            let (returned, exits) = carried(mutator);
            mutator.breakpoints.remove_one_shots();
            let home = Place::Running(home);
            start(mutator, StepAction::Out, home, home, returned);
            keep_exits(mutator, exits);
            settle(mutator);
        }
    }
    keep_exits(mutator, ids);
}

/// Whether the step in progress knows where its frame is: a running
/// activation of the stack, a frame suspended into its object, which the
/// step holds, a frame that returned to its caller, which stands on top at
/// the step's return point it has yet to fetch (`at_return_point`), or a
/// frame whose pop was reported, `Holder::Left`. A frame popped otherwise
/// than by a return to its caller is renamed `Left` as the pop is
/// reported, and the step goes on from the frame the program goes on in
/// at once -- but for an error the popped frame's continuation raised,
/// for whose handler search's report, or the unwind its default action
/// starts, the step waits, `Left`, while the exception's machinery runs.
/// A running name no live activation answers to is a pop no report ever
/// reached: the step could never stop. For the step stress mode's check
/// as it ends a step itself.
#[cfg(debug_assertions)]
pub(crate) fn frame_still_there(mutator: &Mutator<'_>) -> bool {
    let Some(stepping) = &mutator.stepping else {
        return true;
    };
    match stepping.frame {
        Holder::Running(held) => {
            let mut next = mutator.heap.top_activation();
            while let Some(live) = next {
                if held.names(live) {
                    return true;
                }
                next = mutator.heap.activation_sender(live);
            }
            at_return_point(mutator)
        }
        Holder::Suspended(_) | Holder::Left => true,
    }
}

/// Whether a step waiting, `Left`, for the handler search's report of an
/// error still has the error's machinery running: the frame on top when
/// the error was raised is still running, and is not the top, so frames
/// of the search lie above it. A waiting step whose frame went back to
/// the top, or away, without a report replacing the step is one whose
/// report never came. For the step stress mode's check as it ends a step
/// itself.
#[cfg(debug_assertions)]
pub(crate) fn still_waiting_for_a_handler(mutator: &Mutator<'_>) -> bool {
    let Some(stepping) = &mutator.stepping else {
        return true;
    };
    let (Holder::Left, Some(waiting_on)) = (stepping.frame, stepping.waiting_on) else {
        return true;
    };
    let Some(top) = mutator.heap.top_activation() else {
        return false;
    };
    if waiting_on.names(top) {
        return false;
    }
    let mut next = mutator.heap.activation_sender(top);
    while let Some(live) = next {
        if waiting_on.names(live) {
            return true;
        }
        next = mutator.heap.activation_sender(live);
    }
    false
}

/// Whether the step's frame has returned to its caller, which stands on
/// top at the step's return point, the slot it fetches next: a return is
/// a safepoint, so the step stops at the very next fetch.
#[cfg(debug_assertions)]
pub(crate) fn at_return_point(mutator: &Mutator<'_>) -> bool {
    let Some(stepping) = &mutator.stepping else {
        return false;
    };
    let Some((Holder::Running(caller), index)) = stepping.return_point else {
        return false;
    };
    mutator.heap.top_activation().is_some_and(|top| {
        caller.names(top) && mutator.heap.activation_program_counter(top) == index
    })
}

/// The method breakpoints whose exit by an unwind the step in progress
/// reports, taken: its stop is the method's exit.
pub(crate) fn take_exits(mutator: &mut Mutator<'_>) -> Vec<BreakpointId> {
    mutator
        .stepping
        .as_mut()
        .map(|stepping| std::mem::take(&mut stepping.exits))
        .unwrap_or_default()
}

/// The step's own one-shot stopped `activation`, the top frame, where no
/// line breakpoint stands: answers whether the step goes on instead,
/// because the step filters filter the frame (decision 16) -- a return
/// point inside the shipped `Array>>do:` after a step over at a block's
/// last statement, a statement of a filtered body an unwind goes on in
/// -- as a step into from the first unfiltered frame below
/// (`continue_as_step_into`). A frame the step was begun in, stopped
/// there by a breakpoint or with the filter off, is stepped in as any
/// other, and so is a block the compiler lowered inline in it.
pub(crate) fn went_on_past_filtered(
    mutator: &mut Mutator<'_>,
    activation: ActivationReference,
) -> bool {
    let Some(stepping) = &mutator.stepping else {
        return false;
    };
    let (frame, started_filtered) = (stepping.frame, stepping.started_filtered);
    if started_filtered && takes(mutator, frame, activation) {
        return false;
    }
    if !is_filtered(mutator, &StackFrame::Running(activation)) {
        return false;
    }
    continue_as_step_into(mutator, activation);
    true
}

/// Whether the step filters filter `frame` (decision 16): its body is a
/// member, or a block of a member, of a declaration of a shipped module
/// -- its module's root is shipped, as for its metadata -- while the
/// shipped filter is on, or of a declaration a filtered class name names
/// or one nested in it. A frame of no declaration -- the top level -- is
/// never filtered. Decided from the frame, not its body alone, which
/// names no mixin: the declaration is the one the frame's current class
/// or its initializer's level says (`stack::origin`).
pub(crate) fn is_filtered(mutator: &Mutator<'_>, frame: &StackFrame) -> bool {
    let filters = mutator.breakpoints.step_filters();
    if !filters.shipped && filters.classes.is_empty() {
        return false;
    }
    let Some(origin) = stack::origin(&mutator.heap, frame) else {
        return false;
    };
    let Some((root, path)) = declaration_path(mutator, &origin.mixin) else {
        return false;
    };
    (filters.shipped && root.shipped)
        || filters.classes.iter().any(|class| {
            class.module == root.name.as_str()
                && class.path.len() <= path.len()
                && class
                    .path
                    .iter()
                    .zip(&path)
                    .all(|(filtered, declared)| filtered == declared.as_str())
        })
}

/// The first frame from `from` down its senders, through the suspended
/// ones, that the step filters do not filter; `None` when every one is.
fn first_unfiltered<'gc>(mutator: &Mutator<'gc>, from: ActivationReference) -> Option<Place<'gc>> {
    let heap = &mutator.heap;
    let mut next = Some(from);
    let mut suspended = None;
    while let Some(live) = next {
        if !is_filtered(mutator, &StackFrame::Running(live)) {
            return Some(Place::Running(live));
        }
        if let Some(sender) = heap.activation_suspended_sender(live) {
            suspended = Some(sender);
            break;
        }
        next = heap.activation_sender(live);
    }
    while let Some(frame) = suspended {
        let root = heap.stash(Value::object(heap.frame_object(frame)));
        if !is_filtered(mutator, &StackFrame::Suspended(root)) {
            return Some(Place::Suspended(frame));
        }
        suspended = heap.frame_sender(frame);
    }
    None
}

/// The safepoint of a step into that holds its request: a frame on top
/// that the stepping frame, or a block the step flooded for it, sent to
/// -- its sender is one of them -- is a push, and the request goes; not a
/// frame with a `program_counter` of 0, which a backward jump to the
/// stepping body's first instruction, or a frame `retry` restarted, also
/// shows, nor merely a frame above the stepping one: a step taken in a
/// block the compiler lowered inline steps in the block's creator, and
/// the frames between the two -- the receiver's `ifTrue:`, a
/// `whileTrue:`'s -- and whatever they send to are not the step's. A
/// flooded block itself stands for the stepping frame's inline code and
/// is no push either, nor a frame between the one the step was taken in
/// and the stepping frame, which was there already. The push stops at the
/// pushed body's first statement start: here, where its frame stands at
/// index 0, or at a one-shot at index 1 for a body whose first
/// instruction makes its indirection vector (`MakeVector`, emitted before
/// every statement). Anything else -- a backward jump, a caller resumed
/// -- goes on without stopping.
pub(crate) fn held_step(mutator: &mut Mutator<'_>) -> HeldStep {
    let Some(stepping) = &mutator.stepping else {
        drop_request(mutator);
        return HeldStep::GoOn;
    };
    let Some(top) = mutator.heap.top_activation() else {
        return HeldStep::GoOn;
    };
    if stepping.action != StepAction::Into {
        return HeldStep::GoOn;
    }
    let top_position = mutator.heap.activation_position(top);
    if let Some(stepping) = &mut mutator.stepping {
        stepping
            .between
            .retain(|(_, position)| position.is_none_or(|position| position <= top_position));
    }
    let Some(stepping) = &mutator.stepping else {
        return HeldStep::GoOn;
    };
    let was_there = stepping
        .between
        .iter()
        .any(|(held, _)| held.is(Holder::Running(top)));
    if was_there || inline_code_of_step(mutator, top) {
        return HeldStep::GoOn;
    }
    // A filtered body keeps the request: the step goes on through it to
    // the next unfiltered body it pushes, itself or through others.
    if is_filtered(mutator, &StackFrame::Running(top)) || !sent_by_step(mutator, top) {
        return HeldStep::GoOn;
    }
    let code = mutator.heap.activation_code(top);
    drop_request(mutator);
    let first = usize::from(
        code.instruction_count() > 1
            && matches!(code.instruction(0), Instruction::MakeVector { .. }),
    );
    if mutator.heap.activation_program_counter(top) >= first {
        return HeldStep::Stop;
    }
    if let Some(stepping) = &mut mutator.stepping {
        // The value the stepping frame's last call answered is not the
        // pushed frame's.
        stepping.returned = None;
    }
    patch(
        mutator,
        &code,
        first,
        OneShot {
            holder: Holder::Running(top),
            stops: true,
            records: false,
        },
    );
    settle(mutator);
    HeldStep::GoOn
}

/// Whether the step's frame, or a block the step flooded for it, sent to
/// `top`: its sender is one of them, or a frame the step filters filter
/// whose sender is, and so on down -- the shipped `do:` the stepping
/// frame sent, which runs a block of the program. A frame between the
/// one the step was taken in and the stepping frame, and an unfiltered
/// frame, end the walk: what they send is not the step's.
fn sent_by_step(mutator: &Mutator<'_>, top: ActivationReference) -> bool {
    let Some(stepping) = &mutator.stepping else {
        return false;
    };
    let heap = &mutator.heap;
    let between = |held: Holder| stepping.between.iter().any(|(other, _)| other.is(held));
    let Some(mut sender) = sender_of(mutator, top) else {
        return false;
    };
    loop {
        sender = match sender {
            Place::Running(activation) => {
                let held = Holder::Running(activation);
                if stepping.frame.is(held) || inline_code_of_step(mutator, activation) {
                    return true;
                }
                if between(held) || !is_filtered(mutator, &StackFrame::Running(activation)) {
                    return false;
                }
                match sender_of(mutator, activation) {
                    Some(sender) => sender,
                    None => return false,
                }
            }
            Place::Suspended(frame) => {
                if let Some(key) = suspended_key(mutator, frame) {
                    let held = Holder::Suspended(key);
                    if stepping.frame.is(held) {
                        return true;
                    }
                    if between(held) {
                        return false;
                    }
                }
                let root = heap.stash(Value::object(heap.frame_object(frame)));
                if !is_filtered(mutator, &StackFrame::Suspended(root)) {
                    return false;
                }
                match heap.frame_sender(frame) {
                    Some(sender) => Place::Suspended(sender),
                    None => return false,
                }
            }
        };
    }
}

/// The frame `activation` returns to: its sender on the stack, or the
/// one suspended into its frame object.
fn sender_of<'gc>(mutator: &Mutator<'gc>, activation: ActivationReference) -> Option<Place<'gc>> {
    let heap = &mutator.heap;
    match heap.activation_suspended_sender(activation) {
        Some(frame) => Some(Place::Suspended(frame)),
        None => heap.activation_sender(activation).map(Place::Running),
    }
}

/// Whether `activation` runs a block the step flooded for the stepping
/// frame, which stands for that frame's inline code.
fn inline_code_of_step(mutator: &Mutator<'_>, activation: ActivationReference) -> bool {
    let Some(stepping) = &mutator.stepping else {
        return false;
    };
    let code = mutator.heap.activation_code(activation);
    stepping
        .flooded
        .iter()
        .any(|flooded| Rc::ptr_eq(flooded, &code))
        && takes(mutator, stepping.frame, activation)
}

/// Whether a one-shot for `holder` is taken in `running`: in that
/// activation, or in a block's whose creator it is -- the closure's
/// captured frames lead to it -- since only the blocks the compiler
/// lowered inline in it are flooded for it, so that a step over a
/// non-Boolean receiver's `ifTrue:` stops in its fallback block as in the
/// inline copy. Ignored in any other activation of the body: a recursive
/// call, or a block of the same home running in a callee.
pub(crate) fn takes(mutator: &Mutator<'_>, holder: Holder, running: ActivationReference) -> bool {
    if holder.is(Holder::Running(running)) {
        return true;
    }
    let heap = &mutator.heap;
    let mut captured = heap.activation_captured(running);
    while let Some(frame) = captured {
        let creator = match heap.frame_activation(frame) {
            Some(creator) => Some(Holder::Running(creator)),
            None => suspended_key(mutator, frame).map(Holder::Suspended),
        };
        if creator.is_some_and(|creator| holder.is(creator)) {
            return true;
        }
        captured = heap.frame_captured(frame);
    }
    false
}

/// Records the value the instruction before slot `index` of `code`
/// answered into `activation`'s register, when it is a call -- a send, a
/// `ResolveFuture`, an initializer invocation or a `SendMirror` -- whose
/// destination is live there: the value of the last call, which the
/// step's stop reports. A register the leftover analysis finds tainted
/// before `index` -- reached by a jump that skipped the call -- is not
/// read.
pub(crate) fn record(
    mutator: &mut Mutator<'_>,
    activation: ActivationReference,
    code: &CompiledCode,
    index: usize,
) {
    let Some(previous) = index.checked_sub(1) else {
        return;
    };
    let Some(register) = call_destination(&code.instruction(previous)) else {
        return;
    };
    let Ok(program_counter) = u32::try_from(index) else {
        return;
    };
    if code
        .tainted_before(program_counter)
        .iter()
        .any(|range| range.contains(&register))
    {
        return;
    }
    let value = mutator.heap.activation_local(activation, register);
    let root = mutator.heap.stash(value);
    if let Some(stepping) = &mut mutator.stepping {
        stepping.returned = Some(root);
    }
}

/// The suspend stress mode is about to suspend every activation of the
/// run at `floor` or above into its frame: each the step names is held as
/// that frame from here on, which suspending it makes anyway, so that the
/// step follows it to the activation it is resumed as (`resumed`).
#[cfg(debug_assertions)]
pub(crate) fn suspending(mutator: &mut Mutator<'_>, floor: usize) {
    let Some(stepping) = &mutator.stepping else {
        return;
    };
    let mut named = mutator.breakpoints.one_shot_holders();
    named.push(stepping.frame);
    named.extend(stepping.between.iter().map(|(held, _)| *held));
    named.extend(stepping.return_point.map(|(caller, _)| caller));
    // Walked over the live activations, which alone may be read: a handle
    // the step holds is compared, never read through.
    let mut next = mutator.heap.top_activation();
    while let Some(live) = next {
        if mutator.heap.activation_position(live) < floor {
            break;
        }
        let suspended_sender = mutator.heap.activation_suspended_sender(live).is_some();
        let sender = mutator.heap.activation_sender(live);
        if named.iter().any(|held| held.is(Holder::Running(live))) {
            let frame = mutator.heap.reify(live);
            let key = hold(mutator, frame);
            rename(mutator, Holder::Running(live), Holder::Suspended(key));
        }
        if suspended_sender {
            break;
        }
        next = sender;
    }
}

/// `frame`, suspended, was resumed as `activation`: if the step holds it,
/// every name the step knew it by -- the stepping frame's, its return
/// point's, its one-shots' -- is `activation` from here on.
pub(crate) fn resumed<'gc>(
    mutator: &mut Mutator<'gc>,
    frame: FrameReference<'gc>,
    activation: ActivationReference,
) {
    let Some(key) = suspended_key(mutator, frame) else {
        return;
    };
    if let Some(stepping) = &mut mutator.stepping {
        stepping.suspended.retain(|(held, _)| *held != key);
    }
    rename(mutator, Holder::Suspended(key), Holder::Running(activation));
    let position = mutator.heap.activation_position(activation);
    if let Some(stepping) = &mut mutator.stepping {
        for (held, held_position) in &mut stepping.between {
            if held.is(Holder::Running(activation)) {
                *held_position = Some(position);
            }
        }
    }
}

/// The key the step holds the suspended `frame` by, if it does.
fn suspended_key<'gc>(mutator: &Mutator<'gc>, frame: FrameReference<'gc>) -> Option<u32> {
    let stepping = mutator.stepping.as_ref()?;
    let object = mutator.heap.frame_object(frame);
    stepping
        .suspended
        .iter()
        .find(|(_, root)| {
            mutator
                .heap
                .fetch(root)
                .as_object()
                .is_some_and(|held| held.same(object))
        })
        .map(|(key, _)| *key)
}

/// The key of the suspended `frame` among the step's, taken for it if it
/// has none yet.
fn hold<'gc>(mutator: &mut Mutator<'gc>, frame: FrameReference<'gc>) -> u32 {
    if let Some(key) = suspended_key(mutator, frame) {
        return key;
    }
    let root = mutator
        .heap
        .stash(Value::object(mutator.heap.frame_object(frame)));
    let Some(stepping) = &mut mutator.stepping else {
        unreachable!("internal: only a step holds a suspended frame")
    };
    let key = stepping.next_key;
    stepping.next_key += 1;
    stepping.suspended.push((key, root));
    key
}

/// Every name `from` of the step's is `onto` from here on: the stepping
/// frame's, its return point's and its one-shots'.
fn rename(mutator: &mut Mutator<'_>, from: Holder, onto: Holder) {
    if let Some(stepping) = &mut mutator.stepping {
        if stepping.frame.is(from) {
            stepping.frame = onto;
        }
        for (held, position) in &mut stepping.between {
            if held.is(from) {
                *held = onto;
                *position = None;
            }
        }
        if let Some((caller, index)) = stepping.return_point {
            if caller.is(from) {
                stepping.return_point = Some((onto, index));
            }
        }
    }
    mutator.breakpoints.rename_one_shots(from, onto);
}

/// Starts the step of `action` taken in `taken_in`, at or below `top`,
/// the frame on top, `returned` the value recorded so far: in the frame
/// `stepping_frame` finds, flooded unless the step goes out, its return
/// point set, and the request held for a step into; the frames from `top`
/// down to it are there already, none of them a push of the step's. The
/// caller settles the table.
fn start<'gc>(
    mutator: &mut Mutator<'gc>,
    action: StepAction,
    top: Place<'gc>,
    taken_in: Place<'gc>,
    returned: Option<Root>,
) {
    mutator.stepping = Some(Box::new(Stepping {
        action,
        // Named by `stepping_frame`, which may hold `taken_in` as a
        // suspended frame first.
        frame: Holder::Left,
        between: Vec::new(),
        flooded: Vec::new(),
        return_point: None,
        returned,
        suspended: Vec::new(),
        next_key: 0,
        started_filtered: false,
        exits: Vec::new(),
        #[cfg(debug_assertions)]
        waiting_on: None,
    }));
    let (frame, body) = stepping_frame(mutator, taken_in);
    let between = frames_between(mutator, top, frame);
    if let Some(stepping) = &mut mutator.stepping {
        stepping.frame = frame;
        stepping.between = between;
    }
    if action != StepAction::Out {
        flood(mutator, frame, body);
    }
    set_return_point(mutator, frame);
    if action == StepAction::Into {
        hold_request(mutator);
    }
}

/// The frames from `top` down to the stepping frame `frame`, which
/// `top` lies above, with their positions while they run: none when the
/// step steps in `top` itself. Below a suspended sender the walk goes on
/// over the suspended frames, each held by the step as the stepping frame
/// may be.
fn frames_between<'gc>(
    mutator: &mut Mutator<'gc>,
    top: Place<'gc>,
    frame: Holder,
) -> Vec<(Holder, Option<usize>)> {
    let mut between = Vec::new();
    let (mut next, mut suspended) = match top {
        Place::Running(top) => (Some(top), None),
        Place::Suspended(top) => (None, Some(top)),
    };
    while let Some(live) = next {
        if frame.is(Holder::Running(live)) {
            return between;
        }
        let heap = &mutator.heap;
        between.push((Holder::Running(live), Some(heap.activation_position(live))));
        if let Some(sender) = heap.activation_suspended_sender(live) {
            suspended = Some(sender);
            break;
        }
        next = heap.activation_sender(live);
    }
    while let Some(sender) = suspended {
        let held = Holder::Suspended(hold(mutator, sender));
        if frame.is(held) {
            break;
        }
        between.push((held, None));
        suspended = mutator.heap.frame_sender(sender);
    }
    between
}

/// A frame's body, which declaration it belongs to, and its metadata
/// (`stack::running_body`).
type FrameBody = (
    Rc<CompiledCode>,
    Option<BodyOrigin>,
    Option<Rc<BodyMetadata>>,
);

/// The frame a step taken in `top` steps in, and its body: `top`, or,
/// when `top` runs a block the compiler lowered inline in its creator --
/// a fallback closure, or under the reference lowering a block it does
/// not inline -- that creator, and so on outward, so that the step goes
/// on as it would in the inline copy: over the block's end to its
/// creator's next statement start, out of the creator. A creator that
/// returned leaves the block a frame of its own.
fn stepping_frame<'gc>(mutator: &mut Mutator<'gc>, top: Place<'gc>) -> (Holder, FrameBody) {
    let (mut frame, mut body, mut captured) = match top {
        Place::Running(top) => {
            let captured = mutator.heap.activation_captured(top);
            (
                Holder::Running(top),
                stack::running_body(mutator, top),
                captured,
            )
        }
        Place::Suspended(top) => {
            let heap = &mutator.heap;
            let root = heap.stash(Value::object(heap.frame_object(top)));
            let captured = heap.frame_captured(top);
            let key = hold(mutator, top);
            (
                Holder::Suspended(key),
                stack::suspended_body(mutator, &root),
                captured,
            )
        }
    };
    while let Some(creator_frame) = captured {
        let heap = &mutator.heap;
        let (creator, creator_body) = match heap.frame_activation(creator_frame) {
            Some(creator) => (
                Holder::Running(creator),
                stack::running_body(mutator, creator),
            ),
            None if heap.frame_is_suspended(creator_frame) => {
                let root = heap.stash(Value::object(heap.frame_object(creator_frame)));
                let key = hold(mutator, creator_frame);
                (
                    Holder::Suspended(key),
                    stack::suspended_body(mutator, &root),
                )
            }
            None => break,
        };
        let (block, _, _) = &body;
        let (code, _, metadata) = &creator_body;
        let inlined = metadata.as_ref().is_some_and(|metadata| {
            inlined_blocks(code, metadata)
                .iter()
                .any(|inlined| Rc::ptr_eq(inlined, block))
        });
        if !inlined {
            break;
        }
        captured = mutator.heap.frame_captured(creator_frame);
        frame = creator;
        body = creator_body;
    }
    (frame, body)
}

/// Patches every statement start of the body of `frame` and of each block
/// the compiler lowered inline in it -- its fallback closure -- with a
/// one-shot for `frame`, and the index after each of their sends with a
/// recording one (`record`). A body without metadata has no statement
/// starts to flood.
fn flood(mutator: &mut Mutator<'_>, frame: Holder, body: FrameBody) {
    let (code, origin, metadata) = body;
    let Some(origin) = origin else {
        return;
    };
    let mut pending = vec![(code, metadata)];
    while let Some((body, metadata)) = pending.pop() {
        let Some(stepping) = &mut mutator.stepping else {
            return;
        };
        if stepping
            .flooded
            .iter()
            .any(|flooded| Rc::ptr_eq(flooded, &body))
        {
            continue;
        }
        stepping.flooded.push(Rc::clone(&body));
        let Some(metadata) = metadata else {
            continue;
        };
        let count = body.instruction_count();
        let stops: Vec<usize> = metadata
            .statements
            .iter()
            .filter(|entry| !entry.instructions.is_empty())
            .filter_map(|entry| usize::try_from(entry.instructions.start).ok())
            .filter(|index| *index < count)
            .collect();
        let returns: Vec<usize> = metadata
            .sends
            .iter()
            .filter_map(|send| usize::try_from(send.index).ok())
            .map(|index| index + 1)
            .filter(|index| *index < count)
            .collect();
        for index in stops {
            let one_shot = OneShot {
                holder: frame,
                stops: true,
                records: false,
            };
            patch(mutator, &body, index, one_shot);
        }
        for index in returns {
            let one_shot = OneShot {
                holder: frame,
                stops: false,
                records: true,
            };
            patch(mutator, &body, index, one_shot);
        }
        for block in inlined_blocks(&body, &metadata) {
            let metadata = Cache::metadata(mutator, &block, &origin);
            pending.push((block, metadata));
        }
    }
}

/// The block literals of `body` the compiler lowered inline, which run as
/// closures only for a receiver that is no Boolean: under the optimized
/// lowering, a fallback closure, recognized as a block literal whose
/// source encloses one of `body`'s statements -- its inline copy's -- and
/// under the reference lowering, which inlines nothing, a block literal
/// a `SendInlinable` takes as its receiver or an argument before its
/// register is written again: the blocks the optimized lowering inlines.
pub(crate) fn inlined_blocks(
    body: &CompiledCode,
    metadata: &BodyMetadata,
) -> Vec<Rc<CompiledCode>> {
    let instructions = body.instructions();
    let mut blocks = Vec::new();
    for (index, instruction) in instructions.iter().enumerate() {
        let Instruction::MakeClosure {
            destination, code, ..
        } = instruction
        else {
            continue;
        };
        // A simultaneous slot's computation is a closure of its own, which
        // its future's `resolve` runs; its source is the slot's expression,
        // the very span of the initializer's statement for the slot.
        if matches!(
            instructions.get(index + 1),
            Some(Instruction::MakeFuture { closure, .. }) if closure == destination
        ) {
            continue;
        }
        let source = code.source;
        let encloses = metadata
            .statements
            .iter()
            .any(|entry| source.start <= entry.span.start && entry.span.end <= source.end);
        if encloses || taken_by_an_inlinable_send(&instructions[index + 1..], *destination) {
            blocks.push(Rc::clone(code));
        }
    }
    blocks
}

/// Whether a `SendInlinable` of `following` reads `register` before an
/// instruction writes it again.
fn taken_by_an_inlinable_send(following: &[Instruction], register: u16) -> bool {
    for instruction in following {
        if let Instruction::SendInlinable {
            receiver_register,
            argument_sources,
            ..
        } = instruction
        {
            if *receiver_register == register || argument_sources.contains(&register) {
                return true;
            }
        }
        if register_written(instruction) == Some(register) {
            return false;
        }
    }
    false
}

/// Sets the return point of the stepping frame `frame`: a one-shot that
/// stops and records at the index after the instruction its caller
/// stands at, for the caller -- never at that instruction, which
/// `resume_caller` reads again. A caller suspended into its frame is
/// resumed as an activation of its own once the frame returns to it,
/// which the one-shot waits for (`resumed`). None for a frame without a
/// caller.
fn set_return_point(mutator: &mut Mutator<'_>, frame: Holder) {
    let heap = &mutator.heap;
    let sender = match frame {
        Holder::Running(activation) => match heap.activation_suspended_sender(activation) {
            Some(suspended) => Err(suspended),
            None => match heap.activation_sender(activation) {
                Some(caller) => Ok(caller),
                None => return,
            },
        },
        Holder::Suspended(key) => {
            let Some(own) = mutator.stepping.as_ref().and_then(|stepping| {
                stepping
                    .suspended
                    .iter()
                    .find(|(held, _)| *held == key)
                    .and_then(|(_, root)| heap.fetch(root).as_object())
                    .and_then(|object| heap.object_frame(object))
            }) else {
                return;
            };
            let Some(sender) = heap.frame_sender(own) else {
                return;
            };
            match heap.frame_activation(sender) {
                Some(caller) => Ok(caller),
                None => Err(sender),
            }
        }
        Holder::Left => return,
    };
    let (caller, standing, code) = match sender {
        Ok(caller) => (
            Holder::Running(caller),
            heap.activation_program_counter(caller),
            heap.activation_code(caller),
        ),
        Err(suspended) => {
            let Some(code) = heap.frame_code(suspended) else {
                return;
            };
            let standing = heap.frame_stored_program_counter(suspended);
            (Holder::Suspended(hold(mutator, suspended)), standing, code)
        }
    };
    let point = standing + 1;
    if point >= code.instruction_count() || call_destination(&code.instruction(standing)).is_none()
    {
        return;
    }
    let one_shot = OneShot {
        holder: caller,
        stops: true,
        records: true,
    };
    patch(mutator, &code, point, one_shot);
    if let Some(stepping) = &mut mutator.stepping {
        stepping.return_point = Some((caller, point));
    }
}

/// Patches slot `index` of `code` with `one_shot`, giving the body a code
/// id, which a full collection keeps while it is patched, as a line
/// breakpoint's.
fn patch(mutator: &mut Mutator<'_>, code: &Rc<CompiledCode>, index: usize, one_shot: OneShot) {
    mutator.breakpoints.add_one_shot(code, index, one_shot);
    mutator.heap.code_id(code);
}

/// The register a call writes its answer to: a send's, a
/// `ResolveFuture`'s, an initializer invocation's or a `SendMirror`'s
/// destination, where a caller stands while its callee runs; `None` for
/// every other instruction.
fn call_destination(instruction: &Instruction) -> Option<u16> {
    match instruction {
        Instruction::SendLexical { destination, .. }
        | Instruction::SendPublic { destination, .. }
        | Instruction::SendInlinable { destination, .. }
        | Instruction::SendSuper { destination, .. }
        | Instruction::SendMirror { destination, .. }
        | Instruction::ResolveFuture { destination, .. }
        | Instruction::InitializeInstance { destination, .. }
        | Instruction::InitializeSuperclass { destination, .. }
        | Instruction::InitializeAppliedLevel { destination, .. } => Some(*destination),
        _ => None,
    }
}

/// Holds the step into's request bit under the attached debugger's
/// arming lock, as the agent arms the poll.
fn hold_request(mutator: &mut Mutator<'_>) {
    let arming = mutator.breakpoints.arming();
    let _armed = arming
        .as_ref()
        .map(|arming| arming.lock().unwrap_or_else(PoisonError::into_inner));
    mutator.heap.hold_step_request();
}

/// Drops the request bit, if it is held, under the arming lock, whose
/// hold keeps the depth limit's resynchronization from falling between
/// the agent's two writes (`Arming`).
fn drop_request(mutator: &mut Mutator<'_>) {
    if mutator.heap.debugger_bits() & DEBUGGER_STEP == 0 {
        return;
    }
    let arming = mutator.breakpoints.arming();
    let _armed = arming
        .as_ref()
        .map(|arming| arming.lock().unwrap_or_else(PoisonError::into_inner));
    mutator.heap.clear_debugger_bits(DEBUGGER_STEP);
}
