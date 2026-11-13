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

use ns_syntax::Span;

use super::metadata::again::PairedBodies;
use super::metadata::Cache;
use super::remote::{BreakpointId, BreakpointState, Event};
use crate::bytecode::CompiledCode;
use crate::heap::{ActivationReference, CodeId};
use crate::interpreter::Mutator;

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
#[derive(Clone, Copy, PartialEq, Eq)]
enum PatchKind {
    Line(BreakpointId),
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
}

impl Breakpoints {
    /// Where unasked reports go from here on: the attached debugger's
    /// channel, or `None` once none is attached.
    pub(crate) fn report_to(&mut self, events: Option<Sender<Event>>) {
        self.events = events;
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

    /// The ids of the breakpoints standing at slot `index` of the body at
    /// `body`; empty for a slot no breakpoint patched.
    pub(crate) fn ids_at(&self, body: usize, index: usize) -> Vec<BreakpointId> {
        self.patches
            .get(&(body, index))
            .map(|patch| {
                patch
                    .kinds
                    .iter()
                    .map(|kind| match kind {
                        PatchKind::Line(id) => *id,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Whether any module has a breakpoint, resolved or pending.
    fn is_empty(&self) -> bool {
        self.modules.is_empty()
    }

    /// Patches slot `index` of `code` for `kind`.
    fn add(&mut self, code: &Rc<CompiledCode>, index: usize, kind: PatchKind) {
        let patch = self
            .patches
            .entry((address(code), index))
            .or_insert_with(|| Patch {
                code: Rc::clone(code),
                index,
                kinds: Vec::new(),
            });
        if !patch.kinds.contains(&kind) {
            patch.kinds.push(kind);
        }
        code.patch(index);
    }

    /// Removes `kind` from every slot `keep` refuses; a slot whose last
    /// kind goes gets its original back, and a token naming it goes with
    /// it, so that a breakpoint set there again later stops.
    fn remove_where(&mut self, kind: PatchKind, mut keep: impl FnMut(&Patch) -> bool) {
        let mut emptied = Vec::new();
        for (key, patch) in &mut self.patches {
            if patch.kinds.contains(&kind) && !keep(patch) {
                patch.kinds.retain(|held| *held != kind);
                if patch.kinds.is_empty() {
                    emptied.push(*key);
                }
            }
        }
        for key in emptied {
            if let Some(patch) = self.patches.remove(&key) {
                patch.code.restore(patch.index);
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
                table.remove_where(PatchKind::Line(id), |_| false);
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
    }
}

/// Removes every breakpoint, each slot given its original back: the
/// debugger that set them is gone or replaced.
pub(crate) fn clear(mutator: &mut Mutator<'_>) {
    let table = &mut mutator.breakpoints;
    for patch in table.patches.values() {
        patch.code.restore(patch.index);
    }
    table.patches.clear();
    table.modules.clear();
    table.past_once = None;
    table.pending_stop = None;
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
        table.remove_where(PatchKind::Line(id), |patch| {
            !resolution.walked.contains(&address(&patch.code))
                || slots
                    .iter()
                    .any(|(code, index)| Rc::ptr_eq(code, &patch.code) && *index == patch.index)
        });
        for (code, index) in slots {
            table.add(code, *index, PatchKind::Line(id));
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
    let bodies = module_bodies(mutator, name)?;
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
    })
}

/// Every current body of the module whose root class is named `name` --
/// every level's members, both sides, and every block they create -- with
/// its metadata, compiled again; `None` while no such module is built.
fn module_bodies(mutator: &mut Mutator<'_>, name: &str) -> Option<PairedBodies> {
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
            bodies.extend(Cache::level(mutator, &mixin));
        }
    }
    Some(bodies)
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
