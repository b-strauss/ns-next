//! The developer tool's trace (`docs/design/developer-tool.md`, decisions
//! 2, 3, 5 and 8, "Stage 1: tracing"): why the tiers did what they did,
//! as typed events a sink the host installs receives
//! (`Interpreter::set_trace`). The runtime formats nothing: an event is
//! a Rust value, and the line on standard error is the host's.
//!
//! A site that records an event holds only the JIT's state, so it records
//! it pending (`Tracer::record`), the bodies it names held as
//! `Rc<CompiledCode>` and never as a handle, so that no collection between
//! the record and the delivery moves what it names and it keeps no object
//! of the program alive. A flush (`flush`) names them from the heap,
//! applies the filter and delivers each event in order -- at the end of
//! every compile trigger, at the run loop's safepoint and when a host's
//! run returns -- so that the queue never grows with the run.

mod filter;
mod names;

use std::rc::Rc;

use crate::bytecode::CompiledCode;
use crate::interpreter::Mutator;
use crate::tool_names::KnownModule;

pub use filter::TraceFilter;
use names::BodyNames;

/// What a trace event is about, each switched on by its own flag.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TraceCategory {
    /// A baseline compile, its trigger and its refusal.
    Compile,
    /// On-stack replacement compiled and switched into.
    Osr,
    /// An optimization requested, installed or refused.
    Optimize,
    /// The inlining plan's decisions.
    Inlining,
    /// An optimized copy retired, invalidated or freed, and a baseline
    /// body abandoned or revived.
    Deoptimize,
    /// Machine code freed by a compaction or a full collection.
    CodeZone,
    /// An inline cache's transition.
    Caches,
}

impl TraceCategory {
    /// Every category, in the order the documentation lists them.
    pub const ALL: [TraceCategory; 7] = [
        TraceCategory::Compile,
        TraceCategory::Osr,
        TraceCategory::Optimize,
        TraceCategory::Inlining,
        TraceCategory::Deoptimize,
        TraceCategory::CodeZone,
        TraceCategory::Caches,
    ];

    /// The category's name, as `--trace` takes it and a line begins with
    /// it: kebab case.
    pub fn name(self) -> &'static str {
        match self {
            TraceCategory::Compile => "compile",
            TraceCategory::Osr => "osr",
            TraceCategory::Optimize => "optimize",
            TraceCategory::Inlining => "inlining",
            TraceCategory::Deoptimize => "deoptimize",
            TraceCategory::CodeZone => "code-zone",
            TraceCategory::Caches => "caches",
        }
    }

    /// The category named `name`; `None` for a name no category has.
    pub fn from_name(name: &str) -> Option<TraceCategory> {
        TraceCategory::ALL
            .into_iter()
            .find(|category| category.name() == name)
    }

    /// The category's bit in `TraceCategories`.
    fn bit(self) -> u8 {
        1 << (self as u8)
    }
}

/// A set of categories, one bit each, as an event's site tests it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TraceCategories(u8);

impl TraceCategories {
    /// This set and `category`.
    pub fn with(self, category: TraceCategory) -> TraceCategories {
        TraceCategories(self.0 | category.bit())
    }

    /// Whether `category` is in the set.
    pub fn contains(self, category: TraceCategory) -> bool {
        self.0 & category.bit() != 0
    }

    /// Whether the set has no category.
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// One event, as the sink receives it: its category, the body it is
/// about -- named as the debugger names a frame, `Demo/Account>>deposit:`,
/// `(unnamed)` for one no mixin holds -- or `None` for an event about no
/// body, and what happened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceEvent {
    pub category: TraceCategory,
    pub body: Option<String>,
    pub kind: TraceEventKind,
}

/// What happened, with its reasons as values.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TraceEventKind {}

/// What a host installs to receive the events (`Interpreter::set_trace`).
pub trait TraceSink {
    /// Receives `event`, in the order the events happened.
    fn event(&mut self, event: &TraceEvent);
}

/// An event recorded and not yet delivered: the bodies it names held as
/// code, named at the flush.
pub(crate) struct Pending {
    pub(crate) category: TraceCategory,
    pub(crate) body: Option<Rc<CompiledCode>>,
    pub(crate) kind: PendingKind,
}

/// What a pending event says happened: `TraceEventKind`'s variants, a
/// second body held as `Rc<CompiledCode>` and a customization key as
/// `CustomizationKey`, both named at the flush.
pub(crate) enum PendingKind {}

/// The trace's state, held by the JIT's (`JitState::tracer`), since every
/// site but the caches' holds that alone.
#[derive(Default)]
pub(crate) struct Tracer {
    /// The categories the sink asked for; empty without a sink.
    categories: TraceCategories,
    filter: Option<TraceFilter>,
    sink: Option<Box<dyn TraceSink>>,
    /// The events recorded since the last flush, in order.
    pending: Vec<Pending>,
    names: BodyNames,
    /// Whether the filter was checked against the program's modules,
    /// which only a flush can reach.
    filter_checked: bool,
    /// Why the filter names a module the program does not have, as the
    /// first flush found it.
    filter_refusal: Option<String>,
}

impl Tracer {
    /// Whether an event of `category` is recorded: the one test a site
    /// makes before it builds anything.
    #[inline]
    #[expect(dead_code, reason = "the sites that ask are added with their events")]
    pub(crate) fn traces(&self, category: TraceCategory) -> bool {
        self.categories.contains(category)
    }

    /// Records `pending`, delivered at the next flush.
    #[expect(
        dead_code,
        reason = "the sites that record are added with their events"
    )]
    pub(crate) fn record(&mut self, pending: Pending) {
        debug_assert!(
            self.categories.contains(pending.category),
            "internal: an event of a category nothing asked for was recorded"
        );
        self.pending.push(pending);
    }

    /// Whether an event waits for a flush: the test each flush point
    /// makes before it calls `flush`.
    #[inline]
    pub(crate) fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Installs `sink` for `categories` and `filter`, replacing any sink
    /// and dropping what it had not delivered; with no category, no sink.
    pub(crate) fn install(
        &mut self,
        categories: TraceCategories,
        filter: TraceFilter,
        sink: Box<dyn TraceSink>,
    ) {
        *self = Tracer::default();
        if categories.is_empty() {
            return;
        }
        self.categories = categories;
        self.filter = Some(filter);
        self.sink = Some(sink);
    }

    /// Why the installed filter names a module the program does not have,
    /// once a flush checked it; `None` before and when it names none.
    pub(crate) fn filter_refusal(&self) -> Option<String> {
        self.filter_refusal.clone()
    }
}

/// Delivers every pending event: names each body it holds, drops what the
/// filter refuses, hands the rest to the sink in order and empties the
/// queue. The first flush checks the filter against the program's
/// modules (`TraceFilter::check`), which only the heap knows.
#[cold]
#[inline(never)]
#[expect(
    unreachable_code,
    unused_mut,
    unused_variables,
    reason = "no event kind exists yet, so no event is delivered past the conversion"
)]
pub(crate) fn flush(mutator: &mut Mutator<'_>) {
    let pending = std::mem::take(&mut mutator.jit.tracer.pending);
    let mut names = std::mem::take(&mut mutator.jit.tracer.names);
    if !mutator.jit.tracer.filter_checked {
        mutator.jit.tracer.filter_checked = true;
        if let Some(filter) = &mutator.jit.tracer.filter {
            let roots = module_roots(mutator);
            let module_of = |text: &str| known_module(&roots, text);
            mutator.jit.tracer.filter_refusal = filter.check(&module_of).err();
        }
    }
    let mut events = Vec::with_capacity(pending.len());
    for Pending {
        category,
        body,
        kind,
    } in pending
    {
        let body = body.map(|code| names.name(mutator, &code));
        let passes = match (&mutator.jit.tracer.filter, &body) {
            (Some(filter), Some(body)) => filter.matches(body),
            _ => true,
        };
        if passes {
            let kind = event_kind(kind);
            events.push(TraceEvent {
                category,
                body,
                kind,
            });
        }
    }
    mutator.jit.tracer.names = names;
    if let Some(sink) = mutator.jit.tracer.sink.as_mut() {
        for event in &events {
            sink.event(event);
        }
    }
}

/// The event a pending event's kind delivers, its bodies and keys named.
fn event_kind(kind: PendingKind) -> TraceEventKind {
    match kind {}
}

/// The fields that name the customization key `key`: its receiver's class
/// and, only when the application it entered is another, that one's,
/// each qualified as every class is (`qualified_name`); an id no
/// application holds any longer is `#<id>`. Never joined by `@`, which
/// names a version.
#[expect(
    dead_code,
    reason = "the events that name a key are added with their variants"
)]
pub(crate) fn key_names(
    mutator: &Mutator<'_>,
    key: crate::jit::CustomizationKey,
) -> (String, Option<String>) {
    let name = |id: crate::mixin::DispatchId| {
        mutator
            .heap
            .application_of_dispatch_id(id)
            .and_then(|application| mutator.heap.application(application))
            .and_then(|application| {
                crate::debugger::metadata::again::qualified_name(mutator, &application.mixin)
            })
            .unwrap_or_else(|| format!("#{}", id.bits()))
    };
    let entered = (key.entered != key.receiver).then(|| name(key.entered));
    (name(key.receiver), entered)
}

/// The module every root mixin of the heap and every shipped module is,
/// with its version: what a filter's pattern may name.
fn module_roots(mutator: &Mutator<'_>) -> Vec<crate::debugger::ModuleName> {
    let heap = &mutator.heap;
    let mut roots: Vec<crate::debugger::ModuleName> = heap
        .mixin_ids()
        .into_iter()
        .filter(|id| heap.enclosing_mixin_id(*id).is_none())
        .filter_map(|id| heap.mixin(id))
        .filter(|mixin| {
            !mixin.is_class_side() && (mixin.module_identity.is_some() || mixin.text.is_some())
        })
        .map(|root| crate::debugger::ModuleName::of_root(&root))
        .collect();
    roots.extend(
        crate::interpreter::core_library::shipped_source_files()
            .iter()
            .map(|file| {
                crate::debugger::ModuleName::named(
                    crate::interpreter::core_library::shipped_module_id(file.name),
                )
            }),
    );
    roots
}

/// The module of `roots` a pattern's `text` names, as the DAP agent's
/// sources answer it: `text` itself where it is a module's id, and for an
/// undotted `text` the one module whose class it names.
fn known_module(roots: &[crate::debugger::ModuleName], text: &str) -> Option<KnownModule> {
    let id = if roots.iter().any(|root| root.id == text) {
        text.to_string()
    } else if text.contains('.') {
        return None;
    } else {
        let mut by_class = roots
            .iter()
            .filter(|root| root.id.rsplit('.').next() == Some(text));
        let first = by_class.next()?;
        if !by_class.all(|other| other.id == first.id) {
            return None;
        }
        first.id.clone()
    };
    let mut versions: Vec<String> = roots
        .iter()
        .filter(|root| root.id == id)
        .filter_map(|root| root.version.clone())
        .collect();
    versions.sort();
    versions.dedup();
    Some(KnownModule { id, versions })
}
