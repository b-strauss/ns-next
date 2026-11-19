//! The inlining plan (`docs/design/execution.md`, "Inlining",
//! "The snapshot tree and the plan"): which sends of a compile are inlined, as
//! what, and where each inlined scope lies, decided on the snapshot tree
//! before the graph is built.
//!
//! The plan is made greedily: every send of every scope taken so far is a
//! candidate, the most frequent first -- its scope's frequency times
//! `LOOP_WEIGHT` for each loop around it, times the callee's spent share
//! of its optimization budget -- and a candidate is taken when its callee
//! is under its own limit and the compile under its own. What is observed
//! (`on:do:`, `ensure:`, `ifCurtailed:`, every body with an unwind marker
//! or reading an activation through a leaf) is never inlined; what is
//! always inlined (`AlwaysInlined`) costs nothing. An inlined scope lies
//! where the interpreter would push its activation: on its sender's
//! argument slots, so that its register `r` is the physical frame's
//! register `offset + r`.
//!
//! Blocks are decided last, over every scope the plan inlines: a block is
//! eliminated -- never created -- only if every use of the register that
//! holds it, through moves and into inlined parameters and copied values,
//! is a `value`-family send the plan inlines as that block's scope, a
//! move, its copy into a block eliminated too, or an argument of a send
//! every path of which inlines its callee; and only if the registers its
//! values were copied from stay unwritten while it lives, since an exit
//! that materializes it reads them there. A value send of a block that is
//! created after all is sent: its inlined scope is dropped, with every
//! scope below it, and the decision made again until nothing changes.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, HashMap, HashSet};

use crate::Symbol;
use crate::builtins::primitive::{
    BlockPrimitive, BytePrimitive, CollectionPrimitive, IntegerPrimitive, MIRROR, ObjectPrimitive,
    Primitive,
};
use crate::bytecode::{CodeKind, HEADER_REGISTERS};
use crate::heap::{ApplicationIndex, CodeId};
use crate::jit::float_operations::{FloatOperation, FloatReceiver};
use crate::mixin::{AccessorKind, DispatchId, MethodFastPath, QuickConstant};
use crate::trace_events::InlineDecline;
use crate::types::BuiltinClass;

use super::Speculation;
use super::build::{CompiledWay, compiled_ways};
use super::snapshot::{Snapshot, SnapshotInstruction};
use super::tree::{CalleeFacts, ReceiverFacts, SnapshotTree};

/// The weight of an instruction that is no send, closure or non-local
/// return (Strongtalk's `costModel.cpp`, the start the measurement of
/// the limits refines).
pub(crate) const COST_INSTRUCTION: u32 = 1;
/// The weight of a send, a mirror leaf's call included.
pub(crate) const COST_SEND: u32 = 5;
/// The weight of a closure's or an indirection vector's creation, and of
/// a non-local return.
pub(crate) const COST_CLOSURE: u32 = 3;
/// The weight of a send with a fast path inline: an `IntegerBinary`.
pub(crate) const COST_CHEAP_SEND: u32 = 2;
/// The heaviest method callee inlined: Strongtalk's 40. Halved and
/// doubled over the twenty-seven sources it moved the geometric mean
/// +0.8% and -0.6%, within the measurement's noise; 80 inlined more of
/// Json, 10% faster, and of DeltaBlue's, DeltaBlueTon80's and
/// RichardsTon80's callers, 4% to 8% slower (`docs/design/execution.md`,
/// "What inlining moved").
pub(crate) const METHOD_LIMIT: u32 = 40;
/// The heaviest block inlined at a `value`-family send: twice
/// Strongtalk's 70, which the measurement of the limits chose. Against
/// 70, three pinned builds each, two sittings, the geometric mean over
/// the twenty-seven sources moved -3.3% and -2.7%, against the sittings'
/// 0.5% between themselves: Queens, whose blocks lie between the two,
/// runs 61% faster in both. Towers measured 12% and 9% slower, its
/// builds apart in the first sitting, and Sieve 7% slower with its builds
/// apart in the second; their plans are the same at both limits, their
/// medians of eight short runs scatter by a third, and at 200 iterations
/// Towers runs level at both (`docs/design/execution.md`, "What inlining
/// moved").
pub(crate) const BLOCK_LIMIT: u32 = 140;
/// What a callee's limit grows by for each argument that is a block made
/// in the compile, which is then inlined into it.
pub(crate) const BLOCK_ARGUMENT_BONUS: u32 = 35;
/// The heaviest compile: the root and every scope inlined into it.
/// Strongtalk's 12,000 bounds the size of the machine code it emits;
/// this counts instructions of the bytecode's kind, each of which may
/// emit a template many machine instructions long, and every scope's
/// registers and values pass through liveness, allocation and, in a debug
/// build, the replay that verifies them. Halved, doubled and at
/// Strongtalk's 12,000 it moved the geometric mean over the twenty-seven
/// sources +0.2%, +0.7% and -0.3% alone, within the measurement's noise,
/// DeltaBlue 5% slower at 4,000 and 12,000 (`docs/design/execution.md`,
/// "What inlining moved").
pub(crate) const COMPILE_LIMIT: u32 = 2_000;
/// How many times a body may be inlined into a chain it already runs in.
pub(crate) const RECURSION_LIMIT: u32 = 1;
/// How much more often a send runs for each loop around it.
pub(crate) const LOOP_WEIGHT: u32 = 8;

/// The least share of its budget a callee's frequency counts, so that a
/// body whose budget says it barely ran still orders by its loops; and
/// the least a caller the recompilee walk goes to must have spent, so
/// that it never requests a body that barely ran (`jit::find_recompilee`).
pub(crate) const LEAST_BUDGET_SHARE: f64 = 1.0 / 64.0;

/// The limits a plan is made under: the constants above, and a test's own;
/// and whether blocks are inlined at all: a plan without them creates
/// every block and sends every `value` send of one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Limits {
    pub(crate) method: u32,
    pub(crate) block: u32,
    pub(crate) block_argument_bonus: u32,
    pub(crate) compile: u32,
    pub(crate) recursion: u32,
    pub(crate) inline_blocks: bool,
}

impl Limits {
    pub(crate) const STANDARD: Limits = Limits {
        method: METHOD_LIMIT,
        block: BLOCK_LIMIT,
        block_argument_bonus: BLOCK_ARGUMENT_BONUS,
        compile: COMPILE_LIMIT,
        recursion: RECURSION_LIMIT,
        inline_blocks: true,
    };

    /// `STANDARD` with blocks inlined and eliminated: what the plan's
    /// tests of blocks hold it to.
    #[cfg(test)]
    pub(crate) const WITH_BLOCKS: Limits = Limits::STANDARD;

    /// `STANDARD` with every block created and every `value` send of
    /// one sent.
    #[cfg(test)]
    pub(crate) const WITHOUT_BLOCKS: Limits = Limits {
        inline_blocks: false,
        ..Limits::STANDARD
    };
}

/// The weight of `snapshot`'s body: each instruction's cost by its kind.
/// A path that runs only when a fast path fails -- an `IntegerBinary`'s
/// fallback send, a conditional's not-Boolean way up to its send -- costs
/// an instruction each, so that an inlined conditional or an addition
/// weighs what it runs.
#[cfg(test)]
pub(crate) fn weight(snapshot: &Snapshot) -> u32 {
    weight_with(snapshot, |_| false)
}

/// `weight`, with a send whose site `cheap` says reached only slot
/// accessors and method fast paths weighing `COST_CHEAP_SEND`, the
/// accessor send of Strongtalk's cost model: `cheap` is asked the
/// site's number.
pub(crate) fn weight_with(snapshot: &Snapshot, cheap: impl Fn(u32) -> bool) -> u32 {
    let fallback = fallback_instructions(&snapshot.instructions);
    snapshot
        .instructions
        .iter()
        .zip(fallback)
        .zip(&snapshot.sites)
        .map(|((instruction, fallback), site)| {
            if fallback {
                COST_INSTRUCTION
            } else if is_call_site(instruction) && site.is_some_and(|facts| cheap(facts.site)) {
                COST_CHEAP_SEND
            } else {
                cost(instruction)
            }
        })
        .fold(0, u32::saturating_add)
}

/// The weight of `snapshot` with what `tree` recorded of its sites
/// (`weight_with`): a site whose every cached callee is a slot accessor
/// or a method fast path is cheap.
pub(crate) fn weight_in(tree: &SnapshotTree, snapshot: &Snapshot) -> u32 {
    weight_with(snapshot, |site| {
        tree.sites
            .get(&(snapshot.code_id, site))
            .is_some_and(|candidates| {
                !candidates.megamorphic
                    && !candidates.arms.is_empty()
                    && candidates.arms.iter().all(|(_, callee)| {
                        matches!(callee, CalleeFacts::Slot { .. } | CalleeFacts::FastPath(_))
                    })
            })
    })
}

/// What one instruction costs (`weight`).
fn cost(instruction: &SnapshotInstruction) -> u32 {
    match instruction {
        SnapshotInstruction::SendLexical { .. }
        | SnapshotInstruction::SendPublic { .. }
        | SnapshotInstruction::SendInlinable { .. }
        | SnapshotInstruction::SendSuper { .. }
        | SnapshotInstruction::SendMirror { .. }
        | SnapshotInstruction::InitializeSuperclass { .. }
        | SnapshotInstruction::InitializeAppliedLevel { .. }
        | SnapshotInstruction::InitializeInstance { .. }
        | SnapshotInstruction::ResolveFuture { .. } => COST_SEND,
        SnapshotInstruction::IntegerBinary { .. } => COST_CHEAP_SEND,
        SnapshotInstruction::MakeClosure { .. }
        | SnapshotInstruction::MakeVector { .. }
        | SnapshotInstruction::NonLocalReturn { .. } => COST_CLOSURE,
        _ => COST_INSTRUCTION,
    }
}

/// The site of the first instruction past `program_counter` of
/// `snapshot` that reads `register`, a block's, before anything writes it
/// again, if it is a send whose site the caches say neither ran nor
/// trapped.
pub(crate) fn never_run_consumer(
    snapshot: &Snapshot,
    program_counter: usize,
    register: u16,
) -> Option<u32> {
    for (index, instruction) in snapshot
        .instructions
        .iter()
        .enumerate()
        .skip(program_counter + 1)
    {
        if uses_of(instruction)
            .iter()
            .any(|(read, _)| *read == register)
        {
            if !is_call_site(instruction) {
                return None;
            }
            return snapshot
                .sites
                .get(index)
                .copied()
                .flatten()
                .filter(|facts| !facts.executed && !facts.trapped)
                .map(|facts| facts.site);
        }
        if instruction.register_written() == Some(register) {
            return None;
        }
    }
    None
}

/// For each instruction, whether it lies on a fast path's fallback: an
/// `IntegerBinary`'s send at its target, and a `JumpIfNotBoolean`'s way
/// from its target up to the first send, both before any jump.
fn fallback_instructions(instructions: &[SnapshotInstruction]) -> Vec<bool> {
    let mut fallback = vec![false; instructions.len()];
    for instruction in instructions {
        let (target, whole_way) = match instruction {
            SnapshotInstruction::IntegerBinary { target, .. } => (*target, false),
            SnapshotInstruction::JumpIfNotBoolean { target, .. } => (*target, true),
            _ => continue,
        };
        let mut index = target as usize;
        while let Some(next) = instructions.get(index) {
            let send = is_call_site(next);
            if send || whole_way {
                fallback[index] = true;
            }
            if send || next.jump_target().is_some() || !next.falls_through() {
                break;
            }
            index += 1;
        }
    }
    fallback
}

/// Whether `instruction` is a send through a cache: one with a call site.
fn is_call_site(instruction: &SnapshotInstruction) -> bool {
    matches!(
        instruction,
        SnapshotInstruction::SendLexical { .. }
            | SnapshotInstruction::SendPublic { .. }
            | SnapshotInstruction::SendInlinable { .. }
            | SnapshotInstruction::SendSuper { .. }
    )
}

/// A scope of a plan: an index into `InliningPlan::scopes`, the root 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct ScopeId(pub(crate) u32);

impl ScopeId {
    pub(crate) const ROOT: ScopeId = ScopeId(0);

    fn index(self) -> usize {
        self.0 as usize
    }

    fn of_index(index: usize) -> ScopeId {
        ScopeId(u32::try_from(index).expect("internal: a compile's scopes are numbered in 32 bits"))
    }
}

/// What one arm of a site's plan does for its receiver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ArmPlan {
    /// The callee's body, inlined as the scope `callee`.
    Inline { callee: ScopeId },
    /// What the callee does, expressed inline, whatever its weight.
    Always(AlwaysInlined),
}

/// A callee inlined whatever its weight: a slot accessor, a method fast
/// path, a lazy slot's getter and the leaves the graph expresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AlwaysInlined {
    /// A getter: the receiver's field `field`.
    SlotLoad { field: u32 },
    /// A setter: the argument stored into the receiver's field `field`
    /// through the barrier.
    SlotStore { field: u32 },
    /// `^true`, `^false` or `^nil`.
    Constant(QuickConstant),
    /// `^self`.
    Receiver,
    /// `^` the argument at this position.
    Parameter(u8),
    /// A builtin class's accessor.
    BuiltinClass(BuiltinClass),
    /// `^<parameter> value`: a closure argument at this position
    /// activated, anything else the method sent (`MethodFastPath`).
    ValueOfParameter(u8),
    /// `^<parameter> cull: self`, as `ValueOfParameter` (`MethodFastPath`).
    CullParameter(u8),
    /// A lazy slot's getter: its field `field` when it holds anything but
    /// `nil`, the getter sent otherwise.
    LazyGetter { field: u32 },
    /// A leaf the graph expresses: `SmallInteger` arithmetic and
    /// comparison, `==`, `class`, and `Array`'s and `ByteArray`'s `at:`
    /// and `at:put:` (`expressed`).
    Primitive(Primitive),
}

/// What a site's send becomes.
///
/// For a public send, a type-case: `arms` in the cache's order, one for
/// each cached receiver whose callee is inlined, each named by its guard
/// and its application, and the way out for every other receiver,
/// `fall_through`. For a `super` send, a private lexical send at depth 0
/// and a `value`-family send of a block the compile eliminates, whose
/// target the receiver's known application or the block itself fixes,
/// one arm taken without a test, `FallThrough::Unguarded`: the guard and
/// application it names are those the cache recorded, the class the
/// lookup started at for a `super` or lexical send, `Closure`'s for a
/// block. A site with no arm is its send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SitePlan {
    pub(crate) arms: Vec<(DispatchId, ApplicationIndex, ArmPlan)>,
    pub(crate) fall_through: FallThrough,
}

/// What a type-case does for a receiver none of its arms names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FallThrough {
    /// Traps: the site never trapped, it is not megamorphic and every
    /// receiver its cache recorded has an arm. A copy that speculates
    /// nothing (`Speculation::None`) sends instead.
    Trap,
    /// The site's own send through its cell.
    Send,
    /// No receiver falls through: the one arm is taken without a test.
    Unguarded,
}

/// One scope of a compile: the root, or a body inlined at a send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScopePlan {
    pub(crate) code_id: CodeId,
    /// The scope that sends it and the program counter of the send; `None`
    /// for the root.
    pub(crate) parent: Option<(ScopeId, u32)>,
    /// The physical register of its register 0: its sender's offset plus
    /// the send's argument base, where the interpreter pushes its
    /// activation.
    pub(crate) offset: u16,
    /// The receiver's application, where it is known: a customized root's,
    /// an arm's, and for a `super` or lexical send's callee and a block
    /// its sender's or home's.
    pub(crate) receiver: Option<ApplicationIndex>,
    /// The application the scope is entered at -- its current class's --
    /// where it is known.
    pub(crate) entered: Option<ApplicationIndex>,
    /// The own-slot base of `entered`, which fixes the absolute index of
    /// every own slot the body addresses.
    pub(crate) own_slot_base: Option<u32>,
    /// For a method inlined at a send, how many superclass steps its
    /// level lies from the chain's start: the receiver's class for a
    /// public send, the class a `super` or lexical send starts at; `None`
    /// for the root and a block, which run at their home's level.
    pub(crate) level: Option<u16>,
    /// A block scope's home: the scope whose `MakeClosure` made it.
    pub(crate) block_of: Option<ScopeId>,
    /// A block scope's block: only a block the compile eliminates is
    /// inlined at a `value` send.
    pub(crate) block: Option<EliminatedId>,
    /// Before each of the scope's instructions, the registers that hold an
    /// eliminated block on every way there, in the scope's numbering, and
    /// the block: what an exit that stops there materializes in them
    /// (`docs/design/execution.md`, "Closures, frames and non-local return
    /// in inlined scopes"). No register the leftover analysis leaves to the
    /// frame holds an eliminated block on one way and anything else on
    /// another: such a block is created (`Planner::escapes`), so that where
    /// a block is is a fact of the instruction, not of the way taken.
    pub(crate) eliminated_before: Vec<Vec<(u16, EliminatedId)>>,
}

/// The plan of one compile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InliningPlan {
    /// Every scope, `ScopeId::ROOT` the root, each after its sender.
    pub(crate) scopes: Vec<ScopePlan>,
    /// The plan of every send that ran, keyed by its scope and its
    /// program counter. A send that never ran has none and is compiled as
    /// without inlining, a guard that traps where the copy speculates.
    pub(crate) sites: HashMap<(ScopeId, u32), SitePlan>,
    /// Every `MakeClosure` of every scope, by its scope and program
    /// counter: eliminated, or created.
    pub(crate) blocks: HashMap<(ScopeId, u32), BlockPlan>,
    /// The most scopes inlined into one another below the root.
    pub(crate) deepest_chain: u16,
    /// One past the highest physical register any scope's frame reaches.
    pub(crate) deepest_extent: u32,
    /// Every callee a live scope's send that ran could have inlined and
    /// the plan sent instead, with why, in the order the plan decided
    /// them: what the trace's `inlining` category reports.
    pub(crate) declined: Vec<Declined>,
}

/// A callee the plan sent rather than inline: at the send at
/// `program_counter` of the scope running `scope_code`, the body
/// `callee`, `None` for one that is no body, declined for `reason`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Declined {
    pub(crate) scope_code: CodeId,
    pub(crate) program_counter: u32,
    pub(crate) callee: Option<CodeId>,
    pub(crate) reason: InlineDecline,
}

/// An eliminated block of one compile, numbered in the order of its
/// scope and program counter; `BlockId` is the graph's basic block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct EliminatedId(pub(crate) u32);

/// What a `MakeClosure` becomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BlockPlan {
    /// Never created in the compile, materialized at an exit that finds
    /// it live.
    Eliminated(EliminatedId),
    /// Created as the interpreter creates it.
    Created,
}

/// The plan of `tree` under the standard limits, for a copy that
/// speculates as `speculation` says.
pub(crate) fn plan(tree: &SnapshotTree, speculation: Speculation) -> InliningPlan {
    plan_with(tree, &Limits::STANDARD, speculation)
}

/// The plan of `tree` under `limits` (the module's comment). A copy that
/// speculates nothing (`Speculation::None`) has no type-case that traps:
/// every fall-through is its send, so that no block is eliminated on the
/// strength of a trap the copy never takes.
pub(crate) fn plan_with(
    tree: &SnapshotTree,
    limits: &Limits,
    speculation: Speculation,
) -> InliningPlan {
    let mut planner = Planner::new(tree, *limits, speculation);
    while let Some(candidate) = planner.queue.pop() {
        planner.take(&candidate);
    }
    let eliminated = planner.decide_blocks();
    planner.finish(&eliminated)
}

/// A block of the compile: its home scope and its `MakeClosure`'s
/// program counter.
type BlockKey = (ScopeId, u32);

/// The blocks each register may hold before an instruction, and
/// `SOMETHING_ELSE` among them where it may hold anything else on some
/// way; a register absent holds no block of the compile on any way, only
/// something else. Escape is a question of what a register may hold; a
/// `value` send is taken without a test only where its receiver must be
/// one block, a set of that block alone.
type Holdings = BTreeMap<u16, BTreeSet<BlockKey>>;

/// What a register may hold besides the compile's blocks: `nil`, a
/// number, a closure made outside the compile, anything a send answered.
const SOMETHING_ELSE: BlockKey = (ScopeId(u32::MAX), u32::MAX);

/// A scope as the planner keeps it.
struct Scope<'tree> {
    plan: ScopePlan,
    snapshot: &'tree Snapshot,
    /// The scope's estimated frequency, the root's 1.
    frequency: f64,
    /// The chain of `plan.entered`, its own application first.
    entered_chain: Option<Vec<ApplicationIndex>>,
    /// How many scopes lie above it.
    depth: u16,
    /// For a block scope, the block.
    block: Option<BlockKey>,
    /// The blocks each register may hold before each instruction.
    holdings: Vec<Holdings>,
    /// Each instruction's number of enclosing loops.
    loop_depths: Vec<u32>,
    /// What the graph makes of each instruction: a way the feedback says
    /// never ran ends in a trap, and nothing past it is built.
    ways: Vec<CompiledWay>,
    alive: bool,
    /// Its weight, which the compile's total gives back when it is
    /// dropped.
    weight: u32,
}

/// What the plan decided at a send so far.
#[derive(Debug, Clone)]
enum SiteDecision {
    /// A public send's type-case: an arm per cached receiver, `None` for
    /// one not inlined; `complete` when the site is not megamorphic and
    /// its cache was read.
    TypeCase {
        arms: Vec<Option<(DispatchId, ApplicationIndex, ArmPlan)>>,
        complete: bool,
    },
    /// A send taken without a test, or sent.
    Unguarded(Option<(DispatchId, ApplicationIndex, ArmPlan)>),
}

/// A send whose callee the plan may inline.
struct Candidate {
    frequency: f64,
    order: u64,
    scope: ScopeId,
    program_counter: u32,
    kind: CandidateKind,
}

#[derive(Clone, Copy)]
enum CandidateKind {
    /// The cached arm at this index of a public send.
    Arm(usize),
    /// The cached arm at this index of a `super` or lexical send, the one
    /// the scope's known receiver fixes.
    Unguarded(usize),
    /// The block, at a `value`-family send of it, with the guard and
    /// application the site's cache recorded for `Closure`.
    Block(BlockKey, DispatchId, ApplicationIndex),
}

impl PartialEq for Candidate {
    fn eq(&self, other: &Candidate) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Candidate {}

impl PartialOrd for Candidate {
    fn partial_cmp(&self, other: &Candidate) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Candidate {
    /// The more frequent first, and of two as frequent the one found
    /// first, so that the plan is deterministic.
    fn cmp(&self, other: &Candidate) -> Ordering {
        self.frequency
            .total_cmp(&other.frequency)
            .then_with(|| other.order.cmp(&self.order))
    }
}

/// The shape of a send: what its receiver is.
#[derive(Clone, Copy)]
enum SendShape {
    Public { receiver: u16 },
    Lexical { depth: u16 },
    Super,
}

/// A send's parts the plan reads.
struct Send<'instruction> {
    shape: SendShape,
    selector: Symbol,
    argument_base: u16,
    sources: &'instruction [u16],
}

fn send_of(instruction: &SnapshotInstruction) -> Option<Send<'_>> {
    let (shape, selector, argument_base, sources) = match instruction {
        SnapshotInstruction::SendPublic {
            selector,
            receiver_register,
            argument_base,
            argument_sources,
            ..
        }
        | SnapshotInstruction::SendInlinable {
            selector,
            receiver_register,
            argument_base,
            argument_sources,
            ..
        } => (
            SendShape::Public {
                receiver: *receiver_register,
            },
            *selector,
            *argument_base,
            argument_sources,
        ),
        SnapshotInstruction::SendLexical {
            depth,
            selector,
            argument_base,
            argument_sources,
            ..
        } => (
            SendShape::Lexical { depth: *depth },
            *selector,
            *argument_base,
            argument_sources,
        ),
        SnapshotInstruction::SendSuper {
            selector,
            argument_base,
            argument_sources,
            ..
        } => (
            SendShape::Super,
            *selector,
            *argument_base,
            argument_sources,
        ),
        _ => return None,
    };
    Some(Send {
        shape,
        selector,
        argument_base,
        sources: sources.as_slice(),
    })
}

/// How an instruction reads a register.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Use {
    /// Copied into another register: a move or a local's store.
    Move,
    /// Copied into the block the instruction makes.
    Copied,
    /// A send's receiver.
    Receiver,
    /// A send's argument at this position.
    Argument(usize),
    /// Anything else.
    Other,
}

/// Every register `instruction` reads, with how.
fn uses_of(instruction: &SnapshotInstruction) -> Vec<(u16, Use)> {
    match instruction {
        SnapshotInstruction::Move { source, .. }
        | SnapshotInstruction::StoreLocal { source, .. } => {
            vec![(*source, Use::Move)]
        }
        SnapshotInstruction::MakeClosure { copied, .. } => copied
            .iter()
            .map(|register| (*register, Use::Copied))
            .collect(),
        _ => match send_of(instruction) {
            Some(send) => {
                let mut uses: Vec<(u16, Use)> = send
                    .sources
                    .iter()
                    .enumerate()
                    .map(|(position, register)| (*register, Use::Argument(position)))
                    .collect();
                if let SendShape::Public { receiver } = send.shape {
                    uses.push((receiver, Use::Receiver));
                }
                uses
            }
            None => instruction
                .registers_read()
                .into_iter()
                .map(|register| (register, Use::Other))
                .collect(),
        },
    }
}

/// The instructions control may pass to after the one at
/// `program_counter`.
fn successors(instructions: &[SnapshotInstruction], program_counter: usize) -> Vec<usize> {
    let instruction = &instructions[program_counter];
    let mut next = Vec::with_capacity(2);
    if instruction.falls_through() && program_counter + 1 < instructions.len() {
        next.push(program_counter + 1);
    }
    if let Some(target) = instruction.jump_target() {
        let target = target as usize;
        if target < instructions.len() && !next.contains(&target) {
            next.push(target);
        }
    }
    next
}

/// For each instruction, how many loops enclose it: the backward jumps
/// whose range, from their target to themselves, holds it.
fn loop_depths(instructions: &[SnapshotInstruction]) -> Vec<u32> {
    let mut depths = vec![0_u32; instructions.len()];
    for (program_counter, instruction) in instructions.iter().enumerate() {
        if let Some(target) = instruction.jump_target() {
            let target = target as usize;
            if target <= program_counter {
                for depth in &mut depths[target..=program_counter] {
                    *depth += 1;
                }
            }
        }
    }
    depths
}

/// The blocks each register of `scope`'s body may hold before each
/// instruction, from `entry`, what the registers hold on entry: a
/// forward analysis to its fixpoint, the blocks at a merge the union of
/// its ways'.
fn holdings_of(snapshot: &Snapshot, scope: ScopeId, entry: Holdings) -> Vec<Holdings> {
    let instructions = &snapshot.instructions;
    let mut states: Vec<Option<Holdings>> = vec![None; instructions.len()];
    if instructions.is_empty() {
        return Vec::new();
    }
    states[0] = Some(entry);
    let mut work = vec![0_usize];
    while let Some(program_counter) = work.pop() {
        let Some(before) = states[program_counter].clone() else {
            continue;
        };
        let after = transfer(snapshot, scope, program_counter, before);
        for successor in successors(instructions, program_counter) {
            let changed = match &mut states[successor] {
                Some(state) => merge(state, &after),
                slot @ None => {
                    *slot = Some(after.clone());
                    true
                }
            };
            if changed {
                work.push(successor);
            }
        }
    }
    states.into_iter().map(Option::unwrap_or_default).collect()
}

/// `into` with `from`'s holdings added; whether anything was. A register
/// absent on one way holds something else there, so that a register that
/// holds a block on one way and nothing of the compile's on the other
/// may hold something else.
fn merge(into: &mut Holdings, from: &Holdings) -> bool {
    let mut changed = false;
    for (register, held) in into.iter_mut() {
        if !from.contains_key(register) {
            changed |= held.insert(SOMETHING_ELSE);
        }
    }
    for (register, blocks) in from {
        let held = into.entry(*register).or_insert_with(|| {
            changed = true;
            BTreeSet::from([SOMETHING_ELSE])
        });
        for block in blocks {
            changed |= held.insert(*block);
        }
    }
    changed
}

/// What the registers hold after the instruction at `program_counter`.
fn transfer(
    snapshot: &Snapshot,
    scope: ScopeId,
    program_counter: usize,
    mut state: Holdings,
) -> Holdings {
    let instruction = &snapshot.instructions[program_counter];
    let set = |state: &mut Holdings, register: u16, blocks: Option<BTreeSet<BlockKey>>| match blocks
        .filter(|blocks| !blocks.is_empty())
    {
        Some(blocks) => {
            state.insert(register, blocks);
        }
        None => {
            state.remove(&register);
        }
    };
    match instruction {
        SnapshotInstruction::MakeClosure { destination, .. } => {
            let key = (scope, program_counter_u32(program_counter));
            set(&mut state, *destination, Some(BTreeSet::from([key])));
        }
        SnapshotInstruction::Move {
            destination,
            source,
        } => {
            let blocks = state.get(source).cloned();
            set(&mut state, *destination, blocks);
        }
        SnapshotInstruction::StoreLocal { source, slot } => {
            let blocks = state.get(source).cloned();
            set(&mut state, *slot, blocks);
        }
        _ => {
            // A callee's frame lies from its argument slots up, or above
            // the window: nothing there outlives the call.
            let frame_start = match instruction {
                SnapshotInstruction::InitializeInstance { .. }
                | SnapshotInstruction::ResolveFuture { .. } => Some(snapshot.facts.window_end()),
                _ => instruction.argument_slots().map(|(base, _)| base),
            };
            if let Some(start) = frame_start {
                state.retain(|register, _| *register < start);
            }
            if let Some(written) = instruction.register_written() {
                state.remove(&written);
            }
        }
    }
    state
}

/// Whether the leftover analysis taints `register` of `snapshot`'s body
/// before its instruction at `program_counter`: a register no instruction
/// reads before it writes it, whose word an exit leaves to the collector's
/// pre-pass. A snapshot a test made by hand may name no set for it.
fn tainted_at(snapshot: &Snapshot, program_counter: usize, register: u16) -> bool {
    snapshot
        .tainted_before
        .get(program_counter)
        .is_some_and(|ranges| ranges.iter().any(|range| range.contains(&register)))
}

fn program_counter_u32(program_counter: usize) -> u32 {
    u32::try_from(program_counter)
        .expect("internal: a body holds at most INSTRUCTION_LIMIT instructions")
}

/// Whether `snapshot`'s body is observed (`docs/design/execution.md`,
/// "Observers"): it carries an unwind marker -- `on:do:`, `ensure:`,
/// `ifCurtailed:` -- or reads or marks an activation through a leaf, as
/// `signal` and the handler search do; the exception machinery stops at
/// such an activation, so it stays a frame of its own.
fn is_observed(snapshot: &Snapshot) -> bool {
    snapshot.facts.unwind_marker
        || snapshot.instructions.iter().any(|instruction| {
            matches!(
                instruction,
                SnapshotInstruction::SendMirror { primitive, .. }
                    if matches!(
                        MIRROR.get(primitive.index()).map(|row| row.primitive),
                        Some(Primitive::Activation(_))
                    )
            )
        })
}

/// Whether `snapshot`'s method body leaves its activation other than by
/// a `Return`: a lazy slot's getter past its first instruction or a
/// non-local return, which no lowering puts in a method body but an
/// image could. An inlined scope's return is a jump to its sender's next
/// instruction, which neither is.
fn returns_otherwise(snapshot: &Snapshot) -> bool {
    snapshot.instructions.iter().any(|instruction| {
        matches!(
            instruction,
            SnapshotInstruction::ReturnSlotIfFilled { .. }
                | SnapshotInstruction::NonLocalReturn { .. }
        )
    })
}

/// Whether the graph expresses the leaf `primitive` inline for a
/// receiver of `receiver`: a `SmallInteger` fast path, `==`, `class`, an
/// indexed access, or a row machine code computes a float operation of
/// (`FloatOperation::of_primitive`) -- a `SmallInteger` receiver's mixed one only
/// for a `SmallInteger`, since `Number`'s `/` is every number's whose
/// class does not override it, whose operation would always decline.
fn expressed(primitive: Primitive, receiver: &ReceiverFacts) -> bool {
    let integer_or_other = matches!(
        primitive,
        Primitive::Integer(
            IntegerPrimitive::Add
                | IntegerPrimitive::Subtract
                | IntegerPrimitive::Multiply
                | IntegerPrimitive::FloorDivide
                | IntegerPrimitive::Less
                | IntegerPrimitive::Greater
                | IntegerPrimitive::LessOrEqual
                | IntegerPrimitive::GreaterOrEqual
                | IntegerPrimitive::Equal
        ) | Primitive::Object(ObjectPrimitive::Identical | ObjectPrimitive::Class)
            | Primitive::Collection(
                CollectionPrimitive::ElementAt | CollectionPrimitive::ElementAtPut
            )
            | Primitive::Bytes(BytePrimitive::ElementAt | BytePrimitive::ElementAtPut)
    );
    integer_or_other
        || match FloatOperation::of_primitive(primitive) {
            Some(
                FloatOperation::Arithmetic {
                    receiver: FloatReceiver::SmallInteger,
                    ..
                }
                | FloatOperation::Compare {
                    receiver: FloatReceiver::SmallInteger,
                    ..
                }
                | FloatOperation::IntegerAsFloat,
            ) => receiver.small_integer,
            Some(_) => true,
            None => false,
        }
}

/// Whether `callee` is a closure's `value`-family leaf.
fn is_block_value(callee: &CalleeFacts) -> bool {
    let primitive = match callee {
        CalleeFacts::FastPath(MethodFastPath::Mirror(row)) => {
            MIRROR.get(row.index()).map(|row| row.primitive)
        }
        CalleeFacts::Primitive(primitive) => Some(*primitive),
        _ => None,
    };
    primitive == Some(Primitive::Block(BlockPrimitive::Value))
}

/// The number of arguments a `value`-family selector takes: `value`,
/// `value:`, `value:value:` and `value:value:value:`.
fn value_arity(selector: Symbol) -> Option<u16> {
    match selector.as_str() {
        "value" => Some(0),
        "value:" => Some(1),
        "value:value:" => Some(2),
        "value:value:value:" => Some(3),
        _ => None,
    }
}

/// What a callee is to the plan: never inlined, for its reason.
enum Callee<'tree> {
    Always(AlwaysInlined),
    Method {
        snapshot: &'tree Snapshot,
        level: u16,
    },
    Not(InlineDecline),
}

/// The body `callee` runs, when it is a method's the tree snapshotted.
fn callee_code(callee: &CalleeFacts) -> Option<CodeId> {
    match callee {
        CalleeFacts::Method { snapshot, .. } => Some(snapshot.code_id),
        _ => None,
    }
}

/// `callee`, reached for a receiver of `receiver`.
fn classify<'tree>(receiver: &ReceiverFacts, callee: &'tree CalleeFacts) -> Callee<'tree> {
    match callee {
        CalleeFacts::Method { snapshot, level } => {
            if let Some(SnapshotInstruction::ReturnSlotIfFilled { own_index }) =
                snapshot.instructions.first()
            {
                return receiver
                    .own_slot_bases
                    .get(usize::from(*level))
                    .and_then(|base| base.checked_add(*own_index))
                    .map_or(Callee::Not(InlineDecline::NotAMethod), |field| {
                        Callee::Always(AlwaysInlined::LazyGetter { field })
                    });
            }
            if is_observed(snapshot) {
                Callee::Not(InlineDecline::Observed)
            } else if returns_otherwise(snapshot) {
                Callee::Not(InlineDecline::NotAMethod)
            } else {
                Callee::Method {
                    snapshot,
                    level: *level,
                }
            }
        }
        CalleeFacts::Slot { field, kind, .. } => Callee::Always(match kind {
            AccessorKind::Getter => AlwaysInlined::SlotLoad { field: *field },
            AccessorKind::Setter => AlwaysInlined::SlotStore { field: *field },
        }),
        CalleeFacts::FastPath(fast_path) => match fast_path {
            MethodFastPath::Constant(constant) => {
                Callee::Always(AlwaysInlined::Constant(*constant))
            }
            MethodFastPath::Receiver => Callee::Always(AlwaysInlined::Receiver),
            MethodFastPath::Parameter(position) => {
                Callee::Always(AlwaysInlined::Parameter(*position))
            }
            MethodFastPath::BuiltinClass(class) => {
                Callee::Always(AlwaysInlined::BuiltinClass(*class))
            }
            MethodFastPath::ValueOfParameter(position) => {
                Callee::Always(AlwaysInlined::ValueOfParameter(*position))
            }
            MethodFastPath::CullParameter(position) => {
                Callee::Always(AlwaysInlined::CullParameter(*position))
            }
            MethodFastPath::Mirror(row) => match MIRROR.get(row.index()) {
                Some(row) if expressed(row.primitive, receiver) => {
                    Callee::Always(AlwaysInlined::Primitive(row.primitive))
                }
                _ => Callee::Not(InlineDecline::NotAMethod),
            },
        },
        CalleeFacts::Primitive(primitive) if expressed(*primitive, receiver) => {
            Callee::Always(AlwaysInlined::Primitive(*primitive))
        }
        CalleeFacts::Primitive(_) | CalleeFacts::Other => Callee::Not(InlineDecline::NotAMethod),
    }
}

struct Planner<'tree> {
    tree: &'tree SnapshotTree,
    limits: Limits,
    /// Whether the copy speculates: whether a type-case may trap.
    speculates: bool,
    speculation: Speculation,
    scopes: Vec<Scope<'tree>>,
    total: u32,
    queue: BinaryHeap<Candidate>,
    found: u64,
    decisions: HashMap<(ScopeId, u32), SiteDecision>,
    /// Every callee declined, by the scope whose send declined it: only
    /// those of scopes still live at the end are the plan's.
    declined: Vec<(ScopeId, Declined)>,
}

impl<'tree> Planner<'tree> {
    fn new(tree: &'tree SnapshotTree, limits: Limits, speculation: Speculation) -> Planner<'tree> {
        let root = &tree.root;
        let mut planner = Planner {
            tree,
            limits,
            speculates: speculation == Speculation::Traps,
            speculation,
            scopes: Vec::new(),
            total: weight_in(tree, root),
            queue: BinaryHeap::new(),
            found: 0,
            decisions: HashMap::new(),
            declined: Vec::new(),
        };
        planner.add_scope(
            ScopePlan {
                code_id: root.code_id,
                parent: None,
                offset: 0,
                receiver: tree.root_receiver.as_ref().map(|facts| facts.application),
                entered: tree.root_entered.as_ref().map(|facts| facts.application),
                own_slot_base: root
                    .customization
                    .map(|customization| customization.own_slot_base),
                level: None,
                block_of: None,
                block: None,
                eliminated_before: Vec::new(),
            },
            root,
            1.0,
            tree.root_entered
                .as_ref()
                .map(|facts| facts.applications.clone()),
            None,
            Holdings::new(),
        );
        planner
    }

    /// The share of its budget the body `code_id` spent, at least
    /// `LEAST_BUDGET_SHARE`; a parked row ran its whole budget.
    fn budget_share(&self, code_id: CodeId) -> f64 {
        self.tree
            .budgets
            .get(&code_id)
            .map_or(LEAST_BUDGET_SHARE, |share| share.max(LEAST_BUDGET_SHARE))
    }

    /// Adds the scope `plan` and queues its sends as candidates.
    fn add_scope(
        &mut self,
        plan: ScopePlan,
        snapshot: &'tree Snapshot,
        frequency: f64,
        entered_chain: Option<Vec<ApplicationIndex>>,
        block: Option<BlockKey>,
        entry: Holdings,
    ) -> ScopeId {
        let id = ScopeId::of_index(self.scopes.len());
        let depth = plan
            .parent
            .map_or(0, |(parent, _)| self.scopes[parent.index()].depth + 1);
        let holdings = holdings_of(snapshot, id, entry);
        self.scopes.push(Scope {
            plan,
            snapshot,
            frequency,
            entered_chain,
            depth,
            block,
            holdings,
            loop_depths: loop_depths(&snapshot.instructions),
            ways: compiled_ways(snapshot, id != ScopeId::ROOT, self.speculation),
            alive: true,
            weight: weight_in(self.tree, snapshot),
        });
        self.gather(id);
        id
    }

    /// Records that `scope`'s send at `program_counter` sends `callee`
    /// rather than inline it, for `reason`.
    fn decline(
        &mut self,
        scope: ScopeId,
        program_counter: u32,
        callee: Option<CodeId>,
        reason: InlineDecline,
    ) {
        let scope_code = self.scopes[scope.index()].plan.code_id;
        self.declined.push((
            scope,
            Declined {
                scope_code,
                program_counter,
                callee,
                reason,
            },
        ));
    }

    fn push(&mut self, scope: ScopeId, program_counter: u32, callee: CodeId, kind: CandidateKind) {
        let owner = &self.scopes[scope.index()];
        let loops = owner.loop_depths[program_counter as usize];
        let frequency = owner.frequency
            * f64::from(LOOP_WEIGHT).powi(i32::try_from(loops).unwrap_or(i32::MAX))
            * self.budget_share(callee);
        let order = self.found;
        self.found += 1;
        self.queue.push(Candidate {
            frequency,
            order,
            scope,
            program_counter,
            kind,
        });
    }

    /// Decides what it can of each send of `scope` that ran -- an arm
    /// always inlined, a callee never inlined -- and queues the rest.
    fn gather(&mut self, scope: ScopeId) {
        let tree = self.tree;
        let snapshot = self.scopes[scope.index()].snapshot;
        for (index, instruction) in snapshot.instructions.iter().enumerate() {
            let Some(send) = send_of(instruction) else {
                continue;
            };
            let Some(facts) = snapshot.sites.get(index).copied().flatten() else {
                continue;
            };
            if !facts.executed {
                continue;
            }
            let program_counter = program_counter_u32(index);
            let candidates = tree.sites.get(&(snapshot.code_id, facts.site));
            let decision = match send.shape {
                SendShape::Public { receiver } => {
                    // The reference lowering's control flow is a
                    // `SendInlinable`, whose block's activation the
                    // interpreter does not count against the depth limit
                    // while every inlined scope is counted: its block is
                    // created and sent, as the oracle it is.
                    let inlinable =
                        matches!(instruction, SnapshotInstruction::SendInlinable { .. });
                    let block_value = if self.limits.inline_blocks && !inlinable {
                        self.block_value(scope, index, &send, receiver, candidates)
                    } else {
                        None
                    };
                    if let Some(decision) = block_value {
                        decision
                    } else {
                        self.type_case(scope, program_counter, candidates)
                    }
                }
                SendShape::Lexical { depth } => {
                    let private = candidates.is_some_and(|candidates| candidates.private);
                    let known = self.scopes[scope.index()].plan.receiver.is_some();
                    if private {
                        // The guard is N's: fixed by the scope's entered
                        // application only for N itself, at depth 0.
                        let fixed = depth == 0 && !snapshot.facts.declaration_code;
                        self.unguarded(scope, program_counter, candidates, |owner, arm| {
                            fixed && owner.plan.entered == Some(arm.application)
                        })
                    } else if depth == 0 && known {
                        // The guard is the receiver's, `self`'s, known.
                        self.unguarded(scope, program_counter, candidates, |owner, arm| {
                            owner.plan.receiver == Some(arm.application)
                        })
                    } else {
                        // The guard is the receiver's: a type-case on the
                        // enclosing object at the send's depth.
                        self.type_case(scope, program_counter, candidates)
                    }
                }
                SendShape::Super => {
                    self.unguarded(scope, program_counter, candidates, |owner, arm| {
                        owner.entered_chain.as_ref().and_then(|chain| chain.get(1))
                            == Some(&arm.application)
                    })
                }
            };
            self.decisions.insert((scope, program_counter), decision);
        }
    }

    /// A `value`-family send whose receiver can only be one block of the
    /// compile: the block a candidate, the send taken without a test.
    fn block_value(
        &mut self,
        scope: ScopeId,
        index: usize,
        send: &Send<'_>,
        receiver: u16,
        candidates: Option<&super::tree::SiteCandidates>,
    ) -> Option<SiteDecision> {
        let arity = value_arity(send.selector)?;
        let owner = &self.scopes[scope.index()];
        let held = owner.holdings.get(index)?.get(&receiver)?;
        // A register that holds nothing of the compile's on any way is
        // absent; one that may hold something else besides is no known
        // receiver, and neither is something else alone.
        let [block] = held.iter().copied().collect::<Vec<_>>()[..] else {
            return None;
        };
        if block == SOMETHING_ELSE {
            return None;
        }
        let (block_code, parameter_count) = self.made_block(block)?;
        let snapshot = self.tree.blocks.get(&block_code)?;
        if parameter_count != arity || usize::from(arity) != send.sources.len() {
            return None;
        }
        let (facts, _) = candidates?
            .arms
            .iter()
            .find(|(_, callee)| is_block_value(callee))?;
        self.push(
            scope,
            program_counter_u32(index),
            snapshot.code_id,
            CandidateKind::Block(block, facts.dispatch_id, facts.application),
        );
        Some(SiteDecision::Unguarded(None))
    }

    /// The code id and parameter count of the block `block` makes.
    fn made_block(&self, block: BlockKey) -> Option<(CodeId, u16)> {
        let (home, program_counter) = block;
        match self.scopes[home.index()]
            .snapshot
            .instructions
            .get(program_counter as usize)?
        {
            SnapshotInstruction::MakeClosure {
                block,
                parameter_count,
                ..
            } => Some((*block, *parameter_count)),
            _ => None,
        }
    }

    /// A public send's type-case, its arms always inlined decided and
    /// its method arms queued.
    fn type_case(
        &mut self,
        scope: ScopeId,
        program_counter: u32,
        candidates: Option<&'tree super::tree::SiteCandidates>,
    ) -> SiteDecision {
        let Some(candidates) = candidates.filter(|candidates| !candidates.megamorphic) else {
            return SiteDecision::TypeCase {
                arms: Vec::new(),
                complete: false,
            };
        };
        let mut arms = Vec::with_capacity(candidates.arms.len());
        for (position, (receiver, callee)) in candidates.arms.iter().enumerate() {
            arms.push(match classify(receiver, callee) {
                Callee::Always(always) => Some((
                    receiver.dispatch_id,
                    receiver.application,
                    ArmPlan::Always(always),
                )),
                Callee::Method { snapshot, .. } => {
                    self.push(
                        scope,
                        program_counter,
                        snapshot.code_id,
                        CandidateKind::Arm(position),
                    );
                    None
                }
                Callee::Not(reason) => {
                    self.decline(scope, program_counter, callee_code(callee), reason);
                    None
                }
            });
        }
        SiteDecision::TypeCase {
            arms,
            complete: !candidates.arms.is_empty(),
        }
    }

    /// A `super` or lexical send whose guard the scope's known receiver
    /// fixes: the cached arm `fixed` picks -- the one whose guard is the
    /// class the lookup starts at there -- taken without a test, and the
    /// send sent where there is none.
    fn unguarded(
        &mut self,
        scope: ScopeId,
        program_counter: u32,
        candidates: Option<&'tree super::tree::SiteCandidates>,
        fixed: impl Fn(&Scope<'tree>, &ReceiverFacts) -> bool,
    ) -> SiteDecision {
        let owner = &self.scopes[scope.index()];
        let Some(candidates) = candidates.filter(|candidates| !candidates.megamorphic) else {
            return SiteDecision::Unguarded(None);
        };
        if owner.plan.receiver.is_none() {
            return SiteDecision::Unguarded(None);
        }
        let Some((position, (receiver, callee))) = candidates
            .arms
            .iter()
            .enumerate()
            .find(|(_, (receiver, _))| fixed(owner, receiver))
        else {
            return SiteDecision::Unguarded(None);
        };
        match classify(receiver, callee) {
            Callee::Always(always) => SiteDecision::Unguarded(Some((
                receiver.dispatch_id,
                receiver.application,
                ArmPlan::Always(always),
            ))),
            Callee::Method { snapshot, .. } => {
                self.push(
                    scope,
                    program_counter,
                    snapshot.code_id,
                    CandidateKind::Unguarded(position),
                );
                SiteDecision::Unguarded(None)
            }
            Callee::Not(reason) => {
                self.decline(scope, program_counter, callee_code(callee), reason);
                SiteDecision::Unguarded(None)
            }
        }
    }

    /// Takes `candidate` if its callee is under its limit, the compile
    /// under its own, its recursion under the limit and its frame under
    /// the register ceiling, and records why it does not otherwise
    /// (`decline`); a candidate whose arm or block the tree does not hold
    /// decides nothing.
    fn take(&mut self, candidate: &Candidate) {
        let tree = self.tree;
        let sender = &self.scopes[candidate.scope.index()];
        if !sender.alive {
            return;
        }
        let sender_snapshot: &'tree Snapshot = sender.snapshot;
        let sender_offset = sender.plan.offset;
        let index = candidate.program_counter as usize;
        let instruction = &sender_snapshot.instructions[index];
        let send = send_of(instruction).expect("internal: a candidate is a send");
        let block_arguments = send
            .sources
            .iter()
            .filter(|source| {
                sender.holdings[index]
                    .get(source)
                    .is_some_and(|held| held.iter().any(|block| *block != SOMETHING_ELSE))
            })
            .count();
        let bonus = self
            .limits
            .block_argument_bonus
            .saturating_mul(u32::try_from(block_arguments).unwrap_or(u32::MAX));
        let site = sender_snapshot.sites[index]
            .expect("internal: a candidate's send names its site")
            .site;
        let candidates = tree.sites.get(&(sender_snapshot.code_id, site));
        let (snapshot, limit, arm, scope_facts) = match candidate.kind {
            CandidateKind::Arm(position) | CandidateKind::Unguarded(position) => {
                let Some((receiver, callee)) =
                    candidates.and_then(|candidates| candidates.arms.get(position))
                else {
                    return;
                };
                let Callee::Method { snapshot, level } = classify(receiver, callee) else {
                    return;
                };
                let public = matches!(candidate.kind, CandidateKind::Arm(_));
                let facts = ScopeFacts {
                    receiver: if public {
                        Some(receiver.application)
                    } else {
                        sender.plan.receiver
                    },
                    entered_chain: receiver
                        .applications
                        .get(usize::from(level)..)
                        .map(<[_]>::to_vec),
                    own_slot_base: receiver.own_slot_bases.get(usize::from(level)).copied(),
                    level: Some(level),
                    block: None,
                };
                (
                    snapshot,
                    self.limits.method.saturating_add(bonus),
                    (receiver.dispatch_id, receiver.application),
                    facts,
                )
            }
            CandidateKind::Block(block, dispatch_id, application) => {
                let Some((block_code, _)) = self.made_block(block) else {
                    return;
                };
                let Some(snapshot) = tree.blocks.get(&block_code) else {
                    return;
                };
                let reason = if is_observed(snapshot) {
                    Some(InlineDecline::Observed)
                } else if self.reaches_past_the_compile(block, snapshot) {
                    Some(InlineDecline::ReachesPastTheCompile)
                } else {
                    None
                };
                if let Some(reason) = reason {
                    self.decline(
                        candidate.scope,
                        candidate.program_counter,
                        Some(block_code),
                        reason,
                    );
                    return;
                }
                let home = &self.scopes[block.0.index()];
                let facts = ScopeFacts {
                    receiver: home.plan.receiver,
                    entered_chain: home.entered_chain.clone(),
                    own_slot_base: home.plan.own_slot_base,
                    level: None,
                    block: Some(block),
                };
                (
                    snapshot,
                    self.limits.block.saturating_add(bonus),
                    (dispatch_id, application),
                    facts,
                )
            }
        };
        let callee_weight = weight_in(tree, snapshot);
        let offset = sender_offset
            .checked_add(send.argument_base)
            .filter(|offset| {
                usize::from(*offset) + snapshot.register_count() <= usize::from(u16::MAX)
            });
        let declined = if callee_weight > limit {
            Some(InlineDecline::OverWeight)
        } else if self.total.saturating_add(callee_weight) > self.limits.compile {
            Some(InlineDecline::OverCompileLimit)
        } else if self.recursion(candidate.scope, snapshot.code_id) > self.limits.recursion {
            Some(InlineDecline::OverRecursionLimit)
        } else if offset.is_none() {
            Some(InlineDecline::OutOfRegisters)
        } else {
            None
        };
        let offset = match (offset, declined) {
            (Some(offset), None) => offset,
            (_, Some(reason)) => {
                self.decline(
                    candidate.scope,
                    candidate.program_counter,
                    Some(snapshot.code_id),
                    reason,
                );
                return;
            }
            (None, None) => unreachable!("internal: a frame past the ceiling is declined"),
        };
        let entry = self.entry_of(candidate.scope, index, &send, snapshot, scope_facts.block);
        let frequency = candidate.frequency;
        self.total += callee_weight;
        let callee = self.add_scope(
            ScopePlan {
                code_id: snapshot.code_id,
                parent: Some((candidate.scope, candidate.program_counter)),
                offset,
                receiver: scope_facts.receiver,
                entered: scope_facts
                    .entered_chain
                    .as_ref()
                    .and_then(|chain| chain.first().copied()),
                own_slot_base: scope_facts.own_slot_base,
                level: scope_facts.level,
                block_of: scope_facts.block.map(|(home, _)| home),
                block: None,
                eliminated_before: Vec::new(),
            },
            snapshot,
            frequency,
            scope_facts.entered_chain,
            scope_facts.block,
            entry,
        );
        let arm = (arm.0, arm.1, ArmPlan::Inline { callee });
        let key = (candidate.scope, candidate.program_counter);
        match (self.decisions.get_mut(&key), candidate.kind) {
            (Some(SiteDecision::TypeCase { arms, .. }), CandidateKind::Arm(position)) => {
                arms[position] = Some(arm);
            }
            (
                Some(SiteDecision::Unguarded(slot)),
                CandidateKind::Unguarded(_) | CandidateKind::Block(..),
            ) => {
                *slot = Some(arm);
            }
            _ => unreachable!("internal: a candidate's site was decided as its kind"),
        }
    }

    /// Whether the body of `block`, inlined at a `value` send, reaches a
    /// frame the compile does not lay out: an outer local past the
    /// outermost block scope of its chain of homes, or the home of a `^`
    /// -- its first home that is no block -- when that is the root and the
    /// root is a block's body, whose own home lies below the compile. An
    /// inlined block reaches its homes' registers as physical ones and
    /// returns from its home as a jump, which neither can be past the
    /// compile. Nothing else of a block keeps it out: a leaf called inside
    /// its scope reads an activation whose closure is not made yet, and
    /// every reader a leaf reaches -- the kind a body too large to compile
    /// is named after, whether the code is a body-less declaration's own --
    /// decides it from the code (`Heap::activation_runs_a_block`).
    fn reaches_past_the_compile(&self, block: BlockKey, snapshot: &Snapshot) -> bool {
        // The scope that is `depth` frames out from the block's own: its
        // home, then each home's home while each is a block scope.
        let home_at = |depth: u16| {
            let mut scope = block.0;
            for _ in 1..depth {
                scope = self.scopes[scope.index()].block?.0;
            }
            Some(scope)
        };
        snapshot
            .instructions
            .iter()
            .any(|instruction| match instruction {
                SnapshotInstruction::LoadOuterLocal { depth, .. }
                | SnapshotInstruction::StoreOuterLocal { depth, .. } => {
                    *depth > 0 && home_at(*depth).is_none()
                }
                SnapshotInstruction::NonLocalReturn { .. } => {
                    let mut home = block.0;
                    while let Some((outer, _)) = self.scopes[home.index()].block {
                        home = outer;
                    }
                    home == ScopeId::ROOT && self.tree.root.facts.kind == CodeKind::Block
                }
                _ => false,
            })
    }

    /// How many scopes from `scope` up to the root run the body `code_id`.
    fn recursion(&self, scope: ScopeId, code_id: CodeId) -> u32 {
        let mut count = 0;
        let mut next = Some(scope);
        while let Some(scope) = next {
            let owner = &self.scopes[scope.index()];
            if owner.plan.code_id == code_id {
                count += 1;
            }
            next = owner.plan.parent.map(|(parent, _)| parent);
        }
        count
    }

    /// What the callee's registers hold on entry: each parameter what
    /// its argument held at the send, and a block's copied values what
    /// their sources held where it was made.
    fn entry_of(
        &self,
        scope: ScopeId,
        index: usize,
        send: &Send<'_>,
        callee: &Snapshot,
        block: Option<BlockKey>,
    ) -> Holdings {
        let held = &self.scopes[scope.index()].holdings[index];
        let mut entry = Holdings::new();
        for (parameter, source) in (0_u16..).zip(send.sources) {
            if let Some(blocks) = held.get(source) {
                entry.insert(parameter, blocks.clone());
            }
        }
        if let Some((home, program_counter)) = block {
            let home = &self.scopes[home.index()];
            if let Some(SnapshotInstruction::MakeClosure { copied, .. }) =
                home.snapshot.instructions.get(program_counter as usize)
            {
                let made = &home.holdings[program_counter as usize];
                let start = callee.facts.argument_count + HEADER_REGISTERS;
                for (position, source) in (0_u16..).zip(copied) {
                    if let Some(blocks) = made.get(source) {
                        entry.insert(start + position, blocks.clone());
                    }
                }
            }
        }
        entry
    }

    /// The blocks the compile eliminates (the module's comment): every
    /// block of a live scope at first, then each that escapes taken out,
    /// with the scopes inlined for its `value` sends, until none does.
    /// A `MakeClosure` the graph does not build, or lowers as a trap
    /// (`build::compiled_ways`), makes no block of the compile, and one
    /// whose body the tree holds no snapshot of -- which an exit could not
    /// materialize -- is created.
    fn decide_blocks(&mut self) -> BTreeSet<BlockKey> {
        let mut eliminated: BTreeSet<BlockKey> = BTreeSet::new();
        for (index, scope) in self
            .scopes
            .iter()
            .enumerate()
            .filter(|_| self.limits.inline_blocks)
        {
            for (program_counter, instruction) in scope.snapshot.instructions.iter().enumerate() {
                if let SnapshotInstruction::MakeClosure { block, .. } = instruction
                    && scope.ways[program_counter] == CompiledWay::Runs
                    && self.tree.blocks.contains_key(block)
                {
                    eliminated.insert((
                        ScopeId::of_index(index),
                        program_counter_u32(program_counter),
                    ));
                }
            }
        }
        let accessed: Vec<Option<HashSet<u16>>> = self
            .scopes
            .iter()
            .map(|scope| self.outer_accessed(scope))
            .collect();
        loop {
            let escaping: Vec<BlockKey> = eliminated
                .iter()
                .copied()
                .filter(|block| {
                    !self.scopes[block.0.index()].alive
                        || self.escapes(*block, &eliminated, &accessed)
                })
                .collect();
            if !escaping.is_empty() {
                for block in escaping {
                    eliminated.remove(&block);
                    self.drop_scopes_of(block);
                }
                continue;
            }
            // An inlined scope that creates a block leaves optimized code
            // at its `MakeClosure` every time it runs it, until optimized
            // code creates blocks itself: its send is sent instead, which
            // runs the callee's own code, and the blocks it was handed may
            // escape now.
            let creating: Vec<usize> = (1..self.scopes.len())
                .filter(|index| self.creates_a_block(*index, &eliminated))
                .collect();
            if creating.is_empty() {
                return eliminated;
            }
            for index in creating {
                self.drop_scopes_where(|candidate, _| candidate == index);
            }
        }
    }

    /// Whether the live scope at `index` holds a `MakeClosure` whose block
    /// `eliminated` does not hold, which it would create, on a way that
    /// ran: not a fast path's fallback -- a conditional's closure for a
    /// receiver that is no Boolean runs as rarely as such a receiver comes
    /// -- nor, where the copy speculates, a way whose send of the block
    /// never ran, whose `MakeClosure` the graph lowers as a trap on that
    /// send's site (`build::guard_of`), as every way that never ran traps,
    /// nor a way past such a trap, which the graph does not build.
    /// Until optimized code creates blocks itself (stage 4), such a scope
    /// would leave the copy at its `MakeClosure` every time it ran it.
    fn creates_a_block(&self, index: usize, eliminated: &BTreeSet<BlockKey>) -> bool {
        let scope = &self.scopes[index];
        if !scope.alive {
            return false;
        }
        let fallback = fallback_instructions(&scope.snapshot.instructions);
        scope
            .snapshot
            .instructions
            .iter()
            .zip(fallback)
            .enumerate()
            .any(|(program_counter, (instruction, fallback))| {
                matches!(instruction, SnapshotInstruction::MakeClosure { .. })
                    && !fallback
                    && scope.ways[program_counter] == CompiledWay::Runs
                    && !eliminated.contains(&(
                        ScopeId::of_index(index),
                        program_counter_u32(program_counter),
                    ))
            })
    }

    /// Drops every live scope inlined for a `value` send of `block`, and
    /// every scope below it; each such send is sent instead.
    fn drop_scopes_of(&mut self, block: BlockKey) {
        self.drop_scopes_where(|_, scope| scope.block == Some(block));
    }

    /// Drops every live scope `dropped` names, by its index, and every
    /// scope below it; each such send is sent instead.
    fn drop_scopes_where(&mut self, dropped: impl Fn(usize, &Scope<'_>) -> bool) {
        for index in 0..self.scopes.len() {
            let scope = &self.scopes[index];
            let parent_dead = scope
                .plan
                .parent
                .is_some_and(|(parent, _)| !self.scopes[parent.index()].alive);
            if scope.alive && (dropped(index, scope) || parent_dead) {
                if let Some(parent) = scope.plan.parent
                    && let Some(SiteDecision::Unguarded(slot)) = self.decisions.get_mut(&parent)
                    && matches!(slot, Some((_, _, ArmPlan::Inline { callee })) if callee.index() == index)
                {
                    *slot = None;
                }
                self.scopes[index].alive = false;
                self.total = self.total.saturating_sub(self.scopes[index].weight);
            }
        }
    }

    /// The registers of a frame running `scope`'s body that a block
    /// literal nested in it reads or writes by reference
    /// (`LoadOuterLocal`, `StoreOuterLocal`); `None` when a nested block
    /// was not snapshotted, which may reach any. A block the scope makes
    /// only on a way that ends in a trap, or at the trap -- a conditional's
    /// closure for a receiver that is no Boolean, where none ever came --
    /// is made by no code of the compile: the trap's exit hands the frame
    /// to the interpreter, the blocks it holds materialized, before it.
    fn outer_accessed(&self, scope: &Scope<'tree>) -> Option<HashSet<u16>> {
        let mut accessed = HashSet::new();
        let mut work: Vec<(&Snapshot, u16)> = vec![(scope.snapshot, 0)];
        while let Some((body, depth)) = work.pop() {
            for (program_counter, instruction) in body.instructions.iter().enumerate() {
                if depth == 0 && scope.ways[program_counter] != CompiledWay::Runs {
                    continue;
                }
                match instruction {
                    SnapshotInstruction::LoadOuterLocal {
                        depth: reach,
                        index,
                        ..
                    }
                    | SnapshotInstruction::StoreOuterLocal {
                        depth: reach,
                        index,
                        ..
                    } if *reach == depth && depth > 0 => {
                        accessed.insert(*index);
                    }
                    SnapshotInstruction::MakeClosure { block, .. } => {
                        work.push((self.tree.blocks.get(block)?, depth + 1));
                    }
                    _ => {}
                }
            }
        }
        Some(accessed)
    }

    /// Whether `block` escapes, `eliminated` the blocks still taken as
    /// eliminated and `accessed` each scope's registers its nested
    /// blocks reach by reference. A way that ends in a trap lets nothing
    /// escape (`build::compiled_ways`): at the trap the block is where an
    /// exit finds it on every way there, so the exit materializes it and
    /// the interpreter runs the instruction with it -- C2 reallocates an
    /// object that escapes only through an uncommon trap the same way --
    /// and nothing past the trap is built.
    fn escapes(
        &self,
        block: BlockKey,
        eliminated: &BTreeSet<BlockKey>,
        accessed: &[Option<HashSet<u16>>],
    ) -> bool {
        for (index, scope) in self.scopes.iter().enumerate() {
            if !scope.alive {
                continue;
            }
            let id = ScopeId::of_index(index);
            let mut holding = HashSet::new();
            for (program_counter, instruction) in scope.snapshot.instructions.iter().enumerate() {
                let way = scope.ways[program_counter];
                if way == CompiledWay::Unreached {
                    continue;
                }
                let held = &scope.holdings[program_counter];
                for (register, used) in uses_of(instruction) {
                    if !held
                        .get(&register)
                        .is_some_and(|blocks| blocks.contains(&block))
                    {
                        continue;
                    }
                    holding.insert(register);
                    let key = (id, program_counter_u32(program_counter));
                    let harmless = way == CompiledWay::Traps
                        || match used {
                            Use::Move => true,
                            Use::Copied => eliminated.contains(&key),
                            Use::Receiver => matches!(
                                self.decisions.get(&key),
                                Some(SiteDecision::Unguarded(Some((_, _, ArmPlan::Inline { callee }))))
                                    if self.scopes[callee.index()].block == Some(block)
                            ),
                            Use::Argument(_) => self.inlines_every_way(key),
                            Use::Other => false,
                        };
                    if !harmless {
                        return true;
                    }
                }
                for (register, blocks) in held {
                    if blocks.contains(&block) {
                        holding.insert(*register);
                        // Held on one way and not on another, the block
                        // would be where an exit finds it only on some ways
                        // there: its register's word, which the exit leaves
                        // to the frame, is the interpreter's on every one.
                        if blocks.len() > 1
                            && !tainted_at(scope.snapshot, program_counter, *register)
                        {
                            return true;
                        }
                    }
                }
            }
            let reached = match &accessed[index] {
                Some(accessed) => holding.iter().any(|register| accessed.contains(register)),
                None => !holding.is_empty(),
            };
            if reached {
                return true;
            }
            if id == block.0 && self.copied_source_written(block, accessed[index].as_ref()) {
                return true;
            }
        }
        false
    }

    /// Whether every way through the site `key` inlines its callee: all
    /// its arms inlined scopes, and nothing falling through to a send.
    fn inlines_every_way(&self, key: (ScopeId, u32)) -> bool {
        let inline = |arm: &Option<(DispatchId, ApplicationIndex, ArmPlan)>| matches!(arm, Some((_, _, ArmPlan::Inline { callee })) if self.scopes[callee.index()].alive);
        match self.decisions.get(&key) {
            Some(SiteDecision::Unguarded(arm)) => inline(arm),
            Some(SiteDecision::TypeCase { arms, complete }) => {
                *complete && !arms.is_empty() && arms.iter().all(inline) && !self.trapped(key)
            }
            None => false,
        }
    }

    /// Whether the site `key` may not trap: the copy speculates nothing,
    /// or the site's speculation trapped before.
    fn trapped(&self, (scope, program_counter): (ScopeId, u32)) -> bool {
        !self.speculates
            || self.scopes[scope.index()]
                .snapshot
                .sites
                .get(program_counter as usize)
                .copied()
                .flatten()
                .is_none_or(|facts| facts.trapped)
    }

    /// Whether a register `block` copied a value from may be written while
    /// the block lives in its home: on a way from its `MakeClosure` to a
    /// use of it there -- its own, or that of a block it is copied into --
    /// an instruction writes it, or a callee's frame laid on slots at or
    /// below it covers it; or a nested block writes it by reference
    /// (`accessed`, `None` for any).
    fn copied_source_written(&self, block: BlockKey, accessed: Option<&HashSet<u16>>) -> bool {
        let (home, made_at) = block;
        let scope = &self.scopes[home.index()];
        let instructions = &scope.snapshot.instructions;
        let Some(SnapshotInstruction::MakeClosure { copied, .. }) =
            instructions.get(made_at as usize)
        else {
            return false;
        };
        if copied.is_empty() {
            return false;
        }
        if accessed.is_none_or(|accessed| copied.iter().any(|source| accessed.contains(source))) {
            return true;
        }
        let uses = self.uses_in_home(block);
        if uses.is_empty() {
            return false;
        }
        let made_at = made_at as usize;
        // The instructions a way from the block's making reaches before
        // it is made again, and those from which a way reaches a use.
        let reached = reach(instructions, &successors(instructions, made_at), made_at);
        let predecessors = predecessors(instructions);
        let starts: Vec<usize> = uses.iter().copied().collect();
        let reaching = reach_backward(&predecessors, &starts, made_at);
        for (index, instruction) in instructions.iter().enumerate() {
            if !reached[index] {
                continue;
            }
            let before_a_use = successors(instructions, index)
                .iter()
                .any(|successor| reaching[*successor]);
            let at_a_use = uses.contains(&index);
            let writes = instruction
                .register_written()
                .is_some_and(|written| copied.contains(&written));
            let frame_start = match instruction {
                SnapshotInstruction::InitializeInstance { .. }
                | SnapshotInstruction::ResolveFuture { .. } => {
                    Some(scope.snapshot.facts.window_end())
                }
                _ => instruction.argument_slots().map(|(base, _)| base),
            };
            let covers =
                frame_start.is_some_and(|start| copied.iter().any(|source| *source >= start));
            if (writes && before_a_use) || (covers && (before_a_use || at_a_use)) {
                return true;
            }
        }
        false
    }

    /// The instructions of `block`'s home that read a register holding
    /// it, or holding a block it is copied into there.
    fn uses_in_home(&self, block: BlockKey) -> BTreeSet<usize> {
        let (home, _) = block;
        let scope = &self.scopes[home.index()];
        let mut uses = BTreeSet::new();
        let mut work = vec![block];
        let mut seen = HashSet::from([block]);
        while let Some(held_block) = work.pop() {
            for (index, instruction) in scope.snapshot.instructions.iter().enumerate() {
                let held = &scope.holdings[index];
                for (register, used) in uses_of(instruction) {
                    if held
                        .get(&register)
                        .is_some_and(|blocks| blocks.contains(&held_block))
                    {
                        uses.insert(index);
                        let made = (home, program_counter_u32(index));
                        if used == Use::Copied && seen.insert(made) {
                            work.push(made);
                        }
                    }
                }
            }
        }
        uses
    }

    /// The plan: the live scopes renumbered in order, their sites and
    /// blocks.
    fn finish(self, eliminated: &BTreeSet<BlockKey>) -> InliningPlan {
        let declined = self
            .declined
            .iter()
            .filter(|(scope, _)| self.scopes[scope.index()].alive)
            .map(|(_, declined)| *declined)
            .collect();
        let mut renumbered: Vec<Option<ScopeId>> = vec![None; self.scopes.len()];
        let mut next = 0_usize;
        for (index, scope) in self.scopes.iter().enumerate() {
            if scope.alive {
                renumbered[index] = Some(ScopeId::of_index(next));
                next += 1;
            }
        }
        let map = |scope: ScopeId| {
            renumbered[scope.index()].expect("internal: a live scope's relatives live")
        };
        let mut scopes = Vec::with_capacity(next);
        let mut blocks = HashMap::new();
        let mut ids: HashMap<BlockKey, EliminatedId> = HashMap::new();
        let mut eliminated_count = 0_u32;
        let (mut deepest_chain, mut deepest_extent) = (0_u16, 0_u32);
        for (index, scope) in self.scopes.iter().enumerate() {
            if !scope.alive {
                continue;
            }
            let id = ScopeId::of_index(index);
            let mut plan = scope.plan.clone();
            plan.parent = plan
                .parent
                .map(|(parent, program_counter)| (map(parent), program_counter));
            plan.block_of = plan.block_of.map(map);
            deepest_chain = deepest_chain.max(scope.depth);
            deepest_extent = deepest_extent.max(
                u32::from(plan.offset)
                    + u32::try_from(scope.snapshot.register_count()).unwrap_or(u32::MAX),
            );
            for (program_counter, instruction) in scope.snapshot.instructions.iter().enumerate() {
                if matches!(instruction, SnapshotInstruction::MakeClosure { .. }) {
                    let program_counter = program_counter_u32(program_counter);
                    let decision = if eliminated.contains(&(id, program_counter)) {
                        let eliminated_id = EliminatedId(eliminated_count);
                        eliminated_count += 1;
                        ids.insert((id, program_counter), eliminated_id);
                        BlockPlan::Eliminated(eliminated_id)
                    } else {
                        BlockPlan::Created
                    };
                    blocks.insert((map(id), program_counter), decision);
                }
            }
            scopes.push(plan);
        }
        // Where each eliminated block is held, once every block has its
        // id: a scope holds those of its senders and homes besides its own.
        for (plan, scope) in scopes
            .iter_mut()
            .zip(self.scopes.iter().filter(|scope| scope.alive))
        {
            plan.block = scope.block.and_then(|block| ids.get(&block).copied());
            plan.eliminated_before = scope
                .holdings
                .iter()
                .map(|held| {
                    held.iter()
                        .filter_map(|(register, blocks)| {
                            match blocks.iter().collect::<Vec<_>>()[..] {
                                [block] => ids.get(block).map(|id| (*register, *id)),
                                _ => None,
                            }
                        })
                        .collect()
                })
                .collect();
        }
        let mut sites = HashMap::new();
        for ((scope, program_counter), decision) in &self.decisions {
            let Some(renumbered_scope) = renumbered[scope.index()] else {
                continue;
            };
            let live = |arm: &Option<(DispatchId, ApplicationIndex, ArmPlan)>| match arm {
                Some((dispatch_id, application, ArmPlan::Inline { callee })) => renumbered
                    [callee.index()]
                .map(|callee| (*dispatch_id, *application, ArmPlan::Inline { callee })),
                Some((dispatch_id, application, ArmPlan::Always(always))) => {
                    Some((*dispatch_id, *application, ArmPlan::Always(*always)))
                }
                None => None,
            };
            let site = match decision {
                SiteDecision::Unguarded(arm) => match live(arm) {
                    Some(arm) => SitePlan {
                        arms: vec![arm],
                        fall_through: FallThrough::Unguarded,
                    },
                    None => SitePlan {
                        arms: Vec::new(),
                        fall_through: FallThrough::Send,
                    },
                },
                SiteDecision::TypeCase { arms, complete } => {
                    let taken: Vec<_> = arms.iter().filter_map(live).collect();
                    let every_type = *complete && !taken.is_empty() && taken.len() == arms.len();
                    SitePlan {
                        fall_through: if every_type && !self.trapped((*scope, *program_counter)) {
                            FallThrough::Trap
                        } else {
                            FallThrough::Send
                        },
                        arms: taken,
                    }
                }
            };
            sites.insert((renumbered_scope, *program_counter), site);
        }
        InliningPlan {
            scopes,
            sites,
            blocks,
            deepest_chain,
            deepest_extent,
            declined,
        }
    }
}

/// The share of the plain budget, `plain` -- `OPTIMIZATION_BUDGET`, or a
/// test's own -- that a row holding `remaining` spent since it was armed
/// with `armed`, at most 1: the instructions the body ran, in the same
/// units for every body, whether its row waits the plain budget or a
/// retired copy's longer wait (decision 7). A parked row, `i64::MAX`, ran
/// its whole budget and waits for its compile, and under a plain budget
/// of 0 -- the optimize stress mode's -- every spend runs it out.
#[expect(
    clippy::cast_precision_loss,
    reason = "a budget's waits need far fewer than 53 bits"
)]
pub(crate) fn budget_share(remaining: i64, armed: i64, plain: i64) -> f64 {
    if remaining == i64::MAX || plain <= 0 {
        return 1.0;
    }
    let spent = armed.saturating_sub(remaining).max(0);
    (spent as f64 / plain as f64).min(1.0)
}

/// What a scope about to be added knows of its receiver and level.
struct ScopeFacts {
    receiver: Option<ApplicationIndex>,
    entered_chain: Option<Vec<ApplicationIndex>>,
    own_slot_base: Option<u32>,
    level: Option<u16>,
    block: Option<BlockKey>,
}

/// The instructions reachable from `starts`, never passing through
/// `barrier`, where the block is made again.
fn reach(instructions: &[SnapshotInstruction], starts: &[usize], barrier: usize) -> Vec<bool> {
    let mut reached = vec![false; instructions.len()];
    let mut work: Vec<usize> = starts.to_vec();
    while let Some(index) = work.pop() {
        if index == barrier || reached[index] {
            continue;
        }
        reached[index] = true;
        work.extend(successors(instructions, index));
    }
    reached
}

/// Each instruction's predecessors.
fn predecessors(instructions: &[SnapshotInstruction]) -> Vec<Vec<usize>> {
    let mut predecessors = vec![Vec::new(); instructions.len()];
    for index in 0..instructions.len() {
        for successor in successors(instructions, index) {
            predecessors[successor].push(index);
        }
    }
    predecessors
}

/// The instructions from which a way reaches one of `starts`, never
/// passing through `barrier`.
fn reach_backward(predecessors: &[Vec<usize>], starts: &[usize], barrier: usize) -> Vec<bool> {
    let mut reached = vec![false; predecessors.len()];
    let mut work: Vec<usize> = starts.to_vec();
    while let Some(index) = work.pop() {
        if index == barrier || reached[index] {
            continue;
        }
        reached[index] = true;
        work.extend(predecessors[index].iter().copied());
    }
    reached
}

#[cfg(test)]
mod tests {
    use indoc::indoc;

    use super::{
        ArmPlan, BLOCK_ARGUMENT_BONUS, BlockPlan, Declined, EliminatedId, FallThrough,
        InlineDecline, InliningPlan, Limits, METHOD_LIMIT, ScopeId, weight, weight_in,
    };
    use crate::heap::CodeId;
    use crate::interpreter::inline_cache::MethodSide;
    use crate::jit::optimizing::Speculation;
    use crate::jit::optimizing::snapshot::{Snapshot, SnapshotInstruction};
    use crate::jit::optimizing::tree::{CalleeFacts, SnapshotTree};
    use crate::{HostResult, HostSignal, Interpreter, Lowering, Root, SourceOrigin, Symbol};

    /// The plan of `tree` with blocks inlined, which most of these tests
    /// are about.
    fn plan(tree: &SnapshotTree, speculation: Speculation) -> InliningPlan {
        super::plan_with(tree, &Limits::WITH_BLOCKS, speculation)
    }

    /// `super::plan_with` with blocks inlined.
    fn plan_with(tree: &SnapshotTree, limits: &Limits, speculation: Speculation) -> InliningPlan {
        super::plan_with(
            tree,
            &Limits {
                inline_blocks: true,
                ..*limits
            },
            speculation,
        )
    }

    fn expect_ok(result: HostResult, what: &str) -> Root {
        match result {
            Ok(root) => root,
            Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
            Err(HostSignal::Error(error)) => {
                panic!("{what}: unexpected Newspeak error: {}", error.message)
            }
        }
    }

    /// `source` loaded and its `run` sent once, so that the caches hold
    /// the feedback of one run.
    fn run(source: &str) -> (Interpreter, Root) {
        let module = ns_syntax::parse(source).expect("parse");
        let resolved = crate::resolve(module, source.into(), SourceOrigin::File("plan.ns".into()))
            .unwrap_or_else(|_| panic!("the module resolves"));
        let mut interpreter = Interpreter::bootstrap_with(Lowering::Optimized);
        // The run's feedback is what the plan reads: the cache stress mode
        // would vacate it at the run's safepoints.
        #[cfg(debug_assertions)]
        interpreter.set_stress_caches(false);
        // A body the breakpoint stress mode patched is neither optimized
        // nor inlined, and its baseline code exits at every statement.
        #[cfg(debug_assertions)]
        interpreter.set_stress_breakpoints(false);
        let class = interpreter.load_module(&resolved);
        let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        expect_ok(
            interpreter.send(&instance, Symbol::intern("run"), &[]),
            "run",
        );
        (interpreter, class)
    }

    /// The tree of `selector`'s body on `class`'s instance side, taken
    /// under `limits`.
    fn tree_of(
        interpreter: &mut Interpreter,
        class: &Root,
        selector: &str,
        limits: &Limits,
    ) -> SnapshotTree {
        interpreter.enter(|mutator| {
            let code = crate::interpreter::method_code(
                mutator,
                class,
                MethodSide::Instance,
                Symbol::intern(selector),
            )
            .expect("the method");
            let base = mutator.caches.known_base(&code).expect("it ran");
            SnapshotTree::take(
                &code,
                &mut mutator.heap,
                &mutator.caches,
                base,
                None,
                limits,
            )
        })
    }

    /// The code id of `selector`'s body on `class`'s instance side.
    fn code_id_of(interpreter: &mut Interpreter, class: &Root, selector: &str) -> CodeId {
        interpreter.enter(|mutator| {
            let code = crate::interpreter::method_code(
                mutator,
                class,
                MethodSide::Instance,
                Symbol::intern(selector),
            )
            .expect("the method");
            mutator.heap.code_id(&code)
        })
    }

    /// The selector of the send `instruction` is, if it is one.
    fn selector_of(instruction: &SnapshotInstruction) -> Option<Symbol> {
        match instruction {
            SnapshotInstruction::SendPublic { selector, .. }
            | SnapshotInstruction::SendLexical { selector, .. }
            | SnapshotInstruction::SendSuper { selector, .. }
            | SnapshotInstruction::SendInlinable { selector, .. } => Some(*selector),
            _ => None,
        }
    }

    /// The program counters of `snapshot`'s sends of `selector`, in order.
    fn sends_of(snapshot: &Snapshot, selector: &str) -> Vec<u32> {
        snapshot
            .instructions
            .iter()
            .enumerate()
            .filter(|(_, instruction)| selector_of(instruction) == Some(Symbol::intern(selector)))
            .map(|(index, _)| u32::try_from(index).expect("a small body"))
            .collect()
    }

    /// The one send of `selector` in `snapshot`.
    fn send_of(snapshot: &Snapshot, selector: &str) -> u32 {
        let [program_counter] = sends_of(snapshot, selector)[..] else {
            panic!("one send of {selector}: {:#?}", snapshot.instructions);
        };
        program_counter
    }

    /// The program counters of `snapshot`'s `MakeClosure`s, in order.
    fn closures_of(snapshot: &Snapshot) -> Vec<u32> {
        snapshot
            .instructions
            .iter()
            .enumerate()
            .filter(|(_, instruction)| {
                matches!(instruction, SnapshotInstruction::MakeClosure { .. })
            })
            .map(|(index, _)| u32::try_from(index).expect("a small body"))
            .collect()
    }

    /// The program counter of `snapshot`'s one `MakeClosure`.
    fn closure_of(snapshot: &Snapshot) -> u32 {
        let [closure] = closures_of(snapshot)[..] else {
            panic!("one closure: {:#?}", snapshot.instructions);
        };
        closure
    }

    /// The scope inlined at `scope`'s send at `program_counter`, from a
    /// site whose one arm inlines it.
    fn inlined_at(plan: &InliningPlan, scope: ScopeId, program_counter: u32) -> ScopeId {
        let site = &plan.sites[&(scope, program_counter)];
        let [(_, _, ArmPlan::Inline { callee })] = site.arms[..] else {
            panic!("one inlined arm at {scope:?}, {program_counter}: {site:?}");
        };
        callee
    }

    /// The argument base of `snapshot`'s send at `program_counter`.
    fn argument_base_of(snapshot: &Snapshot, program_counter: u32) -> u16 {
        snapshot.instructions[program_counter as usize]
            .argument_slots()
            .expect("a send")
            .0
    }

    /// The scope `id` names.
    fn scope(plan: &InliningPlan, id: ScopeId) -> &super::ScopePlan {
        &plan.scopes[id.0 as usize]
    }

    const MONOMORPHIC: &str = indoc! {"
        class Monomorphic = () (
          public run = (
            ^self helper: 3
          )
          public helper: number = (
            ^number + 1
          )
        )
    "};

    #[test]
    fn a_small_monomorphic_callee_is_inlined() {
        let (mut interpreter, class) = run(MONOMORPHIC);
        let tree = tree_of(&mut interpreter, &class, "run", &Limits::STANDARD);
        let plan = plan(&tree, Speculation::Traps);
        let send = send_of(&tree.root, "helper:");
        assert_eq!(plan.scopes.len(), 2);
        assert_eq!(inlined_at(&plan, ScopeId::ROOT, send), ScopeId(1));
        let site = &plan.sites[&(ScopeId::ROOT, send)];
        assert_eq!(
            site.fall_through,
            FallThrough::Trap,
            "a site that never trapped, its one type inlined, traps for any other"
        );
        let callee = scope(&plan, ScopeId(1));
        assert_eq!(
            callee.code_id,
            code_id_of(&mut interpreter, &class, "helper:")
        );
        assert_eq!(callee.parent, Some((ScopeId::ROOT, send)));
        assert_eq!(callee.receiver, Some(site.arms[0].1));
        assert_eq!(
            callee.entered, callee.receiver,
            "the receiver's own class defines it"
        );
        assert_eq!(callee.level, Some(0));
        assert_eq!(callee.block_of, None);
        assert_eq!(plan.deepest_chain, 1);
    }

    #[test]
    fn an_inlined_scope_lies_on_its_sender_s_argument_slots() {
        let (mut interpreter, class) = run(MONOMORPHIC);
        let tree = tree_of(&mut interpreter, &class, "run", &Limits::STANDARD);
        let plan = plan(&tree, Speculation::Traps);
        let send = send_of(&tree.root, "helper:");
        let root_send_argument_base = argument_base_of(&tree.root, send);
        assert_eq!(plan.scopes[1].offset, root_send_argument_base);
        // Machine code's send lays the callee's frame on the same slots:
        // its register 0 at the argument base.
        #[cfg(ns_jit)]
        {
            let slots = crate::jit::sends::call_slots_of(
                &tree.root.facts,
                &tree.root.instructions[send as usize],
            )
            .expect("a send lays its callee on slots");
            assert_eq!(plan.scopes[1].offset, slots.argument_base);
        }
        let callee = tree
            .snapshot_of(plan.scopes[1].code_id)
            .expect("its snapshot");
        assert!(
            plan.deepest_extent
                >= u32::from(root_send_argument_base)
                    + u32::try_from(callee.register_count()).expect("a small frame")
        );
    }

    const HEAVY: &str = indoc! {"
        class Heavy = () (
          public tick = (
            ^0
          )
          public heavy: number = (
            self tick.
            self tick.
            self tick.
            self tick.
            self tick.
            self tick.
            self tick.
            self tick.
            self tick.
            ^number
          )
          public run = (
            ^self heavy: 3
          )
        )
    "};

    #[test]
    fn a_callee_over_its_limit_is_sent() {
        let (mut interpreter, class) = run(HEAVY);
        let callee = tree_of(&mut interpreter, &class, "heavy:", &Limits::STANDARD);
        assert!(weight_in(&callee, &callee.root) > METHOD_LIMIT);
        let tree = tree_of(&mut interpreter, &class, "run", &Limits::STANDARD);
        let plan = plan(&tree, Speculation::Traps);
        let site = &plan.sites[&(ScopeId::ROOT, send_of(&tree.root, "heavy:"))];
        assert!(site.arms.is_empty(), "{site:?}");
        assert_eq!(site.fall_through, FallThrough::Send);
        assert_eq!(plan.scopes.len(), 1);
    }

    /// The plan says why it sent a callee it could have inlined: under a
    /// method limit `helper:` is over, its one send is declined for its
    /// weight, the callee named.
    #[test]
    fn an_overweight_callee_is_declined_with_its_reason() {
        let (mut interpreter, class) = run(MONOMORPHIC);
        let tree = tree_of(&mut interpreter, &class, "run", &Limits::STANDARD);
        let light = Limits {
            method: 1,
            ..Limits::STANDARD
        };
        let plan = plan_with(&tree, &light, Speculation::Traps);
        let send = send_of(&tree.root, "helper:");
        assert_eq!(plan.scopes.len(), 1);
        assert_eq!(
            plan.declined,
            [Declined {
                scope_code: tree.root.code_id,
                program_counter: send,
                callee: Some(code_id_of(&mut interpreter, &class, "helper:")),
                reason: InlineDecline::OverWeight,
            }]
        );
    }

    /// The send `count:` makes of itself is inlined once, and declined in
    /// the scope inlined there for the recursion limit.
    #[test]
    fn a_recursion_past_the_limit_is_declined() {
        let (mut interpreter, class) = run(RECURSION);
        let tree = tree_of(&mut interpreter, &class, "count:", &Limits::STANDARD);
        let plan = plan(&tree, Speculation::Traps);
        let send = send_of(&tree.root, "count:");
        assert!(
            plan.declined.contains(&Declined {
                scope_code: tree.root.code_id,
                program_counter: send,
                callee: Some(tree.root.code_id),
                reason: InlineDecline::OverRecursionLimit,
            }),
            "{:?}",
            plan.declined
        );
    }

    /// `remember:` hands a block to `keep:`, which stores it: the block
    /// escapes, and `remember:` would create it; `twice:` makes none.
    const CREATING: &str = indoc! {"
        class Creating = (
          | stored |
        ) (
          public keep: action = (
            stored:: action.
            ^0
          )
          public remember: number = (
            self keep: [number].
            ^number
          )
          public twice: number = (
            ^number + number
          )
          public run = (
            ^(self remember: 3) + (self twice: 4)
          )
        )
    "};

    /// A callee whose scope would create a block -- which leaves optimized
    /// code at its `MakeClosure` every time it runs it -- is sent, while a
    /// callee beside it that makes none is inlined.
    #[test]
    fn a_callee_that_would_create_a_block_is_sent() {
        let (mut interpreter, class) = run(CREATING);
        let tree = tree_of(&mut interpreter, &class, "run", &Limits::STANDARD);
        let plan = plan(&tree, Speculation::Traps);
        let remember = &plan.sites[&(ScopeId::ROOT, send_of(&tree.root, "remember:"))];
        assert!(remember.arms.is_empty(), "{remember:?}");
        assert_eq!(remember.fall_through, FallThrough::Send);
        let twice = &plan.sites[&(ScopeId::ROOT, send_of(&tree.root, "twice:"))];
        assert_eq!(twice.arms.len(), 1, "{twice:?}");
        assert_eq!(plan.scopes.len(), 2, "the root and `twice:`");
        assert!(
            plan.blocks
                .values()
                .all(|block| *block != BlockPlan::Created)
        );
    }

    /// `remember:` makes a block only for a number past 100, which `run`
    /// never hands it: the send of the block never ran.
    const CREATING_ON_A_WAY_NOT_RUN: &str = indoc! {"
        class Creating = (
          | stored |
        ) (
          public keep: action = (
            stored:: action.
            ^0
          )
          public remember: number = (
            number > 100 ifTrue: [self keep: [number]].
            ^number
          )
          public run = (
            ^self remember: 3
          )
        )
    "};

    /// A callee that would create a block only on a way that never ran is
    /// inlined where the copy speculates, since that way traps there as
    /// every way that never ran does; a copy that speculates nothing sends
    /// it.
    #[test]
    fn a_callee_creating_a_block_on_a_way_not_run_is_inlined_where_the_copy_speculates() {
        let (mut interpreter, class) = run(CREATING_ON_A_WAY_NOT_RUN);
        let tree = tree_of(&mut interpreter, &class, "run", &Limits::STANDARD);
        let site = (ScopeId::ROOT, send_of(&tree.root, "remember:"));
        let speculating = plan(&tree, Speculation::Traps);
        assert_eq!(speculating.sites[&site].arms.len(), 1, "{speculating:?}");
        let cautious = plan(&tree, Speculation::None);
        assert!(cautious.sites[&site].arms.is_empty(), "{cautious:?}");
    }

    const BONUS: &str = indoc! {"
        class Bonus = () (
          public tick = (
            ^3 + 4
          )
          public apply: aBlock = (
            self tick.
            self tick.
            self tick.
            self tick.
            self tick.
            self tick.
            self tick.
            self tick.
            self tick.
            ^aBlock value
          )
          public run = (
            ^self apply: [3]
          )
        )
    "};

    #[test]
    fn a_block_argument_raises_the_callee_s_limit() {
        let (mut interpreter, class) = run(BONUS);
        let callee = tree_of(&mut interpreter, &class, "apply:", &Limits::STANDARD);
        let callee_weight = weight_in(&callee, &callee.root);
        assert!(
            callee_weight > METHOD_LIMIT && callee_weight <= METHOD_LIMIT + BLOCK_ARGUMENT_BONUS,
            "the callee weighs {callee_weight}"
        );
        let tree = tree_of(&mut interpreter, &class, "run", &Limits::STANDARD);
        let send = send_of(&tree.root, "apply:");
        let with_bonus = plan(&tree, Speculation::Traps);
        let inlined = inlined_at(&with_bonus, ScopeId::ROOT, send);
        assert_eq!(scope(&with_bonus, inlined).code_id, callee.root.code_id);
        let without = plan_with(
            &tree,
            &Limits {
                block_argument_bonus: 0,
                ..Limits::STANDARD
            },
            Speculation::Traps,
        );
        assert!(without.sites[&(ScopeId::ROOT, send)].arms.is_empty());
    }

    #[test]
    fn a_send_reaching_only_a_fast_path_weighs_as_an_accessor_send() {
        let (mut interpreter, class) = run(BONUS);
        let callee = tree_of(&mut interpreter, &class, "apply:", &Limits::STANDARD);
        // `aBlock value` reached `Closure`'s fast path: the one send of
        // the body that weighs `COST_CHEAP_SEND` where its cache is read.
        assert_eq!(
            weight(&callee.root) - weight_in(&callee, &callee.root),
            super::COST_SEND - super::COST_CHEAP_SEND
        );
    }

    const RECURSION: &str = indoc! {"
        class Recursion = () (
          public count: number = (
            number = 0 ifTrue: [^0].
            ^self count: number - 1
          )
          public run = (
            ^self count: 3
          )
        )
    "};

    #[test]
    fn recursion_is_unrolled_once() {
        let (mut interpreter, class) = run(RECURSION);
        let tree = tree_of(&mut interpreter, &class, "count:", &Limits::STANDARD);
        let plan = plan(&tree, Speculation::Traps);
        let send = send_of(&tree.root, "count:");
        let once = inlined_at(&plan, ScopeId::ROOT, send);
        assert_eq!(scope(&plan, once).code_id, tree.root.code_id);
        assert_eq!(plan.scopes.len(), 2, "{:#?}", plan.scopes);
        let again = &plan.sites[&(once, send)];
        assert!(again.arms.is_empty(), "{again:?}");
        assert_eq!(again.fall_through, FallThrough::Send);
    }

    const OBSERVED: &str = indoc! {"
        class Observed = () (
          public run = (
            ^[3] ensure: [4]
          )
        )
    "};

    #[test]
    fn an_observed_method_is_never_inlined() {
        // An `ensure:` callee: snapshotted under limits that would take
        // it, and never inlined.
        let (mut interpreter, class) = run(OBSERVED);
        let generous = Limits {
            method: 10_000,
            block: 10_000,
            ..Limits::STANDARD
        };
        let tree = tree_of(&mut interpreter, &class, "run", &generous);
        let send = send_of(&tree.root, "ensure:");
        let site = tree.root.sites[send as usize].expect("a site").site;
        let candidates = &tree.sites[&(tree.root.code_id, site)];
        assert!(
            matches!(
                &candidates.arms[..],
                [(_, CalleeFacts::Method { snapshot, .. })] if snapshot.facts.unwind_marker
            ),
            "{candidates:?}"
        );
        let plan = plan_with(&tree, &generous, Speculation::Traps);
        assert!(plan.sites[&(ScopeId::ROOT, send)].arms.is_empty());
        assert_eq!(plan.scopes.len(), 1);

        // A body with an unwind marker, however light.
        let (mut interpreter, class) = run(MONOMORPHIC);
        let mut tree = tree_of(&mut interpreter, &class, "run", &Limits::STANDARD);
        for candidates in tree.sites.values_mut() {
            for (_, callee) in &mut candidates.arms {
                if let CalleeFacts::Method { snapshot, .. } = callee {
                    snapshot.facts.unwind_marker = true;
                }
            }
        }
        let plan = super::plan(&tree, Speculation::Traps);
        let send = send_of(&tree.root, "helper:");
        assert!(plan.sites[&(ScopeId::ROOT, send)].arms.is_empty());
        assert_eq!(plan.scopes.len(), 1);
    }

    const FREQUENCY: &str = indoc! {"
        class Frequency = () (
          public helper: number = (
            ^number + 1
          )
          public run = (
            | index |
            self helper: 0.
            index:: 0.
            [index < 3] whileTrue: [
              self helper: index.
              index:: index + 1
            ].
            ^index
          )
        )
    "};

    #[test]
    fn the_compile_limit_takes_the_most_frequent_first() {
        let (mut interpreter, class) = run(FREQUENCY);
        let tree = tree_of(&mut interpreter, &class, "run", &Limits::STANDARD);
        let [outside, inside] = sends_of(&tree.root, "helper:")[..] else {
            panic!("two sends of helper:");
        };
        let helper = tree
            .snapshot_of(code_id_of(&mut interpreter, &class, "helper:"))
            .expect("helper:'s snapshot");
        let room_for_one = Limits {
            compile: weight_in(&tree, &tree.root) + weight_in(&tree, helper),
            ..Limits::STANDARD
        };
        let plan = plan_with(&tree, &room_for_one, Speculation::Traps);
        assert_eq!(plan.scopes.len(), 2);
        assert_eq!(plan.scopes[1].parent, Some((ScopeId::ROOT, inside)));
        assert!(plan.sites[&(ScopeId::ROOT, outside)].arms.is_empty());
        // With room for both, both are taken, the loop's first.
        let both = super::plan(&tree, Speculation::Traps);
        assert_eq!(both.scopes.len(), 3);
        assert_eq!(both.scopes[1].parent, Some((ScopeId::ROOT, inside)));
        assert_eq!(both.scopes[2].parent, Some((ScopeId::ROOT, outside)));
    }

    #[test]
    fn a_site_that_trapped_falls_through_to_its_send() {
        let (mut interpreter, class) = run(MONOMORPHIC);
        let mut tree = tree_of(&mut interpreter, &class, "run", &Limits::STANDARD);
        let send = send_of(&tree.root, "helper:");
        tree.root.sites[send as usize]
            .as_mut()
            .expect("a site")
            .trapped = true;
        let plan = plan(&tree, Speculation::Traps);
        assert_eq!(inlined_at(&plan, ScopeId::ROOT, send), ScopeId(1));
        assert_eq!(
            plan.sites[&(ScopeId::ROOT, send)].fall_through,
            FallThrough::Send
        );
    }

    const MIXED: &str = indoc! {"
        class Mixed = () (
          class Light = () (
            public answer = (
              ^3 + 4
            )
          )
          class Weighty = () (
            public tick = (
              ^0
            )
            public answer = (
              self tick.
              self tick.
              self tick.
              self tick.
              self tick.
              self tick.
              self tick.
              self tick.
              self tick.
              ^0
            )
          )
          public probe: object = (
            ^object answer
          )
          public run = (
            self probe: Light new.
            self probe: Weighty new.
            ^0
          )
        )
    "};

    #[test]
    fn a_type_not_inlined_has_no_arm_and_its_site_falls_through_to_its_send() {
        let (mut interpreter, class) = run(MIXED);
        let tree = tree_of(&mut interpreter, &class, "probe:", &Limits::STANDARD);
        let send = send_of(&tree.root, "answer");
        let site_number = tree.root.sites[send as usize].expect("a site").site;
        let candidates = &tree.sites[&(tree.root.code_id, site_number)];
        assert_eq!(candidates.arms.len(), 2, "both receivers cached");
        let plan = plan(&tree, Speculation::Traps);
        let site = &plan.sites[&(ScopeId::ROOT, send)];
        let [(guard, application, ArmPlan::Inline { callee })] = site.arms[..] else {
            panic!("only the light receiver's arm: {site:?}");
        };
        assert_eq!(guard, candidates.arms[0].0.dispatch_id);
        assert_eq!(application, candidates.arms[0].0.application);
        assert_eq!(scope(&plan, callee).receiver, Some(application));
        assert_eq!(site.fall_through, FallThrough::Send);
    }

    const MEGAMORPHIC: &str = indoc! {"
        class Receivers = () (
          class A = () (
            public answer = (
              ^3 + 4
            )
          )
          class B = () (
            public answer = (
              ^3 + 4
            )
          )
          class C = () (
            public answer = (
              ^3 + 4
            )
          )
          class D = () (
            public answer = (
              ^3 + 4
            )
          )
          class E = () (
            public answer = (
              ^3 + 4
            )
          )
          public probe: object = (
            ^object answer
          )
          public run = (
            self probe: A new.
            self probe: B new.
            self probe: C new.
            self probe: D new.
            self probe: E new.
            ^0
          )
        )
    "};

    #[test]
    fn a_megamorphic_site_gets_no_type_case() {
        let (mut interpreter, class) = run(MEGAMORPHIC);
        let tree = tree_of(&mut interpreter, &class, "probe:", &Limits::STANDARD);
        let plan = plan(&tree, Speculation::Traps);
        let site = &plan.sites[&(ScopeId::ROOT, send_of(&tree.root, "answer"))];
        assert!(site.arms.is_empty());
        assert_eq!(site.fall_through, FallThrough::Send);
    }

    const VALUES: &str = indoc! {"
        class Values = () (
          public apply: aBlock = (
            ^aBlock value: 3
          )
          public run = (
            ^self apply: [:each | each + 1]
          )
        )
    "};

    #[test]
    fn a_block_passed_to_an_inlined_callee_that_only_values_it_is_eliminated() {
        let (mut interpreter, class) = run(VALUES);
        let tree = tree_of(&mut interpreter, &class, "run", &Limits::STANDARD);
        let plan = plan(&tree, Speculation::Traps);
        let closure = closure_of(&tree.root);
        assert_eq!(
            plan.blocks[&(ScopeId::ROOT, closure)],
            BlockPlan::Eliminated(EliminatedId(0))
        );
        let apply = inlined_at(&plan, ScopeId::ROOT, send_of(&tree.root, "apply:"));
        let apply_snapshot = tree
            .snapshot_of(scope(&plan, apply).code_id)
            .expect("apply:'s snapshot");
        let value = send_of(apply_snapshot, "value:");
        assert_eq!(
            plan.sites[&(apply, value)].fall_through,
            FallThrough::Unguarded
        );
        let block = scope(&plan, inlined_at(&plan, apply, value));
        assert_eq!(block.block_of, Some(ScopeId::ROOT));
        assert_eq!(block.parent, Some((apply, value)));
        let SnapshotInstruction::MakeClosure {
            block: block_code, ..
        } = &tree.root.instructions[closure as usize]
        else {
            panic!("a closure");
        };
        assert_eq!(block.code_id, *block_code);
        // Each scope lies on its own sender's slots.
        assert_eq!(
            block.offset,
            scope(&plan, apply).offset + argument_base_of(apply_snapshot, value)
        );
        assert_eq!(plan.deepest_chain, 2);
    }

    /// Limits that inline no block: the block is created, the callee it
    /// is passed to still inlined, and its `value:` sent.
    #[test]
    fn without_block_inlining_every_block_is_created_and_its_value_sent() {
        let (mut interpreter, class) = run(VALUES);
        let tree = tree_of(&mut interpreter, &class, "run", &Limits::STANDARD);
        let plan = super::plan_with(&tree, &Limits::WITHOUT_BLOCKS, Speculation::Traps);
        let closure = closure_of(&tree.root);
        assert_eq!(plan.blocks[&(ScopeId::ROOT, closure)], BlockPlan::Created);
        let apply = inlined_at(&plan, ScopeId::ROOT, send_of(&tree.root, "apply:"));
        let apply_snapshot = tree
            .snapshot_of(scope(&plan, apply).code_id)
            .expect("apply:'s snapshot");
        let value = &plan.sites[&(apply, send_of(apply_snapshot, "value:"))];
        assert!(value.arms.is_empty());
        assert_eq!(value.fall_through, FallThrough::Send);
        assert!(plan.scopes.iter().all(|scope| scope.block_of.is_none()));
        assert_eq!(plan.scopes.len(), 2);
    }

    const ESCAPES: &str = indoc! {"
        class Escapes = (|
          public stored
        |) (
          public tick = (
            ^0
          )
          public keep: aBlock = (
            stored: aBlock.
            ^0
          )
          public sink: aBlock = (
            self tick.
            self tick.
            self tick.
            self tick.
            self tick.
            self tick.
            self tick.
            self tick.
            self tick.
            ^aBlock
          )
          public pass: aBlock = (
            ^self sink: aBlock
          )
          public run = (
            self keep: [3].
            ^self pass: [4]
          )
        )
    "};

    #[test]
    fn a_block_stored_or_passed_on_is_created() {
        let (mut interpreter, class) = run(ESCAPES);
        let tree = tree_of(&mut interpreter, &class, "run", &Limits::STANDARD);
        let plan = plan(&tree, Speculation::Traps);
        // Both callees are inlined, so that each block reaches its use.
        inlined_at(&plan, ScopeId::ROOT, send_of(&tree.root, "keep:"));
        inlined_at(&plan, ScopeId::ROOT, send_of(&tree.root, "pass:"));
        let closures = closures_of(&tree.root);
        assert_eq!(closures.len(), 2);
        for closure in closures {
            assert_eq!(plan.blocks[&(ScopeId::ROOT, closure)], BlockPlan::Created);
        }
    }

    const COPIES: &str = indoc! {"
        class Copies = () (
          public apply: aBlock with: other = (
            ^aBlock value
          )
          public go: captured = (
            ^self apply: [captured] with: 7
          )
          public run = (
            ^self go: 5
          )
        )
    "};

    #[test]
    fn a_block_whose_copied_source_is_written_while_it_lives_is_created() {
        let (mut interpreter, class) = run(COPIES);
        let mut tree = tree_of(&mut interpreter, &class, "go:", &Limits::STANDARD);
        let closure = closure_of(&tree.root);
        assert_eq!(
            plan(&tree, Speculation::Traps).blocks[&(ScopeId::ROOT, closure)],
            BlockPlan::Eliminated(EliminatedId(0)),
            "a block whose copied source stays unwritten"
        );
        let SnapshotInstruction::MakeClosure { copied, .. } =
            &tree.root.instructions[closure as usize]
        else {
            panic!("a closure");
        };
        let [source] = copied[..] else {
            panic!("the block copies the parameter: {copied:?}");
        };
        // The literal argument loaded after the block is made writes the
        // copied source instead: an exit in `apply:with:` that
        // materialized the block would read the literal there.
        let send = send_of(&tree.root, "apply:with:");
        let literal = (closure as usize + 1..send as usize)
            .find(|index| {
                matches!(
                    tree.root.instructions[*index],
                    SnapshotInstruction::LoadLiteral { .. }
                )
            })
            .expect("the literal's load lies between the block and the send");
        if let SnapshotInstruction::LoadLiteral { destination, .. } =
            &mut tree.root.instructions[literal]
        {
            *destination = source;
        }
        let plan = plan(&tree, Speculation::Traps);
        inlined_at(&plan, ScopeId::ROOT, send);
        assert_eq!(plan.blocks[&(ScopeId::ROOT, closure)], BlockPlan::Created);
    }

    const SUPERS: &str = indoc! {"
        class Supers = () (
          class Base = () (
            public step: number = (
              ^number + 1
            )
          )
          class Derived = Base () (
            public step: number = (
              ^(super step: number) + 1
            )
          )
          public run = (
            ^Derived new step: 3
          )
        )
    "};

    #[test]
    fn a_super_send_is_inlined_unguarded_where_its_receiver_is_known() {
        let (mut interpreter, class) = run(SUPERS);
        let tree = tree_of(&mut interpreter, &class, "run", &Limits::STANDARD);
        let plan = plan(&tree, Speculation::Traps);
        let derived = inlined_at(&plan, ScopeId::ROOT, send_of(&tree.root, "step:"));
        let derived_snapshot = tree
            .snapshot_of(scope(&plan, derived).code_id)
            .expect("Derived's step:");
        let super_send = send_of(derived_snapshot, "step:");
        let site = &plan.sites[&(derived, super_send)];
        assert_eq!(site.fall_through, FallThrough::Unguarded);
        let base = scope(&plan, inlined_at(&plan, derived, super_send));
        assert_eq!(base.receiver, scope(&plan, derived).receiver);
        assert_ne!(base.entered, scope(&plan, derived).entered);
        assert_eq!(base.entered, Some(site.arms[0].1), "S, the superclass");
    }

    /// Whether `plan` inlines a block's body at the send at
    /// `program_counter` of `scope_id`.
    fn values_a_block_at(plan: &InliningPlan, scope_id: ScopeId, program_counter: u32) -> bool {
        plan.sites[&(scope_id, program_counter)]
            .arms
            .iter()
            .any(|(_, _, arm)| {
                matches!(arm, ArmPlan::Inline { callee } if scope(plan, *callee).block_of.is_some())
            })
    }

    const ONE_WAY: &str = indoc! {"
        class OneWay = () (
          public pick: flag = (
            | b |
            flag ifTrue: [b:: [1]].
            ^b value
          )
          public run = (
            ^self pick: true
          )
        )
    "};

    #[test]
    fn a_block_held_on_one_way_only_is_no_known_receiver() {
        let (mut interpreter, class) = run(ONE_WAY);
        let tree = tree_of(&mut interpreter, &class, "pick:", &Limits::STANDARD);
        let plan = plan(&tree, Speculation::Traps);
        // On the way that skips the assignment `b` is nil, whose `value`
        // is a send of its own: the block's body is no arm taken untested.
        let value = send_of(&tree.root, "value");
        let site = &plan.sites[&(ScopeId::ROOT, value)];
        assert_ne!(site.fall_through, FallThrough::Unguarded, "{site:?}");
        assert!(!values_a_block_at(&plan, ScopeId::ROOT, value));
        let block = closures_of(&tree.root)[0];
        assert_eq!(plan.blocks[&(ScopeId::ROOT, block)], BlockPlan::Created);
    }

    const PARAMETER_WAY: &str = indoc! {"
        class ParameterWay = () (
          public apply: aBlock = (
            ^aBlock value
          )
          public choose: flag = (
            ^self apply: (flag ifTrue: [[1]] ifFalse: [nil])
          )
          public run = (
            ^self choose: true
          )
        )
    "};

    #[test]
    fn a_parameter_that_may_hold_something_else_is_no_known_receiver() {
        let (mut interpreter, class) = run(PARAMETER_WAY);
        let tree = tree_of(&mut interpreter, &class, "choose:", &Limits::STANDARD);
        let plan = plan(&tree, Speculation::Traps);
        let apply = inlined_at(&plan, ScopeId::ROOT, send_of(&tree.root, "apply:"));
        let apply_snapshot = tree
            .snapshot_of(scope(&plan, apply).code_id)
            .expect("apply:'s snapshot");
        let value = send_of(apply_snapshot, "value");
        let site = &plan.sites[&(apply, value)];
        assert_ne!(site.fall_through, FallThrough::Unguarded, "{site:?}");
        assert!(!values_a_block_at(&plan, apply, value));
        let block = closures_of(&tree.root)[0];
        assert_eq!(plan.blocks[&(ScopeId::ROOT, block)], BlockPlan::Created);
    }

    #[test]
    fn a_trapping_type_case_lets_its_argument_block_be_eliminated_only_in_a_speculating_copy() {
        let (mut interpreter, class) = run(VALUES);
        let tree = tree_of(&mut interpreter, &class, "run", &Limits::STANDARD);
        let closure = closure_of(&tree.root);
        let apply = send_of(&tree.root, "apply:");
        let speculating = plan(&tree, Speculation::Traps);
        assert_eq!(
            speculating.sites[&(ScopeId::ROOT, apply)].fall_through,
            FallThrough::Trap
        );
        assert_eq!(
            speculating.blocks[&(ScopeId::ROOT, closure)],
            BlockPlan::Eliminated(EliminatedId(0))
        );
        let cautious = plan(&tree, Speculation::None);
        assert_eq!(
            cautious.sites[&(ScopeId::ROOT, apply)].fall_through,
            FallThrough::Send
        );
        assert_eq!(inlined_at(&cautious, ScopeId::ROOT, apply), ScopeId(1));
        assert_eq!(
            cautious.blocks[&(ScopeId::ROOT, closure)],
            BlockPlan::Created
        );
        assert_eq!(
            cautious.scopes.len(),
            2,
            "the created block's value scope is dropped"
        );
        assert!(
            cautious
                .sites
                .values()
                .all(|site| site.fall_through != FallThrough::Trap)
        );
    }

    const REFERENCE: &str = indoc! {"
        class Reference = () (
          public apply: aBlock = (
            ^aBlock value
          )
          public go: flag = (
            | b |
            b:: [1].
            flag ifTrue: [b value].
            ^self apply: b
          )
          public run = (
            ^self go: true
          )
        )
    "};

    #[test]
    fn a_block_a_nested_block_reaches_by_reference_escapes() {
        let (mut interpreter, class) = run(REFERENCE);
        let tree = tree_of(&mut interpreter, &class, "go:", &Limits::STANDARD);
        let held = closures_of(&tree.root)[0];
        // The not-Boolean way of `ifTrue:` makes a block that reads `b`
        // in its home's register, by reference.
        let readers: Vec<&Snapshot> = closures_of(&tree.root)[1..]
            .iter()
            .filter_map(|closure| match &tree.root.instructions[*closure as usize] {
                SnapshotInstruction::MakeClosure { block, .. } => tree.blocks.get(block),
                _ => None,
            })
            .filter(|block| {
                block.instructions.iter().any(|instruction| {
                    matches!(
                        instruction,
                        SnapshotInstruction::LoadOuterLocal { depth: 1, .. }
                    )
                })
            })
            .collect();
        assert!(!readers.is_empty(), "{:#?}", tree.root.instructions);
        // A copy that speculates nothing builds that way.
        let plan = plan(&tree, Speculation::None);
        inlined_at(&plan, ScopeId::ROOT, send_of(&tree.root, "apply:"));
        assert_eq!(plan.blocks[&(ScopeId::ROOT, held)], BlockPlan::Created);
    }

    /// Where the copy speculates, the not-Boolean way of `ifTrue:`, which
    /// never ran, is a trap, and the block made on it is made by no code
    /// of the compile: what it reaches by reference lets nothing escape.
    #[test]
    fn a_block_a_nested_block_reaches_only_past_a_trap_is_eliminated() {
        let (mut interpreter, class) = run(REFERENCE);
        let tree = tree_of(&mut interpreter, &class, "go:", &Limits::STANDARD);
        let held = closures_of(&tree.root)[0];
        let plan = plan(&tree, Speculation::Traps);
        inlined_at(&plan, ScopeId::ROOT, send_of(&tree.root, "apply:"));
        assert!(
            matches!(
                plan.blocks[&(ScopeId::ROOT, held)],
                BlockPlan::Eliminated(_)
            ),
            "{:#?}",
            plan.blocks
        );
    }

    /// `go:` hands `apply:` a block that the closure of its `ifTrue:` for
    /// a receiver that is no Boolean reads by reference in its home's
    /// register, on a way that ran: `run` sends `go:` a `Flag` too, which
    /// answers `ifTrue:` itself.
    const REACHED_ON_A_WAY_THAT_RUNS: &str = indoc! {"
        class ReachedOnAWayThatRuns = () (
          class Flag = () (
            public ifTrue: aBlock = (
              ^nil
            )
          )
          public apply: aBlock = (
            ^aBlock value
          )
          public go: flag = (
            | b |
            b:: [1].
            flag ifTrue: [b value].
            ^self apply: b
          )
          public run = (
            self go: Flag new.
            ^self go: true
          )
        )
    "};

    /// Where the copy speculates, a block that a nested block made on a
    /// way that ran reaches by reference still escapes: only a way that
    /// ends in a trap is left out of the escape analysis.
    #[test]
    fn a_block_a_nested_block_reaches_on_a_way_that_ran_escapes_where_the_copy_speculates() {
        let (mut interpreter, class) = run(REACHED_ON_A_WAY_THAT_RUNS);
        let tree = tree_of(&mut interpreter, &class, "go:", &Limits::STANDARD);
        let held = closures_of(&tree.root)[0];
        let plan = plan(&tree, Speculation::Traps);
        inlined_at(&plan, ScopeId::ROOT, send_of(&tree.root, "apply:"));
        assert_eq!(plan.blocks[&(ScopeId::ROOT, held)], BlockPlan::Created);
    }

    /// A block handed to a callee that passes it on only on a way that
    /// never ran -- `Array`'s `do:` hands it to `super do:` where the
    /// mirror cannot read the receiver's size -- is eliminated where the
    /// copy speculates: that way is a trap on the send's site.
    const PASSED_ON_A_WAY_NOT_RUN: &str = indoc! {"
        class PassedOnAWayNotRun = () (
          public walk: aBlock over: count = (
            count > 100 ifTrue: [^self keep: aBlock].
            ^aBlock value: count
          )
          public keep: aBlock = (
            ^aBlock
          )
          public go: count = (
            | total ::= 0. |
            self walk: [:each | total:: total + each] over: count.
            ^total
          )
          public run = (
            ^self go: 3
          )
        )
    "};

    #[test]
    fn a_block_passed_on_only_on_a_way_not_run_is_eliminated_where_the_copy_speculates() {
        let (mut interpreter, class) = run(PASSED_ON_A_WAY_NOT_RUN);
        let tree = tree_of(&mut interpreter, &class, "go:", &Limits::STANDARD);
        let held = closure_of(&tree.root);
        let speculating = plan(&tree, Speculation::Traps);
        inlined_at(
            &speculating,
            ScopeId::ROOT,
            send_of(&tree.root, "walk:over:"),
        );
        assert!(
            matches!(
                speculating.blocks[&(ScopeId::ROOT, held)],
                BlockPlan::Eliminated(_)
            ),
            "{:#?}",
            speculating.blocks
        );
        // Without speculation the way is built, and the block escapes
        // into `keep:`.
        let plain = plan(&tree, Speculation::None);
        assert_eq!(plain.blocks[&(ScopeId::ROOT, held)], BlockPlan::Created);
    }

    const COPIED_INTO: &str = indoc! {"
        class CopiedInto = () (
          public apply: aBlock = (
            ^aBlock value
          )
          public twice: inner = (
            ^self apply: [inner value]
          )
          public run = (
            ^self twice: [1]
          )
        )
    "};

    #[test]
    fn a_block_copied_into_an_eliminated_block_is_eliminated() {
        let (mut interpreter, class) = run(COPIED_INTO);
        let tree = tree_of(&mut interpreter, &class, "run", &Limits::STANDARD);
        let plan = plan(&tree, Speculation::Traps);
        let outer = closure_of(&tree.root);
        assert!(matches!(
            plan.blocks[&(ScopeId::ROOT, outer)],
            BlockPlan::Eliminated(_)
        ));
        let twice = inlined_at(&plan, ScopeId::ROOT, send_of(&tree.root, "twice:"));
        let twice_snapshot = tree
            .snapshot_of(scope(&plan, twice).code_id)
            .expect("twice:'s snapshot");
        let copying = closure_of(twice_snapshot);
        let SnapshotInstruction::MakeClosure { copied, .. } =
            &twice_snapshot.instructions[copying as usize]
        else {
            panic!("a closure");
        };
        assert_eq!(copied, &vec![0], "the block copies twice:'s parameter");
        assert!(matches!(
            plan.blocks[&(twice, copying)],
            BlockPlan::Eliminated(_)
        ));
        // The outer block's body is inlined inside the copying block's.
        let copying_scope = plan
            .scopes
            .iter()
            .position(|plan_scope| plan_scope.block_of == Some(twice))
            .expect("the copying block's scope");
        assert!(plan.scopes.iter().any(|plan_scope| {
            plan_scope.block_of == Some(ScopeId::ROOT)
                && plan_scope.parent.map(|(parent, _)| parent.0 as usize) == Some(copying_scope)
        }));
    }
}
