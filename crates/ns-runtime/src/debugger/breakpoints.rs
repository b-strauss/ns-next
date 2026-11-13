//! The breakpoint table (`docs/design/debugger.md`, decision 2, "Stopping
//! at and going past a breakpoint", "Breakpoints by body, and reclaimed
//! code ids", "A breakpoint names a source position"): each line
//! breakpoint by its module and offset, the slots it patched, keyed by
//! their body and index, the token that lets a resumed stop go past its
//! breakpoint once, and the stop a `Breakpoint` arm asked for. On the
//! `Mutator`, since the run loop's arm, a module's build and an install,
//! which run inside an entry, reach nothing else.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use ns_syntax::Span;

use super::metadata::again::PairedBodies;
use super::metadata::Cache;
use super::remote::{BreakpointId, BreakpointState, Event};
use super::stepping::Holder;
use super::Attachments;
use crate::bytecode::CompiledCode;
use crate::heap::{ActivationReference, CodeId};
use crate::interpreter::Mutator;
use crate::mixin::MixinMethod;

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
    /// its body's address: the restore of the body's last patch, of
    /// whichever kind, switches it back on.
    switched_off: HashMap<usize, Rc<MixinMethod>>,
    /// The attached debugger's arming lock, under which a step holds and
    /// drops its request bit (`debugger::stepping`), as the agent arms
    /// the poll under it; `None` while none is attached.
    arming: Option<Arc<Mutex<Attachments>>>,
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
    /// A step's one-shot breakpoint (`debugger::stepping`).
    OneShot(OneShot),
}

impl PatchKind {
    /// Whether this is the line breakpoint `id`.
    fn is_line(self, id: BreakpointId) -> bool {
        matches!(self, PatchKind::Line(held) if held == id)
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
/// at it, its body's address and its index. The next `Breakpoint` arm the
/// loop executes consumes it when it names that arm's slot and drops it
/// otherwise -- the activation's offset is reused after its pop, so it
/// is never kept past that arm.
#[derive(Clone, Copy)]
pub(crate) struct PastOnce {
    pub(crate) activation: ActivationReference,
    pub(crate) body: usize,
    pub(crate) index: usize,
}

/// A stop a `Breakpoint` arm decided: where, and which breakpoints stand
/// there.
pub(crate) struct PendingStop {
    pub(crate) slot: PastOnce,
    pub(crate) ids: Vec<BreakpointId>,
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

    /// The stop the last `Breakpoint` arm decided, taken.
    pub(crate) fn take_pending_stop(&mut self) -> Option<PendingStop> {
        self.pending_stop.take()
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

    /// Drops the token: a run ended, which may leave its slot unexecuted.
    pub(crate) fn drop_past_once(&mut self) {
        self.past_once = None;
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

    /// The ids of the line breakpoints and the one-shots standing at slot
    /// `index` of the body at `body`; both empty for a slot nothing
    /// patched.
    pub(crate) fn kinds_at(&self, body: usize, index: usize) -> (Vec<BreakpointId>, Vec<OneShot>) {
        let mut ids = Vec::new();
        let mut one_shots = Vec::new();
        if let Some(patch) = self.patches.get(&(body, index)) {
            for kind in &patch.kinds {
                match kind {
                    PatchKind::Line(id) => ids.push(*id),
                    PatchKind::OneShot(one_shot) => one_shots.push(*one_shot),
                }
            }
        }
        (ids, one_shots)
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

    /// Every frame a one-shot is for.
    #[cfg(debug_assertions)]
    pub(crate) fn one_shot_holders(&self) -> Vec<Holder> {
        self.patches
            .values()
            .flat_map(|patch| patch.kinds.iter())
            .filter_map(|kind| match kind {
                PatchKind::OneShot(one_shot) => Some(one_shot.holder),
                PatchKind::Line(_) => None,
            })
            .collect()
    }

    /// Removes every one-shot, each slot no line breakpoint holds given
    /// its original back: the step ended.
    pub(crate) fn remove_one_shots(&mut self) {
        self.remove_kinds(|kind| matches!(kind, PatchKind::OneShot(_)), |_| false);
    }

    /// Whether any module has a breakpoint, resolved or pending.
    fn is_empty(&self) -> bool {
        self.modules.is_empty()
    }

    /// Patches slot `index` of `code` for `kind`; `method`, when given,
    /// is the method with a fast path whose body `code` is, which the
    /// body's first patch switches off.
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
        let held = match kind {
            PatchKind::Line(id) => patch.kinds.iter().any(|held| held.is_line(id)),
            PatchKind::OneShot(_) => false,
        };
        if !held {
            patch.kinds.push(kind);
        }
        // The fast path goes off with the first patch that names its
        // method, whatever patched the body before: a one-shot names none.
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

    /// Gives `patch`'s slot its original back, and the fast path of its
    /// body's method once no slot of the body is patched.
    fn restore(&mut self, patch: &Patch) {
        patch.code.restore(patch.index);
        if !patch.code.has_patches() {
            if let Some(method) = self.switched_off.remove(&address(&patch.code)) {
                method.switch_fast_path(true);
                self.fast_paths_switched = true;
            }
        }
        self.changed.push(Rc::clone(&patch.code));
    }

    /// Removes the line breakpoint `id` from every slot `keep` refuses; a
    /// slot whose last kind goes gets its original back, and a token
    /// naming it goes with it, so that a breakpoint set there again later
    /// stops.
    fn remove_where(&mut self, id: BreakpointId, keep: impl FnMut(&Patch) -> bool) {
        self.remove_kinds(|kind| kind.is_line(id), keep);
    }

    /// Removes every kind `removed` picks from every slot `keep` refuses,
    /// as `remove_where` does.
    fn remove_kinds(
        &mut self,
        removed: impl Fn(PatchKind) -> bool,
        mut keep: impl FnMut(&Patch) -> bool,
    ) {
        let mut emptied = Vec::new();
        for (key, patch) in &mut self.patches {
            if patch.kinds.iter().any(|kind| removed(*kind)) && !keep(patch) {
                patch.kinds.retain(|kind| !removed(*kind));
                if patch.kinds.is_empty() {
                    emptied.push(*key);
                }
            }
        }
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
/// answers their states (`Command::SetBreakpoints`).
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
                table.remove_where(id, |_| false);
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
        let id = BreakpointId(table.next_id);
        table.next_id = table.next_id.wrapping_add(1);
        lines.push(LineBreakpoint {
            id,
            offset,
            statement: None,
        });
    }
    let empty = lines.is_empty();
    table.modules[position].lines = lines;
    resolve_module(mutator, position);
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
/// replaced by an install: its breakpoints are resolved against its
/// current bodies, and a change reported unasked. A slot of a body no
/// longer current keeps its patch, so that a frame still running it stops
/// there.
pub(crate) fn module_built(mutator: &mut Mutator<'_>, name: &str) {
    if let Some(position) = mutator
        .breakpoints
        .modules
        .iter()
        .position(|held| held.name == name)
    {
        resolve_and_report(mutator, position);
        settle(mutator);
    }
}

/// Removes every breakpoint, each slot given its original back, and ends
/// any step: the debugger that set them is gone or replaced.
pub(crate) fn clear(mutator: &mut Mutator<'_>) {
    super::stepping::end(mutator);
    let table = &mut mutator.breakpoints;
    let patches: Vec<Patch> = table.patches.drain().map(|(_, patch)| patch).collect();
    for patch in &patches {
        table.restore(patch);
    }
    table.modules.clear();
    table.past_once = None;
    table.pending_stop = None;
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
        table.remove_where(id, |patch| {
            !resolution.walked.contains(&address(&patch.code))
                || slots
                    .iter()
                    .any(|(code, index)| Rc::ptr_eq(code, &patch.code) && *index == patch.index)
        });
        for (code, index) in slots {
            let method = resolution.fast_path_methods.get(&address(code));
            table.add(code, *index, PatchKind::Line(id), method);
            // A full collection keeps a patched body's id
            // (`Breakpoints::patched_code_ids`); one without an id yet is
            // given one, as a running body is.
            mutator.heap.code_id(code);
        }
    }
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
