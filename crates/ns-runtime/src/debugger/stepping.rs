//! Stepping by one-shot breakpoints and the safepoint
//! (`docs/design/debugger.md`, decision 3, "Step over in recursion and
//! blocks", "Step into's held request", "The return value of the last
//! call"). A step over floods the stepping frame's body -- every
//! statement start of it and of the blocks the compiler lowered inline in
//! it -- with one-shot breakpoints for that frame, and sets one at its
//! return point, the index after the instruction its caller stands at; a
//! step out sets only the return point's, and one taken in a block a send
//! took leaves that whole send (decision 17, `send_left`); a step into is
//! a step over whose request bit (`DEBUGGER_STEP`) every poll takes until
//! a frame is pushed above the stepping one, which stops at its first
//! statement start. The one-shots are a kind of the breakpoint table's,
//! patched and restored as a line breakpoint is, so that their bodies lose
//! their machine code for the step; the next stop, whatever made it, ends
//! the step, and one its host run's end leaves in progress goes on in the
//! program's next run (`run_ended`, `carry_into`). No check enters the
//! run loop: what the step does not patch runs at full speed. A step
//! never lands in a frame the step filters filter -- by default the
//! shipped modules' (decision 16): a stop it would make there goes on as
//! a step into from the first unfiltered frame below, and a step into's
//! request outlasts the pushes of filtered bodies. A step into an
//! instantiation stops in the factory's frame, at its entry, and goes on
//! from there into the initializer it pushes, and one into a superclass
//! clause or a `<:` clause's message stops at the entry of the
//! initializer it enters, at its class's header, but `Object`'s and
//! `Top`'s; on the way out it passes over the factory to its caller
//! (decision 12, `rewound_into_factory`, `enters_at_header`,
//! `went_on_past_filtered`). A method an unwind leaves while a method
//! breakpoint names it is reported by a step of the same kind
//! (`method_left`), whose stop reports the method's exit where the
//! program goes on.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::rc::{Rc, Weak};
use std::sync::PoisonError;

use super::breakpoints::{AfterPast, OneShot, PastOnce, address, settle};
use super::metadata::again::{BodyOrigin, declaration_path};
use super::metadata::{BlockPlace, BlockSendEntry, BodyMetadata, Cache, TakenBlock};
use super::remote::{BreakpointId, ClassName, StepInTargetKind};
use super::stack::{self, StackFrame};
use ns_syntax::Span;

use crate::Symbol;
use crate::bytecode::{CompiledCode, Instruction, register_written};
use crate::heap::{ActivationReference, DEBUGGER_STEP, FrameReference};
use crate::interpreter::Mutator;
use crate::interpreter::instantiate::FACTORY_INITIALIZE;
use crate::types::{ActivationKind, BuiltinClass};
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

/// What the step filters decided for each body they were asked of
/// (`is_filtered`), keyed by the body's address: a step into through
/// shipped code asks at every poll, and deciding afresh builds the body's
/// declaration path each time. A body belongs to one declaration's
/// version, so the decision is the body's; forgotten whenever the filters
/// change and at every install, as the metadata cache is. The body is
/// held weakly, as there, so that no other body takes its address while
/// its entry lives; a dead entry is forgotten at the next miss.
#[derive(Default)]
pub(crate) struct FilterDecisions {
    bodies: RefCell<HashMap<usize, (Weak<CompiledCode>, bool)>>,
}

impl FilterDecisions {
    /// The decision for `code`, if one was made.
    fn get(&self, code: &Rc<CompiledCode>) -> Option<bool> {
        self.bodies
            .borrow()
            .get(&address(code))
            .map(|(_, filtered)| *filtered)
    }

    /// Records `filtered` for `code`, forgetting the entries of dead
    /// bodies first.
    fn record(&self, code: &Rc<CompiledCode>, filtered: bool) {
        let mut bodies = self.bodies.borrow_mut();
        bodies.retain(|_, (body, _)| body.strong_count() > 0);
        bodies.insert(address(code), (Rc::downgrade(code), filtered));
    }

    /// Forgets every decision.
    pub(crate) fn forget(&self) {
        self.bodies.borrow_mut().clear();
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
    pub(crate) fn same_frame(self, other: Holder) -> bool {
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
    /// Where a step out taken in a block a real send took goes on: the
    /// frame that stands at the send, and the index after it, which stops
    /// and records as the return point does (`send_left`).
    after_send: Option<(Holder, usize)>,
    /// The value the stepping frame's last call answered, recorded at the
    /// return point after it, and where that call stands (`record`).
    returned: Option<Returned>,
    /// The frame object of each suspended frame the step names, by its
    /// key (`Holder::Suspended`).
    suspended: Vec<(u32, Root)>,
    /// The key the next suspended frame takes.
    next_key: u32,
    /// Whether the step was begun in a frame the step filters filter,
    /// which it then steps in as in any other (`is_filtered`); decided as
    /// the step begins, whatever filters are set while it runs.
    started_filtered: bool,
    /// The frame the step was begun in when that is not the stepping
    /// frame: an initializer's at its header stop, whose step steps from
    /// its sender (`begin_at_header`). A stop in it is a stop in the frame
    /// the step was begun in (`went_on_past_filtered`). Set only while a
    /// one-shot of the frame's own waits at its first statement, which it
    /// reaches before any return: a return to its caller's send is
    /// reported to no step, so the name must not outlive the frame's run
    /// to that one-shot; every other way it leaves is reported (`popped`).
    begun_in: Option<Holder>,
    /// The method breakpoints whose method an unwind left since the step
    /// began (`method_left`), which its stop reports as the method's exit.
    exits: Vec<BreakpointId>,
    /// The frame on top when the step's frame left and an error was about
    /// to be signalled (`frame_left` with no frame to go on in): the
    /// exception's machinery runs above it until the handler search
    /// reports, which replaces the step. For the step stress mode's check.
    #[cfg(debug_assertions)]
    waiting_on: Option<ActivationReference>,
    /// Whether the step stands for a pause a host run started with
    /// (`stop_at_entry`): its stop is the pause's.
    at_entry: bool,
    /// Whether the step went on as a step into from where the program
    /// went on (`continue_as_step_into`): its frame or its return point's
    /// caller left, a filtered frame was passed, a handler was found or
    /// the next run started. Its stop then stands in another activation
    /// than the one the step was taken in, though it may be one of the
    /// same body at the same depth -- a block's next activation -- which
    /// the agent's step by line tells apart by it (decision 4).
    went_on_elsewhere: bool,
    /// The activation the step was taken in when the step steps in
    /// another frame -- a fallback closure of a block its creator's
    /// inlined form took, whose creator is the stepping frame -- while it
    /// has not left: its return is noted by a one-shot where its caller
    /// goes on (`OneShot::leaves`), an unwind's pop by `popped`, since a
    /// later stop in a block of the same body at the same offset may be
    /// its next activation.
    origin: Option<Holder>,
    /// Whether `origin` left: the step's stop is then in another
    /// activation (`went_on_elsewhere`).
    origin_left: bool,
    /// The one place a smart step into stops before the statement is done
    /// (`begin_smart`); `None` for every other step, and for one that went
    /// on as a step into from elsewhere, which is a step of its own.
    aim: Option<Aim>,
}

/// What a smart step into stops at (`docs/design/debugger.md`, decision
/// 3, "Smart step into a block"). It names bodies and an index, never a
/// handle, so that the stop may keep it while an evaluation runs.
#[derive(Clone)]
pub(crate) enum Aim {
    /// A push whose sender, or the first sender below a run of filtered
    /// frames, runs `code` and stands at its slot `index`, the chosen send.
    Send {
        code: Rc<CompiledCode>,
        index: usize,
    },
    /// The first statement start of `code`, a block's body, in an
    /// activation of a closure the stepping frame made.
    Block { code: Rc<CompiledCode> },
}

/// A place a smart step into may stop at, as `Command::StepInTargets`
/// answers it, what the step aims at, and the statement it was taken at.
pub(crate) struct Target {
    pub(crate) label: String,
    pub(crate) span: Span,
    pub(crate) kind: StepInTargetKind,
    pub(crate) aim: Aim,
    pub(crate) statement: Rc<StatementRegion>,
}

/// The code a smart step into's statement runs as the stepping frame's
/// own: the statement's instructions in the frame's body, and the whole
/// body of every block of an inlinable form in it the frame runs as a
/// closure -- a fallback closure, or the reference lowering's block --
/// whose statement starts its flood leaves out, so that the step stops
/// at the chosen push or, as a step over, only once the statement is
/// done, never at a statement start of an inlined block on the way.
/// The statement's own first start is no part of it: a frame that comes
/// back there -- in the next iteration of a loop the optimized lowering
/// inlines, whose condition may have no statement start of its own --
/// has done the statement and begins it afresh, which a step over stops
/// at as the reference lowering's does in the loop's next block.
pub(crate) struct StatementRegion {
    /// The statement's body and its instructions in it.
    statement: (Rc<CompiledCode>, Range<u32>),
    /// The whole body of each block of an inlinable form in the
    /// statement the frame runs as a closure.
    blocks: Vec<Rc<CompiledCode>>,
}

impl StatementRegion {
    /// Whether slot `index` of `body` lies in the statement, past its
    /// first start.
    fn holds(&self, body: &Rc<CompiledCode>, index: usize) -> bool {
        let (code, range) = &self.statement;
        let in_statement = Rc::ptr_eq(code, body)
            && u32::try_from(index)
                .is_ok_and(|index| index != range.start && range.contains(&index));
        in_statement
            || self
                .blocks
                .iter()
                .any(|block| Rc::ptr_eq(block, body) && index < block.instruction_count())
    }
}

impl Stepping {
    /// Whether a frame the step goes on in left otherwise than by a
    /// return to its caller's send: its own, its return point's caller, or
    /// the frame whose send a step out of a block leaves.
    fn has_left(&self) -> bool {
        let left = |holder: Holder| holder.same_frame(Holder::Left);
        left(self.frame)
            || self.return_point.is_some_and(|(caller, _)| left(caller))
            || self.after_send.is_some_and(|(holder, _)| left(holder))
    }

    /// A step of `action` whose frame is `Holder::Left`, `returned` the
    /// value recorded so far: one `start` is about to name its frame for,
    /// or one waiting for where the program goes on.
    fn new(action: StepAction, returned: Option<Returned>) -> Stepping {
        Stepping {
            action,
            frame: Holder::Left,
            between: Vec::new(),
            flooded: Vec::new(),
            return_point: None,
            after_send: None,
            returned,
            suspended: Vec::new(),
            next_key: 0,
            started_filtered: false,
            begun_in: None,
            exits: Vec::new(),
            #[cfg(debug_assertions)]
            waiting_on: None,
            at_entry: false,
            went_on_elsewhere: false,
            origin: None,
            origin_left: false,
            aim: None,
        }
    }
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
    /// A factory's push, which went on into its initializer's push with
    /// no safepoint between (`Mutator::enter_factory_body`): the
    /// initializer's push undone, the program stops in the factory, now
    /// on top, at its `InitializeInstance` -- a frame other than the one
    /// the run loop left, which it reads again (`rewound_into_factory`).
    StopInFactory,
    /// The push of a superclass's or an applied level's initializer by a
    /// clause: the program stops in the pushed frame at its entry, before
    /// its first statement, placed at its declaration's header
    /// (`enters_at_header`, `begin_at_header`).
    StopAtHeader,
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
    begin_in(mutator, action, shown, None);
}

/// `begin`, its flood leaving out the statement starts `statement` holds:
/// a smart step into's, which goes on as a step over only once its
/// statement is done (`begin_smart`).
fn begin_in(
    mutator: &mut Mutator<'_>,
    action: StepAction,
    shown: Option<&StackFrame>,
    statement: Option<&StatementRegion>,
) {
    end(mutator);
    let Some(top) = mutator.heap.top_activation() else {
        return;
    };
    let taken_in = shown
        .and_then(|frame| place_of(mutator, frame))
        .unwrap_or(Place::Running(top));
    let started_filtered = is_filtered(mutator, shown.unwrap_or(&StackFrame::Running(top)));
    // A step out taken in a block leaves the send that took it, in the
    // frame that send runs in (decision 17).
    let left = match action {
        StepAction::Out => send_left(mutator, taken_in),
        StepAction::Over | StepAction::Into => None,
    };
    let step_from = left.as_ref().map_or(taken_in, SendLeft::frame);
    // Only a step taken in the top frame itself may step in another: the
    // creator of the fallback closure it runs (`note_origin`).
    let taken_on_top = shown.is_none() && left.is_none();
    // A factory has no statement of its own: a step over at its stop goes
    // on, as a step into, to the initializer it pushes (decision 12).
    let action = match (action, step_from) {
        (StepAction::Over, Place::Running(frame))
            if stack::is_factory(&mutator.heap, &StackFrame::Running(frame)) =>
        {
            StepAction::Into
        }
        _ => action,
    };
    let passed = match &left {
        Some(SendLeft::Inlined { left, .. }) => Some(left),
        Some(SendLeft::Called { .. }) | None => None,
    };
    start(
        mutator,
        action,
        Place::Running(top),
        step_from,
        None,
        passed,
        statement,
    );
    match left {
        Some(SendLeft::Called { frame, code, after }) => {
            stop_after_send(mutator, frame, &code, after);
        }
        Some(SendLeft::Inlined {
            frame,
            join: Some(join),
            left,
        }) => stop_after_send(mutator, frame, &left.body, join),
        Some(SendLeft::Inlined { join: None, .. }) | None => {}
    }
    if let Some(stepping) = &mut mutator.stepping {
        stepping.started_filtered = started_filtered;
    }
    if taken_on_top {
        note_origin(mutator, top);
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

/// The places a smart step into from `frame` may stop at (decision 3,
/// "Smart step into a block"): the sends of the innermost statement
/// holding its program counter, from there -- past the send a frame
/// below the top, `waiting`, waits on, which runs already -- to the
/// statement's end, in the order they run, then the block literals those
/// sends take that run as closures, in the order of their sends. None
/// for a body without metadata.
///
/// An inlinable form -- `ifTrue:`, `and:`, `whileTrue:`, a mirror send's
/// failure block -- gives the same list under both lowerings: its own
/// selector is never a target (`forms_own_sends`), and the sends of its
/// blocks are, once each, where the form stands, in the order the source
/// writes them -- under the optimized lowering in the frame's own body,
/// where the inline copy runs, and under the reference lowering in the
/// block's body, which the frame runs as a closure the step floods for
/// it, so that such a send target's body is that block's and its sender
/// the step's inline code (`inline_code_of_step`). No block of such a
/// form is a target: it runs as the frame's own code, where a step over
/// stops in it.
pub(crate) fn targets(mutator: &mut Mutator<'_>, frame: &StackFrame, waiting: bool) -> Vec<Target> {
    let (code, origin, metadata) = stack::body(mutator, frame);
    let (Some(origin), Some(metadata)) = (origin, metadata) else {
        return Vec::new();
    };
    let standing = stack::program_counter(&mutator.heap, frame);
    let Some(statement) = metadata
        .statements
        .iter()
        .filter(|entry| entry.instructions.contains(&standing))
        .min_by_key(|entry| entry.instructions.len())
    else {
        return Vec::new();
    };
    let first = if waiting {
        standing.saturating_add(1)
    } else {
        standing
    };
    let rest = first..statement.instructions.end;
    let in_statement = (Rc::clone(&code), statement.instructions.clone());
    let mut blocks_run = Vec::new();
    // Every target is given the whole statement once it is known.
    let unplaced = Rc::new(StatementRegion {
        statement: in_statement.clone(),
        blocks: Vec::new(),
    });
    // Each target keyed by where it runs: its index, behind the index of
    // the form's send and the block's place among the form's blocks for a
    // block body the reference lowering runs as a closure.
    let mut sends: Vec<(Vec<u32>, Target)> = Vec::new();
    let mut blocks: Vec<(Vec<u32>, Target)> = Vec::new();
    // A body is listed unless it is a fallback closure, whose sends the
    // inline copy lists: it is only part of the statement.
    let mut pending = vec![(code, metadata, rest, Vec::new(), true)];
    while let Some((code, metadata, rest, place, listed)) = pending.pop() {
        let count = code.instruction_count();
        let instruction_at = |index: u32| {
            usize::try_from(index)
                .ok()
                .filter(|index| *index < count)
                .map(|index| (index, code.instruction(index)))
        };
        let keyed = |index: u32| {
            let mut key = place.clone();
            key.push(index);
            key
        };
        let own = forms_own_sends(&code, &metadata);
        for entry in &metadata.sends {
            if !listed || !rest.contains(&entry.index) || own.contains(&entry.index) {
                continue;
            }
            let Some((index, instruction)) = instruction_at(entry.index) else {
                continue;
            };
            let Some(label) = selector_label(&instruction) else {
                continue;
            };
            let target = Target {
                label,
                span: entry.selector.first().copied().unwrap_or(entry.call),
                kind: StepInTargetKind::Send,
                aim: Aim::Send {
                    code: Rc::clone(&code),
                    index,
                },
                statement: Rc::clone(&unplaced),
            };
            sends.push((keyed(entry.index), target));
        }
        for entry in &metadata.block_sends {
            let inline_spans: Vec<Span> = entry
                .blocks
                .iter()
                .filter(|block| matches!(block.place, BlockPlace::Inline(_)))
                .map(|block| block.span)
                .collect();
            let mut expanded: Vec<Span> = Vec::new();
            for (ordinal, block) in (0u32..).zip(entry.blocks.iter()) {
                let BlockPlace::Called { send, closure } = block.place else {
                    continue;
                };
                if !rest.contains(&send) {
                    continue;
                }
                let Some((_, Instruction::MakeClosure { code: body, .. })) =
                    instruction_at(closure)
                else {
                    continue;
                };
                if !entry.inlined {
                    if !listed {
                        continue;
                    }
                    let Some(label) =
                        instruction_at(send).and_then(|(_, taking)| selector_label(&taking))
                    else {
                        continue;
                    };
                    let target = Target {
                        label: format!("[] in {label}"),
                        span: block.span,
                        kind: StepInTargetKind::Block,
                        aim: Aim::Block { code: body },
                        statement: Rc::clone(&unplaced),
                    };
                    blocks.push((keyed(send), target));
                    continue;
                }
                // An inlined form's block: its sends are listed where the
                // optimized lowering's inline copy holds them -- its
                // closure being the fallback's, run only for a receiver
                // that is no Boolean -- or else from the block's body.
                if expanded.contains(&block.span) {
                    continue;
                }
                expanded.push(block.span);
                let all = 0..u32::try_from(body.instruction_count()).unwrap_or(u32::MAX);
                blocks_run.push(Rc::clone(&body));
                let Some(metadata) = Cache::metadata(mutator, &body, &origin) else {
                    continue;
                };
                let mut key = keyed(send);
                key.push(ordinal);
                let lists = listed && !inline_spans.contains(&block.span);
                pending.push((body, metadata, all, key, lists));
            }
        }
    }
    sends.sort_by(|(first, _), (second, _)| first.cmp(second));
    blocks.sort_by(|(first, _), (second, _)| first.cmp(second));
    let statement = Rc::new(StatementRegion {
        statement: in_statement,
        blocks: blocks_run,
    });
    sends
        .into_iter()
        .chain(blocks)
        .map(|(_, mut target)| {
            target.statement = Rc::clone(&statement);
            target
        })
        .collect()
}

/// The indices of `code`'s sends that are an inlinable form's own, never
/// a target, since the optimized lowering's inline copy has no send of
/// the form (`targets`): every `SendInlinable` -- the reference lowering's
/// form and its loop's `value` sends -- the send at which a form's block
/// runs as a closure -- the optimized lowering's fallback send -- and a
/// mirror send whose failure block the optimized lowering inlines, which
/// fails into that block's first instruction.
fn forms_own_sends(code: &CompiledCode, metadata: &BodyMetadata) -> Vec<u32> {
    let inlined_blocks = || {
        metadata
            .block_sends
            .iter()
            .filter(|entry| entry.inlined)
            .flat_map(|entry| entry.blocks.iter())
    };
    let mut own: Vec<u32> = inlined_blocks()
        .filter_map(|block| match block.place {
            BlockPlace::Called { send, .. } => Some(send),
            BlockPlace::Inline(_) => None,
        })
        .collect();
    let inline_starts: Vec<u32> = inlined_blocks()
        .filter_map(|block| match &block.place {
            BlockPlace::Inline(range) => Some(range.start),
            BlockPlace::Called { .. } => None,
        })
        .collect();
    let count = code.instruction_count();
    for entry in &metadata.sends {
        let Some(index) = usize::try_from(entry.index)
            .ok()
            .filter(|index| *index < count)
        else {
            continue;
        };
        match code.instruction(index) {
            Instruction::SendInlinable { .. } => own.push(entry.index),
            Instruction::SendMirror { failure, .. } if inline_starts.contains(&failure) => {
                own.push(entry.index);
            }
            _ => {}
        }
    }
    own
}

/// A target's label for the send `instruction`: its selector as the
/// source writes it -- a super send's without `super` -- or a mirror
/// send's mirror selector; `None` for an instruction that is no send.
fn selector_label(instruction: &Instruction) -> Option<String> {
    let (_, call) = call_of(instruction)?;
    Some(match call {
        Call::Send(selector) | Call::Super(selector) => selector.to_string(),
        Call::Mirror(_)
        | Call::Future
        | Call::Initializer
        | Call::SuperclassInitializer
        | Call::MixinInitializer => call.name(),
    })
}

/// Begins a smart step into aimed at `aim` from the frame a stop stands
/// in, or `shown`, as `begin` does (decision 3). At a send it is a step
/// into whose held request stops only at a push from that send
/// (`stands_at_aim`); at a block a step over plus a one-shot for the
/// stepping frame at the block body's first statement start, which
/// `takes` in every activation of a closure that frame made, wherever
/// the block runs -- in the shipped `do:`, not at a send of the frame.
/// Either goes on as a step over once the statement is done without its
/// aim.
pub(crate) fn begin_smart(
    mutator: &mut Mutator<'_>,
    aim: Aim,
    statement: &StatementRegion,
    shown: Option<&StackFrame>,
) {
    let action = match aim {
        Aim::Send { .. } => StepAction::Into,
        Aim::Block { .. } => StepAction::Over,
    };
    begin_in(mutator, action, shown, Some(statement));
    let Some(stepping) = &mut mutator.stepping else {
        return;
    };
    let frame = stepping.frame;
    let block = match &aim {
        Aim::Block { code } => Some(Rc::clone(code)),
        Aim::Send { .. } => None,
    };
    stepping.aim = Some(aim);
    if let Some(code) = block {
        let one_shot = OneShot {
            holder: frame,
            stops: true,
            records: false,
            leaves: false,
        };
        patch(mutator, &code, first_statement(&code), one_shot);
        settle(mutator);
    }
}

/// Whether `activation`, where a step stops, runs the block a smart step
/// into aimed at: it stopped in a frame the stepping frame did not run,
/// as a step into's push does, so it reports no value of a call.
pub(crate) fn in_aimed_block(mutator: &Mutator<'_>, activation: ActivationReference) -> bool {
    let Some(Aim::Block { code }) = mutator
        .stepping
        .as_ref()
        .and_then(|stepping| stepping.aim.as_ref())
    else {
        return false;
    };
    Rc::ptr_eq(code, &mutator.heap.activation_code(activation))
}

/// The send a step out taken in a block leaves (decision 17), in `frame`,
/// the frame that runs the send.
enum SendLeft<'gc> {
    /// A form the optimized lowering inlines, left for where it answered:
    /// its join, when that is mid-statement (`join_stop`), or else the
    /// next statement start after it, as a step over of the whole form
    /// would leave it -- the stepping frame is flooded as for a step
    /// over, but for the form's own blocks, which the flood passes over.
    Inlined {
        frame: Place<'gc>,
        join: Option<usize>,
        left: LeftBlocks,
    },
    /// A real send, left for its return point, `after` in `code`.
    Called {
        frame: Place<'gc>,
        code: Rc<CompiledCode>,
        after: usize,
    },
}

impl<'gc> SendLeft<'gc> {
    /// The frame that runs the send, which the step is taken in.
    fn frame(&self) -> Place<'gc> {
        match self {
            SendLeft::Inlined { frame, .. } | SendLeft::Called { frame, .. } => *frame,
        }
    }
}

/// The blocks of an inlined form a step out leaves, which its flood passes
/// over: the instructions of `body` the form's inline blocks were lowered
/// to, and the bodies of the closures made of the form's block literals --
/// its fallback closures, or the reference lowering's blocks -- and of
/// every block literal inside its inline blocks.
struct LeftBlocks {
    body: Rc<CompiledCode>,
    extents: Vec<Range<u32>>,
    closures: Vec<Rc<CompiledCode>>,
}

impl LeftBlocks {
    /// Whether the flood passes over slot `index` of `body`.
    fn passes_over(&self, body: &Rc<CompiledCode>, index: usize) -> bool {
        Rc::ptr_eq(body, &self.body)
            && u32::try_from(index)
                .is_ok_and(|index| self.extents.iter().any(|extent| extent.contains(&index)))
    }

    /// Whether the flood passes over the block `body`.
    fn passes_over_block(&self, body: &Rc<CompiledCode>) -> bool {
        self.closures
            .iter()
            .any(|closure| Rc::ptr_eq(closure, body))
    }
}

/// The send a step out taken in `taken_in` leaves, when the frame runs a
/// block a send took (decision 17): the innermost inlined form one of
/// whose inline blocks holds the instruction the frame stands at, in its
/// own body; or else, for a frame that runs a closure, the send its
/// creator stands at, when that send took the closure (`took_closure`) --
/// a real send, or an inlined form's fallback send or reference-lowered
/// counterpart, which the creator's literal entry names. `None` for every
/// other frame -- a method, a block whose creator returned or whose
/// creator's send did not take it -- which a step out leaves for its
/// caller.
fn send_left<'gc>(mutator: &mut Mutator<'gc>, taken_in: Place<'gc>) -> Option<SendLeft<'gc>> {
    let (code, _, metadata) = place_body(mutator, taken_in);
    let standing = place_program_counter(mutator, taken_in);
    if let Some(metadata) = metadata.as_deref()
        && let Some(entry) = inline_send_around(metadata, standing)
    {
        return Some(SendLeft::Inlined {
            frame: taken_in,
            join: join_stop(&code, metadata, entry),
            left: left_blocks(&code, entry),
        });
    }
    let creator = creator_of(mutator, taken_in)?;
    let callee = callee_of(mutator, taken_in, creator)?;
    let (creator_code, _, creator_metadata) = place_body(mutator, creator);
    let waiting_at = place_program_counter(mutator, creator);
    if !took_closure(
        mutator,
        callee,
        taken_in,
        delivers_a_message(&creator_code, waiting_at),
    ) {
        return None;
    }
    // The send's literal entry, when the closure's block is the literal it
    // took, says whether it is an inlined form; a closure the send took
    // otherwise -- through a variable, from an expression -- a real send's.
    let took_this_literal = |taken: &TakenBlock| match taken.place {
        BlockPlace::Called { send, closure } => {
            usize::try_from(send) == Ok(waiting_at)
                && made_by(&creator_code, closure).is_some_and(|made| Rc::ptr_eq(&made, &code))
        }
        BlockPlace::Inline(_) => false,
    };
    let literal = creator_metadata.as_deref().and_then(|metadata| {
        metadata
            .block_sends
            .iter()
            .find(|entry| entry.inlined && entry.blocks.iter().any(took_this_literal))
            .map(|entry| (metadata, entry))
    });
    if let Some((metadata, entry)) = literal {
        return Some(SendLeft::Inlined {
            frame: creator,
            join: join_stop(&creator_code, metadata, entry),
            left: left_blocks(&creator_code, entry),
        });
    }
    let after = waiting_at + 1;
    let is_send = waiting_at < creator_code.instruction_count()
        && call_destination(&creator_code.instruction(waiting_at)).is_some();
    if !is_send || after >= creator_code.instruction_count() {
        return None;
    }
    Some(SendLeft::Called {
        frame: creator,
        code: creator_code,
        after,
    })
}

/// The index of the join of the inlined form `entry` of `code`, where a
/// step out of it stops once the form has answered, when that is mid
/// statement -- the rest of the statement the form is part of, which the
/// flood's next statement start would pass, as a real send's return point
/// is: `None` at a statement start, which the flood stops at, at a loop's
/// back jump, which goes on to the loop's condition, and at or past the
/// end of the innermost statement the form is part of -- at a body's
/// implicit return, or at an enclosing form's code after a form that is a
/// whole statement -- where the flood's next statement start is the step
/// over's.
fn join_stop(
    code: &CompiledCode,
    metadata: &BodyMetadata,
    entry: &BlockSendEntry,
) -> Option<usize> {
    let join = entry.after;
    let after = usize::try_from(join).ok()?;
    if after >= code.instruction_count() {
        return None;
    }
    let starts_a_statement = metadata
        .statements
        .iter()
        .any(|statement| statement.instructions.start == join);
    // The innermost statement the form is part of: the smallest that holds
    // every block the form takes and lies in none of them. An enclosing
    // statement holds a nested form's join too -- an outer form's branch
    // jump after an inner form that is a whole statement of that branch --
    // where a step over of the inner statement goes on at the flood's next
    // statement start, not at the outer form's jump.
    let first = entry.blocks.iter().map(|taken| taken.span.start).min()?;
    let last = entry.blocks.iter().map(|taken| taken.span.end).max()?;
    let inside_a_block = |span: Span| {
        entry
            .blocks
            .iter()
            .any(|taken| taken.span.start <= span.start && span.end <= taken.span.end)
    };
    let statement = metadata
        .statements
        .iter()
        .filter(|statement| {
            statement.span.start <= first
                && last <= statement.span.end
                && !inside_a_block(statement.span)
        })
        .min_by_key(|statement| statement.span.end - statement.span.start)?;
    let mid_statement = statement.instructions.start < join && join < statement.instructions.end;
    let back_jump = matches!(
        code.instruction(after),
        Instruction::Jump { target } if target <= join
    );
    (mid_statement && !starts_a_statement && !back_jump).then_some(after)
}

/// The innermost inlined form of `metadata` one of whose inline blocks
/// holds the instruction at `standing`: the one whose block spans the
/// fewest instructions, since inlined forms nest.
fn inline_send_around(metadata: &BodyMetadata, standing: usize) -> Option<&BlockSendEntry> {
    let standing = u32::try_from(standing).ok()?;
    metadata
        .block_sends
        .iter()
        .filter_map(|entry| {
            entry
                .blocks
                .iter()
                .filter_map(|taken| match &taken.place {
                    BlockPlace::Inline(instructions) if instructions.contains(&standing) => {
                        Some(instructions.end - instructions.start)
                    }
                    _ => None,
                })
                .min()
                .map(|extent| (extent, entry))
        })
        .min_by_key(|(extent, _)| *extent)
        .map(|(_, entry)| entry)
}

/// The blocks of the inlined form `entry` of `code`'s metadata, which a
/// step out of one of them passes over.
fn left_blocks(code: &Rc<CompiledCode>, entry: &BlockSendEntry) -> LeftBlocks {
    let extents: Vec<Range<u32>> = entry
        .blocks
        .iter()
        .filter_map(|taken| match &taken.place {
            BlockPlace::Inline(instructions) => Some(instructions.clone()),
            BlockPlace::Called { .. } => None,
        })
        .collect();
    let mut closures: Vec<Rc<CompiledCode>> = entry
        .blocks
        .iter()
        .filter_map(|taken| match taken.place {
            BlockPlace::Called { closure, .. } => made_by(code, closure),
            BlockPlace::Inline(_) => None,
        })
        .collect();
    for (index, instruction) in code.instructions().iter().enumerate() {
        let inside = u32::try_from(index)
            .is_ok_and(|index| extents.iter().any(|extent| extent.contains(&index)));
        if let (true, Instruction::MakeClosure { code: made, .. }) = (inside, instruction) {
            closures.push(Rc::clone(made));
        }
    }
    LeftBlocks {
        body: Rc::clone(code),
        extents,
        closures,
    }
}

/// The body of the block the `MakeClosure` at `index` of `code` makes.
fn made_by(code: &CompiledCode, index: u32) -> Option<Rc<CompiledCode>> {
    let index = usize::try_from(index).ok()?;
    if index >= code.instruction_count() {
        return None;
    }
    match code.instruction(index) {
        Instruction::MakeClosure { code: made, .. } => Some(made),
        _ => None,
    }
}

/// The frame directly above `creator` walking down the senders of
/// `frame`, through the suspended ones -- `frame` itself when `creator` is
/// its sender: the callee of the send `creator` stands at, within which
/// `frame` runs. `None` when `creator` is not met.
fn callee_of<'gc>(
    mutator: &Mutator<'gc>,
    frame: Place<'gc>,
    creator: Place<'gc>,
) -> Option<Place<'gc>> {
    let heap = &mutator.heap;
    let below = |place: Place<'gc>| match place {
        Place::Running(activation) => sender_of(mutator, activation),
        Place::Suspended(frame) => heap.frame_sender(frame).map(Place::Suspended),
    };
    let mut above = frame;
    let mut next = below(frame);
    while let Some(place) = next {
        if same_place(mutator, place, creator) {
            return Some(above);
        }
        above = place;
        next = below(place);
    }
    None
}

/// Whether `first` and `second` are the same frame of the run.
fn same_place<'gc>(mutator: &Mutator<'gc>, first: Place<'gc>, second: Place<'gc>) -> bool {
    let heap = &mutator.heap;
    match (first, second) {
        (Place::Running(first), Place::Running(second)) => first.names(second),
        (Place::Suspended(first), Place::Suspended(second)) => {
            heap.frame_object(first).same(heap.frame_object(second))
        }
        _ => false,
    }
}

/// Whether the send whose callee is `callee` took the closure `frame`
/// runs, decided at the stop from what the send delivered: the callee is
/// the closure's own activation -- a `value` the send made -- or holds
/// that very closure as its receiver or as one of its parameters, the
/// registers below its body's argument count, which no body writes; or the
/// callee is a `doesNotUnderstand:` handler the send's lookup delivered
/// its arguments to as its `Message`'s -- `delivered`, a send of another
/// selector, whose `Message` the VM built -- and the closure is one of
/// them; a `Message` a program sends `doesNotUnderstand:` itself is an
/// argument like any other. The
/// sender's registers do not decide: the run loop reads an argument where
/// it stands and copies it into the send's slot only for a callee laid on
/// the slots, so a slot may hold what an earlier send took, and the
/// register a named local passed lives in is one an inlined form's
/// fallback closure can assign while the send runs. What the callee holds
/// is what it was given, but for a handler that stores into its own
/// `Message`'s arguments. A closure handed on in a variable or computed by
/// an expression is taken so as much as a literal; one run by another
/// send -- a sibling closure, a slot's -- is not.
fn took_closure<'gc>(
    mutator: &Mutator<'gc>,
    callee: Place<'gc>,
    frame: Place<'gc>,
    delivered: bool,
) -> bool {
    if same_place(mutator, callee, frame) {
        return true;
    }
    let heap = &mutator.heap;
    let closure = match frame {
        Place::Running(activation) => heap.activation_closure(activation),
        Place::Suspended(frame) => heap.frame_closure(frame),
    };
    let Some(closure) = closure else {
        return false;
    };
    let is_closure =
        |value: Value<'gc>| value.as_object().is_some_and(|object| object.same(closure));
    let (receiver, parameters, kind) = match callee {
        Place::Running(activation) => {
            let count = heap.activation_code(activation).argument_count;
            let parameters: Vec<Value<'gc>> = (0..count)
                .map(|index| heap.activation_local(activation, index))
                .collect();
            (
                heap.activation_receiver(activation),
                parameters,
                heap.activation_kind(activation),
            )
        }
        Place::Suspended(suspended) => {
            let count = heap
                .frame_code(suspended)
                .map_or(0, |code| code.argument_count);
            let parameters: Vec<Value<'gc>> = (0..count)
                .map(|index| heap.frame_local(suspended, index))
                .collect();
            (
                heap.frame_receiver(suspended),
                parameters,
                heap.frame_kind(suspended).activation_kind(),
            )
        }
    };
    if is_closure(receiver) || parameters.iter().copied().any(is_closure) {
        return true;
    }
    let handles_a_message = delivered
        && matches!(
            kind,
            ActivationKind::Method(selector) if selector == Symbol::DOES_NOT_UNDERSTAND
        );
    handles_a_message
        && parameters
            .first()
            .and_then(|message| message.as_object())
            .and_then(|message| mutator.kernel_slot(message, BuiltinClass::Message, "arguments"))
            .and_then(|arguments| arguments.as_object())
            .and_then(|arguments| heap.array_elements(arguments))
            .is_some_and(|arguments| arguments.into_iter().any(is_closure))
}

/// Whether a `doesNotUnderstand:` handler the send at `waiting_at` of
/// `code` pushes holds a `Message` the VM built of the send's arguments: a
/// send of any selector but `doesNotUnderstand:`, which a program's own
/// send of it hands the handler as its plain argument.
fn delivers_a_message(code: &CompiledCode, waiting_at: usize) -> bool {
    if waiting_at >= code.instruction_count() {
        return false;
    }
    match code.instruction(waiting_at) {
        Instruction::SendLexical { selector, .. }
        | Instruction::SendPublic { selector, .. }
        | Instruction::SendInlinable { selector, .. }
        | Instruction::SendSuper { selector, .. } => selector != Symbol::DOES_NOT_UNDERSTAND,
        _ => true,
    }
}

/// The frame that created the closure `frame` runs, while it runs or is
/// suspended; `None` for a frame that runs no closure, and for one whose
/// creator returned.
fn creator_of<'gc>(mutator: &Mutator<'gc>, frame: Place<'gc>) -> Option<Place<'gc>> {
    let heap = &mutator.heap;
    let captured = match frame {
        Place::Running(activation) => heap.activation_captured(activation),
        Place::Suspended(frame) => heap.frame_captured(frame),
    }?;
    match heap.frame_activation(captured) {
        Some(creator) => Some(Place::Running(creator)),
        None if heap.frame_is_suspended(captured) => Some(Place::Suspended(captured)),
        None => None,
    }
}

/// The body `place` runs, its declaration and its metadata
/// (`stack::running_body`).
fn place_body<'gc>(mutator: &mut Mutator<'gc>, place: Place<'gc>) -> FrameBody {
    match place {
        Place::Running(activation) => stack::running_body(mutator, activation),
        Place::Suspended(frame) => {
            let heap = &mutator.heap;
            let root = heap.stash(Value::object(heap.frame_object(frame)));
            stack::suspended_body(mutator, &root)
        }
    }
}

/// The index `place` stands at: the instruction it fetches next, or the
/// send it waits on below the top.
fn place_program_counter<'gc>(mutator: &Mutator<'gc>, place: Place<'gc>) -> usize {
    match place {
        Place::Running(activation) => mutator.heap.activation_program_counter(activation),
        Place::Suspended(frame) => mutator.heap.frame_stored_program_counter(frame),
    }
}

/// A step out leaves the send `frame` stands at for `after` in `code`,
/// its return point: a one-shot that stops and records for the frame,
/// beside the stepping frame's own return point, for a frame that returns
/// before the send has answered -- but records nothing after a setter
/// assignment's send, whose answer is the receiver (`answers_a_setter`).
fn stop_after_send<'gc>(
    mutator: &mut Mutator<'gc>,
    frame: Place<'gc>,
    code: &Rc<CompiledCode>,
    after: usize,
) {
    let records = !after
        .checked_sub(1)
        .is_some_and(|call| answers_a_setter(mutator, frame, code, call));
    let holder = match frame {
        Place::Running(activation) => Holder::Running(activation),
        Place::Suspended(frame) => Holder::Suspended(hold(mutator, frame)),
    };
    let one_shot = OneShot {
        holder,
        stops: true,
        records,
        leaves: false,
    };
    patch(mutator, code, after, one_shot);
    if let Some(stepping) = &mut mutator.stepping {
        stepping.after_send = Some((holder, after));
    }
}

/// A host run starts while the debugger's pause is pending, `entry` its
/// first activation, which stands at its first instruction: the pause
/// stops it at its first statement start -- where a step into stops a
/// pushed frame -- rather than at the run's first safepoint, the first
/// push the entry makes, so that a client's stop on entry stands in the
/// method the host sent. The frame is flooded as a step over's is, before
/// it runs anything, so the first one-shot it reaches is its first
/// statement's; that stop is the pause's (`is_at_entry`) and serves the
/// pause bit, which stays set meanwhile, so that a body without metadata,
/// which floods nothing, still stops at the first safepoint. A factory
/// the host sends, which has no statement start, is stopped at its entry
/// (`stop_at_factory_entry`). It stops in
/// the entry frame whatever the step filters say, as a pause stops where
/// the program is. The method exits of a step the run before carried
/// (`run_ended`), `exits`, are the pause's stop's to report, as a pause
/// in a run reports those of the step it ends.
pub(crate) fn stop_at_entry(
    mutator: &mut Mutator<'_>,
    entry: ActivationReference,
    exits: Vec<BreakpointId>,
) {
    end(mutator);
    start(
        mutator,
        StepAction::Over,
        Place::Running(entry),
        Place::Running(entry),
        None,
        None,
        None,
    );
    if let Some(stepping) = &mut mutator.stepping {
        stepping.started_filtered = true;
        stepping.at_entry = true;
    }
    // A factory the host sends has no statement start: the pause stops it
    // at its entry, as a step carried into its run does.
    stop_at_factory_entry(mutator, entry);
    keep_exits(mutator, exits);
    settle(mutator);
}

/// The debugger restarts `frame` (`Command::RestartFrame`), whose unwind
/// is about to start it again: before the unwind, a one-shot for it is
/// patched where a step into stops a pushed frame (`first_stop`) -- its
/// first statement start, index 1 past a vector it keeps, a factory's
/// first instruction, its entry, the first slot of an initializer whose
/// declaration writes no clause, and index 0 or 1 of a body without
/// metadata -- so that the stop is there, a step's. The frame is not
/// flooded: a flood patches only the statement starts the metadata names,
/// which a body without it has none of, and the implicit clause of a
/// clause-less initializer, where no step stops. Its return point is set
/// as a step out's, for an initializer whose implicit clause is all it
/// runs. It stops there whatever the step filters say, as a step begun in
/// a filtered frame steps in it; the frames the unwind pops above it are
/// reported to the step as any unwind's are.
pub(crate) fn stop_at_restart(mutator: &mut Mutator<'_>, frame: &StackFrame) {
    end(mutator);
    let Some(place) = place_of(mutator, frame) else {
        return;
    };
    start(mutator, StepAction::Out, place, place, None, None, None);
    let Some(stepping) = &mut mutator.stepping else {
        return;
    };
    stepping.started_filtered = true;
    let holder = stepping.frame;
    let code = stack::code(&mutator.heap, frame);
    if let Some(first) = first_stop(mutator, frame, &code) {
        let one_shot = OneShot {
            holder,
            stops: true,
            records: false,
            leaves: false,
        };
        patch(mutator, &code, first, one_shot);
    }
    settle(mutator);
}

/// Whether the step in progress stands for a pause a host run started
/// with (`stop_at_entry`).
pub(crate) fn is_at_entry(mutator: &Mutator<'_>) -> bool {
    mutator
        .stepping
        .as_ref()
        .is_some_and(|stepping| stepping.at_entry)
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
/// request bit dropped. Answers the value the step recorded last. A step
/// the last host run carried to the next (`run_ended`) ends too, so that
/// whatever ends a step between runs -- a `Terminate`, a detach, a pause
/// asked between them -- ends that one.
pub(crate) fn end(mutator: &mut Mutator<'_>) -> Option<Returned> {
    let stepping = mutator.stepping.take();
    mutator.breakpoints.take_carried_step();
    mutator.breakpoints.remove_one_shots();
    drop_request(mutator);
    settle(mutator);
    stepping.and_then(|stepping| stepping.returned)
}

/// A host run ends with every activation it ran popped: a step still in
/// progress has left the run without a stop -- its frame returned to the
/// host, or an unwind popped it there -- and goes on in the program's
/// next run, the next place the program reaches (decision 17): it ends
/// here, its frames gone, and its method exits are carried, for the next
/// run's start to go on with (`carry_into`). Its value of the last call
/// is not: it names a call of a run that is over. A step standing for a
/// pause the run started with (`stop_at_entry`) is not carried either:
/// the pause bit its stop would have served stays set, and stops the next
/// run itself.
pub(crate) fn run_ended(mutator: &mut Mutator<'_>) {
    let carried = mutator
        .stepping
        .as_mut()
        .filter(|stepping| !stepping.at_entry)
        .map(|stepping| std::mem::take(&mut stepping.exits));
    end(mutator);
    if let Some(exits) = carried {
        mutator.breakpoints.carry_step(exits);
    }
}

/// A host run starts, `entry` its first activation, which stands at its
/// first instruction, while the run before carried a step (`run_ended`):
/// the step goes on as a step into from the entry, so that it stops at
/// the entry's first statement start -- or at that of a frame the entry
/// pushes before it reaches one -- as the step filters allow (decision
/// 16), a step's stop, which reports `exits` too.
pub(crate) fn carry_into(
    mutator: &mut Mutator<'_>,
    entry: ActivationReference,
    exits: Vec<BreakpointId>,
) {
    continue_as_step_into(mutator, entry);
    keep_exits(mutator, exits);
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
    if let Some((caller, index)) = stepping.return_point
        && gone(caller)
    {
        stepping.return_point = Some((Holder::Left, index));
    }
    if let Some((holder, index)) = stepping.after_send
        && gone(holder)
    {
        stepping.after_send = Some((Holder::Left, index));
    }
    if stepping.begun_in.is_some_and(gone) {
        stepping.begun_in = None;
    }
    if stepping.origin.is_some_and(gone) {
        stepping.origin = None;
        stepping.origin_left = true;
    }
    stepping.between.retain(|(held, _)| !gone(*held));
    mutator.breakpoints.forget_one_shot_holders(gone);
}

/// A frame popped otherwise than by a return to its caller's send, and
/// `now_on_top`, the frame the program goes on in (decision 17): the
/// popped frame's names are forgotten first (`popped`). When it was the
/// stepping frame, or the caller its return point is in, the step goes on
/// as a step into from `now_on_top`, so that it stops at the next
/// statement start the program reaches: in the caller an instruction
/// runs again in, the frame a superclass clause's continuation goes on
/// in, or what that pushes -- an unwind's pops are reported one by one
/// (`popped`) and its stop once (`unwind_stopped`). Any other frame the step named above the stepping
/// one -- one the step was taken in, a pushed frame's one-shot -- is only
/// forgotten. `now_on_top` is `None` when no frame goes on yet -- the run
/// ends, whose end carries the step to the next run (`run_ended`), or an
/// error is about to be signalled, whose handler search, or the unwind it
/// starts, says where the program goes on; the step's frame stays
/// `Holder::Left` until then.
pub(crate) fn frame_left(mutator: &mut Mutator<'_>, now_on_top: Option<ActivationReference>) {
    popped(mutator);
    went_on(mutator, now_on_top);
}

/// An unwind stopped popping, its pops reported one by one as they were
/// made (`popped`): the program goes on in `now_on_top` -- the frame the
/// unwind arrived at, the `ensure:` or `ifCurtailed:` frame whose block it
/// runs, or the frame below the run's floor -- and a step whose frame, or
/// its return point's caller, the pops left goes on as a step into from
/// there (`frame_left`), once for the whole unwind: flooding each frame
/// it passed on the way would patch bodies no stop is made in -- every
/// frame of a deep recursion it unwinds -- and free their machine code.
pub(crate) fn unwind_stopped(mutator: &mut Mutator<'_>, now_on_top: Option<ActivationReference>) {
    went_on(mutator, now_on_top);
}

/// `frame_left` once the pop is forgotten: the step goes on as a step
/// into from `now_on_top` if its frame or its return point's caller left.
fn went_on(mutator: &mut Mutator<'_>, now_on_top: Option<ActivationReference>) {
    let Some(stepping) = &mutator.stepping else {
        return;
    };
    match (stepping.has_left(), now_on_top) {
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
    #[cfg(debug_assertions)]
    if matches!(place, Place::Suspended(_)) {
        mutator.breakpoints.count_step_from_suspended();
    }
    start(
        mutator,
        StepAction::Into,
        place,
        place,
        returned,
        None,
        None,
    );
    if let Some(stepping) = &mut mutator.stepping {
        stepping.went_on_elsewhere = true;
    }
    if let Place::Running(frame) = place {
        stop_at_factory_entry(mutator, frame);
    }
    keep_exits(mutator, exits);
    settle(mutator);
}

/// Whether the step in progress went on as a step into from another frame
/// than the one it was taken in (`Stepping::went_on_elsewhere`), or the
/// activation it was taken in left while the step stepped in another
/// frame (`Stepping::origin_left`): its stop stands in another activation.
/// Read by the step's stop before it ends the step.
pub(crate) fn went_on_elsewhere(mutator: &Mutator<'_>) -> bool {
    mutator
        .stepping
        .as_ref()
        .is_some_and(|stepping| stepping.went_on_elsewhere || stepping.origin_left)
}

/// Notes `top`, the activation a step was just begun in, as its origin
/// when the step steps in another frame -- the creator of the fallback
/// closure `top` runs -- and patches where `top`'s caller goes on with a
/// one-shot that notes its return and never stops: a block a receiver
/// runs again may stop the step in a new activation at `top`'s offset, at
/// the same depth, which only that return tells from `top`.
fn note_origin(mutator: &mut Mutator<'_>, top: ActivationReference) {
    let origin = Holder::Running(top);
    let steps_elsewhere = mutator
        .stepping
        .as_ref()
        .is_some_and(|stepping| !stepping.frame.same_frame(origin));
    if !steps_elsewhere {
        return;
    }
    let Some((caller, code, point, _)) = return_slot(mutator, origin) else {
        return;
    };
    if let Some(stepping) = &mut mutator.stepping {
        stepping.origin = Some(origin);
    }
    let one_shot = OneShot {
        holder: caller,
        stops: false,
        records: false,
        leaves: true,
    };
    patch(mutator, &code, point, one_shot);
}

/// The activation a step was taken in returned to its caller
/// (`OneShot::leaves`).
pub(crate) fn origin_returned(mutator: &mut Mutator<'_>) {
    if let Some(stepping) = &mut mutator.stepping
        && stepping.origin.take().is_some()
    {
        stepping.origin_left = true;
    }
}

/// A step into that goes on from `frame`, when `frame` is a factory's
/// that has not run yet -- the first activation of a host run that sends
/// a factory, which a step the run before left goes on from (decision
/// 17), or which a pause asked before the run stops (`stop_at_entry`) --
/// stops at its entry, as one that pushes the factory does
/// (`held_step`): the factory's one place, since it has no statement
/// start for the flood to patch. A one-shot at its first instruction,
/// which it fetches next, makes the stop. A filtered factory -- a shipped
/// class's -- is none a step stops in.
fn stop_at_factory_entry(mutator: &mut Mutator<'_>, frame: ActivationReference) {
    let heap = &mutator.heap;
    if !stack::is_factory(heap, &StackFrame::Running(frame))
        || heap.activation_program_counter(frame) != 0
        || is_filtered(mutator, &StackFrame::Running(frame))
    {
        return;
    }
    let code = mutator.heap.activation_code(frame);
    patch(
        mutator,
        &code,
        0,
        OneShot {
            holder: Holder::Running(frame),
            stops: true,
            records: false,
            leaves: false,
        },
    );
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
    let left = stepping.has_left();
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
        if held.is_some_and(|held| stepping.frame.same_frame(held)) {
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
fn carried(mutator: &mut Mutator<'_>) -> (Option<Returned>, Vec<BreakpointId>) {
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
/// -- a step into from the frame the unwind stops in for a popped frame,
/// a step out from the method's frame for one that returns -- and whose
/// stop reports the exits (`take_exits`), or an exception's stop served
/// first, beside its exception. For a popped frame the step waits, its
/// frame `Holder::Left`, until the unwind stops popping
/// (`unwind_stopped`), so that no frame it passes is flooded.
pub(crate) fn method_left(mutator: &mut Mutator<'_>, ids: Vec<BreakpointId>, leaving: Leaving) {
    match leaving {
        Leaving::Popped => match &mut mutator.stepping {
            Some(stepping) => stepping.frame = Holder::Left,
            None => {
                mutator.stepping = Some(Box::new(Stepping::new(StepAction::Into, None)));
            }
        },
        Leaving::Returning(home) => {
            let (returned, exits) = carried(mutator);
            mutator.breakpoints.remove_one_shots();
            let home = Place::Running(home);
            start(mutator, StepAction::Out, home, home, returned, None, None);
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
/// reached: the step could never stop. The same holds of the frame whose
/// send a step out of a block leaves (`Stepping::after_send`), which
/// stays on the stack until it fetches its one-shot after the send, or
/// is renamed as it leaves otherwise. For the step stress mode's check as
/// it ends a step itself.
#[cfg(debug_assertions)]
pub(crate) fn frame_still_there(mutator: &Mutator<'_>) -> bool {
    let Some(stepping) = &mutator.stepping else {
        return true;
    };
    let on_the_stack = |held: ActivationReference| {
        let mut next = mutator.heap.top_activation();
        while let Some(live) = next {
            if held.names(live) {
                return true;
            }
            next = mutator.heap.activation_sender(live);
        }
        false
    };
    let frame_there = match stepping.frame {
        Holder::Running(held) => on_the_stack(held) || at_return_point(mutator),
        Holder::Suspended(_) | Holder::Left => true,
    };
    let send_there = match stepping.after_send {
        Some((Holder::Running(held), _)) => on_the_stack(held),
        Some((Holder::Suspended(_) | Holder::Left, _)) | None => true,
    };
    frame_there && send_there
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
/// reports, taken: its stop is the method's exit. Only those still set
/// are answered -- a `SetMethodBreakpoints` served since the unwind, in
/// this run or between runs for a carried step, may have removed one,
/// whose id the client no longer knows.
pub(crate) fn take_exits(mutator: &mut Mutator<'_>) -> Vec<BreakpointId> {
    let mut exits = mutator
        .stepping
        .as_mut()
        .map(|stepping| std::mem::take(&mut stepping.exits))
        .unwrap_or_default();
    exits.retain(|id| mutator.breakpoints.has_method_breakpoint(*id));
    exits
}

/// The step's own one-shot stopped `activation`, the top frame, where no
/// line breakpoint stands: answers whether the step goes on instead,
/// because the step filters filter the frame (decision 16) -- a return
/// point inside the shipped `Array>>do:` after a step over at a block's
/// last statement, a statement of a filtered body an unwind goes on in
/// -- as a step into from the first unfiltered frame below
/// (`continue_as_step_into`). A frame the step was begun in, stopped
/// there by a breakpoint or with the filter off, is stepped in as any
/// other -- an initializer at its header stop too, whose step steps from
/// its sender (`Stepping::begun_in`) -- and so is a block the compiler
/// lowered inline in it. A
/// factory's frame past its entry -- at its return point after a step
/// over or out at its initializer's last statement -- is passed over too,
/// whatever the filters say and wherever the step began (decision 12):
/// the step goes on as a step into from that frame, whose return point
/// in its caller, or the next run's start when it returns to the host, is
/// where it stops. Its entry, the one place a step stops in it, is not
/// passed (`stop_at_factory_entry`). So is `Object`'s initializer, and
/// `Top`'s, always (`passed_by_every_step`).
pub(crate) fn went_on_past_filtered(
    mutator: &mut Mutator<'_>,
    activation: ActivationReference,
) -> bool {
    let Some(stepping) = &mutator.stepping else {
        return false;
    };
    let (frame, started_filtered, begun_in) =
        (stepping.frame, stepping.started_filtered, stepping.begun_in);
    let begun_here = |mutator: &Mutator<'_>| {
        takes(mutator, frame, activation)
            || begun_in.is_some_and(|begun| begun.same_frame(Holder::Running(activation)))
    };
    let past_a_factory_s_entry = stack::is_factory(&mutator.heap, &StackFrame::Running(activation))
        && mutator.heap.activation_program_counter(activation) > 0;
    let passes = past_a_factory_s_entry
        || passed_by_every_step(mutator, activation)
        || (!(started_filtered && begun_here(mutator))
            && is_filtered(mutator, &StackFrame::Running(activation)));
    if !passes {
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
/// or its initializer's level says (`stack::origin`) -- the version that
/// holds the frame's body, so that the decision is the body's, and kept
/// by it (`FilterDecisions`).
pub(crate) fn is_filtered(mutator: &Mutator<'_>, frame: &StackFrame) -> bool {
    let filters = mutator.breakpoints.step_filters();
    if !filters.shipped && filters.classes.is_empty() {
        return false;
    }
    let code = stack::code(&mutator.heap, frame);
    let decisions = mutator.breakpoints.filter_decisions();
    if let Some(filtered) = decisions.get(&code) {
        return filtered;
    }
    let filtered = decide_filtered(mutator, frame, filters);
    decisions.record(&code, filtered);
    filtered
}

/// `is_filtered`, decided afresh.
fn decide_filtered(mutator: &Mutator<'_>, frame: &StackFrame, filters: &StepFilters) -> bool {
    let Some(origin) = stack::origin(&mutator.heap, frame) else {
        return false;
    };
    let Some((root, path)) = declaration_path(mutator, &origin.mixin) else {
        return false;
    };
    (filters.shipped && root.shipped)
        || filters.classes.iter().any(|class| {
            class.module.names(&root)
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
/// index 0, or at a one-shot at the first statement start a later
/// instruction begins (`first_stop`). A factory's push stops in the
/// factory, at its entry, though it has no statement start (decision 12):
/// here, when the run loop switched to it, or, when the send went on at
/// once into the initializer's push, in the factory again once that push
/// is undone (`rewound_into_factory`); the push of an initializer a clause
/// entered stops at its entry, its header stop (`enters_at_header`); and
/// `Object`'s and `Top`'s initializers keep the request, as a filtered
/// body does (`passed_by_every_step`). Anything else -- a backward jump, a
/// caller resumed -- goes on without stopping.
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
        .any(|(held, _)| held.same_frame(Holder::Running(top)));
    if was_there || inline_code_of_step(mutator, top) {
        return HeldStep::GoOn;
    }
    if rewound_into_factory(mutator, top) {
        drop_request(mutator);
        return HeldStep::StopInFactory;
    }
    // A filtered body keeps the request: the step goes on through it to
    // the next body it pushes that it stops in, itself or through others.
    // So does `Object`'s initializer, and `Top`'s, whatever the filters.
    if is_filtered(mutator, &StackFrame::Running(top))
        || passed_by_every_step(mutator, top)
        || !sent_by_step(mutator, top)
    {
        return HeldStep::GoOn;
    }
    drop_request(mutator);
    if enters_at_header(mutator, top) {
        return HeldStep::StopAtHeader;
    }
    match stop_at_first(mutator, top, true) {
        FirstStop::Standing => HeldStep::Stop,
        FirstStop::Patched | FirstStop::Nowhere => HeldStep::GoOn,
    }
}

/// Where `stop_at_first` stops a pushed frame.
enum FirstStop {
    /// It stands at its first statement start: the safepoint stops it.
    Standing,
    /// At a one-shot for it at its first statement start.
    Patched,
    /// Nowhere in it: it runs nothing a step stops at.
    Nowhere,
}

/// Stops `top`, a frame the step pushed, at its first statement start
/// (`first_stop`): `Standing` when it stands there already and
/// `standing_stops` -- the safepoint stops it there -- and otherwise a
/// one-shot there, which the frame reaches as it goes on, or fetches next
/// when it stands there. An initializer with nothing but the clause that
/// stands for `Object new` runs nothing a step stops at: nothing is set,
/// and the step stops where the stepping frame goes on, after the `new`.
fn stop_at_first(
    mutator: &mut Mutator<'_>,
    top: ActivationReference,
    standing_stops: bool,
) -> FirstStop {
    let code = mutator.heap.activation_code(top);
    let Some(first) = first_stop(mutator, &StackFrame::Running(top), &code) else {
        return FirstStop::Nowhere;
    };
    if standing_stops && mutator.heap.activation_program_counter(top) >= first {
        return FirstStop::Standing;
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
            leaves: false,
        },
    );
    settle(mutator);
    FirstStop::Patched
}

/// Whether `top` is the initializer a factory the step pushed went on
/// into with no safepoint between, and its push is undone: a factory sent
/// from a frame runs its first two instructions as part of the send
/// (`Mutator::enter_factory_body`), so the first safepoint after the
/// factory's push is its initializer's, the factory standing at its
/// `InitializeInstance` (`instantiate::FACTORY_INITIALIZE`). The step
/// stops in the factory there, its one place (decision 12): the
/// initializer, which has run nothing -- its frame stands at index 0,
/// never reified -- is popped as an unwind pops a frame, and the factory
/// runs its `InitializeInstance` again once the program goes on, which
/// decides and pushes the same initializer, since deciding it changes
/// nothing and the instance is in its register already. Every other push
/// is left alone: a factory the run loop switched to stopped at its own
/// push, and a filtered one -- a shipped class's -- passes, its
/// initializer with it.
fn rewound_into_factory(mutator: &mut Mutator<'_>, top: ActivationReference) -> bool {
    let heap = &mutator.heap;
    let fresh_initializer = matches!(heap.activation_kind(top), ActivationKind::Initializer(..))
        && heap.activation_program_counter(top) == 0
        && heap.activation_reified(top).is_none()
        && heap.activation_suspended_sender(top).is_none();
    if !fresh_initializer {
        return false;
    }
    let Some(factory) = heap.activation_sender(top) else {
        return false;
    };
    let at_its_initializer = stack::is_factory(heap, &StackFrame::Running(factory))
        && heap.activation_program_counter(factory) == FACTORY_INITIALIZE;
    let steps_in_it = mutator
        .stepping
        .as_ref()
        .is_some_and(|stepping| stepping.frame.same_frame(Holder::Running(factory)));
    if !at_its_initializer
        || steps_in_it
        || is_filtered(mutator, &StackFrame::Running(factory))
        || !sent_by_step(mutator, factory)
    {
        return false;
    }
    mutator.pop_for_unwind_as::<false>(top);
    true
}

/// Whether `top`, an initializer a step into just pushed, stops at its
/// header (decision 12): it stands at its entry and was entered for
/// another level than its sender's own -- its sender, an initializer,
/// stands at a superclass clause's `InitializeSuperclass` or a `<:`
/// clause's `InitializeAppliedLevel`, or a factory of another class, whose
/// own levels run nothing (`instantiate::enter_or_pass`), pushed the
/// first initializer above them that does. An initializer its own class's
/// factory pushes has no header stop: the factory's stop is that level's,
/// and the step from it stops at the initializer's first statement.
fn enters_at_header(mutator: &Mutator<'_>, top: ActivationReference) -> bool {
    let heap = &mutator.heap;
    let ActivationKind::Initializer(level, _) = heap.activation_kind(top) else {
        return false;
    };
    if heap.activation_program_counter(top) != 0 {
        return false;
    }
    let (kind, code) = match sender_of(mutator, top) {
        Some(Place::Running(sender)) => {
            (heap.activation_kind(sender), heap.activation_code(sender))
        }
        Some(Place::Suspended(sender)) => match heap.frame_code(sender) {
            Some(code) => (heap.frame_kind(sender).activation_kind(), code),
            None => return false,
        },
        None => return false,
    };
    match kind {
        ActivationKind::Factory(class) => !class.same(level),
        ActivationKind::Initializer(..) => {
            let Some(sender) = sender_of(mutator, top) else {
                return false;
            };
            let index = place_program_counter(mutator, sender);
            index < code.instruction_count()
                && matches!(
                    code.instruction(index),
                    Instruction::InitializeSuperclass { .. }
                        | Instruction::InitializeAppliedLevel { .. }
                )
        }
        ActivationKind::Method(_)
        | ActivationKind::Closure(_)
        | ActivationKind::TopLevel
        | ActivationKind::SuperclassClause(_) => false,
    }
}

/// Whether `activation` runs `Object`'s initializer or `Top`'s, which no
/// step stops in, whatever the step filters say (decided with the user on
/// 2026-11-15): the header stops of a superclass chain end at the last
/// class that inherits from `Object` directly. Decided by the level the
/// initializer initializes, the kernel's `Object` or `Top`, not by its
/// module's being shipped.
fn passed_by_every_step(mutator: &Mutator<'_>, activation: ActivationReference) -> bool {
    let ActivationKind::Initializer(level, _) = mutator.heap.activation_kind(activation) else {
        return false;
    };
    [BuiltinClass::Object, BuiltinClass::Top]
        .into_iter()
        .any(|kind| mutator.builtin_class(kind).same(level))
}

/// Begins a step of `action` at an initializer's header stop
/// (`HeldStep::StopAtHeader`), which stands at the initializer's entry. A
/// step out leaves the initializer as from any stop, for its sender after
/// the clause. A step over or into goes on, as a step into from the
/// sender, the level whose clause pushed the initializer, to the
/// initializer's first statement (`first_stop`), as the push would have
/// stopped it: the frame stands at its entry, which is not let past,
/// since its first statement may start there; an initializer with nothing
/// but the clause that stands for `Object new` lets the step stop where
/// the sender goes on, after its clause. An initializer without metadata
/// -- an image's without source -- has no statement start to tell its
/// header from: its header stop is the one its push makes, and the step
/// goes on from it as from that.
pub(crate) fn begin_at_header(mutator: &mut Mutator<'_>, action: StepAction) {
    let top = mutator.heap.top_activation();
    // The metadata first, which may compile, before any handle is read.
    let placed = top.is_some_and(|top| stack::running_body(mutator, top).2.is_some());
    let sender = top.and_then(|top| sender_of(mutator, top));
    let (Some(top), Some(sender), StepAction::Over | StepAction::Into, true) =
        (top, sender, action, placed)
    else {
        begin(mutator, action, None);
        return;
    };
    let started_filtered = is_filtered(mutator, &StackFrame::Running(top));
    end(mutator);
    // `top` stands above the sender but is left out of `between`, which
    // only a held request reads: the request is dropped at once.
    start(mutator, StepAction::Into, sender, sender, None, None, None);
    // The push the request waits for has happened: the header stop was
    // its stop.
    drop_request(mutator);
    // The step was begun in the initializer, and steps in it as in any
    // other frame should the filters filter it (decision 16) -- only while
    // a one-shot of its own waits in it, which it reaches before it can
    // return: a return to its caller's send reports no pop (`popped`), so
    // a name of it kept past its return would name the next activation
    // pushed at its offset. An initializer that runs nothing a step stops
    // at leaves the step to its sender, begun nowhere filtered.
    if let FirstStop::Patched = stop_at_first(mutator, top, false)
        && let Some(stepping) = &mut mutator.stepping
    {
        stepping.started_filtered = started_filtered;
        stepping.begun_in = Some(Holder::Running(top));
    }
    settle(mutator);
}

/// The index a step into stops at in `frame`, the frame it just pushed --
/// or one the debugger restarts (`stop_at_restart`) -- running `code`: its first statement start -- index 0, or index 1 for a
/// body whose first instruction makes its indirection vector
/// (`MakeVector`, emitted before every statement) -- but for an
/// initializer of a declaration that writes no superclass clause, whose
/// first statement is the clause that stands for `Object new` (§6.1),
/// placed at the class's name: no step stops there, and the push stops at
/// the first statement start after it, `None` when there is none. The
/// metadata says where the statements are; a body without it, an image's
/// without source, stops where it stands.
fn first_stop(
    mutator: &mut Mutator<'_>,
    frame: &StackFrame,
    code: &Rc<CompiledCode>,
) -> Option<usize> {
    let first = first_statement(code);
    if !stack::is_initializer(&mutator.heap, frame) {
        return Some(first);
    }
    let (_, origin, metadata) = stack::body(mutator, frame);
    let (Some(origin), Some(metadata)) = (origin, metadata) else {
        return Some(first);
    };
    let writes_no_clause = origin
        .mixin
        .declaration
        .as_ref()
        .is_some_and(|declaration| declaration.superclass.is_none());
    let clause = (0..code.instruction_count())
        .find(|index| {
            matches!(
                code.instruction(*index),
                Instruction::InitializeSuperclass { .. }
            )
        })
        .and_then(|index| u32::try_from(index).ok());
    let (true, Some(clause)) = (writes_no_clause, clause) else {
        return Some(first);
    };
    let Some(implicit) = metadata
        .statements
        .iter()
        .filter(|entry| entry.instructions.contains(&clause))
        .max_by_key(|entry| entry.instructions.len())
    else {
        return Some(first);
    };
    metadata
        .statements
        .iter()
        .map(|entry| entry.instructions.start)
        .filter(|start| *start >= implicit.instructions.end)
        .min()
        .and_then(|start| usize::try_from(start).ok())
}

/// The first statement start of `code`: index 0, or index 1 for a body
/// whose first instruction makes its indirection vector (`MakeVector`,
/// emitted before every statement).
fn first_statement(code: &CompiledCode) -> usize {
    usize::from(
        code.instruction_count() > 1
            && matches!(code.instruction(0), Instruction::MakeVector { .. }),
    )
}

/// Whether the step's frame, or a block the step flooded for it, sent to
/// `top`: its sender is one of them, or a frame the step filters filter
/// whose sender is, and so on down: the shipped `do:` the stepping frame
/// sent, which runs a block of the program. A frame between the one the
/// step was taken in and the stepping frame, and a frame the step stops
/// in, end the walk: what they send is not the step's. A smart step into
/// a send takes only what the walk reaches standing at that send
/// (`stands_at_aim`).
fn sent_by_step(mutator: &Mutator<'_>, top: ActivationReference) -> bool {
    let Some(stepping) = &mutator.stepping else {
        return false;
    };
    let stepping_frame = stepping.frame;
    let between: Vec<Holder> = stepping.between.iter().map(|(held, _)| *held).collect();
    let is_between = |held: Holder| between.iter().any(|other| other.same_frame(held));
    let Some(mut sender) = sender_of(mutator, top) else {
        return false;
    };
    loop {
        sender = match sender {
            Place::Running(activation) => {
                let held = Holder::Running(activation);
                if stepping_frame.same_frame(held) || inline_code_of_step(mutator, activation) {
                    let heap = &mutator.heap;
                    let standing = (
                        heap.activation_code(activation),
                        heap.activation_program_counter(activation),
                    );
                    return stands_at_aim(mutator, Some(standing));
                }
                if is_between(held) || !is_filtered(mutator, &StackFrame::Running(activation)) {
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
                    if stepping_frame.same_frame(held) {
                        let heap = &mutator.heap;
                        let standing = heap
                            .frame_code(frame)
                            .map(|code| (code, heap.frame_stored_program_counter(frame)));
                        return stands_at_aim(mutator, standing);
                    }
                    if is_between(held) {
                        return false;
                    }
                }
                let heap = &mutator.heap;
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

/// Whether a frame of the step standing at `standing` -- its body and
/// the slot it stands at, a sender's being its call, or `None` where its
/// body is not known -- sends what the step stops in: any call for a step
/// into, only the chosen send for a smart step into a send (decision 3).
fn stands_at_aim(mutator: &Mutator<'_>, standing: Option<(Rc<CompiledCode>, usize)>) -> bool {
    match mutator
        .stepping
        .as_ref()
        .and_then(|stepping| stepping.aim.as_ref())
    {
        Some(Aim::Send { code: aimed, index }) => {
            standing.is_some_and(|(code, standing)| Rc::ptr_eq(aimed, &code) && *index == standing)
        }
        Some(Aim::Block { .. }) | None => true,
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
    if holder.same_frame(Holder::Running(running)) {
        return true;
    }
    let heap = &mutator.heap;
    let mut captured = heap.activation_captured(running);
    while let Some(frame) = captured {
        let creator = match heap.frame_activation(frame) {
            Some(creator) => Some(Holder::Running(creator)),
            None => suspended_key(mutator, frame).map(Holder::Suspended),
        };
        if creator.is_some_and(|creator| holder.same_frame(creator)) {
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
/// read. The call is noted beside the value, which names it at the
/// stop.
pub(crate) fn record(
    mutator: &mut Mutator<'_>,
    activation: ActivationReference,
    code: &CompiledCode,
    index: usize,
) {
    let Some(previous) = index.checked_sub(1) else {
        return;
    };
    let Some((register, call)) = call_of(&code.instruction(previous)) else {
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
    // Only the call is noted here, as cheaply as the value: its name is
    // read from the body's metadata at the stop, once, rather than at
    // every call a step over a loop runs past. The declaration a body
    // belongs to is the body's, so the previous call's is taken again for
    // a call of the same body, every turn of a loop.
    let code = mutator.heap.activation_code(activation);
    let Some(stepping) = &mut mutator.stepping else {
        return;
    };
    let same_body = stepping
        .returned
        .take()
        .filter(|returned| Rc::ptr_eq(&returned.call.code, &code));
    let origin = match same_body {
        Some(returned) => returned.call.origin,
        None => stack::origin(&mutator.heap, &StackFrame::Running(activation)),
    };
    let value = mutator.heap.activation_local(activation, register);
    let root = mutator.heap.stash(value);
    if let Some(stepping) = &mut mutator.stepping {
        stepping.returned = Some(Returned {
            value: root,
            call: RecordedCall {
                code,
                index: previous,
                call,
                origin,
            },
        });
    }
}

/// The value a step recorded as its last call's answer
/// (`Stepping::returned`), and the call.
pub(crate) struct Returned {
    pub(crate) value: Root,
    pub(crate) call: RecordedCall,
}

/// Where the call a step recorded stands, which names it at the stop
/// (`stack::call_name`): the body, the call instruction's index in it and
/// its kind, and the declaration the body belongs to. It holds no
/// handle, so that it outlives the frame it was recorded in, which a step
/// over a method's last statement leaves before its stop.
pub(crate) struct RecordedCall {
    pub(crate) code: Rc<CompiledCode>,
    pub(crate) index: usize,
    pub(crate) call: Call,
    pub(crate) origin: Option<BodyOrigin>,
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
    named.extend(stepping.after_send.map(|(holder, _)| holder));
    named.extend(stepping.begun_in);
    named.extend(stepping.origin);
    // Walked over the live activations, which alone may be read: a handle
    // the step holds is compared, never read through.
    let mut next = mutator.heap.top_activation();
    while let Some(live) = next {
        if mutator.heap.activation_position(live) < floor {
            break;
        }
        let suspended_sender = mutator.heap.activation_suspended_sender(live).is_some();
        let sender = mutator.heap.activation_sender(live);
        if named
            .iter()
            .any(|held| held.same_frame(Holder::Running(live)))
        {
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
            if held.same_frame(Holder::Running(activation)) {
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
/// frame's, its return point's, the frame it was begun in, its origin and
/// its one-shots'.
fn rename(mutator: &mut Mutator<'_>, from: Holder, onto: Holder) {
    if let Some(stepping) = &mut mutator.stepping {
        if stepping.frame.same_frame(from) {
            stepping.frame = onto;
        }
        for (held, position) in &mut stepping.between {
            if held.same_frame(from) {
                *held = onto;
                *position = None;
            }
        }
        if let Some((caller, index)) = stepping.return_point
            && caller.same_frame(from)
        {
            stepping.return_point = Some((onto, index));
        }
        if let Some((holder, index)) = stepping.after_send
            && holder.same_frame(from)
        {
            stepping.after_send = Some((onto, index));
        }
        if stepping
            .begun_in
            .is_some_and(|begun| begun.same_frame(from))
        {
            stepping.begun_in = Some(onto);
        }
        if stepping
            .origin
            .is_some_and(|origin| origin.same_frame(from))
        {
            stepping.origin = Some(onto);
        }
    }
    mutator.breakpoints.rename_one_shots(from, onto);
}

/// Starts the step of `action` taken in `taken_in`, at or below `top`,
/// the frame on top, `returned` the value recorded so far: in the frame
/// `stepping_frame` finds, flooded unless the step goes out -- or for a
/// step out that leaves an inlined form, `passed`, flooded but for the
/// form's own blocks -- its return point set, and the request held for a
/// step into; the frames from `top` down to it are there already, none of
/// them a push of the step's. A smart step into's flood leaves out the
/// statement starts of its `statement`. The caller settles the table.
fn start<'gc>(
    mutator: &mut Mutator<'gc>,
    action: StepAction,
    top: Place<'gc>,
    taken_in: Place<'gc>,
    returned: Option<Returned>,
    passed: Option<&LeftBlocks>,
    statement: Option<&StatementRegion>,
) {
    // Its frame is named by `stepping_frame`, which may hold `taken_in` as
    // a suspended frame first.
    mutator.stepping = Some(Box::new(Stepping::new(action, returned)));
    let (frame, body) = stepping_frame(mutator, taken_in);
    let between = frames_between(mutator, top, frame);
    if let Some(stepping) = &mut mutator.stepping {
        stepping.frame = frame;
        stepping.between = between;
    }
    if action != StepAction::Out || passed.is_some() {
        flood(mutator, frame, body, passed, statement);
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
        if frame.same_frame(Holder::Running(live)) {
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
        if frame.same_frame(held) {
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
/// recording one (`record`), but for the blocks of the inlined form a step
/// out leaves, `passed`. A body without metadata has no statement starts
/// to flood.
fn flood(
    mutator: &mut Mutator<'_>,
    frame: Holder,
    body: FrameBody,
    passed: Option<&LeftBlocks>,
    statement: Option<&StatementRegion>,
) {
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
        let flooded = |index: &usize| {
            *index < count && !passed.is_some_and(|passed| passed.passes_over(&body, *index))
        };
        let stops: Vec<usize> = metadata
            .statements
            .iter()
            .filter(|entry| !entry.instructions.is_empty())
            .filter_map(|entry| usize::try_from(entry.instructions.start).ok())
            .filter(flooded)
            .filter(|index| !statement.is_some_and(|statement| statement.holds(&body, *index)))
            .collect();
        // A setter's answer is its receiver, never the value its
        // assignment answers: the assigned value's call stays the last.
        let returns: Vec<usize> = metadata
            .sends
            .iter()
            .filter(|send| !send.setter)
            .filter_map(|send| usize::try_from(send.index).ok())
            .map(|index| index + 1)
            .filter(flooded)
            .collect();
        for index in stops {
            let one_shot = OneShot {
                holder: frame,
                stops: true,
                records: false,
                leaves: false,
            };
            patch(mutator, &body, index, one_shot);
        }
        for index in returns {
            let one_shot = OneShot {
                holder: frame,
                stops: false,
                records: true,
                leaves: false,
            };
            patch(mutator, &body, index, one_shot);
        }
        for block in inlined_blocks(&body, &metadata) {
            if passed.is_some_and(|passed| passed.passes_over_block(&block)) {
                continue;
            }
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
            && (*receiver_register == register || argument_sources.contains(&register))
        {
            return true;
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
/// `resume_caller` reads again -- recording nothing where that call is a
/// setter assignment's send, whose answer is the receiver, as a step
/// over's flood records none (`answers_a_setter`). A caller suspended
/// into its frame is
/// resumed as an activation of its own once the frame returns to it,
/// which the one-shot waits for (`resumed`). None for a frame without a
/// caller.
fn set_return_point(mutator: &mut Mutator<'_>, frame: Holder) {
    let Some((caller, code, point, setter)) = return_slot(mutator, frame) else {
        return;
    };
    let one_shot = OneShot {
        holder: caller,
        stops: true,
        records: !setter,
        leaves: false,
    };
    patch(mutator, &code, point, one_shot);
    if let Some(stepping) = &mut mutator.stepping {
        stepping.return_point = Some((caller, point));
    }
}

/// Where `frame`'s return goes on: its caller -- running, or suspended and
/// held by the step -- that caller's body, the index after the call it
/// stands at and whether that call is a setter assignment's send
/// (`answers_a_setter`); `None` for a frame with no caller, or whose
/// caller stands at no call.
fn return_slot<'gc>(
    mutator: &mut Mutator<'gc>,
    frame: Holder,
) -> Option<(Holder, Rc<CompiledCode>, usize, bool)> {
    let heap = &mutator.heap;
    let sender = match frame {
        Holder::Running(activation) => match heap.activation_suspended_sender(activation) {
            Some(suspended) => Err(suspended),
            None => Ok(heap.activation_sender(activation)?),
        },
        Holder::Suspended(key) => {
            let own = mutator.stepping.as_ref().and_then(|stepping| {
                stepping
                    .suspended
                    .iter()
                    .find(|(held, _)| *held == key)
                    .and_then(|(_, root)| heap.fetch(root).as_object())
                    .and_then(|object| heap.object_frame(object))
            })?;
            let sender = heap.frame_sender(own)?;
            match heap.frame_activation(sender) {
                Some(caller) => Ok(caller),
                None => Err(sender),
            }
        }
        Holder::Left => return None,
    };
    let (caller, standing, code, place) = match sender {
        Ok(caller) => (
            Holder::Running(caller),
            heap.activation_program_counter(caller),
            heap.activation_code(caller),
            Place::Running(caller),
        ),
        Err(suspended) => {
            let code = heap.frame_code(suspended)?;
            let standing = heap.frame_stored_program_counter(suspended);
            (
                Holder::Suspended(hold(mutator, suspended)),
                standing,
                code,
                Place::Suspended(suspended),
            )
        }
    };
    let point = standing + 1;
    if point >= code.instruction_count() || call_destination(&code.instruction(standing)).is_none()
    {
        return None;
    }
    let setter = answers_a_setter(mutator, place, &code, standing);
    Some((caller, code, point, setter))
}

/// Whether the call at `index` of `code`, a body `place` runs -- its own
/// or a block lowered inline in it -- is a setter assignment's send
/// (`SendEntry::setter`), whose answer, the receiver, is no value a step
/// reports: a return into it records nothing, as the flood leaves the
/// index after it out, so that the value before stays. A body without
/// metadata records every call.
fn answers_a_setter<'gc>(
    mutator: &mut Mutator<'gc>,
    place: Place<'gc>,
    code: &Rc<CompiledCode>,
    index: usize,
) -> bool {
    let Ok(index) = u32::try_from(index) else {
        return false;
    };
    let (body, origin, metadata) = place_body(mutator, place);
    let metadata = if Rc::ptr_eq(&body, code) {
        metadata
    } else {
        origin.and_then(|origin| Cache::metadata(mutator, code, &origin))
    };
    metadata.is_some_and(|metadata| {
        metadata
            .sends
            .binary_search_by_key(&index, |entry| entry.index)
            .is_ok_and(|found| metadata.sends[found].setter)
    })
}

/// Patches slot `index` of `code` with `one_shot`, giving the body a code
/// id, which a full collection keeps while it is patched, as a line
/// breakpoint's.
fn patch(mutator: &mut Mutator<'_>, code: &Rc<CompiledCode>, index: usize, one_shot: OneShot) {
    mutator.breakpoints.add_one_shot(code, index, one_shot);
    mutator.heap.code_id(code);
}

/// The register a call writes its answer to (`call_of`); `None` for
/// every other instruction.
fn call_destination(instruction: &Instruction) -> Option<u16> {
    call_of(instruction).map(|(destination, _)| destination)
}

/// The register a call writes its answer to -- a send's, a
/// `ResolveFuture`'s, an initializer invocation's or a `SendMirror`'s
/// destination, where a caller stands while its callee runs -- and what
/// kind of call it is, which names it where its body has no metadata:
/// decided together, so that an instruction a step takes for a call is
/// one it can name. `None` for every other instruction.
fn call_of(instruction: &Instruction) -> Option<(u16, Call)> {
    match instruction {
        Instruction::SendLexical {
            destination,
            selector,
            ..
        }
        | Instruction::SendPublic {
            destination,
            selector,
            ..
        }
        | Instruction::SendInlinable {
            destination,
            selector,
            ..
        } => Some((*destination, Call::Send(*selector))),
        Instruction::SendSuper {
            destination,
            selector,
            ..
        } => Some((*destination, Call::Super(*selector))),
        Instruction::SendMirror {
            destination,
            primitive,
            ..
        } => Some((*destination, Call::Mirror(*primitive))),
        Instruction::ResolveFuture { destination, .. } => Some((*destination, Call::Future)),
        Instruction::InitializeInstance { destination, .. } => {
            Some((*destination, Call::Initializer))
        }
        Instruction::InitializeSuperclass { destination, .. } => {
            Some((*destination, Call::SuperclassInitializer))
        }
        Instruction::InitializeAppliedLevel { destination, .. } => {
            Some((*destination, Call::MixinInitializer))
        }
        _ => None,
    }
}

/// The kind of a call a step recorded (`call_of`), which names it where
/// its body has no metadata (`Call::name`).
#[derive(Clone, Copy)]
pub(crate) enum Call {
    /// A send of the selector, lexical, public or inlinable.
    Send(Symbol),
    Super(Symbol),
    Mirror(crate::builtins::primitive::MirrorPrimitive),
    Future,
    Initializer,
    SuperclassInitializer,
    MixinInitializer,
}

impl Call {
    /// The call as the instruction names it: a send's selector, `super`
    /// before a super send's, a mirror send's mirror selector, and the
    /// kind of each other call.
    pub(crate) fn name(self) -> String {
        match self {
            Call::Send(selector) => selector.to_string(),
            Call::Super(selector) => format!("super {selector}"),
            Call::Mirror(primitive) => {
                String::from(crate::builtins::primitive::mirror_entry(primitive).selector)
            }
            Call::Future => String::from("future"),
            Call::Initializer => String::from("initializer"),
            Call::SuperclassInitializer => String::from("superclass initializer"),
            Call::MixinInitializer => String::from("mixin initializer"),
        }
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
