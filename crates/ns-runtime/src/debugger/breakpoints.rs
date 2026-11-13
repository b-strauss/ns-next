//! The breakpoint table (`docs/design/debugger.md`, decision 2, "Stopping
//! at and going past a breakpoint", "Breakpoints by body, and reclaimed
//! code ids", "A breakpoint names a source position", "Method
//! breakpoints"): each line breakpoint by its module and offset, each
//! method breakpoint by its method, the run to cursor, the slots they
//! patched, keyed by their body and index, the token that lets a resumed
//! stop go past its breakpoint once, the stop a `Breakpoint` arm asked
//! for, and, in a debug build, the breakpoint stress mode's patches ("The
//! breakpoint stress mode"). On the `Mutator`, since the run loop's arm,
//! a module's build and an install, which run inside an entry, reach
//! nothing else.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use ns_syntax::Span;

use super::exceptions::{ExceptionBreakpoints, PendingException};
use super::metadata::again::PairedBodies;
use super::metadata::Cache;
use super::remote::{
    BreakpointId, BreakpointState, Event, MethodBreakpoint, MethodBreakpointState, StopReason,
};
use super::stepping::{inlined_blocks, Holder, StepFilters};
use super::Attachments;
use crate::bytecode::{CompiledCode, Instruction};
use crate::heap::{ActivationReference, CodeId, FrameReference, Heap};
use crate::interpreter::Mutator;
use crate::mixin::{Mixin, MixinMethod};
use crate::Root;

/// The size of the breakpoint stress mode's table of resolved bodies from
/// which each doubling forgets the dead ones.
#[cfg(debug_assertions)]
const STRESSED_FLOOR: usize = 1024;

/// Why a run to cursor is refused at a module not built.
const NOT_BUILT: &str = "no module of that name is built";

/// Why a run to cursor is refused at a position no statement follows.
const NO_STATEMENT: &str = "no statement starts at or after that position";

/// Every breakpoint the attached debugger set, and what the run loop
/// needs of them.
#[derive(Default)]
pub(crate) struct Breakpoints {
    /// The number the next breakpoint takes.
    next_id: u32,
    /// Each module's line breakpoints, by the module's name.
    modules: Vec<ModuleBreakpoints>,
    /// Each patched slot, by its body's address and its index. The body
    /// is held, never named by its code id, which a full collection
    /// reclaims and mints again for another body.
    patches: HashMap<(usize, usize), Patch>,
    /// The slot a resumed stop goes past once (`PastOnce`).
    past_once: Option<PastOnce>,
    /// The stop the last `Breakpoint` arm decided, which the safepoint it
    /// left for takes.
    pending_stop: Option<PendingStop>,
    /// What the handler search's rows recorded for the next poll's
    /// safepoint (`debugger::exceptions`), the request bit
    /// (`DEBUGGER_STOP`) leading it there: each runs in the middle of a
    /// stretch -- the leaf in machine code's, which no leaf leaves -- where
    /// nothing is flooded and nothing stops.
    pending_exception: Option<PendingException>,
    /// Whether the debugger terminated the run, whose unwind runs the
    /// pending `ensure:` blocks: an exception one of them raises stops
    /// nothing, as the termination ends a step.
    terminating: bool,
    /// The exception breakpoints the attached debugger set; the default
    /// until it sets them.
    exceptions: ExceptionBreakpoints,
    /// Where the unasked reports go: the attached debugger's event
    /// channel, cloned, so that a module built or an install inside a
    /// leaf reports what it resolved at once.
    events: Option<Sender<Event>>,
    /// Every body a slot of which was patched or restored since the table
    /// last settled its changes (`settle`): machine code compiled before
    /// runs the slot as it was.
    changed: Vec<Rc<CompiledCode>>,
    /// Whether a method's fast path was switched off or on since the
    /// table last settled its changes: the caches and the optimized
    /// copies recorded what it decided.
    fast_paths_switched: bool,
    /// Each method whose fast path a patch of its body switched off, by
    /// its body's address: the removal of the body's last patch, of
    /// whichever kind but the breakpoint stress mode's, switches it back
    /// on (`switch_fast_paths_back`).
    switched_off: HashMap<usize, Rc<MixinMethod>>,
    /// The attached debugger's arming lock, under which a step holds and
    /// drops its request bit (`debugger::stepping`), as the agent arms
    /// the poll under it; `None` while none is attached.
    arming: Option<Arc<Mutex<Attachments>>>,
    /// The frames no step stops in (`stepping::is_filtered`), which the
    /// attached debugger set; the default until it sets them.
    step_filters: StepFilters,
    /// Every method breakpoint, in the order the debugger set them.
    methods: Vec<MethodBreakpointEntry>,
    /// The frame object of the home whose method's exit a stop at a
    /// non-local return already reported, so that the unwind's arrival
    /// there does not report it again (`returned_home_reported`).
    reported_home: Option<Root>,
    /// Every body the breakpoint stress mode patched, or found nothing to
    /// patch in, by its address: each is resolved once. Held weakly, as
    /// the metadata cache holds a body: the allocation stays, so that no
    /// other body takes its address, but a body nothing patched is not
    /// kept alive.
    #[cfg(debug_assertions)]
    stressed: HashMap<usize, std::rc::Weak<CompiledCode>>,
    /// How many times a `Breakpoint` arm went past a slot the breakpoint
    /// stress mode patched since the mode was last switched on.
    #[cfg(debug_assertions)]
    stress_hits: usize,
}

/// One method breakpoint.
struct MethodBreakpointEntry {
    id: BreakpointId,
    method: MethodBreakpoint,
    /// Every body of the method it patched: the current one, and those an
    /// install replaced, whose frames may still run them.
    bodies: Vec<Rc<CompiledCode>>,
    /// Whether the method is declared now.
    verified: bool,
}

/// The line breakpoints of one module.
struct ModuleBreakpoints {
    /// The name of the module's root class.
    name: String,
    lines: Vec<LineBreakpoint>,
}

/// One line breakpoint.
struct LineBreakpoint {
    id: BreakpointId,
    offset: u32,
    /// The statement it resolved to; `None` while pending.
    statement: Option<Span>,
}

/// A patched slot: its body, its index, and every breakpoint standing
/// there. The slot gets its original back when the last goes.
struct Patch {
    code: Rc<CompiledCode>,
    index: usize,
    kinds: Vec<PatchKind>,
}

/// Why a slot is patched.
#[derive(Clone, Copy)]
enum PatchKind {
    /// A line breakpoint the debugger set.
    Line(BreakpointId),
    /// The first statement start of the method of a method breakpoint.
    Entry(BreakpointId),
    /// A return of the method of a method breakpoint: one of its body's,
    /// or a non-local return of a block the compiler lowered inline in it.
    Exit(BreakpointId),
    /// A step's one-shot breakpoint (`debugger::stepping`).
    OneShot(OneShot),
    /// The run to cursor's target, taken in any activation.
    RunTo,
    /// The breakpoint stress mode's, which never stops
    /// (`patch_running_for_stress`).
    #[cfg(debug_assertions)]
    Stress,
}

impl PatchKind {
    /// Whether this stands for the breakpoint `id` the debugger set.
    fn is_breakpoint(self, id: BreakpointId) -> bool {
        matches!(
            self,
            PatchKind::Line(held) | PatchKind::Entry(held) | PatchKind::Exit(held) if held == id
        )
    }

    /// Whether this is the same breakpoint as `other`: a slot holds each
    /// once, and a step's one-shots as many as the step sets.
    fn is_same(self, other: PatchKind) -> bool {
        match (self, other) {
            (PatchKind::Line(held), PatchKind::Line(id))
            | (PatchKind::Entry(held), PatchKind::Entry(id))
            | (PatchKind::Exit(held), PatchKind::Exit(id)) => held == id,
            (PatchKind::RunTo, PatchKind::RunTo) => true,
            #[cfg(debug_assertions)]
            (PatchKind::Stress, PatchKind::Stress) => true,
            _ => false,
        }
    }

    /// Whether this belongs to a step or a run to cursor, which the next
    /// stop ends.
    fn ends_at_next_stop(self) -> bool {
        matches!(self, PatchKind::OneShot(_) | PatchKind::RunTo)
    }

    /// Whether this is the breakpoint stress mode's, which outlives every
    /// debugger.
    fn is_stress(self) -> bool {
        #[cfg(debug_assertions)]
        {
            matches!(self, PatchKind::Stress)
        }
        #[cfg(not(debug_assertions))]
        {
            let _ = self;
            false
        }
    }
}

/// A step's one-shot breakpoint: taken only in the frame it is for, or
/// in a block's whose creator that frame is (`stepping::takes`), and
/// ignored in any other activation of its body -- a recursive call
/// deeper on the stack, a block of the same home running in a callee.
#[derive(Clone, Copy)]
pub(crate) struct OneShot {
    pub(crate) holder: Holder,
    /// Whether taking it stops the program.
    pub(crate) stops: bool,
    /// Whether taking it records the value the instruction before its
    /// slot answered, the value of the last call a step reports
    /// (`stepping::record`).
    pub(crate) records: bool,
}

/// The slot a stop that resumes goes past once: the activation standing
/// at it, its body's address and its index, and what going past it still
/// owes. The next `Breakpoint` arm the loop executes consumes it when it
/// names that arm's slot and drops it otherwise -- the activation's offset
/// is reused after its pop, so it is never kept past that arm.
#[derive(Clone, Copy)]
pub(crate) struct PastOnce {
    pub(crate) activation: ActivationReference,
    pub(crate) body: usize,
    pub(crate) index: usize,
    pub(crate) after: AfterPast,
}

/// What going past a slot once still owes the stop made there.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum AfterPast {
    /// Nothing.
    Nothing,
    /// The stop was a method's entry at a slot that is also one of its
    /// returns -- a body of one instruction, a lazy slot's getter -- whose
    /// exit is reported by a stop of its own before the slot runs.
    ExitDue,
    /// The stop reported a method's exit at a non-local return, so that
    /// the unwind's arrival at the method does not report it again.
    ExitReported,
}

/// A stop a `Breakpoint` arm decided: where, which breakpoints the
/// debugger set stand there, and whether a step's one-shot stops there.
pub(crate) struct PendingStop {
    pub(crate) slot: PastOnce,
    pub(crate) hits: Hits,
    pub(crate) step_stops: bool,
}

impl PendingStop {
    /// The stop as the table stands now, after the commands the safepoint
    /// served before it: only the breakpoints still standing at its slot,
    /// or no stop where none is left and no step's one-shot stops there
    /// -- the frame fetches the slot again as it goes on, and what stands
    /// there then decides afresh. A breakpoint set there meanwhile is not
    /// added: the frame goes past the slot once after the stop, as after
    /// any other.
    pub(crate) fn still_standing(self, table: &Breakpoints) -> Option<PendingStop> {
        let PendingStop {
            slot,
            hits,
            step_stops,
        } = self;
        let standing = table.kinds_at(slot.body, slot.index);
        let still = |ids: Vec<BreakpointId>, standing: &[BreakpointId]| -> Vec<BreakpointId> {
            ids.into_iter().filter(|id| standing.contains(id)).collect()
        };
        let hits = Hits {
            lines: still(hits.lines, &standing.lines),
            entries: still(hits.entries, &standing.entries),
            exits: still(hits.exits, &standing.exits),
            run_to: hits.run_to && standing.run_to,
        };
        if hits.only_a_step() && !step_stops {
            return None;
        }
        Some(PendingStop {
            slot,
            hits,
            step_stops,
        })
    }
}

/// The breakpoints the debugger set that a stop is for.
#[derive(Default)]
pub(crate) struct Hits {
    pub(crate) lines: Vec<BreakpointId>,
    /// The method breakpoints whose method the frame enters.
    pub(crate) entries: Vec<BreakpointId>,
    /// The method breakpoints whose method the frame leaves here.
    pub(crate) exits: Vec<BreakpointId>,
    /// Whether the run to cursor's target stands there.
    pub(crate) run_to: bool,
}

impl Hits {
    /// Whether no breakpoint the debugger set and no run to cursor made
    /// the stop: only a step's one-shot did, or nothing.
    pub(crate) fn only_a_step(&self) -> bool {
        self.lines.is_empty() && self.entries.is_empty() && self.exits.is_empty() && !self.run_to
    }

    /// The reason of the stop, with `unwound` the method breakpoints whose
    /// method an unwind left since the last stop (`stepping::method_left`);
    /// `None` for a step's or a run to cursor's own. A method's entry is
    /// reported first, then its exit, then a line breakpoint, and the
    /// others the stop is for go with it: one stop is reported once, for
    /// every breakpoint it is for.
    pub(crate) fn reason(self, unwound: Vec<BreakpointId>) -> Option<StopReason> {
        let Hits {
            lines,
            entries,
            exits,
            ..
        } = self;
        let entered = !entries.is_empty();
        let mut all: Vec<BreakpointId> = entries.into_iter().chain(exits).chain(unwound).collect();
        if all.is_empty() {
            return (!lines.is_empty()).then_some(StopReason::Breakpoint { ids: lines });
        }
        all.extend(lines);
        let id = all.remove(0);
        Some(if entered {
            StopReason::MethodEntry { id, others: all }
        } else {
            StopReason::MethodExit { id, others: all }
        })
    }
}

/// What stands at a patched slot.
#[derive(Default)]
pub(crate) struct Standing {
    pub(crate) lines: Vec<BreakpointId>,
    pub(crate) entries: Vec<BreakpointId>,
    pub(crate) exits: Vec<BreakpointId>,
    pub(crate) one_shots: Vec<OneShot>,
    pub(crate) run_to: bool,
    /// Whether the breakpoint stress mode patched the slot.
    pub(crate) stress: bool,
}

/// What resolving a module's breakpoints against its bodies found.
struct Resolution {
    /// Each breakpoint's statement, in the order of the module's lines.
    statements: Vec<Option<Span>>,
    /// Each breakpoint's slots: its body and the index of its statement's
    /// first instruction.
    slots: Vec<Vec<(Rc<CompiledCode>, usize)>>,
    /// The address of every body walked, whether or not it has metadata:
    /// a body walked is one of the module's current bodies.
    walked: HashSet<usize>,
    /// Every method of the module with a fast path, by its body's
    /// address.
    fast_path_methods: HashMap<usize, Rc<MixinMethod>>,
}

impl Breakpoints {
    /// Where unasked reports go from here on: the attached debugger's
    /// channel, or `None` once none is attached.
    pub(crate) fn report_to(&mut self, events: Option<Sender<Event>>) {
        self.events = events;
    }

    /// The arming lock of the debugger attached from here on, or `None`
    /// once none is.
    pub(crate) fn arm_under(&mut self, arming: Option<Arc<Mutex<Attachments>>>) {
        self.arming = arming;
    }

    /// The attached debugger's arming lock, if one is attached.
    pub(crate) fn arming(&self) -> Option<Arc<Mutex<Attachments>>> {
        self.arming.clone()
    }

    /// Replaces the step filters.
    pub(crate) fn set_step_filters(&mut self, filters: StepFilters) {
        self.step_filters = filters;
    }

    /// The step filters in effect.
    pub(crate) fn step_filters(&self) -> &StepFilters {
        &self.step_filters
    }

    /// The stop the last `Breakpoint` arm decided, taken.
    pub(crate) fn take_pending_stop(&mut self) -> Option<PendingStop> {
        self.pending_stop.take()
    }

    /// Whether a debugger is attached, for which the exception breakpoints
    /// stop.
    pub(crate) fn attached(&self) -> bool {
        self.arming.is_some()
    }

    /// Replaces the exception breakpoints.
    pub(crate) fn set_exception_breakpoints(&mut self, exceptions: ExceptionBreakpoints) {
        self.exceptions = exceptions;
    }

    /// The exception breakpoints in effect.
    pub(crate) fn exception_breakpoints(&self) -> &ExceptionBreakpoints {
        &self.exceptions
    }

    /// Records what the handler search's rows decided, which the next
    /// poll's safepoint takes.
    pub(crate) fn request_exception_stop(&mut self, pending: PendingException) {
        self.pending_exception = Some(pending);
    }

    /// What the handler search's rows decided, taken.
    pub(crate) fn take_pending_exception(&mut self) -> Option<PendingException> {
        self.pending_exception.take()
    }

    /// Whether the debugger terminated the run in progress.
    pub(crate) fn terminating(&self) -> bool {
        self.terminating
    }

    /// The debugger terminated the run (`true`), or the run ended
    /// (`false`).
    pub(crate) fn set_terminating(&mut self, terminating: bool) {
        self.terminating = terminating;
    }

    /// Records the stop a `Breakpoint` arm decided.
    pub(crate) fn request_stop(&mut self, stop: PendingStop) {
        self.pending_stop = Some(stop);
    }

    /// Lets the next `Breakpoint` arm at `past` go past it once, if a
    /// breakpoint still patches its slot: one removed at the stop left the
    /// original there, and a token no arm consumes would outlive the
    /// activation it names.
    pub(crate) fn go_past_once(&mut self, past: PastOnce) {
        if self.patches.contains_key(&(past.body, past.index)) {
            self.past_once = Some(past);
        }
    }

    /// The token, taken: the arm that reads it consumes or drops it.
    pub(crate) fn take_past_once(&mut self) -> Option<PastOnce> {
        self.past_once.take()
    }

    /// Drops the token and the reported home: a run ended, which may
    /// leave the token's slot unexecuted and the home's unwind not
    /// arrived.
    pub(crate) fn drop_past_once(&mut self) {
        self.past_once = None;
        self.reported_home = None;
    }

    /// The token's activation moved from `from` to `onto`: the suspend
    /// stress mode suspended the stack and resumed its top elsewhere.
    #[cfg(debug_assertions)]
    pub(crate) fn move_past_once(&mut self, from: ActivationReference, onto: ActivationReference) {
        if let Some(past) = &mut self.past_once {
            if past.activation.same(from) {
                past.activation = onto;
            }
        }
    }

    /// Everything standing at slot `index` of the body at `body`; nothing
    /// for a slot nothing patched.
    pub(crate) fn kinds_at(&self, body: usize, index: usize) -> Standing {
        let mut standing = Standing::default();
        if let Some(patch) = self.patches.get(&(body, index)) {
            for kind in &patch.kinds {
                match kind {
                    PatchKind::Line(id) => standing.lines.push(*id),
                    PatchKind::Entry(id) => standing.entries.push(*id),
                    PatchKind::Exit(id) => standing.exits.push(*id),
                    PatchKind::OneShot(one_shot) => standing.one_shots.push(*one_shot),
                    PatchKind::RunTo => standing.run_to = true,
                    #[cfg(debug_assertions)]
                    PatchKind::Stress => standing.stress = true,
                }
            }
        }
        standing
    }

    /// Whether any method breakpoint is set: the unwinder reports a
    /// method's exit only then.
    pub(crate) fn has_method_breakpoints(&self) -> bool {
        !self.methods.is_empty()
    }

    /// The method breakpoints whose method `code` is a body of.
    pub(crate) fn exits_of(&self, code: &Rc<CompiledCode>) -> Vec<BreakpointId> {
        self.methods
            .iter()
            .filter(|entry| entry.bodies.iter().any(|body| Rc::ptr_eq(body, code)))
            .map(|entry| entry.id)
            .collect()
    }

    /// Whether `code` is the body of a method a method breakpoint names:
    /// the cut, which pops compiled frames without the unwinder, leaves a
    /// frame of it to the unwinder, which reports its exit.
    pub(crate) fn reports_exits_of(&self, code: &Rc<CompiledCode>) -> bool {
        self.has_method_breakpoints() && !self.exits_of(code).is_empty()
    }

    /// Records `home`, a frame object, as the home whose method's exit a
    /// stop at a non-local return reported.
    pub(crate) fn report_home(&mut self, home: Root) {
        self.reported_home = Some(home);
    }

    /// Whether the home `frame` is the one whose method's exit a stop at
    /// a non-local return reported, forgetting it if so: the unwind's
    /// arrival there reports nothing again.
    pub(crate) fn returned_home_reported<'gc>(
        &mut self,
        heap: &Heap<'gc>,
        frame: FrameReference<'gc>,
    ) -> bool {
        let reported = self.reported_home.as_ref().is_some_and(|root| {
            heap.fetch(root)
                .as_object()
                .is_some_and(|object| object.same(heap.frame_object(frame)))
        });
        if reported {
            self.reported_home = None;
        }
        reported
    }

    /// Patches slot `index` of `code` with `one_shot`; the table's
    /// `settle` frees the body's machine code.
    pub(crate) fn add_one_shot(
        &mut self,
        code: &Rc<CompiledCode>,
        index: usize,
        one_shot: OneShot,
    ) {
        self.add(code, index, PatchKind::OneShot(one_shot), None);
    }

    /// The one-shots for `from` are for `onto` from here on: the frame
    /// was suspended into its frame object, or resumed from it
    /// (`stepping::suspending`, `stepping::resumed`).
    pub(crate) fn rename_one_shots(&mut self, from: Holder, onto: Holder) {
        for patch in self.patches.values_mut() {
            for kind in &mut patch.kinds {
                if let PatchKind::OneShot(one_shot) = kind {
                    if one_shot.holder.is(from) {
                        one_shot.holder = onto;
                    }
                }
            }
        }
    }

    /// Every one-shot for a frame `gone` says was popped is for
    /// `Holder::Left` from here on, which no activation is, so that an
    /// activation pushed at the popped one's offset does not take it.
    pub(crate) fn forget_one_shot_holders(&mut self, gone: impl Fn(Holder) -> bool) {
        for patch in self.patches.values_mut() {
            for kind in &mut patch.kinds {
                if let PatchKind::OneShot(one_shot) = kind {
                    if gone(one_shot.holder) {
                        one_shot.holder = Holder::Left;
                    }
                }
            }
        }
    }

    /// Every frame a one-shot is for.
    #[cfg(debug_assertions)]
    pub(crate) fn one_shot_holders(&self) -> Vec<Holder> {
        self.patches
            .values()
            .flat_map(|patch| patch.kinds.iter())
            .filter_map(|kind| match kind {
                PatchKind::OneShot(one_shot) => Some(one_shot.holder),
                PatchKind::Line(_)
                | PatchKind::Entry(_)
                | PatchKind::Exit(_)
                | PatchKind::RunTo
                | PatchKind::Stress => None,
            })
            .collect()
    }

    /// Removes every one-shot and the run to cursor, each slot no
    /// breakpoint the debugger set holds given its original back: the
    /// step or the run to cursor ended.
    pub(crate) fn remove_one_shots(&mut self) {
        self.remove_kinds(PatchKind::ends_at_next_stop, |_| false);
    }

    /// Whether no line breakpoint, resolved or pending, and no method
    /// breakpoint is set.
    fn is_empty(&self) -> bool {
        self.modules.is_empty() && self.methods.is_empty()
    }

    /// Patches slot `index` of `code` for `kind`, which a slot holds once;
    /// `method`, when given, is the method whose body `code` is, whose
    /// fast path, if it has one, the first patch that names the method
    /// switches off -- whatever patched the body before, since a step's
    /// one-shot names none.
    fn add(
        &mut self,
        code: &Rc<CompiledCode>,
        index: usize,
        kind: PatchKind,
        method: Option<&Rc<MixinMethod>>,
    ) {
        let patch = self
            .patches
            .entry((address(code), index))
            .or_insert_with(|| Patch {
                code: Rc::clone(code),
                index,
                kinds: Vec::new(),
            });
        let held = patch.kinds.iter().any(|held| held.is_same(kind));
        if !held {
            patch.kinds.push(kind);
        }
        if let Some(method) = method.filter(|method| method.fast_path().is_some()) {
            if !self.switched_off.contains_key(&address(code)) {
                method.switch_fast_path(false);
                self.fast_paths_switched = true;
                self.switched_off.insert(address(code), Rc::clone(method));
            }
        }
        if code.is_patched(index) {
            return;
        }
        code.patch(index);
        self.changed.push(Rc::clone(code));
    }

    /// Gives `patch`'s slot its original back.
    fn restore(&mut self, patch: &Patch) {
        patch.code.restore(patch.index);
        self.changed.push(Rc::clone(&patch.code));
    }

    /// Switches the fast path of each method a patch switched off back on
    /// once no breakpoint but the breakpoint stress mode's stands in its
    /// body: the mode's patches never stop, so they keep no send from the
    /// fast path, and they stay for as long as the mode is on.
    fn switch_fast_paths_back(&mut self) {
        if self.switched_off.is_empty() {
            return;
        }
        let patches = &self.patches;
        let freed: Vec<usize> = self
            .switched_off
            .keys()
            .copied()
            .filter(|body| {
                !patches.values().any(|patch| {
                    address(&patch.code) == *body
                        && patch.kinds.iter().any(|kind| !kind.is_stress())
                })
            })
            .collect();
        for body in freed {
            if let Some(method) = self.switched_off.remove(&body) {
                method.switch_fast_path(true);
                self.fast_paths_switched = true;
            }
        }
    }

    /// Removes every kind `removed` picks from every slot `keep` refuses;
    /// a slot whose last kind goes gets its original back, and a token
    /// naming it goes with it, so that a breakpoint set there again later
    /// stops.
    fn remove_kinds(
        &mut self,
        removed: impl Fn(PatchKind) -> bool,
        keep: impl FnMut(&Patch) -> bool,
    ) {
        self.strip(removed, keep);
        self.sweep();
    }

    /// Takes every kind `removed` picks off every slot `keep` refuses, but
    /// leaves a slot whose last kind went patched until the next `sweep`:
    /// a breakpoint set again at the same slot in between keeps the slot
    /// as it is, and its body its machine code and its fast path's state.
    fn strip(&mut self, removed: impl Fn(PatchKind) -> bool, mut keep: impl FnMut(&Patch) -> bool) {
        for patch in self.patches.values_mut() {
            if patch.kinds.iter().any(|kind| removed(*kind)) && !keep(patch) {
                patch.kinds.retain(|kind| !removed(*kind));
            }
        }
    }

    /// Gives every slot no kind holds any more its original back, and every
    /// method no breakpoint stands in any more its fast path.
    fn sweep(&mut self) {
        let emptied: Vec<(usize, usize)> = self
            .patches
            .iter()
            .filter(|(_, patch)| patch.kinds.is_empty())
            .map(|(key, _)| *key)
            .collect();
        for key in emptied {
            if let Some(patch) = self.patches.remove(&key) {
                self.restore(&patch);
                if self
                    .past_once
                    .is_some_and(|past| (past.body, past.index) == key)
                {
                    self.past_once = None;
                }
            }
        }
        self.switch_fast_paths_back();
    }

    /// Every patched body's code id: a full collection keeps them, as it
    /// keeps a running body's, so that the id a patched body was given
    /// never names another body.
    pub(crate) fn patched_code_ids(&self, mutator: &Mutator<'_>) -> Vec<CodeId> {
        self.patches
            .values()
            .filter_map(|patch| mutator.heap.minted_code_id(&patch.code))
            .collect()
    }

    /// Sends `event` to the debugger, if one is attached; one that is gone
    /// is noticed where the interpreter next talks to it.
    fn report(&self, event: Event) {
        if let Some(events) = &self.events {
            let _ = events.send(event);
        }
    }

    /// The states of the method breakpoints, in the order they were set.
    fn method_states(&self) -> Vec<MethodBreakpointState> {
        self.methods
            .iter()
            .map(|entry| MethodBreakpointState {
                id: entry.id,
                verified: entry.verified,
            })
            .collect()
    }

    /// The number the next breakpoint takes, taken.
    fn take_id(&mut self) -> BreakpointId {
        let id = BreakpointId(self.next_id);
        self.next_id = self.next_id.wrapping_add(1);
        id
    }

    /// The states of the breakpoints of the module at `position`.
    fn states(&self, position: usize) -> Vec<BreakpointState> {
        self.modules[position]
            .lines
            .iter()
            .map(|line| BreakpointState {
                id: line.id,
                offset: line.offset,
                statement: line.statement,
            })
            .collect()
    }
}

/// Replaces every line breakpoint of `module` by one at each of
/// `offsets`, resolved against the module's bodies if it is built, and
/// answers their states (`Command::SetBreakpoints`). Only the slots the
/// two sets do not share change: a slot the old set and the new both
/// patch stays patched, so that sending the same set again frees no
/// machine code and switches no fast path.
pub(crate) fn set_line_breakpoints(
    mutator: &mut Mutator<'_>,
    module: String,
    offsets: Vec<u32>,
) -> Event {
    let table = &mut mutator.breakpoints;
    let position = match table.modules.iter().position(|held| held.name == module) {
        Some(position) => {
            let old: Vec<BreakpointId> = table.modules[position]
                .lines
                .iter()
                .map(|line| line.id)
                .collect();
            for id in old {
                table.strip(|kind| kind.is_breakpoint(id), |_| false);
            }
            position
        }
        None => {
            table.modules.push(ModuleBreakpoints {
                name: module.clone(),
                lines: Vec::new(),
            });
            table.modules.len() - 1
        }
    };
    let mut lines = Vec::with_capacity(offsets.len());
    for offset in offsets {
        lines.push(LineBreakpoint {
            id: table.take_id(),
            offset,
            statement: None,
        });
    }
    let empty = lines.is_empty();
    table.modules[position].lines = lines;
    resolve_module(mutator, position);
    mutator.breakpoints.sweep();
    let breakpoints = mutator.breakpoints.states(position);
    if empty {
        // A module without breakpoints holds no entry.
        mutator.breakpoints.modules.remove(position);
    }
    settle(mutator);
    Event::Breakpoints {
        module,
        breakpoints,
    }
}

/// The module whose root class is named `name` was built, or its bodies
/// replaced by an install: its line and method breakpoints are resolved
/// against its current bodies, and a change reported unasked. A slot of a
/// body no longer current keeps its patch, so that a frame still running
/// it stops there.
pub(crate) fn module_built(mutator: &mut Mutator<'_>, name: &str) {
    if let Some(position) = mutator
        .breakpoints
        .modules
        .iter()
        .position(|held| held.name == name)
    {
        resolve_and_report(mutator, position);
    }
    if mutator
        .breakpoints
        .methods
        .iter()
        .any(|entry| entry.method.class.module == name)
    {
        let before: Vec<bool> = mutator
            .breakpoints
            .methods
            .iter()
            .map(|entry| entry.verified)
            .collect();
        resolve_methods(mutator, Some(name));
        let table = &mutator.breakpoints;
        if table
            .methods
            .iter()
            .zip(&before)
            .any(|(entry, verified)| entry.verified != *verified)
        {
            table.report(Event::MethodBreakpoints(table.method_states()));
        }
    }
    settle(mutator);
}

/// Replaces every method breakpoint by one for each of `requested`, each
/// set on its method if its module declares it, and answers their states
/// (`Command::SetMethodBreakpoints`). As for line breakpoints, only the
/// slots the two sets do not share change.
pub(crate) fn set_method_breakpoints(
    mutator: &mut Mutator<'_>,
    requested: Vec<MethodBreakpoint>,
) -> Event {
    let table = &mut mutator.breakpoints;
    let old: Vec<BreakpointId> = table.methods.iter().map(|entry| entry.id).collect();
    for id in old {
        table.strip(|kind| kind.is_breakpoint(id), |_| false);
    }
    let methods = requested
        .into_iter()
        .map(|method| MethodBreakpointEntry {
            id: table.take_id(),
            method,
            bodies: Vec::new(),
            verified: false,
        })
        .collect();
    table.methods = methods;
    resolve_methods(mutator, None);
    mutator.breakpoints.sweep();
    settle(mutator);
    Event::MethodBreakpoints(mutator.breakpoints.method_states())
}

/// Sets the run to cursor's target: the statement `offset` names in the
/// module `module` names, resolved as a line breakpoint is, every slot of
/// it patched for any activation; the top frame goes past the slot it
/// stands at once, as a step's does, so that a target where it stands
/// stops at the next pass. The reason it is refused, its module not built
/// or no statement at or after the offset, otherwise.
pub(crate) fn run_to(
    mutator: &mut Mutator<'_>,
    module: &str,
    offset: u32,
) -> Result<(), &'static str> {
    let Some(resolution) = resolve(mutator, module, &[offset]) else {
        return Err(NOT_BUILT);
    };
    let slots = match (resolution.statements.first(), resolution.slots.first()) {
        (Some(Some(_)), Some(slots)) if !slots.is_empty() => slots,
        _ => return Err(NO_STATEMENT),
    };
    for (code, index) in slots {
        let method = resolution.fast_path_methods.get(&address(code));
        mutator
            .breakpoints
            .add(code, *index, PatchKind::RunTo, method);
        mutator.heap.code_id(code);
    }
    settle(mutator);
    if let Some(top) = mutator.heap.top_activation() {
        let code = mutator.heap.activation_code(top);
        let standing = PastOnce {
            activation: top,
            body: address(&code),
            index: mutator.heap.activation_program_counter(top),
            after: AfterPast::Nothing,
        };
        mutator.breakpoints.go_past_once(standing);
    }
    Ok(())
}

/// Sets every method breakpoint -- of the module named `module`, or every
/// one for `None` -- on its method's current body in every built module of
/// that name that declares the method, as a line breakpoint resolves in
/// each (`module_bodies`): its entry, its returns and the non-local
/// returns of the blocks the compiler lowered inline in it. One whose
/// method is gone keeps its patches on the bodies it patched, which
/// frames may still run, and is no longer verified.
fn resolve_methods(mutator: &mut Mutator<'_>, module: Option<&str>) {
    for position in 0..mutator.breakpoints.methods.len() {
        let entry = &mutator.breakpoints.methods[position];
        if module.is_some_and(|module| entry.method.class.module != module) {
            continue;
        }
        let request = entry.method.clone();
        let id = entry.id;
        let found = find_methods(mutator, &request);
        mutator.breakpoints.methods[position].verified = !found.is_empty();
        for (mixin, method) in found {
            patch_method(mutator, id, &mixin, &method);
            let bodies = &mut mutator.breakpoints.methods[position].bodies;
            if !bodies.iter().any(|body| Rc::ptr_eq(body, &method.code)) {
                bodies.push(Rc::clone(&method.code));
            }
        }
    }
}

/// The method `request` names in every built module of its module's name
/// that declares it, each with the instance side of the level that
/// declares it.
fn find_methods(
    mutator: &Mutator<'_>,
    request: &MethodBreakpoint,
) -> Vec<(Rc<Mixin>, Rc<MixinMethod>)> {
    let mut found = Vec::new();
    for id in mutator.heap.mixin_ids() {
        let Some(mixin) = mutator.heap.mixin(id) else {
            continue;
        };
        if mixin.is_class_side() {
            continue;
        }
        let (root, path) = mutator.path_from_module_root(id);
        let named = mutator
            .heap
            .mixin(root)
            .is_some_and(|root| root.name.as_str() == request.class.module)
            && path.len() == request.class.path.len()
            && path
                .iter()
                .zip(&request.class.path)
                .all(|(declared, named)| declared.as_str() == named);
        if !named {
            continue;
        }
        let side = if request.class_side {
            mixin.class_side.clone()
        } else {
            Some(Rc::clone(&mixin))
        };
        let method = side.and_then(|side| {
            side.compiled_methods()
                .find(|method| method.selector.as_str() == request.selector)
                .cloned()
        });
        if let Some(method) = method {
            found.push((mixin, method));
        }
    }
    found
}

/// Patches `method`'s body, a member of the level of `mixin`, for the
/// method breakpoint `id`: its first statement start -- index 1 when its
/// first instruction makes its indirection vector, which runs before
/// every statement -- and each of its returns, the body's own and the
/// non-local returns of every block the compiler lowered inline in it, a
/// fallback closure or a block the reference lowering sends, so that both
/// lowerings stop at the same returns.
fn patch_method(
    mutator: &mut Mutator<'_>,
    id: BreakpointId,
    mixin: &Rc<Mixin>,
    method: &Rc<MixinMethod>,
) {
    let code = &method.code;
    let count = code.instruction_count();
    let entry =
        usize::from(count > 1 && matches!(code.instruction(0), Instruction::MakeVector { .. }));
    let table = &mut mutator.breakpoints;
    table.add(code, entry, PatchKind::Entry(id), Some(method));
    for (index, instruction) in code.instructions().iter().enumerate() {
        if matches!(
            instruction,
            Instruction::Return { .. }
                | Instruction::ImplicitReturn { .. }
                | Instruction::ReturnSlotIfFilled { .. }
        ) {
            table.add(code, index, PatchKind::Exit(id), Some(method));
        }
    }
    mutator.heap.code_id(code);
    let level = Cache::level(mutator, mixin);
    let metadata_of = |body: &Rc<CompiledCode>| {
        level
            .iter()
            .find(|(held, _)| Rc::ptr_eq(held, body))
            .and_then(|(_, metadata)| metadata.clone())
    };
    let mut pending: Vec<Rc<CompiledCode>> = vec![Rc::clone(code)];
    let mut walked: Vec<Rc<CompiledCode>> = Vec::new();
    while let Some(body) = pending.pop() {
        if walked.iter().any(|held| Rc::ptr_eq(held, &body)) {
            continue;
        }
        walked.push(Rc::clone(&body));
        let Some(metadata) = metadata_of(&body) else {
            continue;
        };
        for block in inlined_blocks(&body, &metadata) {
            for (index, instruction) in block.instructions().iter().enumerate() {
                if matches!(instruction, Instruction::NonLocalReturn { .. }) {
                    mutator
                        .breakpoints
                        .add(&block, index, PatchKind::Exit(id), None);
                    mutator.heap.code_id(&block);
                }
            }
            pending.push(block);
        }
    }
}

/// Removes every breakpoint, each slot no breakpoint stress patch holds
/// given its original back, ends any step, drops a stop not served yet
/// and puts the step filters and the exception breakpoints back to their
/// default: the debugger that set them is gone or replaced.
pub(crate) fn clear(mutator: &mut Mutator<'_>) {
    super::stepping::end(mutator);
    let table = &mut mutator.breakpoints;
    // The breakpoint stress mode's patches stay: they are the mode's, not
    // the debugger's, and the mode resolves each body once.
    table.remove_kinds(|kind| !kind.is_stress(), |_| false);
    table.modules.clear();
    table.methods.clear();
    table.reported_home = None;
    table.past_once = None;
    table.pending_stop = None;
    table.pending_exception = None;
    table.terminating = false;
    table.step_filters = StepFilters::default();
    table.exceptions = ExceptionBreakpoints::default();
    settle(mutator);
}

/// Settles what the table changed since it last did: the machine code of
/// every body a slot of which it patched or restored is freed, with every
/// optimized copy that inlined the body (`jit::free_for_patch`), so that
/// the body is compiled again with its patches lowered as exits; and once
/// a method's fast path was switched, every cache is vacated and every
/// optimized copy freed, since each recorded what the fast path decided,
/// a copy that inlined a quick method without naming its body among them
/// (`docs/design/debugger.md`, "A breakpoint in a method with a fast
/// path"). Setting a breakpoint is rare, so neither costs a send a check.
pub(crate) fn settle(mutator: &mut Mutator<'_>) {
    let mut changed = std::mem::take(&mut mutator.breakpoints.changed);
    changed.sort_by_key(address);
    changed.dedup_by(|left, right| Rc::ptr_eq(left, right));
    for code in &changed {
        crate::jit::free_for_patch(mutator, code);
    }
    if std::mem::take(&mut mutator.breakpoints.fast_paths_switched) {
        mutator.caches.invalidate();
        crate::jit::invalidate_all_optimized(mutator);
    }
}

/// `resolve_module`, reporting the module's breakpoints when a statement
/// changed.
fn resolve_and_report(mutator: &mut Mutator<'_>, position: usize) {
    let before: Vec<Option<Span>> = mutator.breakpoints.modules[position]
        .lines
        .iter()
        .map(|line| line.statement)
        .collect();
    resolve_module(mutator, position);
    let table = &mutator.breakpoints;
    let changed = table.modules[position]
        .lines
        .iter()
        .zip(&before)
        .any(|(line, before)| line.statement != *before);
    if changed {
        table.report(Event::Breakpoints {
            module: table.modules[position].name.clone(),
            breakpoints: table.states(position),
        });
    }
}

/// Resolves the breakpoints of the module at `position` against its
/// current bodies: each is patched at its statement's slots, and its
/// patch on a current body it no longer stands at removed. A module not
/// built leaves them as they are.
fn resolve_module(mutator: &mut Mutator<'_>, position: usize) {
    let table = &mutator.breakpoints;
    let name = table.modules[position].name.clone();
    let offsets: Vec<u32> = table.modules[position]
        .lines
        .iter()
        .map(|line| line.offset)
        .collect();
    if offsets.is_empty() {
        return;
    }
    let Some(resolution) = resolve(mutator, &name, &offsets) else {
        return;
    };
    let table = &mut mutator.breakpoints;
    let ids: Vec<BreakpointId> = table.modules[position]
        .lines
        .iter()
        .map(|line| line.id)
        .collect();
    for (line, statement) in table.modules[position]
        .lines
        .iter_mut()
        .zip(&resolution.statements)
    {
        line.statement = *statement;
    }
    for (id, slots) in ids.into_iter().zip(&resolution.slots) {
        table.strip(
            |kind| kind.is_breakpoint(id),
            |patch| {
                !resolution.walked.contains(&address(&patch.code))
                    || slots
                        .iter()
                        .any(|(code, index)| Rc::ptr_eq(code, &patch.code) && *index == patch.index)
            },
        );
        for (code, index) in slots {
            let method = resolution.fast_path_methods.get(&address(code));
            table.add(code, *index, PatchKind::Line(id), method);
            // A full collection keeps a patched body's id
            // (`Breakpoints::patched_code_ids`); one without an id yet is
            // given one, as a running body is.
            mutator.heap.code_id(code);
        }
    }
    table.sweep();
}

/// Resolves `offsets` against every current body of the module whose
/// root class is named `name`: for each, the first statement starting at
/// or after it, and the slot of every body with a statement of that span.
/// `None` while no module of that name is built.
fn resolve(mutator: &mut Mutator<'_>, name: &str, offsets: &[u32]) -> Option<Resolution> {
    let (bodies, fast_path_methods) = module_bodies(mutator, name)?;
    let walked = bodies.iter().map(|(code, _)| address(code)).collect();
    let statements: Vec<Option<Span>> = offsets
        .iter()
        .map(|offset| first_statement_from(&bodies, *offset))
        .collect();
    let slots = statements
        .iter()
        .map(|statement| statement.map_or_else(Vec::new, |statement| slots_of(&bodies, statement)))
        .collect();
    Some(Resolution {
        statements,
        slots,
        walked,
        fast_path_methods,
    })
}

/// Every current body of the module whose root class is named `name` --
/// every level's members, both sides, and every block they create -- with
/// its metadata, compiled again, and every method of the module with a
/// fast path, by its body's address; `None` while no such module is
/// built.
fn module_bodies(
    mutator: &mut Mutator<'_>,
    name: &str,
) -> Option<(PairedBodies, HashMap<usize, Rc<MixinMethod>>)> {
    let ids = mutator.heap.mixin_ids();
    let roots: Vec<_> = ids
        .iter()
        .copied()
        .filter(|id| {
            mutator.heap.enclosing_mixin_id(*id).is_none()
                && mutator
                    .heap
                    .mixin(*id)
                    .is_some_and(|mixin| !mixin.is_class_side() && mixin.name.as_str() == name)
        })
        .collect();
    if roots.is_empty() {
        return None;
    }
    let mut bodies = Vec::new();
    let mut fast_path_methods = HashMap::new();
    for id in ids {
        let Some(mixin) = mutator.heap.mixin(id) else {
            continue;
        };
        // A class side is compiled again with its instance side.
        if mixin.is_class_side() {
            continue;
        }
        let (module_root, _) = mutator.path_from_module_root(id);
        if roots.contains(&module_root) {
            let sides = std::iter::once(&mixin).chain(mixin.class_side.as_ref());
            for method in sides.flat_map(|side| side.compiled_methods()) {
                if method.recognized_fast_path().is_some() {
                    fast_path_methods.insert(address(&method.code), Rc::clone(method));
                }
            }
            bodies.extend(Cache::level(mutator, &mixin));
        }
    }
    Some((bodies, fast_path_methods))
}

/// The span of the first statement with instructions that starts at or
/// after `offset` in any of `bodies`; of two starting together, the
/// outer.
fn first_statement_from(bodies: &PairedBodies, offset: u32) -> Option<Span> {
    breakable(bodies)
        .map(|(_, entry_span, _)| entry_span)
        .filter(|span| span.start >= offset)
        .min_by_key(|span| (span.start, std::cmp::Reverse(span.end)))
}

/// The slot of every statement of `bodies` whose span is `statement`:
/// its body and its first instruction.
fn slots_of(bodies: &PairedBodies, statement: Span) -> Vec<(Rc<CompiledCode>, usize)> {
    let mut slots: Vec<(Rc<CompiledCode>, usize)> = Vec::new();
    for (code, span, index) in breakable(bodies) {
        if span == statement
            && !slots
                .iter()
                .any(|(held, held_index)| Rc::ptr_eq(held, code) && *held_index == index)
        {
            slots.push((Rc::clone(code), index));
        }
    }
    slots
}

/// Every statement of `bodies` a breakpoint can stand at -- one that
/// compiled to an instruction of its body -- with its body, its span and
/// its first instruction's index. A body without metadata has none.
fn breakable(bodies: &PairedBodies) -> impl Iterator<Item = (&Rc<CompiledCode>, Span, usize)> {
    bodies.iter().flat_map(|(code, metadata)| {
        metadata
            .iter()
            .flat_map(|metadata| metadata.statements.iter())
            .filter_map(move |entry| {
                let index = usize::try_from(entry.instructions.start).ok()?;
                (!entry.instructions.is_empty() && index < code.instruction_count())
                    .then_some((code, entry.span, index))
            })
    })
}

/// A body's address, which names it while it lives: what a patch and the
/// token key it by.
pub(crate) fn address(code: &Rc<CompiledCode>) -> usize {
    Rc::as_ptr(code).addr()
}

impl Mutator<'_> {
    /// Whether no breakpoint is set: the hooks of a module's build skip
    /// their walk then.
    pub(crate) fn has_no_breakpoints(&self) -> bool {
        self.breakpoints.is_empty()
    }
}

/// The breakpoint stress mode's side of the safepoint
/// (`Interpreter::set_stress_breakpoints`): every running activation's
/// body the mode has not resolved yet, from the top down to the first
/// whose body it has, gets every statement start its metadata names
/// patched with a breakpoint that never stops, and the machine code of
/// every body patched is freed (`settle`), so that every statement start
/// of such a body is a `Breakpoint` slot, interpreted or lowered as an
/// exit. The walk stops at the first body resolved before, which
/// keeps a deep recursion's safepoints from walking its whole stack. A
/// body is found at its first entry: the mode's hold brings an
/// interpreted push here with its frame on top, and a body is compiled
/// only at its second entry. A push in machine code never takes the
/// hold, which lowers no depth limit, so a body first entered there --
/// under the JIT stress mode, or called by the machine code of a body
/// without metadata -- is missed unless a later safepoint finds it above
/// every resolved frame (`docs/design/debugger.md`, "The breakpoint
/// stress mode"). A body without metadata -- an image's without source --
/// is resolved to no slot.
#[cfg(debug_assertions)]
pub(crate) fn patch_running_for_stress(mutator: &mut Mutator<'_>) {
    let mut next = mutator.heap.top_activation();
    while let Some(activation) = next {
        let code = mutator.heap.activation_code(activation);
        if mutator.breakpoints.stressed.contains_key(&address(&code)) {
            break;
        }
        let (code, _, metadata) = super::stack::running_body(mutator, activation);
        let starts: Vec<usize> = metadata
            .iter()
            .flat_map(|metadata| metadata.statements.iter())
            .filter_map(|entry| {
                let index = usize::try_from(entry.instructions.start).ok()?;
                (!entry.instructions.is_empty() && index < code.instruction_count())
                    .then_some(index)
            })
            .collect();
        for index in &starts {
            mutator
                .breakpoints
                .add(&code, *index, PatchKind::Stress, None);
        }
        if !starts.is_empty() {
            mutator.heap.code_id(&code);
        }
        let stressed = &mut mutator.breakpoints.stressed;
        // A dead body's entry only reserves its allocation: the dead are
        // forgotten whenever the table reaches a power of two past the
        // floor, so that it grows with the bodies alive, not with every
        // body that ever ran.
        if stressed.len() >= STRESSED_FLOOR && stressed.len().is_power_of_two() {
            stressed.retain(|_, body| body.strong_count() > 0);
        }
        stressed.insert(address(&code), Rc::downgrade(&code));
        // A sender suspended into its frame object is resolved once it is
        // resumed, at the safepoint its frame next reaches on top.
        if mutator
            .heap
            .activation_suspended_sender(activation)
            .is_some()
        {
            break;
        }
        next = mutator.heap.activation_sender(activation);
    }
    settle(mutator);
}

/// The breakpoint stress mode is switched off: every slot only its
/// patches held gets its original back, the bodies it resolved are
/// forgotten and its count of hits starts again.
#[cfg(debug_assertions)]
pub(crate) fn end_stress(mutator: &mut Mutator<'_>) {
    let table = &mut mutator.breakpoints;
    table.remove_kinds(PatchKind::is_stress, |_| false);
    table.stressed.clear();
    table.stress_hits = 0;
    settle(mutator);
}

#[cfg(debug_assertions)]
impl Breakpoints {
    /// A `Breakpoint` arm reached a slot the breakpoint stress mode
    /// patched.
    pub(crate) fn count_stress_hit(&mut self) {
        self.stress_hits += 1;
    }

    /// How many times a `Breakpoint` arm reached a slot the breakpoint
    /// stress mode patched.
    pub(crate) fn stress_hits(&self) -> usize {
        self.stress_hits
    }
}
