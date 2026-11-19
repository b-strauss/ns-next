//! The developer tool's trace (`docs/design/developer-tool.md`, decisions
//! 2, 3, 5 and 8, "Stage 1: tracing"): why the tiers did what they did,
//! as typed events a sink the host installs receives
//! (`Interpreter::set_trace`). The runtime formats nothing: an event is
//! a Rust value, and the line on standard error is the host's.
//!
//! A site that records an event holds only the JIT's state, so it records
//! it pending (`EventTracer::record`), the bodies it names held as
//! `Rc<CompiledCode>` and never as a handle, so that no collection between
//! the record and the delivery moves what it names and it keeps no object
//! of the program alive. A flush (`flush`) names them from the heap,
//! applies the filter and delivers each event in order -- at the end of
//! every compile trigger the interpreter makes, at the run loop's
//! safepoint and when a host's run returns, never in a leaf machine code
//! calls, whose events, bounded by the program, wait for the next of
//! those -- so that the queue never grows with the run.

mod filter;
mod names;

use std::rc::Rc;

use crate::bytecode::CompiledCode;
use crate::interpreter::Mutator;
use crate::jit::CustomizationKey;
use crate::symbol::Symbol;
use crate::tool_names::KnownModule;

pub use filter::TraceFilter;
use names::BodyNames;

/// What a trace event is about, each switched on by its own flag.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TraceCategory {
    /// A baseline compile, its trigger and its refusal, a twin's compile,
    /// and a baseline body abandoned for its exits, revived, or refused
    /// for its twin.
    Compile,
    /// On-stack replacement compiled and switched into.
    Osr,
    /// An optimization requested, installed or refused.
    Optimize,
    /// The inlining plan's decisions.
    Inlining,
    /// An optimized copy retired or invalidated.
    Deoptimize,
    /// Machine code freed, the code zone compacted, and the zone
    /// replaced.
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
pub enum TraceEventKind {
    /// The body was compiled to baseline code and installed, taking
    /// `bytes` of the code zone (`compiled`).
    Compiled {
        trigger: CompileTrigger,
        bytes: usize,
    },
    /// The body's twin, which reifies its frame, was compiled and
    /// installed, taking `bytes` (`compiled-twin`).
    CompiledTwin { bytes: usize },
    /// The body was not compiled (`refused`): for good, but for a full
    /// zone's refusal, after which the body counts its entries again.
    CompileRefused {
        trigger: CompileTrigger,
        reason: CompileRefusal,
    },
    /// The body's machine code spent its exit budget and was freed; the
    /// body runs interpreted until it is revived (`abandoned`).
    Abandoned { exits: u32 },
    /// The abandoned body counts its entries again, to be compiled anew
    /// (`revived`, its value `by=`).
    Revived { revival: Revival },
    /// The body is refused for good, its machine code freed, since the
    /// compiler refused its twin and every frame that reifies would exit
    /// (`refused-for-its-twin`).
    RefusedForItsTwin,
    /// The body was compiled at an interpreted backward jump to
    /// `loop_head`, the bytecode index of the loop's head
    /// (`osr-compiled`, its value `at=`).
    OsrCompiled { loop_head: u32 },
    /// An interpreted frame switched into the body's machine code at the
    /// backward jump to `loop_head`, the bytecode index of the loop's
    /// head (`osr-entered`, its value `at=`).
    OsrEntered { loop_head: u32 },
    /// The body -- the recompilee -- was requested an optimization, which
    /// the next safepoint compiles (`requested`): `spent_by`'s budget ran
    /// out, and the walk up the stack from its frame stopped at the body
    /// for `walk_end`. `key` names the copy's customization key by its
    /// receiver's class and `entered` the class the body was entered at,
    /// only where it is another; both `None` for a body optimized as one
    /// copy.
    OptimizationRequested {
        spent_by: String,
        walk_end: RecompileWalkEnd,
        key: Option<String>,
        entered: Option<String>,
    },
    /// The walk from `spent_by`'s frame, whose budget ran out, stopped at
    /// the body for `walk_end`, but the body's copy for `key` was settled
    /// meanwhile -- installed or refused -- so that nothing was requested
    /// (`not-due`): `spent_by`'s budget stays parked until the next
    /// safepoint arms it afresh.
    OptimizationNotDue {
        spent_by: String,
        walk_end: RecompileWalkEnd,
        key: Option<String>,
        entered: Option<String>,
    },
    /// The body's optimization was put off, its budget armed afresh
    /// (`deferred`).
    OptimizationDeferred {
        key: Option<String>,
        entered: Option<String>,
        reason: Deferral,
    },
    /// The body's optimized copy was installed, taking `bytes` of the
    /// code zone, with how much it inlined (`optimized`).
    Optimized {
        key: Option<String>,
        entered: Option<String>,
        bytes: usize,
        inlined_scopes: u64,
        inlined_sites: u64,
        type_case_arms: u64,
        eliminated_blocks: u64,
    },
    /// The optimizing compiler refused the body's copy for good
    /// (`refused`).
    OptimizationRefused {
        key: Option<String>,
        entered: Option<String>,
        reason: CompileRefusal,
    },
    /// The plan of the body's copy inlined `callee` at the send at
    /// `program_counter` (the line's `at=`) in the scope running `in_body`
    /// (`inlined`).
    Inlined {
        key: Option<String>,
        entered: Option<String>,
        in_body: String,
        program_counter: u32,
        callee: String,
    },
    /// The plan of the body's copy sent the callee at the send at
    /// `program_counter` (the line's `at=`) in the scope running `in_body`
    /// rather than inline it, for `reason`
    /// (`declined`); `callee` is `None` for a callee that is no body.
    InlineDeclined {
        key: Option<String>,
        entered: Option<String>,
        in_body: String,
        program_counter: u32,
        callee: Option<String>,
        reason: InlineDecline,
    },
    /// The body's optimized copy for `key` was retired for `reason` --
    /// freed, the body going on in its baseline code -- and its version
    /// raised to `version`, after whose longer wait it may be compiled
    /// again; `None` when it failed past the last version and is refused
    /// for good (`retired`, then `version=` the raised version or `none`
    /// when refused for good).
    Retired {
        key: Option<String>,
        entered: Option<String>,
        reason: RetireReason,
        version: Option<u8>,
    },
    /// The body's optimized copy for `key` was freed since what it was
    /// compiled against changed, for `cause`; no version is raised
    /// (`invalidated`).
    Invalidated {
        key: Option<String>,
        entered: Option<String>,
        cause: InvalidationCause,
    },
    /// The code zone was compacted, freeing as `freeing` says and
    /// `bodies_freed` bodies in all -- each also its own `Freed` event --
    /// and the survivors slid together (`compacted`); about no body.
    Compacted {
        freeing: CompactionFreeing,
        bodies_freed: usize,
    },
    /// The body's machine code of `tier` was freed for `cause`, its bytes
    /// reclaimed by the next compaction (`freed`); about no body when the
    /// body's code died, which no mixin holds any longer.
    Freed { tier: CodeTier, cause: FreeCause },
    /// The code zone was replaced -- its size set afresh, or the JIT
    /// switched on or off -- dropping the machine code of `bodies_freed`
    /// bodies at once, none of them its own `Freed` event (`replaced`);
    /// about no body.
    ZoneReplaced { bodies_freed: usize },
    /// The inline cache at send site `site` of the body -- the root of
    /// the family the site is in, its number within the family's range --
    /// moved from `before` to `after` at a miss of a send of `selector`, whose
    /// receiver's class, qualified as every class is, is `receiver`
    /// (the line's event is `<before>-><after>`); about no body for a site of
    /// no family's, a closure's an image decoded.
    CacheTransition {
        before: SiteState,
        after: SiteState,
        selector: String,
        site: u32,
        receiver: String,
    },
}

impl TraceEventKind {
    /// The event's name, as its line spells it: kebab case.
    pub fn name(&self) -> &'static str {
        match self {
            TraceEventKind::Compiled { .. } => "compiled",
            TraceEventKind::CompiledTwin { .. } => "compiled-twin",
            TraceEventKind::CompileRefused { .. } => "refused",
            TraceEventKind::Abandoned { .. } => "abandoned",
            TraceEventKind::Revived { .. } => "revived",
            TraceEventKind::RefusedForItsTwin => "refused-for-its-twin",
            TraceEventKind::OsrCompiled { .. } => "osr-compiled",
            TraceEventKind::OsrEntered { .. } => "osr-entered",
            TraceEventKind::OptimizationRequested { .. } => "requested",
            TraceEventKind::OptimizationNotDue { .. } => "not-due",
            TraceEventKind::OptimizationDeferred { .. } => "deferred",
            TraceEventKind::Optimized { .. } => "optimized",
            TraceEventKind::OptimizationRefused { .. } => "refused",
            TraceEventKind::Inlined { .. } => "inlined",
            TraceEventKind::InlineDeclined { .. } => "declined",
            TraceEventKind::Retired { .. } => "retired",
            TraceEventKind::Invalidated { .. } => "invalidated",
            TraceEventKind::Compacted { .. } => "compacted",
            TraceEventKind::Freed { .. } => "freed",
            TraceEventKind::ZoneReplaced { .. } => "replaced",
            // The line spells the states instead, `<before>-><after>`.
            TraceEventKind::CacheTransition { .. } => "transition",
        }
    }
}

/// What made the JIT compile a body to baseline code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompileTrigger {
    /// An entry the interpreter makes: a push, an initializer's
    /// invocation, a future the interpreter forced.
    Entry,
    /// An entry machine code makes, which found no machine code for its
    /// callee: a compiled send's (the miss leaf), an initializer's
    /// invocation (the initializer leaf), a future forced (the forcing
    /// leaf).
    Call,
    /// An interpreted backward jump, compiled for on-stack replacement.
    BackJump,
}

impl CompileTrigger {
    /// The trigger's name, as a line spells it: kebab case.
    pub fn name(self) -> &'static str {
        match self {
            CompileTrigger::Entry => "entry",
            CompileTrigger::Call => "call",
            CompileTrigger::BackJump => "back-jump",
        }
    }
}

/// Why a body was not compiled to baseline code: the JIT's own reasons
/// (`jit::Refusal`), and the JIT switched off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompileRefusal {
    /// The JIT is off: every body entered meanwhile stays interpreted.
    JitOff,
    /// More instructions than a program counter of the map holds.
    TooManyInstructions,
    /// The assembler could not lay the body out.
    Unassembled,
    /// No room left in the zone: the body counts its entries again.
    ZoneFull,
    /// More bytes than the zone holds past its stubs.
    TooLarge,
    /// The code exits before it reaches a return, a compiled send or a
    /// backward jump.
    ExitsAtOnce,
}

impl CompileRefusal {
    /// The event's value of the JIT's `refusal`. The assembler's error is
    /// left out: it names an encoding, which says nothing a reader of the
    /// trace acts on.
    pub(crate) fn from_refusal(refusal: crate::jit::Refusal) -> CompileRefusal {
        match refusal {
            crate::jit::Refusal::TooManyInstructions => CompileRefusal::TooManyInstructions,
            crate::jit::Refusal::Unassembled(_) => CompileRefusal::Unassembled,
            crate::jit::Refusal::ZoneFull => CompileRefusal::ZoneFull,
            crate::jit::Refusal::TooLarge => CompileRefusal::TooLarge,
            crate::jit::Refusal::ExitsAtOnce => CompileRefusal::ExitsAtOnce,
        }
    }

    /// The refusal's name, as a line spells it: kebab case.
    pub fn name(self) -> &'static str {
        match self {
            CompileRefusal::JitOff => "jit-off",
            CompileRefusal::TooManyInstructions => "too-many-instructions",
            CompileRefusal::Unassembled => "unassembled",
            CompileRefusal::ZoneFull => "zone-full",
            CompileRefusal::TooLarge => "too-large",
            CompileRefusal::ExitsAtOnce => "exits-at-once",
        }
    }
}

/// What revived an abandoned body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Revival {
    /// Its exit count decayed at a collection.
    Decay,
    /// Its interpreted entries added up.
    Entries,
}

impl Revival {
    /// The revival's name, as a line spells it: kebab case.
    pub fn name(self) -> &'static str {
        match self {
            Revival::Decay => "decay",
            Revival::Entries => "entries",
        }
    }
}

/// Why the walk up the stack for the frame to optimize
/// (`jit::find_recompilee`) stopped where it did: the frame it stopped at
/// is the recompilee.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecompileWalkEnd {
    /// No frame lies further up: the bottom of the stack.
    BottomOfStack,
    /// The frame the walk would go to lies further down the senders than
    /// the walk looks.
    NoFramesBetween,
    /// The walk went as many frames as it goes at most.
    SearchLength,
    /// The next frame's body is inlined into a copy of a frame below it,
    /// whose own copy that one would never enter.
    InlinedIntoACopyBelow,
    /// The next frame could not be optimized: no baseline code, a copy
    /// already, refused, or run too rarely.
    NotRecompilable,
    /// The next frame's plan would not inline the frame the walk came from.
    NotInlined,
    /// No walk was made: the optimize stress mode requests the spending
    /// frame's own copy, and so does a test's request.
    NotWalked,
}

impl RecompileWalkEnd {
    /// The end's name, as a line spells it: kebab case.
    pub fn name(self) -> &'static str {
        match self {
            RecompileWalkEnd::BottomOfStack => "bottom-of-stack",
            RecompileWalkEnd::NoFramesBetween => "no-frames-between",
            RecompileWalkEnd::SearchLength => "search-length",
            RecompileWalkEnd::InlinedIntoACopyBelow => "inlined-into-a-copy-below",
            RecompileWalkEnd::NotRecompilable => "not-recompilable",
            RecompileWalkEnd::NotInlined => "not-inlined",
            RecompileWalkEnd::NotWalked => "not-walked",
        }
    }
}

/// Why an optimization was put off rather than compiled or refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Deferral {
    /// A breakpoint patched the body: its baseline code exits at the
    /// patch, which optimized code would run past.
    Patched,
    /// The code zone has no room for the copy, now or as far as it knows.
    ZoneFull,
}

impl Deferral {
    /// The deferral's name, as a line spells it: kebab case.
    pub fn name(self) -> &'static str {
        match self {
            Deferral::Patched => "patched",
            Deferral::ZoneFull => "zone-full",
        }
    }
}

/// Why the inlining plan sent a callee rather than inline it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InlineDecline {
    /// The tree holds no body of the callee to inline: a method past
    /// what the limits allow where it is sent, a nested class's accessor,
    /// a factory, or a target a `doesNotUnderstand:` lookup found.
    NotSnapshotted,
    /// The callee is a leaf or primitive the graph does not express
    /// inline for the receiver.
    NotExpressed,
    /// The callee's body leaves its activation otherwise than by a
    /// return, which an inlined scope's jump back cannot stand for.
    ReturnsOtherwise,
    /// The callee is a lazy slot's getter whose field the receiver's
    /// level does not locate.
    UnknownLazyField,
    /// The callee is observed: an unwind marker or a leaf that reads an
    /// activation keeps it a frame of its own.
    Observed,
    /// The block reaches a frame the compile does not lay out.
    ReachesPastTheCompile,
    /// The callee, a block taken at a `value` send, escapes after all:
    /// the block is created and the send sent.
    BlockEscapes,
    /// The callee was taken, but its scope would create a block, which
    /// would leave optimized code at its `MakeClosure` every time.
    CreatesABlock,
    /// The callee weighs more than its limit.
    OverWeight,
    /// The compile would weigh more than its limit with the callee.
    OverCompileLimit,
    /// The callee already runs in the chain as often as the limit allows.
    OverRecursionLimit,
    /// The callee's frame would lie past the register ceiling.
    OutOfRegisters,
}

impl InlineDecline {
    /// The reason's name, as a line spells it: kebab case.
    pub fn name(self) -> &'static str {
        match self {
            InlineDecline::NotSnapshotted => "not-snapshotted",
            InlineDecline::NotExpressed => "not-expressed",
            InlineDecline::ReturnsOtherwise => "returns-otherwise",
            InlineDecline::UnknownLazyField => "unknown-lazy-field",
            InlineDecline::Observed => "observed",
            InlineDecline::ReachesPastTheCompile => "reaches-past-the-compile",
            InlineDecline::BlockEscapes => "block-escapes",
            InlineDecline::CreatesABlock => "creates-a-block",
            InlineDecline::OverWeight => "over-weight",
            InlineDecline::OverCompileLimit => "over-compile-limit",
            InlineDecline::OverRecursionLimit => "over-recursion-limit",
            InlineDecline::OutOfRegisters => "out-of-registers",
        }
    }
}

/// Why an optimized copy was retired.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetireReason {
    /// A speculation failed again at a site that had trapped before, or
    /// the copy's traps reached their limit: made not entrant.
    Traps,
    /// The copy spent its exit budget: abandoned.
    Exits,
}

impl RetireReason {
    /// The reason's name, as a line spells it: kebab case.
    pub fn name(self) -> &'static str {
        match self {
            RetireReason::Traps => "traps",
            RetireReason::Exits => "exits",
        }
    }
}

/// What changed under an optimized copy, so that it was invalidated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InvalidationCause {
    /// An install rebuilt an application the copy is keyed on or relied
    /// on.
    Edit,
    /// A bulk forward (`elementsOf:forwardIdentityToElementsOf:ifFail:`),
    /// which may have replaced any object a copy was verified for.
    BulkForward,
    /// A full collection reclaimed an application id the copy is keyed on
    /// or tests, which is minted again for another application.
    ReclaimedId,
    /// A full collection moved the feedback base of a body the copy
    /// inlined.
    MovedFeedbackBase,
    /// A breakpoint patched or restored a body the copy inlined, or
    /// switched a method's fast path.
    Breakpoint,
    /// The invalidate stress mode's round.
    Stress,
}

impl InvalidationCause {
    /// The cause's name, as a line spells it: kebab case.
    pub fn name(self) -> &'static str {
        match self {
            InvalidationCause::Edit => "edit",
            InvalidationCause::BulkForward => "bulk-forward",
            InvalidationCause::ReclaimedId => "reclaimed-id",
            InvalidationCause::MovedFeedbackBase => "moved-feedback-base",
            InvalidationCause::Breakpoint => "breakpoint",
            InvalidationCause::Stress => "stress",
        }
    }
}

/// The tier whose machine code was freed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodeTier {
    /// A body's baseline code, or its twin's.
    Baseline,
    /// An optimized copy.
    Optimized,
}

impl CodeTier {
    /// The tier's name, as a line spells it: kebab case.
    pub fn name(self) -> &'static str {
        match self {
            CodeTier::Baseline => "baseline",
            CodeTier::Optimized => "optimized",
        }
    }
}

/// Why machine code was freed. A copy freed by its retirement or its
/// invalidation is the `deoptimize` category's `Retired` or `Invalidated`
/// event, and no `Freed` one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FreeCause {
    /// A compaction freed it: little used, or no longer what its body's
    /// slot names.
    Compaction,
    /// A full collection found its body dead, or its slot no longer
    /// naming it.
    DeadBody,
    /// A full collection moved the feedback base it was compiled against.
    MovedFeedbackBase,
    /// A breakpoint patched or restored its body.
    Breakpoint,
    /// Its body was abandoned for its exits.
    Abandoned,
    /// Its body was refused for good once the compiler refused its twin,
    /// since every frame that reifies would exit.
    RefusedForItsTwin,
}

impl FreeCause {
    /// The cause's name, as a line spells it: kebab case.
    pub fn name(self) -> &'static str {
        match self {
            FreeCause::Compaction => "compaction",
            FreeCause::DeadBody => "dead-body",
            FreeCause::MovedFeedbackBase => "moved-feedback-base",
            FreeCause::Breakpoint => "breakpoint",
            FreeCause::Abandoned => "abandoned",
            FreeCause::RefusedForItsTwin => "refused-for-its-twin",
        }
    }
}

/// Which bodies a compaction freed beyond those always freed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompactionFreeing {
    /// The least used first, until a quarter of the bodies' room is free,
    /// and at least what the compile that found no room needed.
    ToAQuarter,
    /// Every body no activation runs: the zone stress mode's.
    EveryInactiveBody,
}

impl CompactionFreeing {
    /// The freeing's name, as a line spells it: kebab case.
    pub fn name(self) -> &'static str {
        match self {
            CompactionFreeing::ToAQuarter => "to-a-quarter",
            CompactionFreeing::EveryInactiveBody => "every-inactive-body",
        }
    }
}

/// An inline cache site's state (`docs/design/dispatch.md`, "A site's
/// states").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SiteState {
    /// No entry: the next send misses.
    Vacant,
    /// One receiver application's entry.
    Monomorphic,
    /// A block of up to four entries, in the order first seen.
    Polymorphic,
    /// More applications than a block holds: every send goes to the
    /// global cache, and the site stays so.
    Megamorphic,
}

impl SiteState {
    /// The state's name, as a line spells it: kebab case.
    pub fn name(self) -> &'static str {
        match self {
            SiteState::Vacant => "vacant",
            SiteState::Monomorphic => "monomorphic",
            SiteState::Polymorphic => "polymorphic",
            SiteState::Megamorphic => "megamorphic",
        }
    }
}

/// What a host installs to receive the events (`Interpreter::set_trace`).
pub trait TraceSink {
    /// Receives `event`, in the order the events happened. Called outside
    /// machine code only: at the end of a compile trigger the interpreter
    /// makes, at the run loop's safepoint, or when a host's run returns --
    /// never from a leaf machine code calls, whose events wait for the
    /// next of those.
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
pub(crate) enum PendingKind {
    Compiled {
        trigger: CompileTrigger,
        bytes: usize,
    },
    CompiledTwin {
        bytes: usize,
    },
    CompileRefused {
        trigger: CompileTrigger,
        reason: CompileRefusal,
    },
    Abandoned {
        exits: u32,
    },
    Revived {
        revival: Revival,
    },
    RefusedForItsTwin,
    OsrCompiled {
        loop_head: u32,
    },
    OsrEntered {
        loop_head: u32,
    },
    OptimizationRequested {
        spent_by: Rc<CompiledCode>,
        walk_end: RecompileWalkEnd,
        key: Option<CustomizationKey>,
    },
    OptimizationNotDue {
        spent_by: Rc<CompiledCode>,
        walk_end: RecompileWalkEnd,
        key: Option<CustomizationKey>,
    },
    OptimizationDeferred {
        key: Option<CustomizationKey>,
        reason: Deferral,
    },
    Optimized {
        key: Option<CustomizationKey>,
        bytes: usize,
        inlined_scopes: u64,
        inlined_sites: u64,
        type_case_arms: u64,
        eliminated_blocks: u64,
    },
    OptimizationRefused {
        key: Option<CustomizationKey>,
        reason: CompileRefusal,
    },
    Inlined {
        key: Option<CustomizationKey>,
        in_body: Rc<CompiledCode>,
        program_counter: u32,
        callee: Rc<CompiledCode>,
    },
    InlineDeclined {
        key: Option<CustomizationKey>,
        in_body: Rc<CompiledCode>,
        program_counter: u32,
        callee: Option<Rc<CompiledCode>>,
        reason: InlineDecline,
    },
    Retired {
        key: Option<CustomizationKey>,
        reason: RetireReason,
        version: Option<u8>,
    },
    Invalidated {
        key: Option<CustomizationKey>,
        cause: InvalidationCause,
    },
    Compacted {
        freeing: CompactionFreeing,
        bodies_freed: usize,
    },
    Freed {
        tier: CodeTier,
        cause: FreeCause,
    },
    ZoneReplaced {
        bodies_freed: usize,
    },
    CacheTransition {
        before: SiteState,
        after: SiteState,
        selector: Symbol,
        site: u32,
        receiver: String,
    },
}

impl Pending {
    /// A pending event of `category` about `code`.
    pub(crate) fn about(
        category: TraceCategory,
        code: &Rc<CompiledCode>,
        kind: PendingKind,
    ) -> Pending {
        Pending {
            category,
            body: Some(Rc::clone(code)),
            kind,
        }
    }
}

/// The trace's state, held by the JIT's (`JitState::tracer`), since every
/// site but the caches' holds that alone.
#[derive(Default)]
pub(crate) struct EventTracer {
    /// The categories the sink asked for; empty without a sink.
    categories: TraceCategories,
    filter: Option<TraceFilter>,
    sink: Option<Box<dyn TraceSink>>,
    /// The events recorded since the last flush, in order.
    pending: Vec<Pending>,
    /// How many events were recorded since the sink was installed: what a
    /// test reads to see that a site recorded nothing it was not asked
    /// for (`Interpreter::trace_events_recorded`).
    recorded: u64,
    names: BodyNames,
    /// Whether the filter was checked against the program's modules,
    /// which only a flush can reach.
    filter_checked: bool,
    /// Why the filter names a module the program does not have, as the
    /// first flush found it.
    filter_refusal: Option<String>,
}

impl EventTracer {
    /// Whether an event of `category` is recorded: the one test a site
    /// makes before it builds anything.
    #[inline]
    pub(crate) fn traces(&self, category: TraceCategory) -> bool {
        self.categories.contains(category)
    }

    /// Records `pending`, delivered at the next flush.
    pub(crate) fn record(&mut self, pending: Pending) {
        debug_assert!(
            self.categories.contains(pending.category),
            "internal: an event of a category nothing asked for was recorded"
        );
        self.recorded += 1;
        self.pending.push(pending);
    }

    /// How many events were recorded since the sink was installed.
    pub(crate) fn recorded(&self) -> u64 {
        self.recorded
    }

    /// How many events wait for a flush (`Interpreter::trace_queue_length`).
    pub(crate) fn queue_length(&self) -> usize {
        self.pending.len()
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
        *self = EventTracer::default();
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
pub(crate) fn flush(mutator: &mut Mutator<'_>) {
    let pending = std::mem::take(&mut mutator.jit.tracer.pending);
    let mut names = std::mem::take(&mut mutator.jit.tracer.names);
    if !mutator.jit.tracer.filter_checked {
        mutator.jit.tracer.filter_checked = true;
        if let Some(filter) = &mutator.jit.tracer.filter {
            let roots = module_roots(mutator);
            let module_of = |text: &str| KnownModule::resolve(text, &roots);
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
            let kind = event_kind(mutator, &mut names, kind);
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
fn event_kind(mutator: &Mutator<'_>, names: &mut BodyNames, kind: PendingKind) -> TraceEventKind {
    let mut name = |code: &Rc<CompiledCode>| names.name(mutator, code);
    let key_fields = |key: Option<CustomizationKey>| match key {
        Some(key) => {
            let (receiver, entered) = key_names(mutator, key);
            (Some(receiver), entered)
        }
        None => (None, None),
    };
    match kind {
        PendingKind::Compiled { trigger, bytes } => TraceEventKind::Compiled { trigger, bytes },
        PendingKind::CompiledTwin { bytes } => TraceEventKind::CompiledTwin { bytes },
        PendingKind::CompileRefused { trigger, reason } => {
            TraceEventKind::CompileRefused { trigger, reason }
        }
        PendingKind::Abandoned { exits } => TraceEventKind::Abandoned { exits },
        PendingKind::Revived { revival } => TraceEventKind::Revived { revival },
        PendingKind::RefusedForItsTwin => TraceEventKind::RefusedForItsTwin,
        PendingKind::OsrCompiled { loop_head } => TraceEventKind::OsrCompiled { loop_head },
        PendingKind::OsrEntered { loop_head } => TraceEventKind::OsrEntered { loop_head },
        PendingKind::OptimizationRequested {
            spent_by,
            walk_end,
            key,
        } => {
            let (key, entered) = key_fields(key);
            TraceEventKind::OptimizationRequested {
                spent_by: name(&spent_by),
                walk_end,
                key,
                entered,
            }
        }
        PendingKind::OptimizationNotDue {
            spent_by,
            walk_end,
            key,
        } => {
            let (key, entered) = key_fields(key);
            TraceEventKind::OptimizationNotDue {
                spent_by: name(&spent_by),
                walk_end,
                key,
                entered,
            }
        }
        PendingKind::OptimizationDeferred { key, reason } => {
            let (key, entered) = key_fields(key);
            TraceEventKind::OptimizationDeferred {
                key,
                entered,
                reason,
            }
        }
        PendingKind::Optimized {
            key,
            bytes,
            inlined_scopes,
            inlined_sites,
            type_case_arms,
            eliminated_blocks,
        } => {
            let (key, entered) = key_fields(key);
            TraceEventKind::Optimized {
                key,
                entered,
                bytes,
                inlined_scopes,
                inlined_sites,
                type_case_arms,
                eliminated_blocks,
            }
        }
        PendingKind::OptimizationRefused { key, reason } => {
            let (key, entered) = key_fields(key);
            TraceEventKind::OptimizationRefused {
                key,
                entered,
                reason,
            }
        }
        PendingKind::Inlined {
            key,
            in_body,
            program_counter,
            callee,
        } => {
            let (key, entered) = key_fields(key);
            TraceEventKind::Inlined {
                key,
                entered,
                in_body: name(&in_body),
                program_counter,
                callee: name(&callee),
            }
        }
        PendingKind::InlineDeclined {
            key,
            in_body,
            program_counter,
            callee,
            reason,
        } => {
            let (key, entered) = key_fields(key);
            TraceEventKind::InlineDeclined {
                key,
                entered,
                in_body: name(&in_body),
                program_counter,
                callee: callee.map(|callee| name(&callee)),
                reason,
            }
        }
        PendingKind::Retired {
            key,
            reason,
            version,
        } => {
            let (key, entered) = key_fields(key);
            TraceEventKind::Retired {
                key,
                entered,
                reason,
                version,
            }
        }
        PendingKind::Invalidated { key, cause } => {
            let (key, entered) = key_fields(key);
            TraceEventKind::Invalidated {
                key,
                entered,
                cause,
            }
        }
        PendingKind::Compacted {
            freeing,
            bodies_freed,
        } => TraceEventKind::Compacted {
            freeing,
            bodies_freed,
        },
        PendingKind::Freed { tier, cause } => TraceEventKind::Freed { tier, cause },
        PendingKind::ZoneReplaced { bodies_freed } => TraceEventKind::ZoneReplaced { bodies_freed },
        PendingKind::CacheTransition {
            before,
            after,
            selector,
            site,
            receiver,
        } => TraceEventKind::CacheTransition {
            before,
            after,
            selector: selector.as_str().to_string(),
            site,
            receiver,
        },
    }
}

/// The fields that name the customization key `key`: its receiver's class
/// and, only when the application it entered is another, that one's,
/// each qualified as every class is (`qualified_name`); an id no
/// application holds any longer is `#<id>`. Never joined by `@`, which
/// names a version.
pub(crate) fn key_names(mutator: &Mutator<'_>, key: CustomizationKey) -> (String, Option<String>) {
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
