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
//! every compile trigger the interpreter makes, on the way into and out
//! of the run loop's safepoint and when a host's run returns, never in a
//! leaf machine code calls, whose events wait for the next of those --
//! so that the queue never grows with the run. A key whose id a full
//! collection reclaims is named in the collection (`name_freed_keys`),
//! while the id still names its class.

mod filter;
mod listings;
mod names;

use std::collections::HashMap;
use std::rc::{Rc, Weak};

use crate::bytecode::CompiledCode;
use crate::hash::IdentityMap;
use crate::interpreter::Mutator;
use crate::jit::CustomizationKey;
use crate::mixin::DispatchId;
use crate::symbol::Symbol;

pub use filter::TraceFilter;
#[cfg(ns_jit)]
pub(crate) use listings::PendingCode;
pub(crate) use listings::PendingListing;
pub use listings::{
    Annotation, Architecture, BytecodeListing, CodeListing, DataWord, ListedScope, Listing,
    Position,
};
use names::BodyNames;

/// What a trace event is about, each switched on by its own flag.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TraceCategory {
    /// A baseline compile, its trigger, its refusal and its hold, a
    /// twin's compile and hold, and a baseline body abandoned for its
    /// exits, revived, refused for its twin, or refused while abandoned.
    Compile,
    /// On-stack replacement compiled, switched into, or refused.
    Osr,
    /// An optimization requested, installed, refused or dropped.
    Optimize,
    /// The inlining plan's decisions.
    Inlining,
    /// An optimized copy retired or invalidated.
    Deoptimize,
    /// Machine code freed, the code zone compacted, and the zone
    /// replaced.
    CodeZone,
    /// An inline cache's transition, and every site vacated at once.
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

/// Which code a run's listings print (`--print-bytecode`, `--print-code`,
/// `--print-inlining`; `docs/design/developer-tool.md`, "Stage 2: printing
/// code"), each switched on by its own flag.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Printing {
    /// Every body's bytecode, when its mixin is made.
    pub bytecode: bool,
    /// Every body's machine code, baseline and optimized, when it is
    /// installed.
    pub code: bool,
    /// The inlining plan of every optimizing compile that installed a
    /// copy.
    pub inlining: bool,
}

impl Printing {
    /// Whether nothing is printed.
    pub fn is_empty(self) -> bool {
        !(self.bytecode || self.code || self.inlining)
    }
}

/// What a host asks of the trace (`Interpreter::set_trace`): the events of
/// `categories` about the bodies `filter` selects, and the listings
/// `printing` names of the bodies `print_filter` selects. Empty
/// categories and empty printing are no tracing.
pub struct TraceSettings {
    /// The categories whose events the sink receives.
    pub categories: TraceCategories,
    /// The bodies whose events the sink receives.
    pub filter: TraceFilter,
    /// The listings the sink receives.
    pub printing: Printing,
    /// The bodies whose listings the sink receives.
    pub print_filter: TraceFilter,
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
    /// The body came due but its compile was held, for `reason`, and not
    /// tried (`held`): the exhausted zone is known to have no room for
    /// it. Its first hold since it was last compiled only; the body is
    /// tried again once the zone has room.
    CompileHeld {
        trigger: CompileTrigger,
        reason: HoldReason,
    },
    /// The body's twin was needed but its compile was held, for `reason`
    /// (`held-twin`), as `CompileHeld` says of a body: the frame that
    /// needed it goes on interpreted. Its first hold since the twin was
    /// last compiled only.
    TwinHeld { reason: HoldReason },
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
    /// The abandoned body is refused for good, for `reason`, rather than
    /// revived later (`refused-abandoned`): the JIT was switched off.
    AbandonedRefused { reason: CompileRefusal },
    /// The body was compiled at an interpreted backward jump to
    /// `loop_head`, the bytecode index of the loop's head
    /// (`osr-compiled`, its value `at=`).
    OsrCompiled { loop_head: u32 },
    /// An interpreted frame switched into the body's machine code at the
    /// backward jump to `loop_head`, the bytecode index of the loop's
    /// head (`osr-entered`, its value `at=`).
    OsrEntered { loop_head: u32 },
    /// An interpreted frame of the body, which has machine code, did not
    /// switch into it at the backward jump to `loop_head` for `reason`
    /// (`osr-refused`, its value `at=`): it goes on interpreted, and
    /// tries again after as many back jumps as compile a body.
    OsrRefused { loop_head: u32, reason: OsrRefusal },
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
    /// The body's pending request for the copy of `key` was dropped before
    /// the safepoint compiled it, since what it would be compiled against
    /// changed, for `cause`; its budget is armed afresh (`dropped`).
    OptimizationDropped {
        key: Option<String>,
        entered: Option<String>,
        cause: InvalidationCause,
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
    /// body's code died, which no mixin holds any longer. `key` and
    /// `entered` name an optimized copy's customization key, as the
    /// optimizing tier's events do.
    Freed {
        key: Option<String>,
        entered: Option<String>,
        tier: CodeTier,
        cause: FreeCause,
    },
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
    /// no family's, a closure's an image decoded. A receiver whose class
    /// has no declaration, and so no module -- the VM mirror's -- is named
    /// by its class's display name, unqualified.
    CacheTransition {
        before: SiteState,
        after: SiteState,
        selector: String,
        site: u32,
        receiver: String,
    },
    /// Every inline cache was vacated at once -- every site, every
    /// polymorphic block and the global cache -- for `cause` (`vacated`);
    /// about no body. Each site's next miss moves it from `vacant` again.
    CachesVacated { cause: VacateCause },
}

impl TraceEventKind {
    /// The event's name, as its line spells it: kebab case.
    pub fn name(&self) -> &'static str {
        match self {
            TraceEventKind::Compiled { .. } => "compiled",
            TraceEventKind::CompiledTwin { .. } => "compiled-twin",
            TraceEventKind::CompileRefused { .. } => "refused",
            TraceEventKind::CompileHeld { .. } => "held",
            TraceEventKind::TwinHeld { .. } => "held-twin",
            TraceEventKind::Abandoned { .. } => "abandoned",
            TraceEventKind::Revived { .. } => "revived",
            TraceEventKind::RefusedForItsTwin => "refused-for-its-twin",
            TraceEventKind::AbandonedRefused { .. } => "refused-abandoned",
            TraceEventKind::OsrCompiled { .. } => "osr-compiled",
            TraceEventKind::OsrEntered { .. } => "osr-entered",
            TraceEventKind::OsrRefused { .. } => "osr-refused",
            TraceEventKind::OptimizationRequested { .. } => "requested",
            TraceEventKind::OptimizationNotDue { .. } => "not-due",
            TraceEventKind::OptimizationDropped { .. } => "dropped",
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
            TraceEventKind::CachesVacated { .. } => "vacated",
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

/// Why a compile that came due was held rather than tried.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HoldReason {
    /// The zone is exhausted and known to have no room for the body: the
    /// compile waits for a compaction, each hold counting towards the
    /// next one.
    ZoneFull,
}

impl HoldReason {
    /// The reason's name, as a line spells it: kebab case.
    pub fn name(self) -> &'static str {
        match self {
            HoldReason::ZoneFull => "zone-full",
        }
    }
}

/// Why an interpreted frame did not switch into its body's machine code
/// at a backward jump.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OsrRefusal {
    /// The native stack has no room for machine code's frames.
    NativeStackFull,
    /// The body has no machine code in this zone any more.
    NoMachineCode,
    /// The frame runs against another feedback base than the one the
    /// body's machine code was compiled against.
    OtherFeedbackBase,
    /// The frame is reified, so it enters the body's twin, which the
    /// zone has no room for now.
    TwinHeld,
    /// The machine code has no entry at the loop's head.
    NoEntry,
}

impl OsrRefusal {
    /// The refusal's name, as a line spells it: kebab case.
    pub fn name(self) -> &'static str {
        match self {
            OsrRefusal::NativeStackFull => "native-stack-full",
            OsrRefusal::NoMachineCode => "no-machine-code",
            OsrRefusal::OtherFeedbackBase => "other-feedback-base",
            OsrRefusal::TwinHeld => "twin-held",
            OsrRefusal::NoEntry => "no-entry",
        }
    }
}

/// What vacated every inline cache at once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VacateCause {
    /// An install rebuilt applications under unchanged class objects.
    Edit,
    /// A bulk forward (`elementsOf:forwardIdentityToElementsOf:ifFail:`),
    /// an edit's migration of instances among them.
    BulkForward,
    /// A breakpoint switched a method's fast path, which every entry
    /// decided for the method reads.
    Breakpoint,
    /// The JIT was switched on or off, deciding afresh what a cell may
    /// enter.
    JitSwitched,
    /// The code zone was replaced, and every cell named an address in it.
    ZoneReplaced,
    /// The host asked for it (`Interpreter::invalidate_caches`).
    Host,
    /// The cache stress mode's safepoint.
    Stress,
}

impl VacateCause {
    /// The cause's name, as a line spells it: kebab case.
    pub fn name(self) -> &'static str {
        match self {
            VacateCause::Edit => "edit",
            VacateCause::BulkForward => "bulk-forward",
            VacateCause::Breakpoint => "breakpoint",
            VacateCause::JitSwitched => "jit-switched",
            VacateCause::ZoneReplaced => "zone-replaced",
            VacateCause::Host => "host",
            VacateCause::Stress => "stress",
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

/// Which compile of a body machine code is: the tier whose code was
/// freed -- a twin's freed with its lean body, as `Baseline` -- and the
/// compile a listing shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodeTier {
    /// A body's baseline code, its lean body.
    Baseline,
    /// A body's twin, its baseline code that reifies its frame.
    Twin,
    /// An optimized copy.
    Optimized,
}

impl CodeTier {
    /// The tier's name, as a line spells it: kebab case.
    pub fn name(self) -> &'static str {
        match self {
            CodeTier::Baseline => "baseline",
            CodeTier::Twin => "twin",
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

    /// Receives `listing`, at the same flushes as the events, in its
    /// place among them. A sink that prints no code ignores it.
    fn listing(&mut self, listing: &Listing) {
        let _ = listing;
    }
}

/// An event recorded and not yet delivered: the bodies it names held as
/// code, named at the flush.
pub(crate) struct Pending {
    pub(crate) category: TraceCategory,
    pub(crate) body: Option<Rc<CompiledCode>>,
    pub(crate) kind: PendingKind,
    /// The names of the kind's customization key, given before the flush
    /// when a full collection reclaimed an id of the key
    /// (`name_freed_keys`): named at the flush, the id would be no
    /// application's, or another's minted it since.
    named_key: Option<NamedKey>,
}

/// A customization key's names, as `key_names` answers them.
struct NamedKey {
    key: String,
    entered: Option<String>,
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
    CompileHeld {
        trigger: CompileTrigger,
        reason: HoldReason,
    },
    TwinHeld {
        reason: HoldReason,
    },
    Abandoned {
        exits: u32,
    },
    Revived {
        revival: Revival,
    },
    RefusedForItsTwin,
    AbandonedRefused {
        reason: CompileRefusal,
    },
    OsrCompiled {
        loop_head: u32,
    },
    OsrEntered {
        loop_head: u32,
    },
    OsrRefused {
        loop_head: u32,
        reason: OsrRefusal,
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
    OptimizationDropped {
        key: Option<CustomizationKey>,
        cause: InvalidationCause,
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
        key: Option<CustomizationKey>,
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
    CachesVacated {
        cause: VacateCause,
    },
    /// Not an event: a listing, queued among the events so that it keeps
    /// its place after the event of the compile it shows.
    Listing(PendingListing),
}

impl Pending {
    /// A pending event of `category` about `code`.
    pub(crate) fn about(
        category: TraceCategory,
        code: &Rc<CompiledCode>,
        kind: PendingKind,
    ) -> Pending {
        Pending::about_body(category, Some(Rc::clone(code)), kind)
    }

    /// A pending event of `category` about `body`, `None` for one about
    /// no body.
    pub(crate) fn about_body(
        category: TraceCategory,
        body: Option<Rc<CompiledCode>>,
        kind: PendingKind,
    ) -> Pending {
        Pending {
            category,
            body,
            kind,
            named_key: None,
        }
    }
}

impl PendingKind {
    /// The customization key the event names, if any.
    fn key(&self) -> Option<CustomizationKey> {
        match self {
            PendingKind::OptimizationRequested { key, .. }
            | PendingKind::OptimizationNotDue { key, .. }
            | PendingKind::OptimizationDeferred { key, .. }
            | PendingKind::Optimized { key, .. }
            | PendingKind::OptimizationRefused { key, .. }
            | PendingKind::Inlined { key, .. }
            | PendingKind::InlineDeclined { key, .. }
            | PendingKind::Retired { key, .. }
            | PendingKind::Invalidated { key, .. }
            | PendingKind::OptimizationDropped { key, .. }
            | PendingKind::Freed { key, .. } => *key,
            PendingKind::Listing(listing) => listing.key(),
            PendingKind::Compiled { .. }
            | PendingKind::CompiledTwin { .. }
            | PendingKind::CompileRefused { .. }
            | PendingKind::CompileHeld { .. }
            | PendingKind::TwinHeld { .. }
            | PendingKind::Abandoned { .. }
            | PendingKind::Revived { .. }
            | PendingKind::RefusedForItsTwin
            | PendingKind::AbandonedRefused { .. }
            | PendingKind::OsrCompiled { .. }
            | PendingKind::OsrEntered { .. }
            | PendingKind::OsrRefused { .. }
            | PendingKind::Compacted { .. }
            | PendingKind::ZoneReplaced { .. }
            | PendingKind::CacheTransition { .. }
            | PendingKind::CachesVacated { .. } => None,
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
    /// The listings the sink asked for; empty without a sink.
    printing: Printing,
    /// The bodies whose listings the sink receives.
    print_filter: Option<TraceFilter>,
    sink: Option<Box<dyn TraceSink>>,
    /// The events recorded since the last flush, in order.
    pending: Vec<Pending>,
    /// How many events were recorded since the sink was installed: what a
    /// test reads to see that a site recorded nothing it was not asked
    /// for (`Interpreter::trace_events_recorded`).
    recorded: u64,
    names: BodyNames,
    /// Each body whose compile -- or its twin's, the flag set -- was held
    /// since it was last compiled, by its address, held weakly as the
    /// names are: so that a body the exhausted zone holds at every entry
    /// is traced held once, not at each.
    held: HashMap<(usize, bool), Weak<CompiledCode>>,
    /// Each body some frame of which was refused the switch into its
    /// machine code since it was last compiled, by its address, held
    /// weakly, with the loop heads and the reason traced at each: so that
    /// a frame that can never switch is traced refused once per reason,
    /// not once per wait of its back jumps.
    osr_refusals: HashMap<usize, (Weak<CompiledCode>, OsrRefusals)>,
    /// How many entries `held` and `osr_refusals` may hold before the
    /// dead ones are dropped: twice what lived at the last drop, so that
    /// the drops cost as much as the inserts, and no more.
    prune_at: usize,
}

/// The loop heads at which a body's frames were refused the switch, each
/// with the reason traced there.
type OsrRefusals = Vec<(u32, OsrRefusal)>;

/// The fewest entries the tracer's memories of holds and refusals hold
/// before they drop dead ones.
const FIRST_PRUNE: usize = 1024;

impl EventTracer {
    /// Whether an event of `category` is recorded: the one test a site
    /// makes before it builds anything.
    #[inline]
    pub(crate) fn traces(&self, category: TraceCategory) -> bool {
        self.categories.contains(category)
    }

    /// Whether machine code is printed: what makes the JIT annotate the
    /// code it assembles, and capture each body at its install.
    #[inline]
    pub(crate) fn prints_code(&self) -> bool {
        self.printing.code
    }

    /// Records the listing of `code`, delivered at the next flush after
    /// the events recorded before it. Not an event: no category selects
    /// it, and it is not counted among the events recorded.
    pub(crate) fn record_listing(&mut self, code: &Rc<CompiledCode>, listing: PendingListing) {
        debug_assert!(
            self.printing.code,
            "internal: a listing nothing asked for was recorded"
        );
        self.pending.push(Pending {
            // Unread: the print filter selects a listing, not a category.
            category: TraceCategory::Compile,
            body: Some(Rc::clone(code)),
            kind: PendingKind::Listing(listing),
            named_key: None,
        });
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

    /// Whether this is the first hold of `code`'s compile -- its twin's
    /// for `twin` -- since it was last compiled, remembering it if so.
    pub(crate) fn first_hold(&mut self, code: &Rc<CompiledCode>, twin: bool) -> bool {
        let address = Rc::as_ptr(code) as usize;
        if let Some(held) = self.held.get(&(address, twin))
            && held.upgrade().is_some_and(|held| Rc::ptr_eq(&held, code))
        {
            return false;
        }
        self.prune();
        self.held.insert((address, twin), Rc::downgrade(code));
        true
    }

    /// Whether this is the first refusal for `reason` of a switch into
    /// `code`'s machine code at the loop head `loop_head` since the body
    /// was last compiled or a frame switched there, remembering it if so.
    pub(crate) fn first_osr_refusal(
        &mut self,
        code: &Rc<CompiledCode>,
        loop_head: u32,
        reason: OsrRefusal,
    ) -> bool {
        let address = Rc::as_ptr(code) as usize;
        let own = self
            .osr_refusals
            .get(&address)
            .is_some_and(|(body, _)| body.upgrade().is_some_and(|body| Rc::ptr_eq(&body, code)));
        if !own {
            self.prune();
            self.osr_refusals
                .insert(address, (Rc::downgrade(code), Vec::new()));
        }
        let Some((_, refusals)) = self.osr_refusals.get_mut(&address) else {
            unreachable!("internal: the body's entry was just made");
        };
        match refusals.iter_mut().find(|(head, _)| *head == loop_head) {
            Some((_, known)) if *known == reason => false,
            Some((_, known)) => {
                *known = reason;
                true
            }
            None => {
                refusals.push((loop_head, reason));
                true
            }
        }
    }

    /// Forgets the refusal at `loop_head` of `code`, whose frame switched
    /// there, so that its next refusal there is traced.
    pub(crate) fn osr_entered(&mut self, code: &Rc<CompiledCode>, loop_head: u32) {
        if let Some((body, refusals)) = self.osr_refusals.get_mut(&(Rc::as_ptr(code) as usize))
            && body.upgrade().is_some_and(|body| Rc::ptr_eq(&body, code))
        {
            refusals.retain(|(head, _)| *head != loop_head);
        }
    }

    /// Forgets the hold of `code`'s compile -- its twin's for `twin` --
    /// and every refusal of a switch into its machine code, now that it
    /// was compiled, so that its next hold and refusals are traced.
    pub(crate) fn body_compiled(&mut self, code: &Rc<CompiledCode>, twin: bool) {
        let address = Rc::as_ptr(code) as usize;
        self.held.remove(&(address, twin));
        self.osr_refusals.remove(&address);
    }

    /// Drops the memories of dead bodies once there are twice as many as
    /// lived at the last drop.
    fn prune(&mut self) {
        if self.held.len() + self.osr_refusals.len() < self.prune_at.max(FIRST_PRUNE) {
            return;
        }
        self.held.retain(|_, held| held.strong_count() > 0);
        self.osr_refusals
            .retain(|_, (body, _)| body.strong_count() > 0);
        self.prune_at = 2 * (self.held.len() + self.osr_refusals.len());
    }

    /// How many events were recorded since the sink was installed.
    pub(crate) fn recorded(&self) -> u64 {
        self.recorded
    }

    /// How many events wait for a flush (`Interpreter::trace_queue_length`).
    pub(crate) fn queue_length(&self) -> usize {
        self.pending.len()
    }

    /// Whether a sink listens for any category or listing: the test a
    /// site that serves several categories' events makes first.
    #[inline]
    pub(crate) fn is_on(&self) -> bool {
        !self.categories.is_empty() || !self.printing.is_empty()
    }

    /// Whether an event waits for a flush: the test a compile trigger
    /// makes before it calls `flush`.
    #[inline]
    pub(crate) fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Whether a flush may deliver something: an event waits, or bytecode
    /// is printed, whose bodies a flush finds by looking for the mixins
    /// made since the last one. The test of every flush point but a
    /// compile trigger's -- the safepoint's and a host's -- so that a
    /// mixin made mid-run is listed at the next of them (decision 10).
    #[inline]
    pub(crate) fn flush_due(&self) -> bool {
        self.has_pending() || self.printing.bytecode
    }

    /// Installs `sink` for `settings`, replacing any sink and dropping
    /// what it had not delivered; with no category and nothing printed,
    /// no sink.
    pub(crate) fn install(&mut self, settings: TraceSettings, sink: Box<dyn TraceSink>) {
        *self = EventTracer::default();
        let TraceSettings {
            categories,
            filter,
            printing,
            print_filter,
        } = settings;
        if categories.is_empty() && printing.is_empty() {
            return;
        }
        self.categories = categories;
        self.filter = Some(filter);
        self.printing = printing;
        self.print_filter = Some(print_filter);
        self.sink = Some(sink);
    }
}

/// What a flush hands the sink, in order.
enum Delivery {
    Event(TraceEvent),
    Listing(Listing),
}

/// Delivers every pending event: names each body it holds, drops what the
/// filter refuses, hands the rest to the sink in order and empties the
/// queue. When bytecode is printed, the bodies made since the last flush
/// are listed first, so that a body's listing precedes its events.
#[cold]
#[inline(never)]
pub(crate) fn flush(mutator: &mut Mutator<'_>) {
    let pending = std::mem::take(&mut mutator.jit.tracer.pending);
    let mut names = std::mem::take(&mut mutator.jit.tracer.names);
    let mut deliveries = Vec::with_capacity(pending.len());
    if mutator.jit.tracer.printing.bytecode {
        // Indexed before any event is named: naming indexes the same
        // sides, which would then be listed by no flush.
        let made = names.index_new_sides(mutator);
        if let Some(filter) = &mutator.jit.tracer.print_filter {
            deliveries.extend(
                listings::bytecode_listings(made, filter)
                    .into_iter()
                    .map(Delivery::Listing),
            );
        }
    }
    for Pending {
        category,
        body,
        kind,
        named_key,
    } in pending
    {
        if let PendingKind::Listing(listing) = kind {
            if let Some(listing) = listing_delivered(mutator, &mut names, body, listing, named_key)
            {
                deliveries.push(Delivery::Listing(listing));
            }
            continue;
        }
        let body = body.map(|code| names.name(mutator, &code));
        let passes = match (&mutator.jit.tracer.filter, &body) {
            (Some(filter), Some(body)) => filter.matches(body),
            _ => true,
        };
        if passes {
            let kind = event_kind(mutator, &mut names, kind, named_key);
            deliveries.push(Delivery::Event(TraceEvent {
                category,
                body,
                kind,
            }));
        }
    }
    mutator.jit.tracer.names = names;
    if let Some(sink) = mutator.jit.tracer.sink.as_mut() {
        for delivery in &deliveries {
            match delivery {
                Delivery::Event(event) => sink.event(event),
                Delivery::Listing(listing) => sink.listing(listing),
            }
        }
    }
}

/// The listing `listing` of `body` delivers, its body and key named -- its
/// key by `named_key` where it was named before the flush -- or `None`
/// when the print filter refuses the body.
fn listing_delivered(
    mutator: &Mutator<'_>,
    names: &mut BodyNames,
    body: Option<Rc<CompiledCode>>,
    listing: PendingListing,
    named_key: Option<NamedKey>,
) -> Option<Listing> {
    let code = body?;
    let name = names.name(mutator, &code);
    let selected = mutator
        .jit
        .tracer
        .print_filter
        .as_ref()
        .is_some_and(|filter| filter.matches(&name));
    if !selected {
        return None;
    }
    let key = match (listing.key(), named_key) {
        (Some(_), Some(named)) => Some((named.key, named.entered)),
        (Some(key), None) => Some(key_names(mutator, key)),
        (None, _) => None,
    };
    Some(listing.delivered(&code, name, key))
}

/// The event a pending event's kind delivers, its bodies and keys named --
/// its key by `named_key` where it was named before the flush.
fn event_kind(
    mutator: &Mutator<'_>,
    names: &mut BodyNames,
    kind: PendingKind,
    named_key: Option<NamedKey>,
) -> TraceEventKind {
    let mut name = |code: &Rc<CompiledCode>| names.name(mutator, code);
    let mut named_key = named_key;
    let mut key_fields = |key: Option<CustomizationKey>| match (key, named_key.take()) {
        (Some(_), Some(named)) => (Some(named.key), named.entered),
        (Some(key), None) => {
            let (receiver, entered) = key_names(mutator, key);
            (Some(receiver), entered)
        }
        (None, _) => (None, None),
    };
    match kind {
        PendingKind::Compiled { trigger, bytes } => TraceEventKind::Compiled { trigger, bytes },
        PendingKind::CompiledTwin { bytes } => TraceEventKind::CompiledTwin { bytes },
        PendingKind::CompileRefused { trigger, reason } => {
            TraceEventKind::CompileRefused { trigger, reason }
        }
        PendingKind::CompileHeld { trigger, reason } => {
            TraceEventKind::CompileHeld { trigger, reason }
        }
        PendingKind::TwinHeld { reason } => TraceEventKind::TwinHeld { reason },
        PendingKind::Abandoned { exits } => TraceEventKind::Abandoned { exits },
        PendingKind::Revived { revival } => TraceEventKind::Revived { revival },
        PendingKind::RefusedForItsTwin => TraceEventKind::RefusedForItsTwin,
        PendingKind::AbandonedRefused { reason } => TraceEventKind::AbandonedRefused { reason },
        PendingKind::OsrCompiled { loop_head } => TraceEventKind::OsrCompiled { loop_head },
        PendingKind::OsrEntered { loop_head } => TraceEventKind::OsrEntered { loop_head },
        PendingKind::OsrRefused { loop_head, reason } => {
            TraceEventKind::OsrRefused { loop_head, reason }
        }
        PendingKind::OptimizationDropped { key, cause } => {
            let (key, entered) = key_fields(key);
            TraceEventKind::OptimizationDropped {
                key,
                entered,
                cause,
            }
        }
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
        PendingKind::Freed { key, tier, cause } => {
            let (key, entered) = key_fields(key);
            TraceEventKind::Freed {
                key,
                entered,
                tier,
                cause,
            }
        }
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
        PendingKind::CachesVacated { cause } => TraceEventKind::CachesVacated { cause },
        PendingKind::Listing(_) => {
            unreachable!("internal: a flush delivers a listing before it names an event")
        }
    }
}

/// The fields that name the customization key `key`: its receiver's class
/// and, only when the application it entered is another, that one's,
/// each qualified as every class is (`qualified_name`); an id no
/// application holds any longer is `#<id>`. Never joined by `@`, which
/// names a version.
pub(crate) fn key_names(mutator: &Mutator<'_>, key: CustomizationKey) -> (String, Option<String>) {
    let name = |id: DispatchId| class_name_of_id(mutator, id);
    let entered = (key.entered != key.receiver).then(|| name(key.entered));
    (name(key.receiver), entered)
}

/// The qualified name of the class whose application holds the dispatch
/// id `id` now; `#<id>` where none does.
fn class_name_of_id(mutator: &Mutator<'_>, id: DispatchId) -> String {
    mutator
        .heap
        .application_of_dispatch_id(id)
        .and_then(|application| mutator.heap.application(application))
        .and_then(|application| {
            crate::debugger::metadata::again::qualified_name(mutator, &application.mixin)
        })
        .unwrap_or_else(|| format!("#{}", id.bits()))
}

/// Names now the key of every pending event whose key holds an id a full
/// collection reclaims, `freed` naming each such id's class as it was
/// before the collection freed it (`Mutator::reclaim_ids`): named at the
/// flush, the id would be no application's -- `#<id>` -- or, minted again
/// meanwhile, another class's. Run in the collection, once the copies it
/// frees with the ids have recorded their events.
#[cold]
#[inline(never)]
pub(crate) fn name_freed_keys(mutator: &mut Mutator<'_>, freed: &IdentityMap<DispatchId, String>) {
    let mut pending = std::mem::take(&mut mutator.jit.tracer.pending);
    for event in &mut pending {
        if event.named_key.is_some() {
            continue;
        }
        let Some(key) = event.kind.key() else {
            continue;
        };
        if !freed.contains_key(&key.receiver) && !freed.contains_key(&key.entered) {
            continue;
        }
        let name = |id: DispatchId| {
            freed
                .get(&id)
                .cloned()
                .unwrap_or_else(|| class_name_of_id(mutator, id))
        };
        event.named_key = Some(NamedKey {
            key: name(key.receiver),
            entered: (key.entered != key.receiver).then(|| name(key.entered)),
        });
    }
    mutator.jit.tracer.pending = pending;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A body of its own for each test's events.
    fn body() -> Rc<CompiledCode> {
        Rc::new(crate::compiler::factory_body(3))
    }

    /// A frame that can never switch is refused once per loop head and
    /// reason, however many waits it tries again; a new reason, a switch
    /// there or a compile of the body lets the next refusal through.
    #[test]
    fn a_refused_switch_is_traced_once_per_reason() {
        let mut tracer = EventTracer::default();
        let code = body();
        assert!(tracer.first_osr_refusal(&code, 12, OsrRefusal::OtherFeedbackBase));
        for _ in 0..1000 {
            assert!(!tracer.first_osr_refusal(&code, 12, OsrRefusal::OtherFeedbackBase));
        }
        assert!(tracer.first_osr_refusal(&code, 30, OsrRefusal::OtherFeedbackBase));
        assert!(tracer.first_osr_refusal(&code, 12, OsrRefusal::TwinHeld));
        assert!(!tracer.first_osr_refusal(&code, 12, OsrRefusal::TwinHeld));
        tracer.osr_entered(&code, 12);
        assert!(tracer.first_osr_refusal(&code, 12, OsrRefusal::TwinHeld));
        assert!(!tracer.first_osr_refusal(&code, 30, OsrRefusal::OtherFeedbackBase));
        tracer.body_compiled(&code, false);
        assert!(tracer.first_osr_refusal(&code, 30, OsrRefusal::OtherFeedbackBase));
        // Another body's refusals are its own.
        let other = body();
        assert!(tracer.first_osr_refusal(&other, 30, OsrRefusal::OtherFeedbackBase));
    }

    /// The memories of dead bodies are dropped once they pass the
    /// threshold, not at every first hold, and a live body's memory stays.
    #[test]
    fn dead_bodies_are_pruned_past_the_threshold() {
        let mut tracer = EventTracer::default();
        let live = body();
        assert!(tracer.first_hold(&live, false));
        let mut dropped = Vec::new();
        for _ in 0..FIRST_PRUNE - 1 {
            let code = body();
            assert!(tracer.first_hold(&code, false));
            dropped.push(code);
        }
        drop(dropped);
        // Below the threshold, nothing was dropped yet.
        assert_eq!(tracer.held.len(), FIRST_PRUNE);
        let next = body();
        assert!(tracer.first_hold(&next, false));
        assert_eq!(tracer.held.len(), 2, "the dead dropped at the threshold");
        assert_eq!(tracer.prune_at, 2);
        assert!(!tracer.first_hold(&live, false), "a live body's hold stays");
    }
}
