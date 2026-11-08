//! The graph builder (`docs/design/execution.md`, "One tier, an SSA
//! graph, the macro assembler"): one pass over a body's instructions in
//! order, abstractly interpreting its registers -- each register mapped to
//! the value that last defined it -- with a block at every jump target and
//! after every instruction that jumps or stops, and a phi where blocks
//! meet that disagree on a register.
//!
//! Two facts about the body are found before the pass, both from the
//! instructions alone: which registers are live at each block's top, so
//! that a phi is made only for a register some path still reads -- and no
//! frame word is ever read that the interpreter would not read, since a
//! dead register's word may be one a callee left -- and which registers a
//! loop may change, so that its head takes a phi for each of those it
//! needs before its back edges are seen.
//!
//! **Inlined scopes.** A compile that inlines (`build_inlined`) lays its
//! scopes' instructions out as one body before the pass (`Flat`): at a
//! send the plan inlines, each arm's callee's instructions follow the
//! send's type-case, their registers numbered physically -- register `r`
//! of a scope at `offset` is the frame's `offset + r`, where the
//! interpreter would push its activation -- each preceded by the scope's
//! entry, which binds its parameters to the send's arguments, its receiver
//! and current class (`HeaderBinding`) and its named temporaries to `nil`,
//! and each `Return` a jump to the instruction after the send that binds
//! the send's destination, where the returns meet in a phi. The pass then
//! runs over that body as over any other, so every rule above holds of
//! the physical frame.
//!
//! **Blocks.** A block the plan eliminates (`plan::BlockPlan::Eliminated`)
//! is made by nothing: its `MakeClosure` binds its destination to a value
//! no machine code reads (`Node::EliminatedBlock`), `nil` in every frame
//! word bound to it, which the exit that finds the block live replaces with
//! the closure it makes. A `value` send of it is its block's scope, laid
//! out where the interpreter would push the closure's activation, entered
//! as the push enters it -- its parameters the send's arguments, its
//! receiver and current class its home's, its copied values what its
//! home's registers hold, which the plan keeps unwritten while it lives --
//! its outer locals its homes' registers, and its `^` a jump to the return
//! of its home that is no block's, all of them scopes of the compile.
//!
//! **Callees always inlined.** An arm whose callee the plan always
//! inlines (`AlwaysInlined`) -- an accessor, a quick method, a lazy slot's
//! getter, `==`, `class`, `SmallInteger` arithmetic and an `Array`'s or a
//! `ByteArray`'s `at:` and `at:put:` -- is no scope: its step lies in its
//! sender at the send (`Step::Always`), its simple nodes compute what the
//! callee answers, and it binds the send's argument slots and destination
//! as the interpreter's send leaves them. Every check such a callee makes
//! -- a lazy slot unfilled, an index out of bounds, an object of another
//! kind, an operand of no known answer -- is a trap that runs the send
//! again in the interpreter, which decides it and raises its own error;
//! so an arm with a check is expressed only in a copy that speculates,
//! at a site that never trapped (`Laying::expressed`).

use std::collections::HashMap;
use std::ops::Range;

use crate::builtins::primitive::{
    BytePrimitive, CollectionPrimitive, IntegerPrimitive, ObjectPrimitive, Primitive,
};
use crate::bytecode::IntegerOperation;
use crate::heap::{ApplicationIndex, Immediate};
use crate::mixin::{DispatchId, QuickConstant};

use super::graph::{
    tainted_at, Block, BlockId, ClassRecipe, ClassStart, End, Graph, GraphScope, HeaderBinding,
    IndexedKind, Node, Phi, ScopeId, ValueId, ValueKind, FRAME_BOUND,
};
use super::plan::{AlwaysInlined, ArmPlan, BlockPlan, EliminatedId, FallThrough, InliningPlan};
use super::snapshot::{Snapshot, SnapshotInstruction};
use super::tree::SnapshotTree;
use super::verify::immediate_dominators;
use super::Speculation;

/// The input of a loop head's phi whose back edge is not seen yet.
const UNFILLED: ValueId = ValueId(u32::MAX);

/// The first absolute field index a customized copy does not read as a
/// simple node: past it a field's displacement from its object might not
/// fit a load's, and the template, which computes the address, reads it.
const FIXED_FIELD_LIMIT: u32 = 1 << 24;

/// The graph of `snapshot`'s body. Only the blocks reachable from the
/// body's first instruction are built: an unreachable block has no
/// register state to start from -- nothing defines its registers -- and
/// never runs, so keeping it would mean inventing one. A body whose first
/// instruction heads a loop is given an entry block of its own, with no
/// instruction, so that the head's phis have an input for the entry.
///
/// The head of a loop of the program's leaves a register its frame word,
/// with no phi, wherever every way in from before the loop brings a value
/// that frame word holds, or the loop may change it; the back edges, built later, must bring one, and
/// a head one refutes is built again with the phi (`Builder::forced_phis`),
/// until none does. The rule decides what the code loads and stores, never
/// what it computes: a way that brings a value its frame word does not hold
/// writes it back on its way in (`liveness::join`).
///
/// Under `Speculation::Traps` a site the feedback says never ran is a
/// guard (`Guard`): an integer fast path whose fallback never ran has no
/// fallback, a conditional whose non-Boolean way never ran no third way,
/// and a send never executed ends its block; each traps where the
/// speculation fails. A value's kind (`ValueKind`) is tracked, so that a
/// known `SmallInteger` operand tests no tag and a known Boolean
/// condition has no third way: `JumpIfNotBoolean`'s, a block of its own,
/// is dropped by building the graph again without it, until no known
/// Boolean condition is left with one. Under `Speculation::None` the graph
/// is stage 1's: every fallback, third way and send kept, and every value
/// of no known kind. The JIT builds every body as `build_inlined`'s root,
/// which lays one that inlines nothing out as this does.
#[cfg(test)]
pub(crate) fn build(snapshot: &Snapshot, speculation: Speculation) -> Graph {
    build_flat(&Flat::of_snapshot(snapshot), speculation)
}

/// The graph of `tree`'s root with the scopes `plan` inlines
/// (`docs/design/execution.md`, "Inlining"), laid out as one body
/// (`Flat::of_plan`) and built as `build` builds one. `speculation` is the
/// one the plan was made under: a type-case falls through to a trap only
/// where the plan says, which it says only for a copy that speculates.
/// A site whose arms the graph does not express yet -- an arm always
/// inlined, a lexical send's past depth 0, whose receiver the enclosing
/// object's walk finds -- is its send.
pub(crate) fn build_inlined(
    tree: &SnapshotTree,
    plan: &InliningPlan,
    speculation: Speculation,
) -> Graph {
    debug_assert!(
        speculation == Speculation::Traps
            || plan
                .sites
                .values()
                .all(|site| site.fall_through != FallThrough::Trap),
        "internal: a copy that speculates nothing is planned with no trap"
    );
    build_flat(&Flat::of_plan(tree, plan, speculation), speculation)
}

/// The graph of `flat`, built as `build` says.
fn build_flat(flat: &Flat<'_>, speculation: Speculation) -> Graph {
    let count = flat.steps.len();
    let mut forced = None;
    // The conditionals built without their third way, their condition
    // being a known Boolean; and those a build refuted, which keep it.
    let mut known_boolean = vec![false; count];
    let mut refuted_boolean = vec![false; count];
    let graph = loop {
        let mut builder = Builder::new(flat, speculation, &known_boolean);
        if let Some(forced) = forced.take() {
            builder.forced_phis = forced;
        }
        let guards = builder.guards.clone();
        let mut graph = match builder.build() {
            Ok(graph) => graph,
            Err(more) => {
                forced = Some(more);
                continue;
            }
        };
        if speculation == Speculation::None {
            graph.forget_kinds();
            break graph;
        }
        let (more, refuted) = drop_boolean_third_ways(&mut graph, flat, &known_boolean, &guards);
        let more: Vec<usize> = more
            .into_iter()
            .filter(|index| !refuted_boolean[*index])
            .collect();
        if more.is_empty() && refuted.is_empty() {
            break graph;
        }
        for index in more {
            known_boolean[index] = true;
        }
        for index in refuted {
            known_boolean[index] = false;
            refuted_boolean[index] = true;
        }
        // The blocks are laid out afresh, so the phis an earlier build
        // forced name other blocks.
        forced = None;
    };
    debug_assert!(
        super::verify::verify(&graph).is_ok(),
        "internal: the built graph is refused: {:?}",
        super::verify::verify(&graph)
    );
    graph
}

/// Drops the third way of every branch of `graph` whose condition is a
/// known Boolean: in place where it is one of the other two ways, which
/// leaves the edges as they are. Answers the steps of `flat` -- the
/// `JumpIfNotBoolean`s -- whose third way is a block of its own, which the
/// next build lays out without it, and of those `known_boolean` built
/// without one whose condition is no known Boolean after all and which no
/// guard of theirs (`guards`) traps for, which the next build gives it
/// back: a known Boolean seen in one build may not survive the blocks the
/// next lays out differently.
fn drop_boolean_third_ways(
    graph: &mut Graph,
    flat: &Flat<'_>,
    known_boolean: &[bool],
    guards: &[Guard],
) -> (Vec<usize>, Vec<usize>) {
    let kinds: Vec<ValueKind> = graph.values.iter().map(|value| value.kind).collect();
    let mut more = Vec::new();
    let mut refuted = Vec::new();
    for block in &mut graph.blocks {
        let scope = block.scope;
        let End::Branch {
            program_counter,
            condition,
            if_true,
            if_false,
            not_boolean,
        } = &mut block.end
        else {
            continue;
        };
        let index = flat.index_of(scope, *program_counter);
        let boolean = kinds[condition.index()] == ValueKind::Boolean;
        match *not_boolean {
            Some(third) if boolean => {
                if third == *if_true || third == *if_false {
                    *not_boolean = None;
                } else {
                    more.push(index);
                }
            }
            None if !boolean && known_boolean[index] && guards[index] != Guard::NotBoolean => {
                refuted.push(index);
            }
            _ => {}
        }
    }
    (more, refuted)
}

/// `build` under `Speculation::Traps`, as the JIT compiles: the passes'
/// tests' hand-written snapshots record no site, so that it builds no
/// guard in them.
#[cfg(test)]
pub(crate) fn build_speculating(snapshot: &Snapshot) -> Graph {
    build(snapshot, Speculation::Traps)
}

/// What the feedback lets the graph speculate at an instruction
/// (`docs/design/execution.md`, "Speculation", "Guards and traps"): only at a
/// site that neither ran nor trapped (`SiteFacts`), and only under
/// `Speculation::Traps`.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Guard {
    /// The instruction keeps every way stage 1 gives it.
    #[default]
    None,
    /// An integer fast path whose fallback never ran: its failure traps.
    Fallback,
    /// A `JumpIfNotBoolean` whose non-Boolean way never ran: any other
    /// value traps.
    NotBoolean,
    /// A send never executed: the send itself traps.
    Send { site: u32 },
}

/// What a step becomes in the graph.
enum Kind {
    Constant {
        destination: u16,
        word: u64,
    },
    Receiver {
        destination: u16,
    },
    Copy {
        destination: u16,
        source: u16,
    },
    IntegerBinary {
        operation: IntegerOperation,
        destination: u16,
        left: u16,
        right: u16,
        /// Whether the fast path's failure traps (`Guard::Fallback`).
        trap: bool,
    },
    Return {
        register: u16,
    },
    /// A customized copy's own-slot read (`Node::OwnSlotLoad`).
    OwnSlotLoad {
        destination: u16,
        absolute_index: u32,
    },
    /// A customized copy's `ReturnSlotIfFilled` (`End::ReturnUnlessNil`).
    ReturnUnlessNil {
        absolute_index: u32,
    },
    Jump,
    /// A branch instruction, which goes to its target for one outcome and
    /// to the next instruction for the other two.
    Branch {
        register: u16,
        taken: Outcome,
        /// Whether the branch keeps its third way: not where its
        /// non-Boolean way never ran, nor where the condition is a known
        /// Boolean (`build`).
        third_way: bool,
    },
    /// A send never executed (`Guard::Send`).
    Trap {
        site: u32,
    },
    Frame,
    /// An inlined send's type-case (`Step::TypeCase`).
    TypeCase,
    /// An inlined scope's entry (`Step::Enter`).
    Enter,
    /// An inlined scope's return (`Step::Return`).
    InlinedReturn,
    /// A callee always inlined (`Step::Always`).
    Always,
    /// An eliminated block's `MakeClosure` (`Step::Eliminated`).
    EliminatedBlock {
        destination: u16,
        block: EliminatedId,
    },
}

/// The outcome of a Boolean test a branch instruction jumps for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Outcome {
    True,
    False,
    NotBoolean,
}

/// What `instruction`, the step at `index`, becomes. Every instruction
/// not named is a frame node, emitted by the baseline's template for it,
/// which is right for any instruction; a simple node is a choice made
/// here, one instruction at a time.
///
/// An `IntegerBinary` whose fallback is the way taken (`send_only`,
/// `Snapshot::fallback_taken`) is a jump to its fallback: the interpreter's
/// instruction, failing, writes nothing and goes there, so the frame at the
/// fallback send is the one the interpreter has there, and no fast path,
/// diamond or merge is built for a test the program's numbers fail.
///
/// In a customized copy, whose entered application's own slots start at
/// `own_slot_base`, an own-slot read is a load at a fixed field
/// (`own_slot_index`); `InitializeSlot` and `InitializeSlotProvisionally`
/// keep their templates, since an initializer runs once per instance.
///
/// A guard (`Choice::guard`) makes a send never executed a trap, an
/// integer fast path's failure a trap rather than a way to its fallback,
/// and a conditional's non-Boolean value a trap rather than its third way;
/// a conditional whose condition is a known Boolean has no third way at
/// all (`Choice::known_boolean`).
fn kind_of(
    instruction: &SnapshotInstruction,
    index: usize,
    choice: Choice,
    own_slot_base: Option<u32>,
) -> Kind {
    if let Guard::Send { site } = choice.guard {
        return Kind::Trap { site };
    }
    match instruction {
        SnapshotInstruction::IntegerBinary { .. } if choice.send_only => Kind::Jump,
        SnapshotInstruction::LoadOwnSlot {
            destination,
            own_index,
        } => match own_slot_index(own_slot_base, *own_index) {
            Some(absolute_index) => Kind::OwnSlotLoad {
                destination: *destination,
                absolute_index,
            },
            None => Kind::Frame,
        },
        SnapshotInstruction::ReturnSlotIfFilled { own_index } => {
            match own_slot_index(own_slot_base, *own_index) {
                Some(absolute_index) => Kind::ReturnUnlessNil { absolute_index },
                None => Kind::Frame,
            }
        }
        SnapshotInstruction::LoadLiteral {
            destination,
            literal,
        } => match crate::jit::immediate_literal(literal) {
            Some(immediate) => Kind::Constant {
                destination: *destination,
                word: immediate.bits(),
            },
            None => Kind::Frame,
        },
        SnapshotInstruction::LoadSelf { destination } => Kind::Receiver {
            destination: *destination,
        },
        SnapshotInstruction::Move {
            destination,
            source,
        }
        | SnapshotInstruction::StoreLocal {
            source,
            slot: destination,
        } => Kind::Copy {
            destination: *destination,
            source: *source,
        },
        // A fast path whose fallback is the next instruction would leave
        // both ways into one block with the destination bound on one and
        // not the other, which one phi input per predecessor cannot say;
        // no lowering makes one, and the template does it exactly.
        SnapshotInstruction::IntegerBinary {
            operation,
            destination,
            left,
            right,
            target,
        } if usize::try_from(*target).ok() != index.checked_add(1) => Kind::IntegerBinary {
            operation: *operation,
            destination: *destination,
            left: *left,
            right: *right,
            trap: choice.guard == Guard::Fallback,
        },
        SnapshotInstruction::Return { register }
        | SnapshotInstruction::ImplicitReturn { register } => Kind::Return {
            register: *register,
        },
        SnapshotInstruction::Jump { .. } => Kind::Jump,
        SnapshotInstruction::JumpIfNotBoolean { register, .. } => Kind::Branch {
            register: *register,
            taken: Outcome::NotBoolean,
            third_way: choice.guard != Guard::NotBoolean && !choice.known_boolean,
        },
        SnapshotInstruction::BranchIfFalse { register, .. } => Kind::Branch {
            register: *register,
            taken: Outcome::False,
            third_way: true,
        },
        SnapshotInstruction::BranchIfTrue { register, .. } => Kind::Branch {
            register: *register,
            taken: Outcome::True,
            third_way: true,
        },
        _ => Kind::Frame,
    }
}

/// The absolute field index of own slot `own_index` in a copy whose
/// entered application's own slots start at `own_slot_base`, if the copy
/// reads it as a simple node: with no customization the template finds
/// the base and checks the guard.
fn own_slot_index(own_slot_base: Option<u32>, own_index: u32) -> Option<u32> {
    own_slot_base?
        .checked_add(own_index)
        .filter(|index| *index < FIXED_FIELD_LIMIT)
}

/// What decides an instruction's kind beside the instruction itself.
#[derive(Clone, Copy, Default)]
struct Choice {
    /// Whether its fast path is lowered to its fallback alone
    /// (`Snapshot::fallback_taken`).
    send_only: bool,
    guard: Guard,
    /// Whether it is a `JumpIfNotBoolean` whose condition an earlier
    /// build found to be a known Boolean.
    known_boolean: bool,
}

impl Choice {
    /// Whether the graph keeps the instruction's jump target: not an
    /// integer fast path's whose failure traps, nor a conditional's third
    /// way where it has none.
    fn keeps_target(self) -> bool {
        self.guard != Guard::Fallback && self.guard != Guard::NotBoolean && !self.known_boolean
    }

    /// Whether control may go on at the next instruction: not after a
    /// jump to the fallback alone, nor after a send that traps.
    fn may_fall_through(self) -> bool {
        !self.send_only && !matches!(self.guard, Guard::Send { .. })
    }
}

/// The guard the feedback lets the instruction at `index` of `snapshot`
/// be under `speculation` (`Guard`): only at a site that neither ran nor
/// trapped, since a site a speculation trapped at may have run unrecorded
/// -- a send resolved to `doesNotUnderstand:` fills no cache -- and
/// speculating on it again would trap again.
fn guard_of(
    snapshot: &Snapshot,
    (index, inlined): (usize, bool),
    send_only: bool,
    speculation: Speculation,
) -> Guard {
    if speculation == Speculation::None || send_only {
        return Guard::None;
    }
    // A block an inlined scope creates where its send never ran: a
    // `MakeClosure` there would leave the copy at every run of the way,
    // and traps on the send's site instead, as the way's send would, so
    // that a way that starts running retires the copy and its next
    // compile sends the callee (`plan::Planner::creates_a_block`).
    if let SnapshotInstruction::MakeClosure { destination, .. } = &snapshot.instructions[index] {
        return match super::plan::never_run_consumer(snapshot, index, *destination) {
            Some(site) if inlined => Guard::Send { site },
            _ => Guard::None,
        };
    }
    let Some(facts) = snapshot.sites.get(index).copied().flatten() else {
        return Guard::None;
    };
    if facts.executed || facts.trapped {
        return Guard::None;
    }
    match &snapshot.instructions[index] {
        // Only a fast path has a failure to trap: one whose fallback is
        // the next instruction keeps its template (`kind_of`).
        SnapshotInstruction::IntegerBinary { target, .. }
            if usize::try_from(*target).ok() != index.checked_add(1) =>
        {
            Guard::Fallback
        }
        SnapshotInstruction::JumpIfNotBoolean { .. } => Guard::NotBoolean,
        SnapshotInstruction::SendLexical { .. }
        | SnapshotInstruction::SendPublic { .. }
        | SnapshotInstruction::SendInlinable { .. }
        | SnapshotInstruction::SendSuper { .. } => Guard::Send { site: facts.site },
        _ => Guard::None,
    }
}

/// Where an inlined send's receiver is: in a physical register, or, for
/// a send to `self`, the receiver of the scope that sends it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReceiverOf {
    Register(u16),
    SelfOf(ScopeId),
}

/// An inlined scope's entry (`Step::Enter`): what it binds before its
/// first instruction, every register physical.
#[derive(Debug, Clone, PartialEq)]
struct Entry {
    receiver: ReceiverOf,
    /// Each parameter's register and the sender's register its argument
    /// is in, the send's argument source.
    arguments: Vec<(u16, u16)>,
    /// Where its current class's steps start: at the class of the
    /// receiver, or at the sender's current class, in the register given.
    class_start: ClassStart,
    sender_class: Option<u16>,
    /// The scope whose current class `sender_class` holds.
    class_source: Option<ScopeId>,
    class_steps: u16,
    receiver_register: u16,
    class_register: u16,
    /// The named temporaries, which a push fills with `nil`.
    temporaries: Range<u16>,
    /// What the callee's frame lays over that it binds nothing in
    /// (`Node::Overlay`).
    overlay: Vec<Range<u16>>,
}

/// One step of a flat body (`Flat`).
#[derive(Debug, Clone, PartialEq)]
enum Step {
    /// An instruction of the step's scope: `physical` numbers its
    /// registers physically and its jump targets as steps, `original` is
    /// as the scope's body holds it.
    Instruction {
        physical: SnapshotInstruction,
        original: SnapshotInstruction,
    },
    /// A send lowered as its arms' callees inlined (`End::TypeCase`): the
    /// step of each arm's entry, and of the send itself, `None` for a trap.
    TypeCase {
        receiver: ReceiverOf,
        arms: Vec<(DispatchId, usize)>,
        fall_through: Option<usize>,
        site: u32,
    },
    /// An inlined scope's entry; it goes on at its first instruction.
    Enter(Box<Entry>),
    /// An inlined scope's `Return`: `source`'s value bound to the send's
    /// `destination`, the registers from `slot_end` on but that one left
    /// to the sender as the callee leaves them, and on at `continuation`,
    /// the step of the instruction after the send.
    Return {
        source: u16,
        destination: u16,
        slot_end: u16,
        continuation: usize,
    },
    /// A callee always inlined at its sender's send: an arm of the send's
    /// type-case, or the send itself where it is taken without a test.
    Always(Box<AlwaysStep>),
    /// A `MakeClosure` whose block the compile eliminates: `destination`
    /// bound to the block (`Node::EliminatedBlock`), nothing made.
    Eliminated {
        destination: u16,
        block: EliminatedId,
    },
}

/// A callee always inlined (`Step::Always`), every register physical: what
/// it computes from the send's receiver and its arguments, read from
/// their sources; the slots the send lays them on, bound to them as the
/// interpreter's send leaves them; the send's destination; and the step of
/// the instruction after the send, where it goes on.
#[derive(Debug, Clone, PartialEq)]
struct AlwaysStep {
    expressed: Expressed,
    receiver: ReceiverOf,
    /// Each argument's slot and the sender's register it is in.
    arguments: Vec<(u16, u16)>,
    destination: u16,
    continuation: usize,
}

/// What a callee always inlined computes, as the graph expresses it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expressed {
    /// A getter's load of its receiver's field.
    FieldLoad { field: u32 },
    /// A setter's store into its receiver's field.
    FieldStore { field: u32 },
    /// A lazy slot's getter: its field, unless it is `nil`.
    LazyGetter { field: u32 },
    /// A quick method's constant word.
    Constant(u64),
    /// `^self`.
    Receiver,
    /// `^` the argument at this position.
    Parameter(usize),
    /// `==`.
    Identity,
    /// `class`.
    ClassOf,
    /// `at:`.
    IndexedLoad(IndexedKind),
    /// `at:put:`.
    IndexedStore(IndexedKind),
    /// `SmallInteger` arithmetic or comparison.
    Integer(IntegerOperation),
}

/// A step and where it comes from: its scope, and its program counter in
/// the scope's body -- an entry's is 0, a type-case's its send's.
#[derive(Debug, Clone, PartialEq)]
struct FlatStep {
    scope: ScopeId,
    program_counter: u32,
    step: Step,
}

/// One scope of a flat body: its snapshot and the own-slot base its
/// receiver's application fixes, if it is known.
struct FlatScope<'snapshot> {
    snapshot: &'snapshot Snapshot,
    own_slot_base: Option<u32>,
}

/// A compile's scopes laid out as one body of steps, every register
/// physical, which the builder builds as it builds one body: a body
/// alone is its instructions; a plan's scopes are laid out at their sends
/// (`of_plan`).
struct Flat<'snapshot> {
    steps: Vec<FlatStep>,
    scopes: Vec<FlatScope<'snapshot>>,
    graph_scopes: Vec<GraphScope>,
    /// Each scope's step of each of its instructions, `usize::MAX` for one
    /// not laid out.
    steps_of: Vec<Vec<usize>>,
    /// The physical registers the leftover analysis taints before each
    /// step (`graph::tainted_at`).
    tainted: Vec<Vec<Range<u16>>>,
    register_count: usize,
    /// The applications the arms always inlined were decided for
    /// (`Graph::arm_applications`).
    arm_applications: Vec<ApplicationIndex>,
    /// The sites laid out as their arms (`Graph::inlined_sites`).
    inlined_sites: u32,
}

impl<'snapshot> Flat<'snapshot> {
    /// `snapshot`'s body alone: each instruction a step, at its own index.
    #[cfg(test)]
    fn of_snapshot(snapshot: &'snapshot Snapshot) -> Flat<'snapshot> {
        let steps = snapshot
            .instructions
            .iter()
            .enumerate()
            .map(|(index, instruction)| FlatStep {
                scope: ScopeId::ROOT,
                program_counter: u32::try_from(index)
                    .expect("internal: an instruction index fits a u32"),
                step: Step::Instruction {
                    physical: instruction.clone(),
                    original: instruction.clone(),
                },
            })
            .collect();
        let register_count = snapshot.register_count();
        Flat {
            steps,
            scopes: vec![FlatScope {
                snapshot,
                own_slot_base: snapshot
                    .customization
                    .map(|customization| customization.own_slot_base),
            }],
            graph_scopes: vec![graph_scope_of(snapshot, None, 0, None, Vec::new())],
            steps_of: vec![(0..snapshot.instructions.len()).collect()],
            // A test's snapshot may name fewer sets than instructions: the
            // rest taint nothing.
            tainted: (0..snapshot.instructions.len())
                .map(|index| {
                    snapshot
                        .tainted_before
                        .get(index)
                        .cloned()
                        .unwrap_or_default()
                })
                .collect(),
            register_count,
            arm_applications: Vec::new(),
            inlined_sites: 0,
        }
    }

    /// The scopes of `plan` over `tree` laid out as one body: each scope's
    /// instructions in order, and at a send the plan inlines its type-case,
    /// then each arm's entry and callee, then the send itself if the
    /// type-case falls through to it, and then the instruction after the
    /// send, where every return of every arm goes on. A send taken
    /// without a test lays its one callee out right where it stands.
    fn of_plan(
        tree: &'snapshot SnapshotTree,
        plan: &InliningPlan,
        speculation: Speculation,
    ) -> Flat<'snapshot> {
        let snapshots: Vec<&Snapshot> = plan
            .scopes
            .iter()
            .map(|scope| {
                tree.snapshot_of(scope.code_id)
                    .expect("internal: the tree holds every planned scope's snapshot")
            })
            .collect();
        let graph_scopes: Vec<GraphScope> = plan
            .scopes
            .iter()
            .zip(&snapshots)
            .map(|(scope, snapshot)| {
                graph_scope_of(
                    snapshot,
                    scope.parent,
                    scope.offset,
                    scope.block,
                    scope.eliminated_before.clone(),
                )
            })
            .collect();
        let mut laying = Laying {
            tree,
            plan,
            speculation,
            snapshots: &snapshots,
            graph_scopes: &graph_scopes,
            steps: Vec::new(),
            steps_of: snapshots
                .iter()
                .map(|snapshot| vec![usize::MAX; snapshot.instructions.len()])
                .collect(),
            entries: vec![usize::MAX; snapshots.len()],
            sends: HashMap::new(),
            arm_applications: Vec::new(),
            inlined_sites: 0,
        };
        laying.lay_out();
        let Laying {
            steps,
            steps_of,
            entries,
            sends,
            arm_applications,
            inlined_sites,
            ..
        } = laying;
        let mut graph_scopes = graph_scopes;
        for (scope, _, pending) in &steps {
            if let Pending::Enter(entry) = pending {
                graph_scopes[scope.0 as usize].class = Some(ClassRecipe {
                    from: entry.class_source,
                    start: entry.class_start,
                    steps: entry.class_steps,
                });
            }
        }
        let steps: Vec<FlatStep> = steps
            .into_iter()
            .map(|(scope, program_counter, pending)| {
                let step_of = |program_counter: u32| {
                    let step = steps_of[scope.0 as usize][program_counter as usize];
                    debug_assert_ne!(step, usize::MAX, "internal: a jump target is laid out");
                    u32::try_from(step).expect("internal: a compile's steps number fewer than 2^32")
                };
                let step = match pending {
                    Pending::Instruction {
                        mut physical,
                        original,
                    } => {
                        retarget(&mut physical, step_of);
                        Step::Instruction { physical, original }
                    }
                    Pending::TypeCase {
                        receiver,
                        arms,
                        sent,
                        site,
                    } => Step::TypeCase {
                        receiver,
                        arms: arms
                            .into_iter()
                            .map(|(guard, target)| match target {
                                ArmTarget::Scope(callee) => (guard, entries[callee.0 as usize]),
                                ArmTarget::Step(step) => (guard, step),
                            })
                            .collect(),
                        fall_through: sent.then(|| sends[&(scope, program_counter)]),
                        site,
                    },
                    Pending::Enter(entry) => Step::Enter(entry),
                    Pending::Return {
                        source,
                        destination,
                        slot_end,
                        sender,
                        call,
                    } => Step::Return {
                        source,
                        destination,
                        slot_end,
                        continuation: steps_of[sender.0 as usize][call as usize + 1],
                    },
                    Pending::Always(mut always) => {
                        always.continuation =
                            steps_of[scope.0 as usize][program_counter as usize + 1];
                        Step::Always(always)
                    }
                    Pending::Eliminated { destination, block } => {
                        Step::Eliminated { destination, block }
                    }
                };
                FlatStep {
                    scope,
                    program_counter,
                    step,
                }
            })
            .collect();
        let tainted = steps
            .iter()
            .map(|step| {
                let (scope, program_counter) = taint_point_of(&graph_scopes, step);
                tainted_at(&graph_scopes, scope, program_counter)
            })
            .collect();
        let register_count = graph_scopes
            .iter()
            .map(|scope| usize::from(scope.top()))
            .max()
            .unwrap_or(0);
        Flat {
            steps,
            scopes: plan
                .scopes
                .iter()
                .zip(&snapshots)
                .map(|(scope, snapshot)| FlatScope {
                    snapshot,
                    own_slot_base: scope.own_slot_base,
                })
                .collect(),
            graph_scopes,
            steps_of,
            tainted,
            register_count,
            arm_applications,
            inlined_sites,
        }
    }

    /// The scope and instruction whose leftover analysis holds before the
    /// step at `index` (`Block::taint_point`): an inlined scope's entry's
    /// is its sender's send, its frame not laid yet.
    fn taint_point(&self, index: usize) -> (ScopeId, u32) {
        taint_point_of(&self.graph_scopes, &self.steps[index])
    }

    /// The step of the instruction at `program_counter` of `scope`.
    fn index_of(&self, scope: ScopeId, program_counter: u32) -> usize {
        self.steps_of[scope.0 as usize][program_counter as usize]
    }

    /// The scope `scope` names.
    fn scope(&self, scope: ScopeId) -> &FlatScope<'snapshot> {
        &self.scopes[scope.0 as usize]
    }
}

/// The physical register `register` of a scope at `offset`.
fn physical(offset: u16, register: u16) -> u16 {
    offset.checked_add(register).expect(FRAME_BOUND)
}

/// `Flat::taint_point` of `step`, among `scopes`.
fn taint_point_of(scopes: &[GraphScope], step: &FlatStep) -> (ScopeId, u32) {
    match (&step.step, scopes[step.scope.0 as usize].parent) {
        (Step::Enter(_), Some(sent_at)) => sent_at,
        _ => (step.scope, step.program_counter),
    }
}

/// The graph's scope of `snapshot`'s body at `offset`, sent from `parent`,
/// running the eliminated block `block` if it is a block's, the blocks
/// `eliminated_before` says held before each of its instructions.
fn graph_scope_of(
    snapshot: &Snapshot,
    parent: Option<(ScopeId, u32)>,
    offset: u16,
    block: Option<EliminatedId>,
    eliminated_before: Vec<Vec<(u16, EliminatedId)>>,
) -> GraphScope {
    GraphScope {
        code_id: snapshot.code_id,
        parent,
        offset,
        argument_count: snapshot.facts.argument_count,
        register_count: u16::try_from(snapshot.register_count())
            .expect("internal: a body names at most u16::MAX registers"),
        tainted_before: snapshot.tainted_before.clone(),
        block,
        eliminated_before,
        class: None,
    }
}

/// A step laid out whose targets are not numbered yet: they name a
/// scope's instruction, entry or send.
enum Pending {
    Instruction {
        physical: SnapshotInstruction,
        original: SnapshotInstruction,
    },
    TypeCase {
        receiver: ReceiverOf,
        arms: Vec<(DispatchId, ArmTarget)>,
        sent: bool,
        site: u32,
    },
    Enter(Box<Entry>),
    Return {
        source: u16,
        destination: u16,
        slot_end: u16,
        sender: ScopeId,
        call: u32,
    },
    /// Its continuation not numbered yet: the step after its send's.
    Always(Box<AlwaysStep>),
    Eliminated {
        destination: u16,
        block: EliminatedId,
    },
}

/// Where a type-case's arm goes: the entry of its callee's scope, or the
/// step of its callee always inlined.
enum ArmTarget {
    Scope(ScopeId),
    Step(usize),
}

/// An arm as it is laid out: its callee inlined as a scope, or always
/// inlined at the send.
enum LaidArm {
    Scope(ScopeId),
    Always(Box<AlwaysStep>),
}

/// What is left to lay out: a scope's instructions from one on, or the
/// send a type-case falls through to.
enum Work {
    Scope {
        scope: ScopeId,
        program_counter: u32,
    },
    Send {
        scope: ScopeId,
        program_counter: u32,
    },
}

/// How a send the plan inlines is laid out.
enum Lowered {
    /// Its one callee, taken without a test.
    Unguarded(LaidArm),
    /// A type-case over its arms' callees, falling through to the send
    /// itself if `sent`, to a trap otherwise.
    TypeCase {
        receiver: ReceiverOf,
        arms: Vec<(DispatchId, LaidArm)>,
        sent: bool,
    },
}

/// `Flat::of_plan`'s work.
struct Laying<'plan, 'snapshot> {
    tree: &'plan SnapshotTree,
    plan: &'plan InliningPlan,
    speculation: Speculation,
    snapshots: &'plan [&'snapshot Snapshot],
    graph_scopes: &'plan [GraphScope],
    steps: Vec<(ScopeId, u32, Pending)>,
    steps_of: Vec<Vec<usize>>,
    /// Each inlined scope's entry step.
    entries: Vec<usize>,
    /// The step of each send a type-case falls through to.
    sends: HashMap<(ScopeId, u32), usize>,
    /// The applications of the arms always inlined laid out, each once.
    arm_applications: Vec<ApplicationIndex>,
    /// The sites laid out as their arms.
    inlined_sites: u32,
}

impl Laying<'_, '_> {
    /// Lays every scope out, from the root's first instruction, by a
    /// stack of what is left rather than a recursion, which the chain of
    /// inlined scopes could take deeper than the native stack allows.
    fn lay_out(&mut self) {
        let mut work = vec![Work::Scope {
            scope: ScopeId::ROOT,
            program_counter: 0,
        }];
        while let Some(item) = work.pop() {
            match item {
                Work::Send {
                    scope,
                    program_counter,
                } => {
                    self.sends
                        .insert((scope, program_counter), self.steps.len());
                    let pending = self.instruction(scope, program_counter);
                    self.steps.push((scope, program_counter, pending));
                }
                Work::Scope {
                    scope,
                    program_counter,
                } => {
                    let snapshot = self.snapshots[scope.0 as usize];
                    if program_counter == 0 && scope != ScopeId::ROOT {
                        self.entries[scope.0 as usize] = self.steps.len();
                        let entry = self.entry(scope);
                        self.steps.push((scope, 0, Pending::Enter(Box::new(entry))));
                    }
                    let index = program_counter as usize;
                    if index == snapshot.instructions.len() {
                        assert!(
                            index == 0 || !snapshot.instructions[index - 1].falls_through(),
                            "internal: a body does not run off its end"
                        );
                        continue;
                    }
                    self.steps_of[scope.0 as usize][index] = self.steps.len();
                    work.push(Work::Scope {
                        scope,
                        program_counter: program_counter + 1,
                    });
                    let lowered = self.lowered(scope, program_counter);
                    if lowered.is_some() {
                        self.inlined_sites += 1;
                    }
                    match lowered {
                        None => {
                            let pending = self.instruction(scope, program_counter);
                            self.steps.push((scope, program_counter, pending));
                        }
                        Some(Lowered::Unguarded(LaidArm::Scope(callee))) => {
                            work.push(Work::Scope {
                                scope: callee,
                                program_counter: 0,
                            });
                        }
                        Some(Lowered::Unguarded(LaidArm::Always(always))) => {
                            self.steps
                                .push((scope, program_counter, Pending::Always(always)));
                        }
                        Some(Lowered::TypeCase {
                            receiver,
                            arms,
                            sent,
                        }) => {
                            let site = snapshot.sites[index]
                                .expect("internal: a planned send names its site")
                                .site;
                            if sent {
                                work.push(Work::Send {
                                    scope,
                                    program_counter,
                                });
                            }
                            // The arms always inlined lie right after the
                            // type-case, in its arms' order; the scopes'
                            // entries follow them.
                            let mut next_step = self.steps.len() + 1;
                            let mut targets = Vec::with_capacity(arms.len());
                            let mut always_steps = Vec::new();
                            let mut callees = Vec::new();
                            for (guard, arm) in arms {
                                match arm {
                                    LaidArm::Scope(callee) => {
                                        targets.push((guard, ArmTarget::Scope(callee)));
                                        callees.push(callee);
                                    }
                                    LaidArm::Always(always) => {
                                        targets.push((guard, ArmTarget::Step(next_step)));
                                        next_step += 1;
                                        always_steps.push(always);
                                    }
                                }
                            }
                            for callee in callees.into_iter().rev() {
                                work.push(Work::Scope {
                                    scope: callee,
                                    program_counter: 0,
                                });
                            }
                            self.steps.push((
                                scope,
                                program_counter,
                                Pending::TypeCase {
                                    receiver,
                                    arms: targets,
                                    sent,
                                    site,
                                },
                            ));
                            for always in always_steps {
                                self.steps
                                    .push((scope, program_counter, Pending::Always(always)));
                            }
                        }
                    }
                }
            }
        }
    }

    /// How the send at `program_counter` of `scope` is laid out, if the
    /// plan inlines it and the graph expresses its arms: an arm always
    /// inlined the graph does not express (`expressed`) is not taken, so a
    /// site with one falls through to its send; a lexical send past depth
    /// 0, whose receiver the walk out to its enclosing object finds, a
    /// frame node's, is sent.
    fn lowered(&mut self, scope: ScopeId, program_counter: u32) -> Option<Lowered> {
        let site = self.plan.sites.get(&(scope, program_counter))?;
        let offset = self.graph_scopes[scope.0 as usize].offset;
        let instruction = &self.snapshots[scope.0 as usize].instructions[program_counter as usize];
        let receiver = match instruction {
            SnapshotInstruction::SendPublic {
                receiver_register, ..
            }
            | SnapshotInstruction::SendInlinable {
                receiver_register, ..
            } => Some(ReceiverOf::Register(physical(offset, *receiver_register))),
            // A lexical send at depth 0 and a `super` send are the sender's
            // own receiver's.
            SnapshotInstruction::SendLexical { depth: 0, .. }
            | SnapshotInstruction::SendSuper { .. } => Some(ReceiverOf::SelfOf(scope)),
            _ => None,
        };
        let mut arms = Vec::with_capacity(site.arms.len());
        let mut applications = Vec::new();
        for (guard, application, arm) in &site.arms {
            match arm {
                ArmPlan::Inline { callee } => arms.push((*guard, LaidArm::Scope(*callee))),
                ArmPlan::Always(always) => {
                    let Some(receiver) = receiver else {
                        continue;
                    };
                    if let Some(step) = self.always_step(scope, program_counter, *always, receiver)
                    {
                        arms.push((*guard, LaidArm::Always(Box::new(step))));
                        applications.push(*application);
                    }
                }
            }
        }
        if arms.is_empty() {
            return None;
        }
        let every_arm = arms.len() == site.arms.len();
        let lowered = if site.fall_through == FallThrough::Unguarded {
            if !every_arm {
                return None;
            }
            Lowered::Unguarded(arms.pop().map(|(_, arm)| arm)?)
        } else {
            // Only a public send, or a lexical one at depth 0, tests its
            // receiver.
            let receiver = receiver
                .filter(|_| !matches!(instruction, SnapshotInstruction::SendSuper { .. }))?;
            Lowered::TypeCase {
                receiver,
                arms,
                sent: !(site.fall_through == FallThrough::Trap
                    && every_arm
                    && self.speculation == Speculation::Traps),
            }
        };
        for application in applications {
            if !self.arm_applications.contains(&application) {
                self.arm_applications.push(application);
            }
        }
        Some(lowered)
    }

    /// The step of `always`, the callee of the send at `program_counter`
    /// of `scope` whose receiver is `receiver`, if the graph expresses it.
    fn always_step(
        &self,
        scope: ScopeId,
        program_counter: u32,
        always: AlwaysInlined,
        receiver: ReceiverOf,
    ) -> Option<AlwaysStep> {
        let offset = self.graph_scopes[scope.0 as usize].offset;
        let snapshot = self.snapshots[scope.0 as usize];
        let sent = &snapshot.instructions[program_counter as usize];
        let (base, sources) = sent.argument_slots()?;
        let expressed = self.expressed(snapshot, program_counter, always, sources.len())?;
        Some(AlwaysStep {
            expressed,
            receiver,
            arguments: (0_u16..)
                .zip(sources)
                .map(|(position, source)| {
                    (
                        physical(offset, base.checked_add(position).expect(FRAME_BOUND)),
                        physical(offset, *source),
                    )
                })
                .collect(),
            destination: physical(offset, sent.register_written()?),
            continuation: usize::MAX,
        })
    }

    /// What the graph expresses of `always`, sent at `program_counter` of
    /// `snapshot` with `argument_count` arguments; `None` where the graph
    /// sends it. A callee with a check -- one whose answer the operands may
    /// leave to the interpreter -- is expressed only in a copy that
    /// speculates, at a site that never trapped: a copy that speculates
    /// nothing never traps, and a check that failed at the site once would
    /// fail again. A builtin class's accessor answers what the heap's
    /// tables name, which no simple node reads, and `^<parameter> value`
    /// and `cull:` activate a block, a scope of its own: both are sent.
    /// `SmallInteger` arithmetic is not expressed at a fast path's
    /// fallback send, where the same test already failed.
    fn expressed(
        &self,
        snapshot: &Snapshot,
        program_counter: u32,
        always: AlwaysInlined,
        argument_count: usize,
    ) -> Option<Expressed> {
        let index = program_counter as usize;
        let checks = self.speculation == Speculation::Traps
            && snapshot
                .sites
                .get(index)
                .copied()
                .flatten()
                .is_some_and(|facts| !facts.trapped);
        let fixed = |field: u32| field < FIXED_FIELD_LIMIT;
        let expressed = match always {
            AlwaysInlined::SlotLoad { field } if fixed(field) && argument_count == 0 => {
                Expressed::FieldLoad { field }
            }
            AlwaysInlined::SlotStore { field } if fixed(field) && argument_count == 1 => {
                Expressed::FieldStore { field }
            }
            AlwaysInlined::LazyGetter { field }
                if checks && fixed(field) && argument_count == 0 =>
            {
                Expressed::LazyGetter { field }
            }
            AlwaysInlined::Constant(constant) => Expressed::Constant(match constant {
                QuickConstant::True => Immediate::boolean(true).bits(),
                QuickConstant::False => Immediate::boolean(false).bits(),
                QuickConstant::Nil => Immediate::NIL.bits(),
            }),
            AlwaysInlined::Receiver => Expressed::Receiver,
            AlwaysInlined::Parameter(position) if usize::from(position) < argument_count => {
                Expressed::Parameter(usize::from(position))
            }
            AlwaysInlined::Primitive(primitive) => {
                return expressed_primitive(primitive, argument_count, checks).filter(|expressed| {
                    !matches!(expressed, Expressed::Integer(_))
                        || !is_fallback_send(snapshot, program_counter)
                })
            }
            AlwaysInlined::SlotLoad { .. }
            | AlwaysInlined::SlotStore { .. }
            | AlwaysInlined::LazyGetter { .. }
            | AlwaysInlined::Parameter(_)
            | AlwaysInlined::BuiltinClass(_)
            | AlwaysInlined::ValueOfParameter(_)
            | AlwaysInlined::CullParameter(_) => return None,
        };
        Some(expressed)
    }

    /// The return of the inlined scope `scope` with the value in its
    /// register `register`: the value bound to its send's destination in
    /// its sender, where it goes on.
    fn return_of(&self, scope: ScopeId, register: u16) -> Pending {
        let owner = &self.graph_scopes[scope.0 as usize];
        let (sender, call) = owner
            .parent
            .expect("internal: an inlined scope has a sender");
        let sent = &self.snapshots[sender.0 as usize].instructions[call as usize];
        let destination = sent
            .register_written()
            .expect("internal: an inlined send writes its destination");
        Pending::Return {
            source: physical(owner.offset, register),
            destination: physical(self.graph_scopes[sender.0 as usize].offset, destination),
            slot_end: physical(owner.offset, owner.argument_count),
            sender,
            call,
        }
    }

    /// The scope `depth` frames out from the block scope `scope`, as its
    /// closures' captured frames chain them: its home, then each home's
    /// own while it is a block's; `scope` itself for 0. The plan inlines
    /// no block whose body reaches past the compile
    /// (`Planner::reaches_past_the_compile`).
    fn reached(&self, scope: ScopeId, depth: u16) -> ScopeId {
        let mut reached = scope;
        for _ in 0..depth {
            reached = self.plan.scopes[reached.0 as usize]
                .block_of
                .expect("internal: an inlined block reaches no frame past the compile");
        }
        reached
    }

    /// The instruction at `program_counter` of `scope` as a step: its
    /// registers physical, a `Return` of an inlined scope the scope's
    /// return to its sender, and its `LoadSelf` a read of the receiver its
    /// header binds. In an inlined block's scope an outer local is its
    /// home's register, and a `^` its home's return, both in the compile;
    /// a `MakeClosure` the plan eliminates makes nothing.
    fn instruction(&self, scope: ScopeId, program_counter: u32) -> Pending {
        let owner = &self.graph_scopes[scope.0 as usize];
        let original =
            self.snapshots[scope.0 as usize].instructions[program_counter as usize].clone();
        if let SnapshotInstruction::MakeClosure { destination, .. } = original {
            if let Some(BlockPlan::Eliminated(block)) =
                self.plan.blocks.get(&(scope, program_counter))
            {
                return Pending::Eliminated {
                    destination: physical(owner.offset, destination),
                    block: *block,
                };
            }
        }
        if owner.parent.is_some() {
            let block_scope = self.plan.scopes[scope.0 as usize].block_of.is_some();
            match original {
                SnapshotInstruction::Return { register }
                | SnapshotInstruction::ImplicitReturn { register } => {
                    return self.return_of(scope, register);
                }
                // A block's `^` returns from its first home that is no
                // block's: the root's own return, or an inlined method's.
                SnapshotInstruction::NonLocalReturn { register } if block_scope => {
                    let mut home = scope;
                    while let Some(outer) = self.plan.scopes[home.0 as usize].block_of {
                        home = outer;
                    }
                    if home != ScopeId::ROOT {
                        let returned = self.return_of(home, 0);
                        let Pending::Return {
                            destination,
                            slot_end,
                            sender,
                            call,
                            ..
                        } = returned
                        else {
                            unreachable!("internal: a return is a return");
                        };
                        return Pending::Return {
                            source: physical(owner.offset, register),
                            destination,
                            slot_end,
                            sender,
                            call,
                        };
                    }
                    let returned = SnapshotInstruction::Return {
                        register: physical(owner.offset, register),
                    };
                    return Pending::Instruction {
                        physical: returned,
                        original,
                    };
                }
                SnapshotInstruction::LoadOuterLocal {
                    destination,
                    depth,
                    index,
                } if block_scope => {
                    let reached = &self.graph_scopes[self.reached(scope, depth).0 as usize];
                    return Pending::Instruction {
                        physical: SnapshotInstruction::Move {
                            destination: physical(owner.offset, destination),
                            source: physical(reached.offset, index),
                        },
                        original,
                    };
                }
                SnapshotInstruction::StoreOuterLocal {
                    source,
                    depth,
                    index,
                } if block_scope => {
                    let reached = &self.graph_scopes[self.reached(scope, depth).0 as usize];
                    return Pending::Instruction {
                        physical: SnapshotInstruction::StoreLocal {
                            source: physical(owner.offset, source),
                            slot: physical(reached.offset, index),
                        },
                        original,
                    };
                }
                SnapshotInstruction::ReturnSlotIfFilled { .. }
                | SnapshotInstruction::NonLocalReturn { .. } => {
                    panic!("internal: an inlined method returns only by a `Return`")
                }
                _ => {}
            }
        }
        let receiver = owner
            .parent
            .map(|_| owner.header_register(HeaderBinding::Receiver));
        Pending::Instruction {
            physical: relocate(&original, owner.offset, receiver),
            original,
        }
    }

    /// The entry of the inlined scope `scope`: its parameters bound to the
    /// send's arguments, its receiver to the send's, its current class to
    /// the one the send's lookup found its method at -- `level` steps up
    /// from the receiver's class for a public or lexical send, from the
    /// sender's current class for a private one, and one step more for a
    /// `super` send, which starts at the superclass -- and its named
    /// temporaries to `nil`, as the interpreter's push does.
    fn entry(&self, scope: ScopeId) -> Entry {
        let callee = &self.graph_scopes[scope.0 as usize];
        let planned = &self.plan.scopes[scope.0 as usize];
        if let Some(home) = planned.block_of {
            return self.block_entry(scope, home);
        }
        let (sender, call) = planned
            .parent
            .expect("internal: an inlined scope has a sender");
        let sender_scope = &self.graph_scopes[sender.0 as usize];
        let sender_snapshot = self.snapshots[sender.0 as usize];
        let sent = &sender_snapshot.instructions[call as usize];
        let (_, sources) = sent
            .argument_slots()
            .expect("internal: an inlined send lays its arguments on slots");
        let level = planned.level.unwrap_or(0);
        let private = sender_snapshot
            .sites
            .get(call as usize)
            .copied()
            .flatten()
            .and_then(|facts| self.tree.sites.get(&(sender_snapshot.code_id, facts.site)))
            .is_some_and(|candidates| candidates.private);
        let sender_class = sender_scope.header_register(HeaderBinding::CurrentClass);
        let (receiver, class_start, sender_class, class_steps) = match sent {
            SnapshotInstruction::SendPublic {
                receiver_register, ..
            }
            | SnapshotInstruction::SendInlinable {
                receiver_register, ..
            } => (
                ReceiverOf::Register(physical(sender_scope.offset, *receiver_register)),
                ClassStart::ClassOf,
                None,
                level,
            ),
            SnapshotInstruction::SendLexical { .. } if private => (
                ReceiverOf::SelfOf(sender),
                ClassStart::Class,
                Some(sender_class),
                level,
            ),
            SnapshotInstruction::SendLexical { .. } => {
                (ReceiverOf::SelfOf(sender), ClassStart::ClassOf, None, level)
            }
            SnapshotInstruction::SendSuper { .. } => (
                ReceiverOf::SelfOf(sender),
                ClassStart::Class,
                Some(sender_class),
                level
                    .checked_add(1)
                    .expect("internal: a level fits a u16 with a step to spare"),
            ),
            _ => panic!("internal: only a send is inlined"),
        };
        let class_source = sender_class.map(|_| sender);
        let facts = &self.snapshots[scope.0 as usize].facts;
        let named_start = physical(callee.argument_count, crate::bytecode::HEADER_REGISTERS);
        let named_end = facts.local_count.max(named_start);
        Entry {
            receiver,
            arguments: (0_u16..)
                .zip(sources)
                .map(|(index, source)| {
                    (
                        physical(callee.offset, index),
                        physical(sender_scope.offset, *source),
                    )
                })
                .collect(),
            class_start,
            sender_class,
            class_source,
            class_steps,
            receiver_register: callee.header_register(HeaderBinding::Receiver),
            class_register: callee.header_register(HeaderBinding::CurrentClass),
            temporaries: physical(callee.offset, named_start)..physical(callee.offset, named_end),
            overlay: vec![
                callee.exit_words(),
                physical(callee.offset, named_end)..u16::MAX,
            ],
        }
    }

    /// The entry of the block scope `scope`, whose block `home` made: as
    /// the interpreter's push of a closure lays it -- its parameters bound
    /// to the `value` send's arguments, its receiver and current class to
    /// its home's, its copied values to what the registers they were
    /// copied from hold, which the plan keeps unwritten while the block
    /// lives (`Planner::copied_source_written`), and its other named
    /// temporaries to `nil`.
    fn block_entry(&self, scope: ScopeId, home: ScopeId) -> Entry {
        let callee = &self.graph_scopes[scope.0 as usize];
        let planned = &self.plan.scopes[scope.0 as usize];
        let (sender, call) = planned
            .parent
            .expect("internal: an inlined scope has a sender");
        let sender_scope = &self.graph_scopes[sender.0 as usize];
        let home_scope = &self.graph_scopes[home.0 as usize];
        let sent = &self.snapshots[sender.0 as usize].instructions[call as usize];
        let (_, sources) = sent
            .argument_slots()
            .expect("internal: an inlined send lays its arguments on slots");
        let block = planned
            .block
            .expect("internal: a block's scope runs an eliminated block");
        let made_at = self
            .plan
            .blocks
            .iter()
            .find(|(_, planned)| **planned == BlockPlan::Eliminated(block))
            .map(|((_, program_counter), _)| *program_counter)
            .expect("internal: an eliminated block is made in its home");
        let SnapshotInstruction::MakeClosure { copied, .. } =
            &self.snapshots[home.0 as usize].instructions[made_at as usize]
        else {
            panic!("internal: a block is made by a `MakeClosure`");
        };
        let copied_start = physical(callee.argument_count, crate::bytecode::HEADER_REGISTERS);
        let mut arguments: Vec<(u16, u16)> = (0_u16..)
            .zip(sources)
            .map(|(index, source)| {
                (
                    physical(callee.offset, index),
                    physical(sender_scope.offset, *source),
                )
            })
            .collect();
        arguments.extend((0_u16..).zip(copied).map(|(position, source)| {
            (
                physical(callee.offset, physical(copied_start, position)),
                physical(home_scope.offset, *source),
            )
        }));
        let facts = &self.snapshots[scope.0 as usize].facts;
        let named_start = physical(copied_start, facts.copied_count);
        let named_end = facts.local_count.max(named_start);
        Entry {
            receiver: ReceiverOf::SelfOf(home),
            arguments,
            // Its home's current class, no step from it.
            class_start: ClassStart::Class,
            sender_class: Some(home_scope.header_register(HeaderBinding::CurrentClass)),
            class_source: Some(home),
            class_steps: 0,
            receiver_register: callee.header_register(HeaderBinding::Receiver),
            class_register: callee.header_register(HeaderBinding::CurrentClass),
            temporaries: physical(callee.offset, named_start)..physical(callee.offset, named_end),
            overlay: vec![
                callee.exit_words(),
                physical(callee.offset, named_end)..u16::MAX,
            ],
        }
    }
}

/// What the graph expresses of the leaf `primitive` sent with
/// `argument_count` arguments, `checks` saying whether a callee with a
/// check is expressed (`Laying::expressed`): only `class` has none.
fn expressed_primitive(
    primitive: Primitive,
    argument_count: usize,
    checks: bool,
) -> Option<Expressed> {
    let expressed = match (primitive, argument_count) {
        (Primitive::Object(ObjectPrimitive::Class), 0) => return Some(Expressed::ClassOf),
        (Primitive::Object(ObjectPrimitive::Identical), 1) => Expressed::Identity,
        (Primitive::Collection(CollectionPrimitive::ElementAt), 1) => {
            Expressed::IndexedLoad(IndexedKind::Array)
        }
        (Primitive::Collection(CollectionPrimitive::ElementAtPut), 2) => {
            Expressed::IndexedStore(IndexedKind::Array)
        }
        (Primitive::Bytes(BytePrimitive::ElementAt), 1) => {
            Expressed::IndexedLoad(IndexedKind::ByteArray)
        }
        (Primitive::Bytes(BytePrimitive::ElementAtPut), 2) => {
            Expressed::IndexedStore(IndexedKind::ByteArray)
        }
        (Primitive::Integer(integer), 1) => Expressed::Integer(match integer {
            IntegerPrimitive::Add => IntegerOperation::Add,
            IntegerPrimitive::Subtract => IntegerOperation::Subtract,
            IntegerPrimitive::Multiply => IntegerOperation::Multiply,
            IntegerPrimitive::FloorDivide => IntegerOperation::FloorDivide,
            IntegerPrimitive::Less => IntegerOperation::Less,
            IntegerPrimitive::Greater => IntegerOperation::Greater,
            IntegerPrimitive::LessOrEqual => IntegerOperation::LessOrEqual,
            IntegerPrimitive::GreaterOrEqual => IntegerOperation::GreaterOrEqual,
            IntegerPrimitive::Equal => IntegerOperation::Equal,
            _ => return None,
        }),
        _ => return None,
    };
    checks.then_some(expressed)
}

/// Whether the send at `program_counter` of `snapshot` is an integer fast
/// path's fallback.
fn is_fallback_send(snapshot: &Snapshot, program_counter: u32) -> bool {
    snapshot.instructions.iter().any(|instruction| {
        matches!(instruction, SnapshotInstruction::IntegerBinary { target, .. } if *target == program_counter)
    })
}

/// `instruction` of a scope at `offset`, its registers numbered
/// physically; a `LoadSelf` of an inlined scope, whose receiver is the
/// binding of its header word `receiver`, a read of that word. Jump
/// targets stay the scope's (`retarget`), and so does an outer local's
/// index, a register of another frame.
fn relocate(
    instruction: &SnapshotInstruction,
    offset: u16,
    receiver: Option<u16>,
) -> SnapshotInstruction {
    let moved = |register: u16| {
        register
            .checked_add(offset)
            .expect("internal: a scope's registers fit the physical frame")
    };
    let all = |registers: &[u16]| registers.iter().map(|register| moved(*register)).collect();
    match instruction.clone() {
        SnapshotInstruction::LoadSelf { destination } => match receiver {
            Some(receiver) => SnapshotInstruction::Move {
                destination: moved(destination),
                source: receiver,
            },
            None => SnapshotInstruction::LoadSelf {
                destination: moved(destination),
            },
        },
        SnapshotInstruction::LoadLiteral {
            destination,
            literal,
        } => SnapshotInstruction::LoadLiteral {
            destination: moved(destination),
            literal,
        },
        SnapshotInstruction::LoadEnclosingObject { destination, depth } => {
            SnapshotInstruction::LoadEnclosingObject {
                destination: moved(destination),
                depth,
            }
        }
        SnapshotInstruction::Move {
            destination,
            source,
        } => SnapshotInstruction::Move {
            destination: moved(destination),
            source: moved(source),
        },
        SnapshotInstruction::StoreLocal { source, slot } => SnapshotInstruction::StoreLocal {
            source: moved(source),
            slot: moved(slot),
        },
        SnapshotInstruction::LoadOuterLocal {
            destination,
            depth,
            index,
        } => SnapshotInstruction::LoadOuterLocal {
            destination: moved(destination),
            depth,
            index,
        },
        SnapshotInstruction::StoreOuterLocal {
            source,
            depth,
            index,
        } => SnapshotInstruction::StoreOuterLocal {
            source: moved(source),
            depth,
            index,
        },
        SnapshotInstruction::SendLexical {
            depth,
            selector,
            argument_base,
            argument_sources,
            destination,
            call_site,
        } => SnapshotInstruction::SendLexical {
            depth,
            selector,
            argument_base: moved(argument_base),
            argument_sources: all(&argument_sources),
            destination: moved(destination),
            call_site,
        },
        SnapshotInstruction::SendPublic {
            selector,
            receiver_register,
            argument_base,
            argument_sources,
            destination,
            call_site,
        } => SnapshotInstruction::SendPublic {
            selector,
            receiver_register: moved(receiver_register),
            argument_base: moved(argument_base),
            argument_sources: all(&argument_sources),
            destination: moved(destination),
            call_site,
        },
        SnapshotInstruction::SendInlinable {
            selector,
            receiver_register,
            argument_base,
            argument_sources,
            destination,
            call_site,
        } => SnapshotInstruction::SendInlinable {
            selector,
            receiver_register: moved(receiver_register),
            argument_base: moved(argument_base),
            argument_sources: all(&argument_sources),
            destination: moved(destination),
            call_site,
        },
        SnapshotInstruction::SendSuper {
            selector,
            argument_base,
            argument_sources,
            destination,
            call_site,
        } => SnapshotInstruction::SendSuper {
            selector,
            argument_base: moved(argument_base),
            argument_sources: all(&argument_sources),
            destination: moved(destination),
            call_site,
        },
        SnapshotInstruction::SendMirror {
            primitive,
            mirror_register,
            subject_register,
            argument_base,
            argument_sources,
            destination,
            failure,
        } => SnapshotInstruction::SendMirror {
            primitive,
            mirror_register: moved(mirror_register),
            subject_register: moved(subject_register),
            argument_base: moved(argument_base),
            argument_sources: all(&argument_sources),
            destination: moved(destination),
            failure,
        },
        SnapshotInstruction::MakeArray {
            destination,
            element_registers,
        } => SnapshotInstruction::MakeArray {
            destination: moved(destination),
            element_registers: all(&element_registers),
        },
        SnapshotInstruction::MakeVector { destination, size } => SnapshotInstruction::MakeVector {
            destination: moved(destination),
            size,
        },
        SnapshotInstruction::LoadVector {
            destination,
            vector,
            index,
        } => SnapshotInstruction::LoadVector {
            destination: moved(destination),
            vector: moved(vector),
            index,
        },
        SnapshotInstruction::StoreVector {
            vector,
            index,
            source,
        } => SnapshotInstruction::StoreVector {
            vector: moved(vector),
            index,
            source: moved(source),
        },
        SnapshotInstruction::InitializeSlot { source, own_index } => {
            SnapshotInstruction::InitializeSlot {
                source: moved(source),
                own_index,
            }
        }
        SnapshotInstruction::ReturnSlotIfFilled { own_index } => {
            SnapshotInstruction::ReturnSlotIfFilled { own_index }
        }
        SnapshotInstruction::MakeFuture {
            destination,
            closure,
        } => SnapshotInstruction::MakeFuture {
            destination: moved(destination),
            closure: moved(closure),
        },
        SnapshotInstruction::ResolveFuture {
            destination,
            source,
        } => SnapshotInstruction::ResolveFuture {
            destination: moved(destination),
            source: moved(source),
        },
        SnapshotInstruction::LoadOwnSlot {
            destination,
            own_index,
        } => SnapshotInstruction::LoadOwnSlot {
            destination: moved(destination),
            own_index,
        },
        SnapshotInstruction::InitializeSlotProvisionally { source, own_index } => {
            SnapshotInstruction::InitializeSlotProvisionally {
                source: moved(source),
                own_index,
            }
        }
        SnapshotInstruction::Return { register } => SnapshotInstruction::Return {
            register: moved(register),
        },
        SnapshotInstruction::ImplicitReturn { register } => SnapshotInstruction::ImplicitReturn {
            register: moved(register),
        },
        SnapshotInstruction::NonLocalReturn { register } => SnapshotInstruction::NonLocalReturn {
            register: moved(register),
        },
        SnapshotInstruction::SignalNonBoolean { register } => {
            SnapshotInstruction::SignalNonBoolean {
                register: moved(register),
            }
        }
        SnapshotInstruction::Jump { target } => SnapshotInstruction::Jump { target },
        SnapshotInstruction::JumpIfNotBoolean { register, target } => {
            SnapshotInstruction::JumpIfNotBoolean {
                register: moved(register),
                target,
            }
        }
        SnapshotInstruction::BranchIfFalse { register, target } => {
            SnapshotInstruction::BranchIfFalse {
                register: moved(register),
                target,
            }
        }
        SnapshotInstruction::BranchIfTrue { register, target } => {
            SnapshotInstruction::BranchIfTrue {
                register: moved(register),
                target,
            }
        }
        SnapshotInstruction::IntegerBinary {
            operation,
            destination,
            left,
            right,
            target,
        } => SnapshotInstruction::IntegerBinary {
            operation,
            destination: moved(destination),
            left: moved(left),
            right: moved(right),
            target,
        },
        SnapshotInstruction::AllocateInstance { destination } => {
            SnapshotInstruction::AllocateInstance {
                destination: moved(destination),
            }
        }
        SnapshotInstruction::InitializeInstance {
            instance,
            destination,
        } => SnapshotInstruction::InitializeInstance {
            instance: moved(instance),
            destination: moved(destination),
        },
        SnapshotInstruction::CheckAppliedClause { steps, selector } => {
            SnapshotInstruction::CheckAppliedClause { steps, selector }
        }
        SnapshotInstruction::InitializeSuperclass {
            steps,
            selector,
            argument_base,
            argument_sources,
            destination,
        } => SnapshotInstruction::InitializeSuperclass {
            steps,
            selector,
            argument_base: moved(argument_base),
            argument_sources: all(&argument_sources),
            destination: moved(destination),
        },
        SnapshotInstruction::InitializeAppliedLevel {
            steps,
            argument_base,
            argument_sources,
            destination,
        } => SnapshotInstruction::InitializeAppliedLevel {
            steps,
            argument_base: moved(argument_base),
            argument_sources: all(&argument_sources),
            destination: moved(destination),
        },
        SnapshotInstruction::MakeClosure {
            destination,
            block,
            parameter_count,
            copied,
        } => SnapshotInstruction::MakeClosure {
            destination: moved(destination),
            block,
            parameter_count,
            copied: all(&copied),
        },
    }
}

/// Renumbers `instruction`'s jump target, if it has one, by `step_of`.
fn retarget(instruction: &mut SnapshotInstruction, step_of: impl Fn(u32) -> u32) {
    match instruction {
        SnapshotInstruction::Jump { target }
        | SnapshotInstruction::JumpIfNotBoolean { target, .. }
        | SnapshotInstruction::BranchIfFalse { target, .. }
        | SnapshotInstruction::BranchIfTrue { target, .. }
        | SnapshotInstruction::IntegerBinary { target, .. }
        | SnapshotInstruction::SendMirror {
            failure: target, ..
        } => *target = step_of(*target),
        _ => {}
    }
}

/// A set of registers, one bit each.
#[derive(Clone, PartialEq, Eq)]
struct RegisterSet {
    words: Vec<u64>,
}

impl RegisterSet {
    fn empty(register_count: usize) -> RegisterSet {
        RegisterSet {
            words: vec![0; register_count.div_ceil(64)],
        }
    }

    fn full(register_count: usize) -> RegisterSet {
        let mut set = RegisterSet::empty(register_count);
        for register in 0..register_count {
            set.insert(register);
        }
        set
    }

    fn contains(&self, register: usize) -> bool {
        self.words[register / 64] & (1 << (register % 64)) != 0
    }

    fn insert(&mut self, register: usize) {
        self.words[register / 64] |= 1 << (register % 64);
    }

    fn remove(&mut self, register: usize) {
        self.words[register / 64] &= !(1 << (register % 64));
    }

    fn union_with(&mut self, other: &RegisterSet) {
        for (word, other) in self.words.iter_mut().zip(&other.words) {
            *word |= other;
        }
    }

    fn intersect_with(&mut self, other: &RegisterSet) {
        for (word, other) in self.words.iter_mut().zip(&other.words) {
            *word &= other;
        }
    }

    fn registers(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.words.len() * 64).filter(|register| self.contains(*register))
    }
}

/// What a pass around the loops through one head may do to the
/// registers live at the head.
struct LoopFacts {
    /// The registers live at the head that an instruction in a loop
    /// writes.
    written: RegisterSet,
    /// Whether a frame node lies in a loop, which leaves every register to
    /// its frame word.
    frame_node: bool,
    /// Whether the head dominates every block a back edge leaves: a loop
    /// of the program's, not the way back from a fallback laid out after
    /// the fast path it rejoins.
    natural: bool,
}

/// Where control goes from a step: on, after the step's writes, at
/// `next` -- the following step, or a return's continuation; without them
/// at `target`, its jump target; and at each of `others`, a type-case's
/// arms and fall-through. `ends_block` says whether the step's kind ends
/// its block whatever follows it, and `starts_block` whether it begins
/// one: an inlined scope's entry, so that a block holds one scope's steps.
struct Edges {
    next: Option<usize>,
    target: Option<usize>,
    others: Vec<usize>,
    ends_block: bool,
    starts_block: bool,
}

/// A block before its nodes are built: the steps it holds, empty for an
/// entry block or an edge block, and where control goes after it.
struct Shape {
    scope: ScopeId,
    program_counter: u32,
    instructions: Range<usize>,
    /// The step whose leftover analysis holds at the block's top: its
    /// first, or, for a block with none, that of the block it leads to.
    top: usize,
    /// The block the last step falls through to; for a block with no
    /// step, the block it leads to.
    next: Option<usize>,
    /// The block the last step's jump target is.
    target: Option<usize>,
    /// The blocks a type-case the block ends in leads to.
    others: Vec<usize>,
}

impl Shape {
    fn successors(&self) -> Vec<usize> {
        let mut successors = Vec::with_capacity(2 + self.others.len());
        for block in [self.next, self.target]
            .into_iter()
            .flatten()
            .chain(self.others.iter().copied())
        {
            if !successors.contains(&block) {
                successors.push(block);
            }
        }
        successors
    }
}

/// The register state at a point of a path: each register's binding,
/// `None` for a register whose value is its frame word, and the
/// generation of its frame word on that path.
#[derive(Clone, Default)]
struct State {
    bindings: Vec<Option<ValueId>>,
    /// A simple node's write of a register begins a new generation of its
    /// word on its own path alone, and so does a merge of paths that
    /// disagree: a read of the word says what the word holds only where
    /// the path's generation is the one it was made in (`is_frame_word`).
    /// A write on another path -- an exit arm a loop never passes through
    /// -- leaves the read the word, so the loop takes no phi for it.
    generations: Vec<u32>,
}

impl State {
    /// Every register its frame word, in the first generation.
    fn unbound(register_count: usize) -> State {
        State {
            bindings: vec![None; register_count],
            generations: vec![0; register_count],
        }
    }

    /// Leaves every register to its frame word, for a frame node. It
    /// begins no generation: no binding made before the node survives it,
    /// so no read made before it is taken for the word after it, and a
    /// read made after it reads what the node left.
    fn unbind_all(&mut self) {
        self.bindings.fill(None);
    }
}

impl std::ops::Index<usize> for State {
    type Output = Option<ValueId>;

    fn index(&self, register: usize) -> &Option<ValueId> {
        &self.bindings[register]
    }
}

impl std::ops::IndexMut<usize> for State {
    fn index_mut(&mut self, register: usize) -> &mut Option<ValueId> {
        &mut self.bindings[register]
    }
}

struct Builder<'flat> {
    flat: &'flat Flat<'flat>,
    /// What decides each step's kind beside it (`kind_of`).
    choices: Vec<Choice>,
    /// Each step's guard, as `choices` holds it.
    guards: Vec<Guard>,
    reads: Vec<Vec<u16>>,
    written: Vec<Vec<u16>>,
    register_count: usize,
    shapes: Vec<Shape>,
    /// The block each run of steps begins, by its first step.
    block_at: HashMap<usize, usize>,
    blocks: Vec<Block>,
    live_in: Vec<RegisterSet>,
    /// For each loop head, what a pass around a loop through it may do
    /// to its registers.
    loops: Vec<Option<LoopFacts>>,
    /// Each built block's state where its end leaves it, and, for a block
    /// ending in an integer fast path, the state its fallback is taken
    /// with: the destination still unbound.
    exits: Vec<State>,
    fallback_exits: Vec<Option<State>>,
    /// A loop head's state at its top, against which its back edges are
    /// checked.
    head_entries: Vec<Option<State>>,
    /// For each value that is a frame read, the register it reads and the
    /// generation of the register's word it was made in (`State`).
    frame_read_of: Vec<Option<(u16, u32)>>,
    /// The last generation begun.
    last_generation: u32,
    /// For each loop head, the live registers it takes a phi for though
    /// the rule of `build` would leave them their frame words: those a back
    /// edge of an earlier build brought otherwise.
    forced_phis: Vec<RegisterSet>,
    /// For each loop head, the registers it left their frame words on
    /// that rule, which its back edges must bring as such.
    unbound_at_head: Vec<RegisterSet>,
    /// The loop heads' registers a back edge brought as a value its frame
    /// word does not hold, though the head left them their frame words.
    refuted: Vec<(usize, usize)>,
}

impl<'flat> Builder<'flat> {
    fn new(
        flat: &'flat Flat<'flat>,
        speculation: Speculation,
        known_boolean: &[bool],
    ) -> Builder<'flat> {
        let steps = &flat.steps;
        assert!(
            !steps.is_empty(),
            "internal: a body holds at least one instruction"
        );
        let choices: Vec<Choice> = steps
            .iter()
            .enumerate()
            .map(|(index, step)| {
                let Step::Instruction { physical, .. } = &step.step else {
                    return Choice::default();
                };
                let snapshot = flat.scope(step.scope).snapshot;
                let program_counter = step.program_counter as usize;
                let send_only = matches!(physical, SnapshotInstruction::IntegerBinary { .. })
                    && snapshot
                        .fallback_taken
                        .get(program_counter)
                        .copied()
                        .unwrap_or(false);
                Choice {
                    send_only,
                    guard: guard_of(
                        snapshot,
                        (program_counter, step.scope != ScopeId::ROOT),
                        send_only,
                        speculation,
                    ),
                    // Only a `JumpIfNotBoolean`'s third way is a jump target.
                    known_boolean: matches!(physical, SnapshotInstruction::JumpIfNotBoolean { .. })
                        && known_boolean.get(index).copied().unwrap_or(false),
                }
            })
            .collect();
        // A jump to the fallback reads nothing and writes nothing: the
        // fallback send reads the operands. A send that traps writes
        // nothing either; it reads what the interpreter's send, run again,
        // reads.
        let reads: Vec<Vec<u16>> = steps
            .iter()
            .zip(&choices)
            .enumerate()
            .map(|(index, (step, choice))| step_reads(flat, step, index, *choice))
            .collect();
        let written: Vec<Vec<u16>> = steps
            .iter()
            .zip(&choices)
            .map(|(step, choice)| step_writes(step, *choice))
            .collect();
        let register_count = flat.register_count;
        let edges: Vec<Edges> = (0..steps.len())
            .map(|index| edges_of(flat, index, choices[index]))
            .collect();
        let (shapes, block_at) = shapes_of(flat, &edges);
        let block_count = shapes.len();
        let mut builder = Builder {
            flat,
            guards: choices.iter().map(|choice| choice.guard).collect(),
            choices,
            reads,
            written,
            register_count,
            shapes,
            block_at,
            blocks: Vec::with_capacity(block_count),
            live_in: Vec::new(),
            loops: Vec::new(),
            exits: vec![State::default(); block_count],
            fallback_exits: vec![None; block_count],
            head_entries: vec![None; block_count],
            frame_read_of: Vec::new(),
            last_generation: 0,
            forced_phis: vec![RegisterSet::empty(register_count); block_count],
            unbound_at_head: vec![RegisterSet::empty(register_count); block_count],
            refuted: Vec::new(),
        };
        builder.lay_out_blocks();
        builder.live_in = builder.liveness();
        builder.loops = builder.loops();
        builder
    }

    /// What the step at `index` becomes.
    fn kind(&self, index: usize) -> Kind {
        let step = &self.flat.steps[index];
        match &step.step {
            Step::Instruction { physical, .. } => kind_of(
                physical,
                index,
                self.choices[index],
                self.flat.scope(step.scope).own_slot_base,
            ),
            Step::TypeCase { .. } => Kind::TypeCase,
            Step::Enter(_) => Kind::Enter,
            Step::Return { .. } => Kind::InlinedReturn,
            Step::Always(_) => Kind::Always,
            Step::Eliminated { destination, block } => Kind::EliminatedBlock {
                destination: *destination,
                block: *block,
            },
        }
    }

    /// The graph, or, when a back edge refuted a register its loop head
    /// left its frame word, the phis to force in the next build.
    fn build(mut self) -> Result<Graph, Vec<RegisterSet>> {
        for block in 0..self.blocks.len() {
            self.build_block(block);
        }
        if !self.refuted.is_empty() {
            let mut forced = self.forced_phis;
            for (head, register) in self.refuted {
                forced[head].insert(register);
            }
            return Err(forced);
        }
        debug_assert!(
            self.blocks
                .iter()
                .all(|block| block.phis.iter().all(|phi| !phi.inputs.contains(&UNFILLED))),
            "internal: every back edge filled its head's phis"
        );
        let mut graph = Graph::with_scopes(self.blocks, self.flat.graph_scopes.clone());
        graph
            .arm_applications
            .clone_from(&self.flat.arm_applications);
        graph.inlined_sites = self.flat.inlined_sites;
        Ok(graph)
    }

    /// The blocks, each with its predecessors and whether it heads a
    /// loop, their phis, nodes and ends still to build.
    fn lay_out_blocks(&mut self) {
        let mut predecessors: Vec<Vec<BlockId>> = vec![Vec::new(); self.shapes.len()];
        for (block, shape) in self.shapes.iter().enumerate() {
            for successor in shape.successors() {
                predecessors[successor].push(BlockId::of_index(block));
            }
        }
        for (block, (shape, predecessors)) in self.shapes.iter().zip(predecessors).enumerate() {
            let loop_head = predecessors
                .iter()
                .any(|predecessor| predecessor.index() >= block);
            self.blocks.push(Block {
                scope: shape.scope,
                program_counter: shape.program_counter,
                taint_point: self.flat.taint_point(shape.top),
                phis: Vec::new(),
                nodes: Vec::new(),
                end: End::Frame,
                predecessors,
                loop_head,
            });
        }
    }

    /// The registers live at each block's top: those some path from there
    /// reads before it writes them. Control leaves an integer fast path
    /// for its fallback, and a mirror send for its failure code, with the
    /// destination possibly unwritten, so the destination is written only
    /// on the way to the next instruction.
    fn liveness(&self) -> Vec<RegisterSet> {
        let count = self.shapes.len();
        let mut live_in = vec![RegisterSet::empty(self.register_count); count];
        let mut changed = true;
        while changed {
            changed = false;
            for block in (0..count).rev() {
                let shape = &self.shapes[block];
                let mut live = RegisterSet::empty(self.register_count);
                let mut instructions = shape.instructions.clone().rev();
                match instructions.next() {
                    None => {
                        for successor in shape.successors() {
                            live.union_with(&live_in[successor]);
                        }
                    }
                    Some(last) => {
                        if let Some(next) = shape.next {
                            live.union_with(&live_in[next]);
                        }
                        for written in &self.written[last] {
                            live.remove(usize::from(*written));
                        }
                        for other in shape.target.iter().chain(&shape.others) {
                            live.union_with(&live_in[*other]);
                        }
                        self.add_reads(&mut live, last);
                        for index in instructions {
                            for written in &self.written[index] {
                                live.remove(usize::from(*written));
                            }
                            self.add_reads(&mut live, index);
                        }
                    }
                }
                if live != live_in[block] {
                    live_in[block] = live;
                    changed = true;
                }
            }
        }
        live_in
    }

    fn add_reads(&self, live: &mut RegisterSet, index: usize) {
        for register in &self.reads[index] {
            live.insert(usize::from(*register));
        }
    }

    /// For each loop head, what a pass around a loop through it may do to
    /// its registers. A head that does not dominate every block a back
    /// edge leaves is entered elsewhere too, and what those other ways
    /// bring is not known before they are built, so every register live
    /// there counts as written.
    fn loops(&self) -> Vec<Option<LoopFacts>> {
        let successors: Vec<Vec<usize>> = self.shapes.iter().map(Shape::successors).collect();
        let dominators = immediate_dominators(&successors);
        let dominates = |dominator: usize, block: usize| {
            let mut block = block;
            loop {
                if block == dominator {
                    return true;
                }
                match dominators[block] {
                    Some(parent) if parent != block => block = parent,
                    _ => return false,
                }
            }
        };
        let mut loops = Vec::with_capacity(self.blocks.len());
        for (head, block) in self.blocks.iter().enumerate() {
            if !block.loop_head {
                loops.push(None);
                continue;
            }
            let back_edges: Vec<usize> = block
                .predecessors
                .iter()
                .map(|predecessor| predecessor.index())
                .filter(|predecessor| *predecessor >= head)
                .collect();
            let natural = back_edges
                .iter()
                .all(|predecessor| dominates(head, *predecessor));
            let mut facts = LoopFacts {
                written: RegisterSet::empty(self.register_count),
                frame_node: false,
                natural,
            };
            if natural {
                // The loop's body: the head, every block a back edge
                // leaves from, and every block that reaches one without
                // passing the head.
                let mut in_loop = vec![false; self.blocks.len()];
                in_loop[head] = true;
                let mut pending = back_edges;
                while let Some(member) = pending.pop() {
                    if std::mem::replace(&mut in_loop[member], true) {
                        continue;
                    }
                    pending.extend(
                        self.blocks[member]
                            .predecessors
                            .iter()
                            .map(|predecessor| predecessor.index()),
                    );
                }
                for (member, inside) in in_loop.iter().enumerate() {
                    if !inside {
                        continue;
                    }
                    for index in self.shapes[member].instructions.clone() {
                        if matches!(self.kind(index), Kind::Frame) {
                            facts.frame_node = true;
                        }
                        for written in &self.written[index] {
                            facts.written.insert(usize::from(*written));
                        }
                    }
                }
            } else {
                facts.written = RegisterSet::full(self.register_count);
            }
            facts.written.intersect_with(&self.live_in[head]);
            loops.push(Some(facts));
        }
        loops
    }

    /// A value, a read of `frame_read`'s register's word in its generation
    /// if it is one.
    fn new_value(&mut self, frame_read: Option<(u16, u32)>) -> ValueId {
        let value = ValueId(
            u32::try_from(self.frame_read_of.len())
                .expect("internal: a body defines fewer than 2^32 values"),
        );
        self.frame_read_of.push(frame_read);
        value
    }

    fn new_generation(&mut self) -> u32 {
        self.last_generation = self
            .last_generation
            .checked_add(1)
            .expect("internal: a body begins fewer than 2^32 generations");
        self.last_generation
    }

    /// Binds `destination` to `value`, which a simple node wrote, and
    /// begins a new generation of the register's word on this path: every
    /// read of the word made earlier on it stops saying what the word
    /// holds. The allocator may store the binding the node made, or a
    /// later one, into the word -- a spill writes the bound value back --
    /// so a register bound again to such a read, `t := x. x := 0. ...
    /// x := t`, is bound to a value the word may no longer hold, and a
    /// merge or a loop's head that took it for the word would read the
    /// word instead of the value. A path that never passes the write
    /// keeps its generation, and its reads stay the word's.
    fn bind(&mut self, state: &mut State, destination: u16, value: ValueId) {
        let register = usize::from(destination);
        state.generations[register] = self.new_generation();
        state[register] = Some(value);
    }

    /// Ends the bindings of the registers `ranges` names, past which their
    /// words are an inlined callee's (`Node::Overlay`), and begins a new
    /// generation of each: what the callee writes there is no read made
    /// before.
    fn overlay(&mut self, block: usize, state: &mut State, ranges: Vec<Range<u16>>) {
        for range in &ranges {
            let end = usize::from(range.end).min(self.register_count);
            for register in usize::from(range.start)..end {
                state[register] = None;
                state.generations[register] = self.new_generation();
            }
        }
        self.blocks[block].nodes.push(Node::Overlay { ranges });
    }

    /// Whether `binding` of `register`, on a path in `generation` of the
    /// register's word, says what that word holds: no value, or a read of
    /// that very word made in that generation, which nothing on the path
    /// has written since -- a frame node would have unbound the register,
    /// and a write back stores the value it is bound to.
    fn is_frame_word(&self, binding: Option<ValueId>, register: usize, generation: u32) -> bool {
        binding.is_none_or(|value| {
            self.frame_read_of[value.index()].is_some_and(|(read, made_in)| {
                usize::from(read) == register && made_in == generation
            })
        })
    }

    /// The state control leaves `predecessor` for `block` with.
    fn edge_state(&self, predecessor: usize, block: usize) -> &State {
        match &self.fallback_exits[predecessor] {
            Some(fallback) if self.shapes[predecessor].target == Some(block) => fallback,
            _ => &self.exits[predecessor],
        }
    }

    /// What `register` is bound to where control leaves `predecessor`
    /// for `block`.
    fn edge_binding(&self, predecessor: usize, block: usize, register: usize) -> Option<ValueId> {
        self.edge_state(predecessor, block)[register]
    }

    /// Whether `register` is its frame word where control leaves
    /// `predecessor` for `block`.
    fn edge_is_frame_word(&self, predecessor: usize, block: usize, register: usize) -> bool {
        let state = self.edge_state(predecessor, block);
        self.is_frame_word(state[register], register, state.generations[register])
    }

    /// A value for `register` where control leaves `predecessor` for
    /// `block`: its binding, or a read of its frame word at the end of
    /// `predecessor`. Every edge that may need such a read leaves a block
    /// with that one way out (`shapes_of` gives the others a block of
    /// their own), so the read runs only on its way to `block`, where the
    /// register is live: a word the interpreter would not read -- one a
    /// callee left, or a debug build's poison -- is never read.
    fn edge_value(&mut self, predecessor: usize, block: usize, register: usize) -> ValueId {
        if let Some(value) = self.edge_binding(predecessor, block, register) {
            return value;
        }
        assert!(
            self.shapes[predecessor].successors().len() == 1,
            "internal: a frame read for one way out is placed in a block with that way alone"
        );
        let register_number =
            u16::try_from(register).expect("internal: a register number fits a u16");
        let generation = self.edge_state(predecessor, block).generations[register];
        let value = self.new_value(Some((register_number, generation)));
        self.blocks[predecessor].nodes.push(Node::FrameRead {
            register: register_number,
            value,
        });
        self.exits[predecessor][register] = Some(value);
        value
    }

    /// The state at `block`'s top, from its predecessors already built,
    /// with a phi for each live register they disagree on and for each a
    /// loop through the block may change.
    fn merge(&mut self, block: usize) -> State {
        let mut state = State::unbound(self.register_count);
        let predecessors: Vec<usize> = self.blocks[block]
            .predecessors
            .iter()
            .map(|predecessor| predecessor.index())
            .collect();
        let forward: Vec<usize> = predecessors
            .iter()
            .copied()
            .filter(|predecessor| *predecessor < block)
            .collect();
        // A register's word keeps its generation where every way in agrees
        // on it and no pass around a loop through the block writes it;
        // elsewhere a read made before the block is not the word after it.
        for register in 0..self.register_count {
            let mut generations = forward
                .iter()
                .map(|predecessor| self.edge_state(*predecessor, block).generations[register]);
            let first = generations.next();
            let agreed = first.filter(|first| generations.all(|generation| generation == *first));
            let written_around = self.loops[block]
                .as_ref()
                .is_some_and(|facts| facts.written.contains(register));
            state.generations[register] = match agreed {
                Some(generation) if !written_around => generation,
                _ => self.new_generation(),
            };
        }
        let live: Vec<usize> = self.live_in[block].registers().collect();
        let loop_head = self.loops[block].is_some();
        let flat = self.flat;
        let tainted_here = &flat.tainted[self.shapes[block].top];
        for register in live {
            let bindings: Vec<Option<ValueId>> = forward
                .iter()
                .map(|predecessor| self.edge_binding(*predecessor, block, register))
                .collect();
            let frame_words = !bindings.is_empty()
                && forward
                    .iter()
                    .all(|predecessor| self.edge_is_frame_word(*predecessor, block, register));
            // A register that is its frame word on every way in is read
            // again at its next use, with no phi and nothing reloaded or
            // stored on the way in: a read made before the block does not
            // survive a frame node on a later pass around a loop through
            // it, nor any other way that reaches its use through one.
            if frame_words && !loop_head {
                continue;
            }
            // Around a loop, a register changes if the loop writes it, or
            // if a frame node in it leaves the register to its frame word
            // and the register came in bound to anything else.
            let loop_changes = self.loops[block].as_ref().is_some_and(|facts| {
                facts.written.contains(register) || (facts.frame_node && !frame_words)
            });
            // The head of a loop of the program's leaves such a register
            // its frame word too, and so one the loop changes, which a way
            // in from before the loop may bring bound to a value the frame
            // word does not hold yet: that way writes it back once, on its
            // way in (`liveness::join`), rather than every pass around the
            // loop storing and reloading a phi. The back edges must bring
            // the frame word's value, which `fill_back_edge` checks; a head
            // one refutes takes its phi in the next build. Where a fallback
            // rejoins its fast path from behind, the way in is the fast
            // path, the one the program takes, and every live register
            // keeps its phi, which only the fallback's way loads.
            let natural = self.loops[block]
                .as_ref()
                .is_some_and(|facts| facts.natural);
            // A register the leftover analysis taints at the head may hold
            // a word no way in writes back (`Binding::STALE`), so it keeps
            // its phi.
            let tainted = u16::try_from(register)
                .is_ok_and(|register| tainted_here.iter().any(|range| range.contains(&register)));
            if natural
                && (frame_words || loop_changes)
                && !tainted
                && !self.forced_phis[block].contains(register)
            {
                self.unbound_at_head[block].insert(register);
                continue;
            }
            // A register a back edge of an earlier build refuted takes its
            // phi though every way in agrees: the back edge brings another
            // value.
            if !loop_changes && !self.forced_phis[block].contains(register) {
                if let Some(first) = bindings.first() {
                    if bindings.iter().all(|binding| binding == first) {
                        state[register] = *first;
                        continue;
                    }
                }
            }
            let value = self.new_value(None);
            let inputs = predecessors
                .iter()
                .map(|predecessor| {
                    if *predecessor < block {
                        self.edge_value(*predecessor, block, register)
                    } else {
                        UNFILLED
                    }
                })
                .collect();
            self.blocks[block].phis.push(Phi {
                register: u16::try_from(register).expect("internal: a register number fits a u16"),
                value,
                inputs,
            });
            state[register] = Some(value);
        }
        state
    }

    /// Fills the inputs `predecessor`, just built, gives the phis of
    /// `head`, a block at or before it.
    fn fill_back_edge(&mut self, predecessor: usize, head: usize) {
        let position = self.blocks[head]
            .predecessors
            .iter()
            .position(|block| block.index() == predecessor)
            .expect("internal: a back edge's block is its head's predecessor");
        for phi in 0..self.blocks[head].phis.len() {
            let register = usize::from(self.blocks[head].phis[phi].register);
            let value = self.edge_value(predecessor, head, register);
            self.blocks[head].phis[phi].inputs[position] = value;
        }
        // A register the head left its frame word must come back as its
        // frame word's value, or the head is built again with its phi.
        let unbound: Vec<usize> = self.unbound_at_head[head].registers().collect();
        for register in unbound {
            if !self.edge_is_frame_word(predecessor, head, register) {
                self.refuted.push((head, register));
            }
        }
        if cfg!(debug_assertions) {
            // Any other live register the head took no phi for is one no
            // pass around the loop changes: it entered bound to a value no
            // frame node in the loop can reach, which the back edge brings
            // again, or it entered as its frame word, which the back edge
            // may bring as a read of that word.
            let entry = self.head_entries[head]
                .as_ref()
                .expect("internal: a loop head records its state");
            for register in self.live_in[head].registers() {
                let has_phi = self.blocks[head]
                    .phis
                    .iter()
                    .any(|phi| usize::from(phi.register) == register);
                let binding = self.edge_binding(predecessor, head, register);
                debug_assert!(
                    has_phi
                        || self.unbound_at_head[head].contains(register)
                        || (entry[register].is_none()
                            && self.edge_is_frame_word(predecessor, head, register))
                        || (entry[register].is_some() && binding == entry[register]),
                    "internal: register {register} changes around a loop without a phi"
                );
            }
        }
    }

    /// The value `register` holds at this point of the block, read from
    /// its frame word if it is bound to none.
    fn read(&mut self, block: usize, state: &mut State, register: u16) -> ValueId {
        let index = usize::from(register);
        if let Some(value) = state[index] {
            return value;
        }
        let value = self.new_value(Some((register, state.generations[index])));
        self.blocks[block]
            .nodes
            .push(Node::FrameRead { register, value });
        state[index] = Some(value);
        value
    }

    /// The receiver of `scope` at this point of the block: the root's read
    /// from its frame's header, an inlined scope's its header binding.
    fn receiver_of(&mut self, block: usize, state: &mut State, scope: ScopeId) -> ValueId {
        if scope == ScopeId::ROOT {
            let value = self.new_value(None);
            self.blocks[block].nodes.push(Node::Receiver { value });
            return value;
        }
        let register =
            self.flat.graph_scopes[scope.0 as usize].header_register(HeaderBinding::Receiver);
        self.read(block, state, register)
    }

    /// The value of an inlined send's receiver at this point of the block.
    fn receiver_value(&mut self, block: usize, state: &mut State, receiver: ReceiverOf) -> ValueId {
        match receiver {
            ReceiverOf::Register(register) => self.read(block, state, register),
            ReceiverOf::SelfOf(scope) => self.receiver_of(block, state, scope),
        }
    }

    /// The receiver's field at `absolute_index`, read by a new
    /// `Node::OwnSlotLoad` at the end of `block`, the receiver `scope`'s.
    fn own_slot_load(
        &mut self,
        block: usize,
        state: &mut State,
        scope: ScopeId,
        absolute_index: u32,
    ) -> ValueId {
        let receiver = self.receiver_of(block, state, scope);
        let value = self.new_value(None);
        self.blocks[block].nodes.push(Node::OwnSlotLoad {
            value,
            receiver,
            absolute_index,
        });
        value
    }

    fn build_block(&mut self, block: usize) {
        let mut state = if block == 0 {
            State::unbound(self.register_count)
        } else {
            self.merge(block)
        };
        if self.blocks[block].loop_head {
            self.head_entries[block] = Some(state.clone());
        }
        let shape = &self.shapes[block];
        let next = shape.next.map(BlockId::of_index);
        let target = shape.target.map(BlockId::of_index);
        let instructions = shape.instructions.clone();
        let mut end = None;
        for index in instructions {
            end = self.lower(block, index, &mut state, next, target);
        }
        self.blocks[block].end = end.unwrap_or_else(|| {
            End::Jump(next.expect("internal: a block that does not stop goes on"))
        });
        self.exits[block] = state;
        for successor in self.shapes[block].successors() {
            if successor <= block {
                self.fill_back_edge(block, successor);
            }
        }
    }

    /// Lowers the step at `index`, the last of its block only if it
    /// answers an end other than a fall into `next`.
    fn lower(
        &mut self,
        block: usize,
        index: usize,
        state: &mut State,
        next: Option<BlockId>,
        target: Option<BlockId>,
    ) -> Option<End> {
        let flat = self.flat;
        let step = &flat.steps[index];
        let scope = step.scope;
        let program_counter = step.program_counter;
        match self.kind(index) {
            Kind::Constant { destination, word } => {
                let value = self.new_value(None);
                let nodes = &mut self.blocks[block].nodes;
                nodes.push(Node::Constant { value, word });
                nodes.push(Node::Copy {
                    destination_register: destination,
                    source: value,
                });
                self.bind(state, destination, value);
                None
            }
            Kind::Receiver { destination } => {
                let value = self.receiver_of(block, state, scope);
                self.blocks[block].nodes.push(Node::Copy {
                    destination_register: destination,
                    source: value,
                });
                self.bind(state, destination, value);
                None
            }
            Kind::Copy {
                destination,
                source,
            } => {
                let value = self.read(block, state, source);
                self.blocks[block].nodes.push(Node::Copy {
                    destination_register: destination,
                    source: value,
                });
                self.bind(state, destination, value);
                None
            }
            Kind::IntegerBinary {
                operation,
                destination,
                left,
                right,
                trap,
            } => {
                let left = self.read(block, state, left);
                let right = self.read(block, state, right);
                // A fast path whose failure traps has no way to a fallback,
                // and its block need not end at it.
                let fallback = if trap {
                    None
                } else {
                    self.fallback_exits[block] = Some(state.clone());
                    Some(target.expect("internal: a fast path has its fallback"))
                };
                let value = self.new_value(None);
                self.blocks[block].nodes.push(Node::IntegerBinary {
                    operation,
                    program_counter,
                    destination_register: destination,
                    value,
                    left,
                    right,
                    fallback,
                });
                self.bind(state, destination, value);
                None
            }
            Kind::Return { register } => {
                let value = self.read(block, state, register);
                Some(End::Return { value })
            }
            Kind::OwnSlotLoad {
                destination,
                absolute_index,
            } => {
                let value = self.own_slot_load(block, state, scope, absolute_index);
                self.blocks[block].nodes.push(Node::Copy {
                    destination_register: destination,
                    source: value,
                });
                self.bind(state, destination, value);
                None
            }
            Kind::ReturnUnlessNil { absolute_index } => {
                let value = self.own_slot_load(block, state, scope, absolute_index);
                Some(End::ReturnUnlessNil {
                    value,
                    next: next.expect("internal: a lazy slot's getter goes on to its initializer"),
                })
            }
            Kind::Jump => Some(End::Jump(target.expect("internal: a jump has its target"))),
            Kind::Branch {
                register,
                taken,
                third_way,
            } => {
                let condition = self.read(block, state, register);
                let other = next.expect("internal: a branch falls through");
                // A branch to the next instruction goes there either way.
                let taken_block = target.unwrap_or(other);
                let pick = |outcome: Outcome| {
                    if outcome == taken {
                        taken_block
                    } else {
                        other
                    }
                };
                Some(End::Branch {
                    program_counter,
                    condition,
                    if_true: pick(Outcome::True),
                    if_false: pick(Outcome::False),
                    not_boolean: third_way.then(|| pick(Outcome::NotBoolean)),
                })
            }
            Kind::Trap { site } => Some(End::Trap {
                program_counter,
                site,
            }),
            Kind::Frame => {
                let Step::Instruction { physical, original } = &step.step else {
                    unreachable!("internal: only an instruction is a frame node");
                };
                // A template may read its frame's current class: an inlined
                // scope's is computed before each of its frame nodes, and
                // written back with the others.
                if scope != ScopeId::ROOT {
                    let class = self.scope_class(block, state, scope);
                    let register = self.flat.graph_scopes[scope.0 as usize]
                        .header_register(HeaderBinding::CurrentClass);
                    self.blocks[block].nodes.push(Node::Copy {
                        destination_register: register,
                        source: class,
                    });
                    self.bind(state, register, class);
                }
                // What the class's computation reads is no read of the
                // instruction's, which its frame node names.
                let own = physical.registers_read();
                let registers: Vec<u16> = self.reads[index]
                    .iter()
                    .copied()
                    .filter(|register| own.contains(register))
                    .collect();
                let reads = registers
                    .into_iter()
                    .map(|register| (register, self.read(block, state, register)))
                    .collect();
                let falls_through = physical.falls_through();
                self.blocks[block].nodes.push(Node::Frame {
                    scope,
                    program_counter,
                    instruction: original.clone(),
                    reads,
                    written: self.written[index].first().copied(),
                });
                // What the instruction ran may have written any of the
                // frame's registers by reference, or moved what a value
                // names: every register is its frame word again.
                state.unbind_all();
                match (falls_through, next, target) {
                    (false, _, _) => Some(End::Frame),
                    (true, Some(next), Some(target)) if next != target => {
                        Some(End::FrameJump { next, target })
                    }
                    _ => None,
                }
            }
            Kind::TypeCase => {
                let Step::TypeCase {
                    receiver,
                    arms,
                    fall_through,
                    site,
                } = &step.step
                else {
                    unreachable!("internal: a type-case's kind is its step's");
                };
                let receiver = self.receiver_value(block, state, *receiver);
                let block_of = |step: usize| BlockId::of_index(self.block_at[&step]);
                Some(End::TypeCase {
                    program_counter,
                    scope,
                    receiver,
                    arms: arms
                        .iter()
                        .map(|(guard, step)| (*guard, block_of(*step)))
                        .collect(),
                    fall_through: fall_through.map(block_of),
                    site: *site,
                })
            }
            Kind::Enter => {
                let Step::Enter(entry) = &step.step else {
                    unreachable!("internal: an entry's kind is its step's");
                };
                self.enter(block, state, entry);
                None
            }
            Kind::InlinedReturn => {
                let Step::Return {
                    source,
                    destination,
                    slot_end,
                    ..
                } = &step.step
                else {
                    unreachable!("internal: a return's kind is its step's");
                };
                let value = self.read(block, state, *source);
                self.blocks[block].nodes.push(Node::Copy {
                    destination_register: *destination,
                    source: value,
                });
                self.bind(state, *destination, value);
                // What the callee left past its slots is the sender's to
                // write before it reads it, the destination apart.
                let mut ranges = Vec::with_capacity(2);
                if destination >= slot_end {
                    ranges.push(*slot_end..*destination);
                    ranges.push(destination + 1..u16::MAX);
                } else {
                    ranges.push(*slot_end..u16::MAX);
                }
                ranges.retain(|range| !range.is_empty());
                self.overlay(block, state, ranges);
                None
            }
            Kind::Always => {
                let Step::Always(always) = &step.step else {
                    unreachable!("internal: an always-inlined callee's kind is its step's");
                };
                self.always(block, state, always, program_counter);
                None
            }
            Kind::EliminatedBlock {
                destination,
                block: eliminated,
            } => {
                let value = self.new_value(None);
                let nodes = &mut self.blocks[block].nodes;
                nodes.push(Node::EliminatedBlock {
                    value,
                    block: eliminated,
                });
                nodes.push(Node::Copy {
                    destination_register: destination,
                    source: value,
                });
                self.bind(state, destination, value);
                None
            }
        }
    }

    /// A callee always inlined (`AlwaysStep`) at the send at
    /// `program_counter`: its receiver and arguments read, its nodes --
    /// every check among them before anything is bound, so that a trap
    /// leaves the frame as it was before the send -- then the send's
    /// argument slots bound to the arguments and its destination to the
    /// answer, as the interpreter's send leaves them.
    fn always(
        &mut self,
        block: usize,
        state: &mut State,
        always: &AlwaysStep,
        program_counter: u32,
    ) {
        let receiver = self.receiver_value(block, state, always.receiver);
        let arguments: Vec<ValueId> = always
            .arguments
            .iter()
            .map(|(_, source)| self.read(block, state, *source))
            .collect();
        let argument = |position: usize| arguments[position];
        // A setter's store, which checks nothing, laid after the bindings.
        let mut store = None;
        let answer = match always.expressed {
            Expressed::FieldLoad { field } => {
                let value = self.new_value(None);
                self.blocks[block].nodes.push(Node::FieldLoad {
                    value,
                    object: receiver,
                    field,
                });
                value
            }
            Expressed::LazyGetter { field } => {
                let value = self.new_value(None);
                let nodes = &mut self.blocks[block].nodes;
                nodes.push(Node::FieldLoad {
                    value,
                    object: receiver,
                    field,
                });
                nodes.push(Node::TrapIfNil {
                    value,
                    program_counter,
                });
                value
            }
            Expressed::FieldStore { field } => {
                store = Some(Node::FieldStore {
                    object: receiver,
                    field,
                    source: argument(0),
                });
                receiver
            }
            Expressed::Constant(word) => {
                let value = self.new_value(None);
                self.blocks[block]
                    .nodes
                    .push(Node::Constant { value, word });
                value
            }
            Expressed::Receiver => receiver,
            Expressed::Parameter(position) => argument(position),
            Expressed::Identity => {
                let value = self.new_value(None);
                self.blocks[block].nodes.push(Node::IdentityCompare {
                    value,
                    left: receiver,
                    right: argument(0),
                    program_counter,
                });
                value
            }
            Expressed::ClassOf => {
                let value = self.new_value(None);
                self.blocks[block].nodes.push(Node::ClassOf {
                    value,
                    object: receiver,
                });
                value
            }
            Expressed::IndexedLoad(kind) => {
                let value = self.new_value(None);
                self.blocks[block].nodes.push(Node::IndexedLoad {
                    kind,
                    value,
                    object: receiver,
                    index: argument(0),
                    program_counter,
                });
                value
            }
            Expressed::IndexedStore(kind) => {
                self.blocks[block].nodes.push(Node::IndexedStore {
                    kind,
                    object: receiver,
                    index: argument(0),
                    source: argument(1),
                    program_counter,
                });
                argument(1)
            }
            Expressed::Integer(operation) => {
                let value = self.new_value(None);
                self.blocks[block].nodes.push(Node::IntegerBinary {
                    operation,
                    program_counter,
                    destination_register: always.destination,
                    value,
                    left: receiver,
                    right: argument(0),
                    fallback: None,
                });
                value
            }
        };
        let bindings = always
            .arguments
            .iter()
            .map(|(slot, _)| *slot)
            .zip(arguments.iter().copied())
            .chain([(always.destination, answer)]);
        for (register, value) in bindings.collect::<Vec<_>>() {
            self.blocks[block].nodes.push(Node::Copy {
                destination_register: register,
                source: value,
            });
            self.bind(state, register, value);
        }
        // A store with no check may follow the bindings, no interrupt point
        // between: so the receiver it answers -- a value of the header,
        // bound to no register before -- crosses the barrier's leaf call
        // bound to the destination, whose frame word the allocation can
        // keep it in if no register that survives the call is free.
        if let Some(store) = store {
            self.blocks[block].nodes.push(store);
        }
    }

    /// An inlined scope's entry (`Entry`): the values its sender hands it
    /// read first, then bound to its parameters, its header's receiver and
    /// current class and its named temporaries, and the rest of what its
    /// frame lays over left unbound.
    fn enter(&mut self, block: usize, state: &mut State, entry: &Entry) {
        let receiver = self.receiver_value(block, state, entry.receiver);
        let arguments: Vec<(u16, ValueId)> = entry
            .arguments
            .iter()
            .map(|(parameter, source)| (*parameter, self.read(block, state, *source)))
            .collect();
        if self.flat.scopes[self.blocks[block].scope.0 as usize]
            .snapshot
            .facts
            .addresses_own_slots
        {
            self.blocks[block].nodes.push(Node::ResetOwnSlotGuard);
        }
        let mut bindings = arguments;
        bindings.push((entry.receiver_register, receiver));
        if !entry.temporaries.is_empty() {
            let nil = self.new_value(None);
            self.blocks[block].nodes.push(Node::Constant {
                value: nil,
                word: Immediate::NIL.bits(),
            });
            bindings.extend(entry.temporaries.clone().map(|register| (register, nil)));
        }
        for (register, value) in bindings {
            self.blocks[block].nodes.push(Node::Copy {
                destination_register: register,
                source: value,
            });
            self.bind(state, register, value);
        }
        // The current class is computed where a frame node reads it
        // (`Builder::scope_class`): until then its header word is the
        // exit's to write, among the overlaid words.
        self.overlay(block, state, entry.overlay.clone());
    }

    /// `scope`'s current class, an inlined scope's, computed as a compiled
    /// call computes its callee's: from the scope's receiver, or from the
    /// current class of the scope its recipe names, that one's computed
    /// in turn unless it is the root's, which its frame's header holds.
    fn scope_class(&mut self, block: usize, state: &mut State, scope: ScopeId) -> ValueId {
        let graph_scope = &self.flat.graph_scopes[scope.0 as usize];
        let recipe = graph_scope
            .class
            .expect("internal: an inlined scope has its class's recipe");
        let receiver_register = graph_scope.header_register(HeaderBinding::Receiver);
        let from = match recipe.from {
            None => self.read(block, state, receiver_register),
            Some(ScopeId::ROOT) => {
                let register =
                    self.flat.graph_scopes[0].header_register(HeaderBinding::CurrentClass);
                self.read(block, state, register)
            }
            Some(source) => self.scope_class(block, state, source),
        };
        let value = self.new_value(None);
        self.blocks[block].nodes.push(Node::CurrentClass {
            value,
            from,
            start: recipe.start,
            steps: recipe.steps,
        });
        value
    }
}

/// The physical registers the step reads, as the builder's liveness
/// counts them.
fn step_reads(flat: &Flat<'_>, step: &FlatStep, index: usize, choice: Choice) -> Vec<u16> {
    match &step.step {
        Step::Instruction { .. } if choice.send_only => Vec::new(),
        Step::Instruction { physical, .. } => {
            let mut reads = physical.registers_read();
            // An inlined scope's frame node is preceded by its current
            // class's computation (`Builder::scope_class`).
            if step.scope != ScopeId::ROOT
                && matches!(
                    kind_of(
                        physical,
                        index,
                        choice,
                        flat.scope(step.scope).own_slot_base
                    ),
                    Kind::Frame
                )
            {
                reads.extend(class_reads(flat, step.scope));
            }
            reads
        }
        Step::TypeCase { receiver, .. } => receiver_reads(flat, *receiver),
        Step::Enter(entry) => {
            let mut reads = receiver_reads(flat, entry.receiver);
            reads.extend(entry.arguments.iter().map(|(_, source)| *source));
            reads
        }
        Step::Return { source, .. } => vec![*source],
        Step::Always(always) => {
            let mut reads = receiver_reads(flat, always.receiver);
            reads.extend(always.arguments.iter().map(|(_, source)| *source));
            reads
        }
        // Its copied values are read where its scope is entered.
        Step::Eliminated { .. } => Vec::new(),
    }
}

/// The registers the computation of the inlined scope `scope`'s current
/// class reads (`Builder::scope_class`): its receiver's, or what the scope
/// its recipe names reads, the root's current class word for the root.
fn class_reads(flat: &Flat<'_>, scope: ScopeId) -> Vec<u16> {
    let graph_scope = &flat.graph_scopes[scope.0 as usize];
    let recipe = graph_scope
        .class
        .expect("internal: an inlined scope has its class's recipe");
    match recipe.from {
        None => vec![graph_scope.header_register(HeaderBinding::Receiver)],
        Some(ScopeId::ROOT) => {
            vec![flat.graph_scopes[0].header_register(HeaderBinding::CurrentClass)]
        }
        Some(source) => class_reads(flat, source),
    }
}

/// The register an inlined send's receiver is read from, if it is one:
/// an inlined sender's `self` is its header's binding.
fn receiver_reads(flat: &Flat<'_>, receiver: ReceiverOf) -> Vec<u16> {
    match receiver {
        ReceiverOf::Register(register) => vec![register],
        ReceiverOf::SelfOf(scope) if scope != ScopeId::ROOT => {
            vec![flat.graph_scopes[scope.0 as usize].header_register(HeaderBinding::Receiver)]
        }
        ReceiverOf::SelfOf(_) => Vec::new(),
    }
}

/// The physical registers the step writes on its way to the next.
fn step_writes(step: &FlatStep, choice: Choice) -> Vec<u16> {
    match &step.step {
        Step::Instruction { physical, .. } if choice.may_fall_through() => {
            physical.register_written().into_iter().collect()
        }
        Step::Instruction { .. } | Step::TypeCase { .. } => Vec::new(),
        Step::Enter(entry) => {
            let mut written: Vec<u16> = entry.arguments.iter().map(|(slot, _)| *slot).collect();
            written.push(entry.receiver_register);
            written.push(entry.class_register);
            written.extend(entry.temporaries.clone());
            written
        }
        Step::Return { destination, .. } | Step::Eliminated { destination, .. } => {
            vec![*destination]
        }
        Step::Always(always) => always
            .arguments
            .iter()
            .map(|(slot, _)| *slot)
            .chain([always.destination])
            .collect(),
    }
}

/// Where control goes from the step at `index` of `flat` (`Edges`).
fn edges_of(flat: &Flat<'_>, index: usize, choice: Choice) -> Edges {
    let step = &flat.steps[index];
    match &step.step {
        Step::Instruction { physical, .. } => {
            // A jump to the fallback does not fall through, nor a send
            // that traps; and a fast path whose failure traps, or a
            // conditional with no third way, has no jump target
            // (`kind_of`).
            let falls_through = choice.may_fall_through() && physical.falls_through();
            let kind = kind_of(
                physical,
                index,
                choice,
                flat.scope(step.scope).own_slot_base,
            );
            Edges {
                next: falls_through.then_some(index + 1),
                target: physical
                    .jump_target()
                    .filter(|_| choice.keeps_target())
                    .map(|target| usize::try_from(target).expect("internal: a step fits a usize")),
                others: Vec::new(),
                // A customized copy's `ReturnSlotIfFilled` ends its block
                // in the slot's return, and every branch in its test --
                // what decides for a `JumpIfNotBoolean` built without its
                // third way, which keeps no jump target.
                ends_block: matches!(kind, Kind::ReturnUnlessNil { .. } | Kind::Branch { .. }),
                starts_block: false,
            }
        }
        Step::TypeCase {
            arms, fall_through, ..
        } => Edges {
            next: None,
            target: None,
            others: arms
                .iter()
                .map(|(_, step)| *step)
                .chain(*fall_through)
                .collect(),
            ends_block: true,
            starts_block: false,
        },
        Step::Enter(_) => Edges {
            next: Some(index + 1),
            target: None,
            others: Vec::new(),
            ends_block: false,
            starts_block: true,
        },
        Step::Return { continuation, .. } => Edges {
            next: Some(*continuation),
            target: None,
            others: Vec::new(),
            ends_block: true,
            starts_block: false,
        },
        // An arm goes on after its send, where the type-case's ways meet;
        // a callee taken without a test lies where the send stands.
        Step::Always(always) => Edges {
            next: Some(always.continuation),
            target: None,
            others: Vec::new(),
            ends_block: false,
            starts_block: false,
        },
        Step::Eliminated { .. } => Edges {
            next: Some(index + 1),
            target: None,
            others: Vec::new(),
            ends_block: false,
            starts_block: false,
        },
    }
}

/// A run of steps, the run its last step falls through to, the run its
/// jump target begins, and the runs a type-case leads to.
type Run = (Range<usize>, Option<usize>, Option<usize>, Vec<usize>);

/// The blocks of `flat`'s steps: one for every run that starts at the
/// first step, a jump target, an inlined scope's entry or after a step
/// that jumps or stops and is reachable from the first, in order; an
/// entry block of its own before them when the first step heads a loop;
/// and an edge block, with no step, for each way out of a block with two
/// ways out -- a branch, an integer fast path, a mirror send -- into a
/// block other ways enter too, or back to a block at or before it. Only
/// such a block's top can take a phi, and a phi's input may need a read
/// of a frame word placed on its way alone: on the other way the register
/// may be dead, its word one a callee left, and a mirror send's template
/// jumps for its failure itself, leaving no room after it on one way only.
/// A type-case's ways out each enter a block only it enters: an arm's
/// entry, or the send it falls through to. A customized copy's
/// `ReturnSlotIfFilled` ends its run too (`Edges::ends_block`), its block
/// ending in the slot's return. Answers the shapes and, for each run, the
/// block it begins by its first step.
fn shapes_of(flat: &Flat<'_>, edges: &[Edges]) -> (Vec<Shape>, HashMap<usize, usize>) {
    let count = edges.len();
    let checked = |step: usize| {
        assert!(step < count, "internal: a jump target lies in its body");
        step
    };
    let mut starts = vec![false; count];
    starts[0] = true;
    for (index, edge) in edges.iter().enumerate() {
        if edge.starts_block {
            starts[index] = true;
        }
        for target in edge.target.iter().chain(&edge.others) {
            starts[checked(*target)] = true;
        }
        let falls_on = edge.next == Some(index + 1);
        if let Some(next) = edge.next.filter(|_| !falls_on) {
            starts[checked(next)] = true;
        }
        if (edge.target.is_some() || !edge.others.is_empty() || !falls_on || edge.ends_block)
            && index + 1 < count
        {
            starts[index + 1] = true;
        }
    }
    // The runs, each with the runs it may go to.
    let mut run_of = vec![usize::MAX; count];
    let mut runs: Vec<Run> = Vec::new();
    for index in 0..count {
        if starts[index] {
            runs.push((index..index + 1, None, None, Vec::new()));
        } else {
            runs.last_mut()
                .expect("internal: the first step starts a run")
                .0
                .end = index + 1;
        }
        run_of[index] = runs.len() - 1;
    }
    for run in &mut runs {
        let last = run.0.end - 1;
        let edge = &edges[last];
        let target = edge.target.map(|target| run_of[target]);
        let next = edge.next.map(|next| {
            assert!(next < count, "internal: a body does not run off its end");
            run_of[next]
        });
        // A step whose target is the next goes on there either way.
        run.1 = next;
        run.2 = if target == next { None } else { target };
        run.3 = edge.others.iter().map(|other| run_of[*other]).collect();
    }
    // The runs reachable from the first.
    let mut reachable = vec![false; runs.len()];
    let mut pending = vec![0usize];
    while let Some(run) = pending.pop() {
        if std::mem::replace(&mut reachable[run], true) {
            continue;
        }
        pending.extend([runs[run].1, runs[run].2].into_iter().flatten());
        pending.extend(runs[run].3.iter().copied());
    }
    let mut entered = vec![0usize; runs.len()];
    for (run, (_, next, target, others)) in runs.iter().enumerate() {
        if reachable[run] {
            for successor in [*next, *target]
                .into_iter()
                .flatten()
                .chain(others.iter().copied())
            {
                entered[successor] += 1;
            }
        }
    }
    let entry_block = entered[0] > 0;
    if entry_block {
        // The body's entry is a way into the first run too.
        entered[0] += 1;
    }
    // Each reachable run's block, and the edge blocks its ways out need.
    let mut block_of_run = vec![usize::MAX; runs.len()];
    let mut block_count = usize::from(entry_block);
    let mut edge_blocks: Vec<(usize, [bool; 2])> = Vec::new();
    for (run, (_, next, target, others)) in runs.iter().enumerate() {
        if !reachable[run] {
            continue;
        }
        debug_assert!(
            others
                .iter()
                .all(|other| entered[*other] == 1 && *other > run),
            "internal: a type-case's every way enters a block only it enters"
        );
        block_of_run[run] = block_count;
        block_count += 1;
        let forks = target.is_some() && next.is_some();
        let needs_edge = |successor: Option<usize>| {
            forks && successor.is_some_and(|successor| entered[successor] > 1 || successor <= run)
        };
        let split = [needs_edge(*next), needs_edge(*target)];
        if split.iter().any(|split| *split) {
            edge_blocks.push((run, split));
            block_count += split.iter().filter(|split| **split).count();
        }
    }
    let place = |run: usize| {
        let step = &flat.steps[runs[run].0.start];
        (step.scope, step.program_counter)
    };
    let mut shapes = Vec::with_capacity(block_count);
    if entry_block {
        shapes.push(Shape {
            scope: ScopeId::ROOT,
            program_counter: 0,
            instructions: 0..0,
            top: 0,
            next: Some(block_of_run[0]),
            target: None,
            others: Vec::new(),
        });
    }
    let mut block_at = HashMap::new();
    let mut edge_blocks = edge_blocks.into_iter().peekable();
    for (run, (range, next, target, others)) in runs.iter().enumerate() {
        if !reachable[run] {
            continue;
        }
        let (scope, program_counter) = place(run);
        block_at.insert(range.start, shapes.len());
        let mut shape = Shape {
            scope,
            program_counter,
            instructions: range.clone(),
            top: range.start,
            next: next.map(|next| block_of_run[next]),
            target: target.map(|target| block_of_run[target]),
            others: others.iter().map(|other| block_of_run[*other]).collect(),
        };
        let mut edges = Vec::new();
        if edge_blocks
            .peek()
            .is_some_and(|(edge_run, _)| *edge_run == run)
        {
            let (_, split) = edge_blocks.next().expect("internal: peeked");
            let mut following = shapes.len() + 1;
            for (way, to_run, successor) in [
                (split[0], *next, &mut shape.next),
                (split[1], *target, &mut shape.target),
            ] {
                if way {
                    let to_run = to_run.expect("internal: a split way exists");
                    // An edge block holds nothing of the block it leads
                    // to: on the way into an inlined scope's entry it is
                    // still its sender's, at the send.
                    let (scope, program_counter) = flat.taint_point(runs[to_run].0.start);
                    edges.push(Shape {
                        scope,
                        program_counter,
                        instructions: range.end..range.end,
                        top: runs[to_run].0.start,
                        next: Some(block_of_run[to_run]),
                        target: None,
                        others: Vec::new(),
                    });
                    *successor = Some(following);
                    following += 1;
                }
            }
        }
        shapes.push(shape);
        shapes.extend(edges);
    }
    (shapes, block_at)
}

#[cfg(test)]
mod tests {
    use ns_syntax::Span;

    use super::build_speculating as build;
    use crate::bytecode::{
        CallSite, CodeKind, CompiledCode, Instruction, IntegerOperation, Literal,
    };
    use crate::heap::CodeId;
    use crate::interpreter::inline_cache::FeedbackBase;
    use crate::jit::optimizing::graph::{BlockId, Definition, End, Graph, Node, ValueKind};
    use crate::jit::optimizing::snapshot::{
        tainted_before_each, BodyFacts, Customization, SiteFacts, Snapshot, SnapshotInstruction,
    };
    use crate::jit::optimizing::verify::verify;
    use crate::jit::optimizing::{
        corpus_of_bodies, shipped_bodies, unrun_every_site, unrun_site, Speculation,
    };
    use crate::jit::CustomizationKey;
    use crate::mixin::DispatchId;
    use crate::Symbol;

    /// The snapshot of a method of `instructions`, none of which makes a
    /// closure, with registers to spare.
    fn snapshot_of(instructions: Vec<Instruction>) -> Snapshot {
        let code = CompiledCode::new(
            CodeKind::Method,
            instructions,
            0,
            30,
            0,
            Vec::new(),
            Span::new(0, 0),
        );
        Snapshot {
            code_id: CodeId::from_raw(0),
            facts: BodyFacts::of(&code),
            feedback_base: FeedbackBase::NOT_ASSIGNED,
            instructions: code
                .instructions()
                .iter()
                .map(|instruction| {
                    SnapshotInstruction::of_unclosed(instruction).expect("no closure")
                })
                .collect(),
            tainted_before: tainted_before_each(&code),
            fallback_taken: vec![false; code.instructions().len()],
            sites: vec![None; code.instructions().len()],
            customization: None,
        }
    }

    fn send(receiver: u16, argument: u16, destination: u16) -> Instruction {
        Instruction::SendPublic {
            selector: Symbol::NEW,
            receiver_register: receiver,
            argument_base: argument,
            argument_sources: vec![argument],
            destination,
            call_site: CallSite { number: 0 },
        }
    }

    #[test]
    fn a_move_binds_a_register_to_its_source_s_value() {
        let snapshot = snapshot_of(vec![
            Instruction::LoadSelf { destination: 3 },
            Instruction::Move {
                destination: 4,
                source: 3,
            },
            Instruction::Return { register: 4 },
        ]);
        let graph = build(&snapshot);
        let End::Return { value } = graph.blocks[0].end else {
            panic!("the body returns");
        };
        assert!(matches!(
            graph.definition(value),
            Definition::Node(Node::Receiver { .. })
        ));
    }

    /// `r3 := 0. [r3 := r3 + 1. r3 < 10] whileTrue`, each operation's
    /// fast path beside its fallback send.
    fn counting_loop() -> Vec<Instruction> {
        vec![
            Instruction::LoadLiteral {
                destination: 3,
                literal: Literal::SmallInteger(0),
            },
            // 1: the loop's head.
            Instruction::LoadLiteral {
                destination: 5,
                literal: Literal::SmallInteger(1),
            },
            Instruction::IntegerBinary {
                operation: IntegerOperation::Add,
                destination: 6,
                left: 3,
                right: 5,
                target: 4,
            },
            Instruction::Jump { target: 5 },
            send(3, 5, 6),
            Instruction::Move {
                destination: 3,
                source: 6,
            },
            Instruction::LoadLiteral {
                destination: 7,
                literal: Literal::SmallInteger(10),
            },
            Instruction::IntegerBinary {
                operation: IntegerOperation::Less,
                destination: 4,
                left: 3,
                right: 7,
                target: 9,
            },
            Instruction::Jump { target: 10 },
            send(3, 7, 4),
            Instruction::BranchIfTrue {
                register: 4,
                target: 1,
            },
            Instruction::Return { register: 3 },
        ]
    }

    #[test]
    fn a_loop_head_takes_a_phi_for_the_register_its_body_writes() {
        let graph = build(&snapshot_of(counting_loop()));
        let head = graph.loop_heads().next().expect("the body loops");
        assert_eq!(graph.block(head).program_counter, 1);
        assert_eq!(graph.block(head).phis.len(), 1);
        assert_eq!(graph.block(head).phis[0].register, 3);
    }

    /// `counting_loop` with each fast path's fallback the way the program
    /// takes: each operation is its fallback send alone, entered by a jump
    /// from where the operation stood, and the fast paths' jumps over their
    /// sends are never built.
    #[test]
    fn a_fast_path_whose_fallback_is_taken_is_its_fallback_send_alone() {
        let mut snapshot = snapshot_of(counting_loop());
        snapshot.fallback_taken[2] = true;
        snapshot.fallback_taken[7] = true;
        let graph = build(&snapshot);
        assert_eq!(verify(&graph), Ok(()));
        let nodes = || graph.blocks.iter().flat_map(|block| &block.nodes);
        assert!(nodes().all(|node| !matches!(node, Node::IntegerBinary { .. })));
        let frames: Vec<u32> = nodes()
            .filter_map(|node| match node {
                Node::Frame {
                    program_counter, ..
                } => Some(*program_counter),
                _ => None,
            })
            .collect();
        assert_eq!(
            frames,
            vec![4, 9],
            "the two fallback sends, at their own counters"
        );
        assert!(
            graph
                .blocks
                .iter()
                .all(|block| block.program_counter != 3 && block.program_counter != 8),
            "no fast path's jump over its send"
        );
        // Nothing merges after a send: every block has one way in but the
        // loop's head.
        assert!(graph
            .blocks
            .iter()
            .filter(|block| !block.loop_head)
            .all(|block| block.predecessors.len() <= 1));
    }

    /// `r5 := 1. r6 := r3 new: r5. [r3 := r3 new: r5. r3 < r5] whileTrue.
    /// ^r3`: the send before the loop and the one in it leave r3 and r5
    /// their frame words on every way into the head, the back edge
    /// included, so the head takes no phi for either, though the loop
    /// writes r3.
    fn a_loop_that_writes_a_register_only_by_a_send() -> Vec<Instruction> {
        vec![
            Instruction::LoadLiteral {
                destination: 5,
                literal: Literal::SmallInteger(1),
            },
            send(3, 5, 6),
            // 2: the loop's head.
            send(3, 5, 3),
            Instruction::IntegerBinary {
                operation: IntegerOperation::Less,
                destination: 4,
                left: 3,
                right: 5,
                target: 5,
            },
            Instruction::Jump { target: 6 },
            send(3, 5, 4),
            Instruction::BranchIfTrue {
                register: 4,
                target: 2,
            },
            Instruction::Return { register: 3 },
        ]
    }

    #[test]
    fn a_loop_head_takes_no_phi_for_a_register_every_way_brings_as_its_frame_word() {
        let graph = build(&snapshot_of(a_loop_that_writes_a_register_only_by_a_send()));
        assert_eq!(verify(&graph), Ok(()));
        let head = graph.loop_heads().next().expect("the body loops");
        assert_eq!(graph.block(head).program_counter, 2);
        assert!(graph.block(head).phis.is_empty());
        // Where the fast path and its fallback meet, only the destination
        // the branch reads takes a phi: r3 and r5 are frame words there.
        let merge = graph
            .blocks
            .iter()
            .find(|block| block.program_counter == 6 && block.predecessors.len() == 2)
            .expect("the merge");
        let registers: Vec<u16> = merge.phis.iter().map(|phi| phi.register).collect();
        assert_eq!(registers, vec![4]);
    }

    /// `r5 := 1. r6 := r3 new: r5. [r3 := r3 + r5. r3 < r5] whileTrue.
    /// ^r3`: every way into the head seen before the loop brings r3 and r5
    /// as their frame words, so the first build leaves both so; the back
    /// edge brings r3 bound to the fast path's result, which refutes it,
    /// and the second build forces r3's phi alone.
    #[test]
    fn a_back_edge_bringing_another_value_forces_the_head_s_phi() {
        let graph = build(&snapshot_of(vec![
            Instruction::LoadLiteral {
                destination: 5,
                literal: Literal::SmallInteger(1),
            },
            send(3, 5, 6),
            // 2: the loop's head.
            Instruction::IntegerBinary {
                operation: IntegerOperation::Add,
                destination: 3,
                left: 3,
                right: 5,
                target: 4,
            },
            Instruction::Jump { target: 5 },
            send(3, 5, 3),
            Instruction::IntegerBinary {
                operation: IntegerOperation::Less,
                destination: 4,
                left: 3,
                right: 5,
                target: 7,
            },
            Instruction::Jump { target: 8 },
            send(3, 5, 4),
            Instruction::BranchIfTrue {
                register: 4,
                target: 2,
            },
            Instruction::Return { register: 3 },
        ]));
        assert_eq!(verify(&graph), Ok(()));
        let head = graph.loop_heads().next().expect("the body loops");
        assert_eq!(graph.block(head).program_counter, 2);
        let registers: Vec<u16> = graph
            .block(head)
            .phis
            .iter()
            .map(|phi| phi.register)
            .collect();
        assert_eq!(registers, vec![3]);
    }

    /// `a_loop_that_writes_a_register_only_by_a_send` with r3 tainted at
    /// the head: its word may be one no way in writes back, so the head
    /// takes r3's phi, and leaves r5, untainted, its frame word.
    #[test]
    fn a_register_tainted_at_a_loop_head_keeps_its_phi() {
        let mut snapshot = snapshot_of(a_loop_that_writes_a_register_only_by_a_send());
        snapshot.tainted_before[2] = std::iter::once(3..4).collect();
        let graph = build(&snapshot);
        assert_eq!(verify(&graph), Ok(()));
        let head = graph.loop_heads().next().expect("the body loops");
        assert_eq!(graph.block(head).program_counter, 2);
        let registers: Vec<u16> = graph
            .block(head)
            .phis
            .iter()
            .map(|phi| phi.register)
            .collect();
        assert_eq!(registers, vec![3]);
    }

    /// `r5 := 1. r4 := r3 + r5. ^r3`, the addition's fallback send laid
    /// out after the return and jumping back to it: the return's block is
    /// a loop head that does not dominate its back edge, so the rule that
    /// leaves a natural head's registers their frame words does not hold
    /// there, and r3 -- its frame word on both ways in -- keeps its phi.
    #[test]
    fn a_head_a_fallback_rejoins_from_behind_keeps_its_phis() {
        let graph = build(&snapshot_of(vec![
            Instruction::LoadLiteral {
                destination: 5,
                literal: Literal::SmallInteger(1),
            },
            Instruction::IntegerBinary {
                operation: IntegerOperation::Add,
                destination: 4,
                left: 3,
                right: 5,
                target: 3,
            },
            // 2: entered by the fast path and, from behind, the fallback.
            Instruction::Return { register: 3 },
            send(3, 5, 4),
            Instruction::Jump { target: 2 },
        ]));
        assert_eq!(verify(&graph), Ok(()));
        let head = graph
            .blocks
            .iter()
            .position(|block| block.program_counter == 2 && block.loop_head)
            .map(BlockId::of_index)
            .expect("the return's block heads a loop");
        let registers: Vec<u16> = graph
            .block(head)
            .phis
            .iter()
            .map(|phi| phi.register)
            .collect();
        assert_eq!(registers, vec![3]);
    }

    /// `t := r1. r1 := 0. r1 := t. r2 ifTrue: [...]. ^r1`: r1 is bound
    /// again to the read of its frame word made before the `0` was
    /// written, which the allocator may have stored into the word since,
    /// so where the ways meet r1 is that read's value, not its frame word.
    #[test]
    fn a_register_bound_again_to_a_read_made_before_its_write_is_not_its_frame_word() {
        let graph = build(&snapshot_of(vec![
            Instruction::Move {
                destination: 20,
                source: 1,
            },
            Instruction::LoadLiteral {
                destination: 1,
                literal: Literal::SmallInteger(0),
            },
            Instruction::Move {
                destination: 1,
                source: 20,
            },
            Instruction::BranchIfTrue {
                register: 2,
                target: 5,
            },
            Instruction::Move {
                destination: 6,
                source: 2,
            },
            Instruction::Return { register: 1 },
        ]));
        assert_eq!(verify(&graph), Ok(()));
        let merge = graph
            .blocks
            .iter()
            .find(|block| matches!(block.end, End::Return { .. }))
            .expect("the merge returns");
        assert_eq!(merge.predecessors.len(), 2, "the ways meet at the return");
        let End::Return { value } = merge.end else {
            unreachable!("the merge returns");
        };
        assert!(
            matches!(
                graph.definition(value),
                Definition::Node(Node::FrameRead { register: 1, .. })
            ),
            "the read made at the start"
        );
        assert!(merge.nodes.is_empty(), "r1's word is not read again");
    }

    /// `r6 := r3 new. [r1 ifTrue: [r1 := 0. ^r1]. r2 := r1. r2] whileTrue.
    /// ^r2`: the loop reads r1 from its frame word, and only the exit arm,
    /// laid out before the back edge, writes it. The arm's write is on no
    /// path around the loop, so the back edge brings r1 as its frame word
    /// and the head takes no phi for it.
    #[test]
    fn a_write_in_a_loop_s_exit_arm_leaves_the_loop_s_read_its_frame_word() {
        let graph = build(&snapshot_of(vec![
            unary_send(3, 6),
            // 1: the loop's head.
            Instruction::BranchIfFalse {
                register: 1,
                target: 4,
            },
            Instruction::LoadLiteral {
                destination: 1,
                literal: Literal::SmallInteger(0),
            },
            Instruction::Return { register: 1 },
            Instruction::Move {
                destination: 2,
                source: 1,
            },
            Instruction::BranchIfTrue {
                register: 2,
                target: 1,
            },
            Instruction::Return { register: 2 },
        ]));
        assert_eq!(verify(&graph), Ok(()));
        let head = graph.loop_heads().next().expect("the body loops");
        assert_eq!(graph.block(head).program_counter, 1);
        assert!(graph.block(head).phis.is_empty());
    }

    /// `r6 := r3 new. [r5 := 1. r4 := r3 + r5. r7 := r3. r7] whileTrue.
    /// ^r3`, the addition's fallback send laid out after the return and
    /// jumping back into the loop: the block it rejoins does not dominate
    /// its back edge, so it takes r3's phi, which the loop's back edge
    /// brings. The loop's head, refuted, takes r3's phi in the next build,
    /// though every way into it brings r3 as its frame word.
    #[test]
    fn a_head_a_back_edge_refuted_takes_its_phi_though_every_way_in_agrees() {
        let graph = build(&snapshot_of(vec![
            unary_send(3, 6),
            // 1: the loop's head.
            Instruction::LoadLiteral {
                destination: 5,
                literal: Literal::SmallInteger(1),
            },
            Instruction::IntegerBinary {
                operation: IntegerOperation::Add,
                destination: 4,
                left: 3,
                right: 5,
                target: 6,
            },
            // 3: entered by the fast path and, from behind, the fallback.
            Instruction::Move {
                destination: 7,
                source: 3,
            },
            Instruction::BranchIfTrue {
                register: 7,
                target: 1,
            },
            Instruction::Return { register: 3 },
            send(3, 5, 4),
            Instruction::Jump { target: 3 },
        ]));
        assert_eq!(verify(&graph), Ok(()));
        let head = graph
            .blocks
            .iter()
            .position(|block| block.program_counter == 1 && block.loop_head)
            .map(BlockId::of_index)
            .expect("the loop's head");
        let registers: Vec<u16> = graph
            .block(head)
            .phis
            .iter()
            .map(|phi| phi.register)
            .collect();
        assert_eq!(registers, vec![3]);
    }

    #[test]
    fn a_send_is_a_frame_node_reading_its_argument_sources() {
        let graph = build(&snapshot_of(vec![
            send(3, 5, 6),
            Instruction::Return { register: 6 },
        ]));
        let Node::Frame { reads, written, .. } = &graph.blocks[0].nodes[2] else {
            panic!("the third node is the send");
        };
        assert_eq!(
            reads
                .iter()
                .map(|(register, _)| *register)
                .collect::<Vec<_>>(),
            vec![3, 5]
        );
        assert_eq!(*written, Some(6));
    }

    #[test]
    fn the_fast_path_and_its_fallback_meet_in_a_phi() {
        let graph = build(&snapshot_of(counting_loop()));
        // The `Move` at 5 reads the sum the fast path or the send made.
        let rejoin = graph
            .blocks
            .iter()
            .find(|block| block.program_counter == 5)
            .expect("a block starts where the two ways meet");
        assert_eq!(rejoin.phis.len(), 1);
        assert_eq!(rejoin.phis[0].register, 6);
        let mut kinds: Vec<_> = rejoin.phis[0]
            .inputs
            .iter()
            .map(|input| match graph.definition(*input) {
                Definition::Node(Node::IntegerBinary { .. }) => "fast path",
                Definition::Node(Node::FrameRead { register: 6, .. }) => "result of the send",
                _ => "other",
            })
            .collect();
        kinds.sort_unstable();
        assert_eq!(kinds, ["fast path", "result of the send"]);
    }

    #[test]
    fn a_branch_goes_on_at_the_next_instruction_for_its_other_two_outcomes() {
        let graph = build(&snapshot_of(vec![
            Instruction::BranchIfFalse {
                register: 3,
                target: 2,
            },
            Instruction::Return { register: 3 },
            Instruction::Return { register: 4 },
        ]));
        let End::Branch {
            if_true,
            if_false,
            not_boolean,
            ..
        } = graph.blocks[0].end
        else {
            panic!("the body branches");
        };
        assert_eq!(graph.block(if_false).program_counter, 2);
        assert_eq!(graph.block(if_true).program_counter, 1);
        assert_eq!(not_boolean, Some(if_true));
    }

    #[test]
    fn unreachable_code_is_not_built() {
        let graph = build(&snapshot_of(vec![
            Instruction::Return { register: 3 },
            Instruction::LoadSelf { destination: 4 },
            Instruction::Return { register: 4 },
        ]));
        assert_eq!(graph.blocks.len(), 1);
    }

    #[test]
    fn a_body_whose_first_instruction_heads_a_loop_gets_an_entry_block() {
        let graph = build(&snapshot_of(vec![
            send(3, 5, 4),
            Instruction::BranchIfTrue {
                register: 4,
                target: 0,
            },
            Instruction::Return { register: 4 },
        ]));
        assert!(graph.blocks[0].nodes.is_empty());
        assert!(!graph.blocks[0].loop_head);
        assert_eq!(graph.blocks[0].end, End::Jump(BlockId(1)));
        assert!(graph.blocks[1].loop_head);
        assert_eq!(graph.blocks[1].program_counter, 0);
        // The branch's way back has a block of its own, right after it.
        assert_eq!(graph.blocks[1].predecessors, vec![BlockId(0), BlockId(2)]);
        assert!(graph.blocks[2].nodes.is_empty());
        assert_eq!(graph.blocks[2].end, End::Jump(BlockId(1)));
    }

    #[test]
    fn a_frame_node_leaves_every_register_to_its_frame_word() {
        let graph = build(&snapshot_of(vec![
            Instruction::LoadSelf { destination: 3 },
            send(3, 5, 6),
            Instruction::Return { register: 3 },
        ]));
        let End::Return { value } = graph.blocks[0].end else {
            panic!("the body returns");
        };
        assert!(matches!(
            graph.definition(value),
            Definition::Node(Node::FrameRead { register: 3, .. })
        ));
    }

    #[test]
    fn a_mirror_send_s_way_into_a_merge_gets_a_block_of_its_own() {
        use crate::builtins::primitive::MirrorPrimitive;
        let mut snapshot = snapshot_of(vec![Instruction::Return { register: 3 }]);
        snapshot.instructions = vec![
            SnapshotInstruction::SendMirror {
                primitive: MirrorPrimitive::from_index(0),
                mirror_register: 3,
                subject_register: 4,
                argument_base: 5,
                argument_sources: vec![],
                destination: 6,
                failure: 2,
            },
            SnapshotInstruction::LoadLiteral {
                destination: 6,
                literal: Literal::SmallInteger(1),
            },
            SnapshotInstruction::Return { register: 6 },
        ];
        let graph = build(&snapshot);
        verify(&graph).expect("the graph verifies");
        let End::FrameJump { next, target } = graph.blocks[0].end else {
            panic!("the mirror send ends its block");
        };
        assert_eq!(graph.block(next).program_counter, 1);
        // The failure's way: a block of its own, reading the frame word
        // the phi at the merge takes.
        let edge = graph.block(target);
        assert_eq!(edge.program_counter, 2);
        assert!(matches!(
            edge.nodes.as_slice(),
            [Node::FrameRead { register: 6, .. }]
        ));
        assert!(matches!(edge.end, End::Jump(merge) if graph.block(merge).phis.len() == 1));
    }

    /// A send with no argument.
    fn unary_send(receiver: u16, destination: u16) -> Instruction {
        Instruction::SendPublic {
            selector: Symbol::NEW,
            receiver_register: receiver,
            argument_base: destination,
            argument_sources: vec![],
            destination,
            call_site: CallSite { number: 0 },
        }
    }

    #[test]
    fn a_read_before_a_loop_is_not_reused_after_the_loop_s_frame_node() {
        let graph = build(&snapshot_of(vec![
            unary_send(2, 3),
            Instruction::Move {
                destination: 4,
                source: 3,
            },
            // 2: the loop's head, whose send runs on every pass.
            unary_send(3, 5),
            Instruction::BranchIfTrue {
                register: 5,
                target: 2,
            },
            Instruction::Return { register: 4 },
        ]));
        verify(&graph).expect("the graph verifies");
        let head = graph.loop_heads().next().expect("the body loops");
        let Some(Node::Frame { reads, .. }) = graph
            .block(head)
            .nodes
            .iter()
            .find(|node| matches!(node, Node::Frame { .. }))
        else {
            panic!("the head sends");
        };
        // The receiver is read from its frame word on each pass, after
        // the previous pass's send.
        let (register, receiver) = reads[0];
        assert_eq!(register, 3);
        assert_eq!(graph.values[receiver.index()].block, head);
        assert!(matches!(
            graph.definition(receiver),
            Definition::Node(Node::FrameRead { register: 3, .. })
        ));
    }

    #[test]
    fn a_frame_read_for_one_way_out_runs_on_that_way_alone() {
        // On the way to 1 the register 4 is dead: 1 writes it before
        // anything reads it. Only the way to 3 reads it from its frame
        // word, for the phi at 3.
        let graph = build(&snapshot_of(vec![
            Instruction::BranchIfTrue {
                register: 3,
                target: 3,
            },
            Instruction::LoadLiteral {
                destination: 4,
                literal: Literal::SmallInteger(1),
            },
            Instruction::Jump { target: 3 },
            Instruction::Return { register: 4 },
        ]));
        verify(&graph).expect("the graph verifies");
        let frame_reads_of_4 = |block: &crate::jit::optimizing::graph::Block| {
            block
                .nodes
                .iter()
                .filter(|node| matches!(node, Node::FrameRead { register: 4, .. }))
                .count()
        };
        assert_eq!(frame_reads_of_4(&graph.blocks[0]), 0);
        let End::Branch { if_true, .. } = graph.blocks[0].end else {
            panic!("the body branches");
        };
        let edge = graph.block(if_true);
        assert_eq!(edge.program_counter, 3);
        assert_eq!(frame_reads_of_4(edge), 1);
        assert!(matches!(edge.end, End::Jump(_)));
    }

    #[test]
    fn a_mirror_send_s_way_back_to_a_block_only_it_enters_gets_a_block_of_its_own() {
        use crate::builtins::primitive::MirrorPrimitive;
        let mut snapshot = snapshot_of(vec![Instruction::Return { register: 3 }]);
        snapshot.instructions = vec![
            SnapshotInstruction::Jump { target: 2 },
            // 1: entered only by the failure of the send at 2.
            SnapshotInstruction::Return { register: 4 },
            SnapshotInstruction::SendMirror {
                primitive: MirrorPrimitive::from_index(0),
                mirror_register: 3,
                subject_register: 5,
                argument_base: 6,
                argument_sources: vec![],
                destination: 4,
                failure: 1,
            },
            SnapshotInstruction::Return { register: 4 },
        ];
        let graph = build(&snapshot);
        verify(&graph).expect("the graph verifies");
        let mirror_send = graph
            .blocks
            .iter()
            .find(|block| matches!(block.end, End::FrameJump { .. }))
            .expect("the mirror send ends its block");
        let End::FrameJump { target, .. } = mirror_send.end else {
            unreachable!("matched above");
        };
        let edge = graph.block(target);
        assert!(edge
            .nodes
            .iter()
            .all(|node| matches!(node, Node::FrameRead { .. })));
        assert!(
            matches!(edge.end, End::Jump(failure) if graph.block(failure).program_counter == 1)
        );
    }

    #[test]
    fn every_built_graph_verifies() {
        let bodies = corpus_of_bodies();
        assert!(bodies.len() > 50, "only {} bodies", bodies.len());
        let mut loop_heads = 0;
        let mut phis = 0;
        for snapshot in bodies {
            let graph = build(&snapshot);
            if let Err(error) = verify(&graph) {
                panic!("{error:?} in {:?}\n{graph:#?}", snapshot.instructions);
            }
            loop_heads += graph.loop_heads().count();
            phis += graph
                .blocks
                .iter()
                .map(|block| block.phis.len())
                .sum::<usize>();
        }
        // The corpus holds loops and merges, so the check saw phis.
        assert!(
            loop_heads > 0 && phis > 0,
            "{loop_heads} loops, {phis} phis"
        );
    }

    #[test]
    fn every_shipped_body_s_graph_verifies() {
        let mut mirror_sends = 0;
        for snapshot in shipped_bodies() {
            let graph = build(&snapshot);
            if let Err(error) = verify(&graph) {
                panic!("{error:?} in {:?}\n{graph:#?}", snapshot.instructions);
            }
            mirror_sends += graph
                .blocks
                .iter()
                .filter(|block| matches!(block.end, End::FrameJump { .. }))
                .count();
        }
        assert!(mirror_sends > 0, "the core library sends to the mirror");
    }

    /// The customization of a copy entered at an application whose own
    /// slots start at field 3.
    fn customized_at_three() -> Customization {
        Customization {
            key: CustomizationKey {
                receiver: DispatchId::from_bits(7),
                entered: DispatchId::from_bits(7),
            },
            own_slot_base: 3,
        }
    }

    fn nodes(graph: &Graph) -> Vec<&Node> {
        graph.blocks.iter().flat_map(|block| &block.nodes).collect()
    }

    fn own_slot_read() -> Vec<Instruction> {
        vec![
            Instruction::LoadOwnSlot {
                destination: 1,
                own_index: 0,
            },
            Instruction::Return { register: 1 },
        ]
    }

    #[test]
    fn a_copy_reads_an_own_slot_as_a_simple_node() {
        let mut snapshot = snapshot_of(own_slot_read());
        snapshot.customization = Some(customized_at_three());
        let graph = build(&snapshot);
        let graph_nodes = nodes(&graph);
        assert!(graph_nodes.iter().any(|node| matches!(
            node,
            Node::OwnSlotLoad {
                absolute_index: 3,
                ..
            }
        )));
        assert!(!graph_nodes
            .iter()
            .any(|node| matches!(node, Node::Frame { .. })));
    }

    #[test]
    fn a_shared_body_keeps_its_own_slot_template() {
        let graph = build(&snapshot_of(own_slot_read()));
        let graph_nodes = nodes(&graph);
        assert!(graph_nodes
            .iter()
            .any(|node| matches!(node, Node::Frame { .. })));
        assert!(!graph_nodes
            .iter()
            .any(|node| matches!(node, Node::OwnSlotLoad { .. })));
    }

    /// A lazy slot's getter: its slot returned once filled, its
    /// initializer -- here a send -- run and stored otherwise.
    fn lazy_getter() -> Vec<Instruction> {
        vec![
            Instruction::ReturnSlotIfFilled { own_index: 1 },
            Instruction::LoadSelf { destination: 4 },
            send(4, 4, 5),
            Instruction::InitializeSlot {
                source: 5,
                own_index: 1,
            },
            Instruction::Return { register: 5 },
        ]
    }

    #[test]
    fn a_copy_s_lazy_getter_returns_its_slot_unless_it_is_nil() {
        let mut snapshot = snapshot_of(lazy_getter());
        snapshot.customization = Some(customized_at_three());
        let graph = build(&snapshot);
        let End::ReturnUnlessNil { value, next } = graph.blocks[0].end else {
            panic!("the getter's first block returns its slot: {graph:#?}");
        };
        assert!(matches!(
            graph.definition(value),
            Definition::Node(Node::OwnSlotLoad {
                absolute_index: 4,
                ..
            })
        ));
        assert_eq!(graph.block(next).program_counter, 1, "the initializer");
        // The initializer's store keeps its template.
        assert!(nodes(&graph).iter().any(|node| matches!(
            node,
            Node::Frame {
                instruction: SnapshotInstruction::InitializeSlot { .. },
                ..
            }
        )));
        verify(&graph).expect("the graph verifies");
    }

    #[test]
    fn a_shared_lazy_getter_keeps_its_template() {
        let graph = build(&snapshot_of(lazy_getter()));
        assert!(matches!(
            graph.blocks[0].nodes.first(),
            Some(Node::Frame {
                instruction: SnapshotInstruction::ReturnSlotIfFilled { .. },
                ..
            })
        ));
    }

    /// Every body of the corpus and of the shipped library, built as a
    /// copy customized at a slot base, verifies: its lazy getters among
    /// them, whose first block ends in the slot's return.
    #[test]
    fn every_body_s_customized_graph_verifies() {
        let mut returns = 0;
        for mut snapshot in corpus_of_bodies().into_iter().chain(shipped_bodies()) {
            snapshot.customization = Some(customized_at_three());
            let graph = build(&snapshot);
            if let Err(error) = verify(&graph) {
                panic!("{error:?} in {:?}\n{graph:#?}", snapshot.instructions);
            }
            returns += graph
                .blocks
                .iter()
                .filter(|block| matches!(block.end, End::ReturnUnlessNil { .. }))
                .count();
        }
        assert!(returns > 0, "the shipped library has lazy slots");
    }

    /// The nodes of `graph` that are integer fast paths.
    fn integer_paths(graph: &Graph) -> Vec<&Node> {
        nodes(graph)
            .into_iter()
            .filter(|node| matches!(node, Node::IntegerBinary { .. }))
            .collect()
    }

    /// `counting_loop`, each operation's fallback never run: each fast
    /// path's failure traps, so that neither fallback send is built and
    /// the loop's counter is the fast path's own result, never a merge
    /// with the send's.
    #[test]
    fn an_integer_operation_whose_fallback_never_ran_has_no_fallback_block() {
        let mut snapshot = snapshot_of(counting_loop());
        unrun_site(&mut snapshot, 2, 0);
        unrun_site(&mut snapshot, 7, 1);
        let graph = build(&snapshot);
        assert_eq!(verify(&graph), Ok(()));
        let paths = integer_paths(&graph);
        assert_eq!(paths.len(), 2);
        for integer_node in paths {
            assert!(matches!(
                integer_node,
                Node::IntegerBinary { fallback: None, .. }
            ));
        }
        assert!(
            nodes(&graph)
                .iter()
                .all(|node| !matches!(node, Node::Frame { .. })),
            "no fallback send is built"
        );
    }

    /// A site that ran, or that a speculation trapped at -- it may have
    /// run unrecorded, as a send resolved to `doesNotUnderstand:` does --
    /// keeps its fallback.
    #[test]
    fn a_fallback_that_ran_or_trapped_is_kept() {
        for (executed, trapped) in [(true, false), (false, true)] {
            let mut snapshot = snapshot_of(counting_loop());
            for index in [2, 7] {
                snapshot.sites[index] = Some(SiteFacts {
                    site: 0,
                    executed,
                    trapped,
                });
            }
            let graph = build(&snapshot);
            for integer_node in integer_paths(&graph) {
                assert!(matches!(
                    integer_node,
                    Node::IntegerBinary {
                        fallback: Some(_),
                        ..
                    }
                ));
            }
        }
    }

    /// `r4 := r1 < r2`, then `r4 ifTrue: [1] ifFalse: [2]`.
    fn a_comparison_and_its_conditional() -> Vec<Instruction> {
        vec![
            Instruction::IntegerBinary {
                operation: IntegerOperation::Less,
                destination: 4,
                left: 1,
                right: 2,
                target: 9,
            },
            // 1: the conditional.
            Instruction::JumpIfNotBoolean {
                register: 4,
                target: 7,
            },
            Instruction::BranchIfFalse {
                register: 4,
                target: 5,
            },
            Instruction::LoadLiteral {
                destination: 5,
                literal: Literal::SmallInteger(1),
            },
            Instruction::Return { register: 5 },
            Instruction::LoadLiteral {
                destination: 5,
                literal: Literal::SmallInteger(2),
            },
            Instruction::Return { register: 5 },
            // 7: the non-Boolean way.
            send(4, 4, 5),
            Instruction::Return { register: 5 },
            // 9: the comparison's fallback.
            send(1, 2, 4),
            Instruction::Jump { target: 1 },
        ]
    }

    /// `a_comparison_and_its_conditional`, the comparison's fallback never
    /// run and the conditional's non-Boolean way run: the condition is the
    /// fast path's Boolean, so neither branch has a third way, though the
    /// feedback alone would keep it, and the non-Boolean way is not built.
    #[test]
    fn a_comparison_s_branch_has_no_third_way() {
        let mut snapshot = snapshot_of(a_comparison_and_its_conditional());
        unrun_site(&mut snapshot, 0, 1);
        snapshot.sites[1] = Some(SiteFacts {
            site: 0,
            executed: true,
            trapped: false,
        });
        let graph = build(&snapshot);
        assert_eq!(verify(&graph), Ok(()));
        let branches: Vec<&End> = graph
            .blocks
            .iter()
            .map(|block| &block.end)
            .filter(|end| matches!(end, End::Branch { .. }))
            .collect();
        assert_eq!(branches.len(), 2, "`JumpIfNotBoolean` and `BranchIfFalse`");
        for branch_end in branches {
            assert!(matches!(
                branch_end,
                End::Branch {
                    not_boolean: None,
                    ..
                }
            ));
            let End::Branch { condition, .. } = branch_end else {
                unreachable!("filtered");
            };
            assert_eq!(graph.kind(*condition), ValueKind::Boolean);
        }
        assert!(
            graph.blocks.iter().all(|block| block.program_counter != 7),
            "the non-Boolean way is not built"
        );
        assert!(
            (0..graph.blocks.len()).all(|block| graph.end_trap(BlockId::of_index(block)).is_none())
        );
    }

    /// The same conditional on a comparison whose fallback ran: its
    /// condition merges the fast path's Boolean with the send's answer,
    /// which may be anything, so the conditional keeps its third way.
    #[test]
    fn a_condition_of_no_known_kind_keeps_its_third_way() {
        let graph = build(&snapshot_of(a_comparison_and_its_conditional()));
        assert!(graph.blocks.iter().any(|block| matches!(
            block.end,
            End::Branch {
                not_boolean: Some(_),
                ..
            }
        )));
    }

    /// `r1 ifTrue: [1] ifFalse: [2]`, whose non-Boolean way never ran:
    /// any other value traps, at the conditional, and that way is not
    /// built.
    #[test]
    fn a_non_boolean_way_that_never_ran_traps() {
        let mut instructions = a_comparison_and_its_conditional();
        instructions[0] = Instruction::Move {
            destination: 4,
            source: 1,
        };
        let mut snapshot = snapshot_of(instructions);
        unrun_site(&mut snapshot, 1, 0);
        let graph = build(&snapshot);
        assert_eq!(verify(&graph), Ok(()));
        let traps: Vec<u32> = (0..graph.blocks.len())
            .filter_map(|block| graph.end_trap(BlockId::of_index(block)))
            .collect();
        assert_eq!(traps, vec![1]);
        assert!(graph.blocks.iter().all(|block| block.program_counter != 7));
    }

    /// `r3 := self. r4 := r3 new: r3. ^r4`, the send never executed: it
    /// ends its block in a trap, and nothing after it is built.
    #[test]
    fn a_send_never_executed_ends_its_block_in_a_trap() {
        let mut snapshot = snapshot_of(vec![
            Instruction::LoadSelf { destination: 3 },
            send(3, 3, 4),
            Instruction::Return { register: 4 },
        ]);
        unrun_site(&mut snapshot, 1, 6);
        let graph = build(&snapshot);
        assert_eq!(verify(&graph), Ok(()));
        assert_eq!(graph.blocks.len(), 1);
        assert_eq!(
            graph.blocks[0].end,
            End::Trap {
                program_counter: 1,
                site: 6
            }
        );
        assert!(nodes(&graph)
            .iter()
            .all(|node| !matches!(node, Node::Frame { .. })));
    }

    /// `Speculation::None` builds stage 1's graph whatever the feedback
    /// says: every body of the corpus and of the shipped library, every
    /// site recorded as never run, is built as with no site recorded at
    /// all, with every fallback, third way and send, and no value of a
    /// known kind, so that its code tests every operand and condition.
    #[test]
    fn no_speculation_builds_stage_one_s_graph() {
        let mut built = 0;
        for snapshot in corpus_of_bodies().into_iter().chain(shipped_bodies()) {
            let mut unrun = snapshot.clone();
            unrun_every_site(&mut unrun);
            let mut unrecorded = snapshot;
            unrecorded.sites.fill(None);
            let graph = super::build(&unrun, Speculation::None);
            assert_eq!(graph, super::build(&unrecorded, Speculation::None));
            assert!(graph
                .values
                .iter()
                .all(|value| value.kind == ValueKind::Unknown));
            for block in &graph.blocks {
                assert!(!matches!(block.end, End::Trap { .. }));
                assert!(!matches!(
                    block.end,
                    End::Branch {
                        not_boolean: None,
                        ..
                    }
                ));
                assert!(block
                    .nodes
                    .iter()
                    .all(|node| !matches!(node, Node::IntegerBinary { fallback: None, .. })));
            }
            built += 1;
        }
        assert!(built > 0);
    }

    /// Every body of the corpus and of the shipped library, every site
    /// recorded as never run, verifies with its guards: the graph, and the
    /// kinds it gives its values.
    #[test]
    fn every_speculating_graph_verifies() {
        let mut traps = 0;
        for mut snapshot in corpus_of_bodies().into_iter().chain(shipped_bodies()) {
            unrun_every_site(&mut snapshot);
            let graph = build(&snapshot);
            if let Err(error) = verify(&graph) {
                panic!("{error:?} in {:?}\n{graph:#?}", snapshot.instructions);
            }
            traps += (0..graph.blocks.len())
                .filter(|block| graph.end_trap(BlockId::of_index(*block)).is_some())
                .count();
            traps += nodes(&graph)
                .iter()
                .filter(|node| node.trap().is_some())
                .count();
        }
        assert!(traps > 0);
    }
}

/// The builder's inlined scopes, built from trees of real runs and from
/// trees made by hand.
#[cfg(test)]
pub(crate) mod inlining_tests {
    use indoc::indoc;

    use super::build_inlined;
    use crate::bytecode::{CallSite, Instruction, Literal};
    use crate::heap::{CodeId, Immediate};
    use crate::jit::optimizing::graph::{
        BlockId, Definition, End, Graph, HeaderBinding, IndexedKind, Node, ScopeId, ValueId,
        ValueKind,
    };
    use crate::jit::optimizing::plan::{plan, plan_with, ArmPlan, FallThrough, InliningPlan};
    use crate::jit::optimizing::snapshot::{Snapshot, SnapshotInstruction};
    use crate::jit::optimizing::tree::{CalleeFacts, SnapshotTree};
    use crate::jit::optimizing::verify::verify;
    use crate::jit::optimizing::{
        inlining_corpus, ran_site, test_receiver, test_snapshot_with_arguments, test_tree,
        tree_of_run, Speculation, SWEEP_LIMITS,
    };
    use crate::Symbol;

    /// The plan of `tree` and its graph, speculating.
    fn built(tree: &SnapshotTree) -> (InliningPlan, Graph) {
        let plan = plan(tree, Speculation::Traps);
        let graph = build_inlined(tree, &plan, Speculation::Traps);
        assert_eq!(verify(&graph), Ok(()));
        (plan, graph)
    }

    /// The one send of `selector` in `snapshot`.
    fn send_of(snapshot: &Snapshot, selector: &str) -> u32 {
        let sends: Vec<u32> = snapshot
            .instructions
            .iter()
            .enumerate()
            .filter(|(_, instruction)| match instruction {
                SnapshotInstruction::SendPublic { selector: sent, .. }
                | SnapshotInstruction::SendLexical { selector: sent, .. }
                | SnapshotInstruction::SendSuper { selector: sent, .. } => {
                    *sent == Symbol::intern(selector)
                }
                _ => false,
            })
            .map(|(index, _)| u32::try_from(index).expect("a small body"))
            .collect();
        let [send] = sends[..] else {
            panic!("one send of {selector}: {:#?}", snapshot.instructions);
        };
        send
    }

    /// The block ending in the type-case of `scope`'s send at
    /// `program_counter`.
    fn type_case(graph: &Graph, scope: ScopeId, program_counter: u32) -> &End {
        graph
            .blocks
            .iter()
            .map(|block| &block.end)
            .find(|end| {
                matches!(end, End::TypeCase { scope: sender, program_counter: counter, .. }
                    if *sender == scope && *counter == program_counter)
            })
            .expect("the send's type-case")
    }

    /// The root's one return's value.
    fn returned(graph: &Graph) -> ValueId {
        let returns: Vec<ValueId> = graph
            .blocks
            .iter()
            .filter_map(|block| match block.end {
                End::Return { value } => Some(value),
                _ => None,
            })
            .collect();
        let [value] = returns[..] else {
            panic!("one return: {graph:#?}");
        };
        value
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
    fn an_inlined_method_s_return_binds_the_send_s_destination() {
        let tree = tree_of_run(MONOMORPHIC, "run");
        let (plan, graph) = built(&tree);
        assert_eq!(plan.scopes.len(), 2);
        let send = send_of(&tree.root, "helper:");
        let destination = tree.root.instructions[send as usize]
            .register_written()
            .expect("the send's destination");
        // The root returns what the callee's `number + 1` answered, with no
        // send between: the callee's return bound the send's destination.
        let value = returned(&graph);
        let defining = graph.values[value.index()].block;
        assert_eq!(graph.block(defining).scope, ScopeId(1));
        assert!(matches!(
            graph.definition(value),
            Definition::Node(Node::IntegerBinary { .. })
        ));
        assert!(graph
            .blocks
            .iter()
            .filter(|block| block.scope == ScopeId(1))
            .flat_map(|block| &block.nodes)
            .any(|node| matches!(
                node,
                Node::Copy { destination_register, source }
                    if *destination_register == destination && *source == value
            )));
        assert!(graph
            .blocks
            .iter()
            .flat_map(|block| &block.nodes)
            .all(|node| !matches!(node, Node::Frame { .. })));
    }

    const TWO_RETURNS: &str = indoc! {"
        class TwoReturns = () (
          public run = (
            ^self pick: true
          )
          public pick: flag = (
            flag ifTrue: [^1].
            ^2
          )
        )
    "};

    #[test]
    fn two_returns_of_an_inlined_method_meet_in_a_phi() {
        let tree = tree_of_run(TWO_RETURNS, "run");
        let (_, graph) = built(&tree);
        let send = send_of(&tree.root, "pick:");
        let destination = tree.root.instructions[send as usize]
            .register_written()
            .expect("the send's destination");
        let value = returned(&graph);
        let Definition::Phi(phi) = graph.definition(value) else {
            panic!("the returns meet in a phi: {graph:#?}");
        };
        assert_eq!(phi.register, destination);
        let merge = graph.block(graph.values[value.index()].block);
        assert_eq!(
            (merge.scope, merge.program_counter),
            (ScopeId::ROOT, send + 1)
        );
        let mut words: Vec<u64> = phi
            .inputs
            .iter()
            .map(|input| match graph.definition(*input) {
                Definition::Node(Node::Constant { word, .. }) => *word,
                other => panic!("a returned constant: {other:?}"),
            })
            .collect();
        words.sort_unstable();
        let small = |integer: i64| {
            Immediate::small_integer(integer)
                .expect("a SmallInteger")
                .bits()
        };
        let mut expected = vec![small(1), small(2)];
        expected.sort_unstable();
        assert_eq!(words, expected);
        for predecessor in &merge.predecessors {
            assert_eq!(graph.block(*predecessor).scope, ScopeId(1));
        }
    }

    /// A method of `instructions` taking `argument_count` arguments, its
    /// code id `code_id`.
    fn method(code_id: u32, argument_count: u16, instructions: Vec<Instruction>) -> Snapshot {
        let mut snapshot = test_snapshot_with_arguments(instructions, argument_count);
        snapshot.code_id = CodeId::from_raw(code_id);
        snapshot
    }

    /// `receiver selector` with `sources` as its arguments, laid from
    /// `argument_base`, its site 0.
    fn send(
        selector: &str,
        receiver: u16,
        argument_base: u16,
        sources: Vec<u16>,
        destination: u16,
    ) -> Instruction {
        Instruction::SendPublic {
            selector: Symbol::intern(selector),
            receiver_register: receiver,
            argument_base,
            argument_sources: sources,
            destination,
            call_site: CallSite { number: 0 },
        }
    }

    /// A root whose send at `program_counter`, its site 0, is
    /// monomorphic over `callee`, its receiver's own slots from
    /// `own_slot_base`.
    fn monomorphic(
        mut root: Snapshot,
        program_counter: usize,
        callee: Snapshot,
        own_slot_base: u32,
    ) -> SnapshotTree {
        ran_site(&mut root, program_counter, 0);
        let code_id = root.code_id;
        test_tree(
            root,
            vec![(
                code_id,
                0,
                vec![(
                    test_receiver(1, 1, own_slot_base),
                    CalleeFacts::Method {
                        snapshot: Box::new(callee.clone()),
                        level: 0,
                    },
                )],
            )],
            vec![callee],
        )
    }

    fn literal(destination: u16, value: i64) -> Instruction {
        Instruction::LoadLiteral {
            destination,
            literal: Literal::SmallInteger(value),
        }
    }

    /// `r14 := 2. r25 := 7. r21 := r13 foo: 5`, the argument laid at
    /// r20, `r15 := r14. r16 := r25. ^r21`: the callee answers its
    /// argument. No compiler reads r25 after the send -- the leftover
    /// analysis taints every register past the send's slots there, and a
    /// send's slots keep the arguments, which the callee never writes -- so
    /// the body is one no lowering makes, written to pin what the builder
    /// does with the sender's registers the callee's frame lays over: it
    /// forgets them, where a register below the slots keeps its value.
    #[test]
    fn an_inlined_callee_overlays_the_caller_s_scratch_registers_only() {
        let root = method(
            0,
            0,
            vec![
                Instruction::LoadSelf { destination: 13 },
                literal(14, 2),
                literal(25, 7),
                literal(20, 5),
                send("foo:", 13, 20, vec![20], 21),
                Instruction::Move {
                    destination: 15,
                    source: 14,
                },
                Instruction::Move {
                    destination: 16,
                    source: 25,
                },
                Instruction::Return { register: 21 },
            ],
        );
        let callee = method(1, 1, vec![Instruction::Return { register: 0 }]);
        let tree = monomorphic(root, 4, callee, 0);
        let (plan, graph) = built(&tree);
        assert_eq!(plan.scopes[1].offset, 20);
        let copied = |register: u16| {
            graph
                .blocks
                .iter()
                .filter(|block| block.scope == ScopeId::ROOT)
                .flat_map(|block| &block.nodes)
                .find_map(|node| match node {
                    Node::Copy {
                        destination_register,
                        source,
                    } if *destination_register == register => Some(*source),
                    _ => None,
                })
                .expect("the copy")
        };
        // Below the send's argument base the caller's binding holds across.
        assert!(matches!(
            graph.definition(copied(15)),
            Definition::Node(Node::Constant { word, .. })
                if *word == Immediate::small_integer(2).expect("small").bits()
        ));
        // At or above it the callee's frame lay: the caller's 7 is gone,
        // and its register is read again from its word.
        assert!(matches!(
            graph.definition(copied(16)),
            Definition::Node(Node::FrameRead { register: 25, .. })
        ));
        // The callee's argument, which it answers, is the caller's 5.
        assert!(matches!(
            graph.definition(returned(&graph)),
            Definition::Node(Node::Constant { word, .. })
                if *word == Immediate::small_integer(5).expect("small").bits()
        ));
        let overlays: Vec<&Node> = graph
            .blocks
            .iter()
            .flat_map(|block| &block.nodes)
            .filter(|node| matches!(node, Node::Overlay { .. }))
            .collect();
        assert_eq!(overlays.len(), 2, "one at the entry, one at the return");
        for overlay in overlays {
            let Node::Overlay { ranges } = overlay else {
                unreachable!();
            };
            assert!(ranges.iter().all(|range| range.start >= 21));
        }
    }

    const POLYMORPHIC: &str = indoc! {"
        class Polymorphic = () (
          class First = () (
            public answer = (
              ^1 + 1
            )
          )
          class Second = () (
            public answer = (
              ^2 + 2
            )
          )
          public ask: thing = (
            ^thing answer
          )
          public run = (
            ^(self ask: First new) + (self ask: Second new)
          )
        )
    "};

    #[test]
    fn a_type_case_tests_its_arms_in_the_cache_s_order() {
        let tree = tree_of_run(POLYMORPHIC, "ask:");
        let (plan, graph) = built(&tree);
        let send = send_of(&tree.root, "answer");
        let site = tree.root.sites[send as usize].expect("a site").site;
        let cached: Vec<_> = tree.sites[&(tree.root.code_id, site)]
            .arms
            .iter()
            .map(|(receiver, _)| receiver.dispatch_id)
            .collect();
        assert_eq!(cached.len(), 2, "First, then Second");
        let End::TypeCase {
            receiver,
            arms,
            fall_through,
            ..
        } = type_case(&graph, ScopeId::ROOT, send)
        else {
            unreachable!();
        };
        let tested: Vec<_> = arms.iter().map(|(guard, _)| *guard).collect();
        assert_eq!(tested, cached);
        assert_eq!(*fall_through, None);
        // Each arm enters its own callee's scope.
        let planned = &plan.sites[&(ScopeId::ROOT, send)];
        for ((_, block), (_, _, arm)) in arms.iter().zip(&planned.arms) {
            let ArmPlan::Inline { callee } = arm else {
                panic!("an inlined arm");
            };
            assert_eq!(graph.block(*block).scope, *callee);
            assert_eq!(graph.block(*block).program_counter, 0);
        }
        // The receiver tested is `thing`, the root's argument.
        assert!(matches!(
            graph.definition(*receiver),
            Definition::Node(Node::FrameRead { register: 0, .. })
        ));
    }

    #[test]
    fn a_type_case_whose_site_never_trapped_traps_past_its_arms() {
        let tree = tree_of_run(MONOMORPHIC, "run");
        let (plan, graph) = built(&tree);
        let send = send_of(&tree.root, "helper:");
        assert_eq!(
            plan.sites[&(ScopeId::ROOT, send)].fall_through,
            FallThrough::Trap
        );
        let block = graph
            .blocks
            .iter()
            .position(|block| matches!(block.end, End::TypeCase { .. }))
            .expect("the type-case");
        let End::TypeCase {
            arms, fall_through, ..
        } = &graph.blocks[block].end
        else {
            unreachable!();
        };
        assert_eq!(arms.len(), 1);
        assert_eq!(*fall_through, None);
        assert_eq!(graph.end_trap(BlockId::of_index(block)), Some(send));
    }

    #[test]
    fn a_type_case_after_a_trap_falls_through_to_the_send() {
        let mut tree = tree_of_run(MONOMORPHIC, "run");
        let send = send_of(&tree.root, "helper:");
        tree.root.sites[send as usize]
            .as_mut()
            .expect("a site")
            .trapped = true;
        let (_, graph) = built(&tree);
        let End::TypeCase {
            arms, fall_through, ..
        } = type_case(&graph, ScopeId::ROOT, send)
        else {
            unreachable!();
        };
        assert_eq!(arms.len(), 1);
        let sent = graph.block(fall_through.expect("the send"));
        assert_eq!((sent.scope, sent.program_counter), (ScopeId::ROOT, send));
        assert!(sent.nodes.iter().any(|node| matches!(
            node,
            Node::Frame {
                scope: ScopeId::ROOT,
                instruction: SnapshotInstruction::SendLexical { .. },
                program_counter,
                ..
            } if *program_counter == send
        )));
        // The send and the arm's return meet after it.
        let End::Jump(after) = sent.end else {
            panic!("the send goes on");
        };
        assert_eq!(graph.block(after).predecessors.len(), 2);
    }

    /// `r13 := self. r20 := 5. r14 := r13 foo: r20. ^r14`, its `foo:` a
    /// callee that makes an array of its argument.
    pub(crate) fn a_callee_with_a_frame_node() -> SnapshotTree {
        let root = method(
            0,
            0,
            vec![
                Instruction::LoadSelf { destination: 13 },
                literal(20, 5),
                send("foo:", 13, 20, vec![20], 14),
                Instruction::Return { register: 14 },
            ],
        );
        let callee = method(
            1,
            1,
            vec![
                Instruction::MakeArray {
                    destination: 14,
                    element_registers: vec![0],
                },
                Instruction::Return { register: 14 },
            ],
        );
        monomorphic(root, 2, callee, 0)
    }

    #[test]
    fn an_inlined_scope_s_frame_node_names_its_scope() {
        let tree = a_callee_with_a_frame_node();
        let (_, graph) = built(&tree);
        let (block, scope, program_counter, instruction, reads, written) = graph
            .blocks
            .iter()
            .flat_map(|block| block.nodes.iter().map(move |node| (block, node)))
            .find_map(|(block, node)| match node {
                Node::Frame {
                    scope,
                    program_counter,
                    instruction,
                    reads,
                    written,
                } => Some((block, scope, program_counter, instruction, reads, written)),
                _ => None,
            })
            .expect("the callee's frame node");
        assert_eq!(*scope, ScopeId(1));
        assert_eq!(block.scope, ScopeId(1));
        assert_eq!(*program_counter, 0);
        // The template's instruction as the callee holds it; the graph's
        // reads and write in the physical frame, 20 registers up.
        assert_eq!(
            *instruction,
            SnapshotInstruction::MakeArray {
                destination: 14,
                element_registers: vec![0],
            }
        );
        assert_eq!(reads.len(), 1);
        assert_eq!(reads[0].0, 20);
        assert_eq!(*written, Some(34));
        assert!(matches!(
            graph.definition(reads[0].1),
            Definition::Node(Node::Constant { .. })
        ));
    }

    /// `r13 := self. r14 := r13 slot. ^r14`, its `slot` a method that
    /// reads its receiver's own slot 1.
    #[test]
    fn an_inlined_arm_reads_its_receiver_s_own_slot_at_a_fixed_offset() {
        let root = method(
            0,
            0,
            vec![
                Instruction::LoadSelf { destination: 13 },
                send("slot", 13, 20, Vec::new(), 14),
                Instruction::Return { register: 14 },
            ],
        );
        let callee = method(
            1,
            0,
            vec![
                Instruction::LoadOwnSlot {
                    destination: 13,
                    own_index: 1,
                },
                Instruction::Return { register: 13 },
            ],
        );
        let tree = monomorphic(root, 1, callee, 5);
        let (plan, graph) = built(&tree);
        assert_eq!(plan.scopes[1].own_slot_base, Some(5));
        let End::TypeCase { receiver, .. } = type_case(&graph, ScopeId::ROOT, 1) else {
            unreachable!();
        };
        let Definition::Node(Node::OwnSlotLoad {
            receiver: read_from,
            absolute_index,
            ..
        }) = graph.definition(returned(&graph))
        else {
            panic!("the callee's slot read: {graph:#?}");
        };
        assert_eq!(*absolute_index, 6);
        assert_eq!(read_from, receiver, "the type-case's receiver");
        // The callee's receiver is its header's binding.
        let header = graph
            .scope(ScopeId(1))
            .header_register(HeaderBinding::Receiver);
        assert_eq!(header, 20);
        assert!(graph
            .blocks
            .iter()
            .flat_map(|block| &block.nodes)
            .any(|node| matches!(
                node,
                Node::Copy { destination_register, source }
                    if *destination_register == header && source == receiver
            )));
    }

    /// `root` whose lexical send at `program_counter`, its site 0, is
    /// taken without a test: the root's receiver is known, of the one
    /// application the site cached, whose method is `callee`.
    fn unguarded(root: Snapshot, program_counter: usize, callee: Snapshot) -> SnapshotTree {
        let mut tree = monomorphic(root, program_counter, callee, 0);
        tree.root_receiver = Some(test_receiver(1, 1, 0));
        tree.root_entered = Some(test_receiver(1, 1, 0));
        tree
    }

    /// `self foo` into r14, its argument base r20.
    fn send_to_self(destination: u16) -> Instruction {
        Instruction::SendLexical {
            depth: 0,
            selector: Symbol::intern("foo"),
            argument_base: 20,
            argument_sources: Vec::new(),
            destination,
            call_site: CallSite { number: 0 },
        }
    }

    /// A method answering `true`.
    fn answers_true() -> Snapshot {
        method(
            1,
            0,
            vec![
                Instruction::LoadLiteral {
                    destination: 13,
                    literal: Literal::Boolean(true),
                },
                Instruction::Return { register: 13 },
            ],
        )
    }

    /// `r15 := 0. [r14 := self foo. r14] whileFalse. ^r14`, its `foo`
    /// inlined without a test at the loop's head.
    pub(crate) fn a_loop_headed_by_an_unguarded_send() -> SnapshotTree {
        let root = method(
            0,
            0,
            vec![
                literal(15, 0),
                send_to_self(14),
                Instruction::BranchIfFalse {
                    register: 14,
                    target: 1,
                },
                Instruction::Return { register: 14 },
            ],
        );
        unguarded(root, 1, answers_true())
    }

    /// `flag ifFalse: [r15 := 1]. r14 := self foo. ^r14`, its `foo`
    /// inlined without a test where the two ways meet, which the branch
    /// enters through a block of its own.
    pub(crate) fn an_unguarded_send_where_two_ways_meet() -> SnapshotTree {
        let root = method(
            0,
            1,
            vec![
                Instruction::BranchIfFalse {
                    register: 0,
                    target: 2,
                },
                literal(15, 1),
                send_to_self(14),
                Instruction::Return { register: 14 },
            ],
        );
        unguarded(root, 2, answers_true())
    }

    /// The block the inlined scope's entry begins.
    fn entry_block(graph: &Graph) -> BlockId {
        let entries: Vec<BlockId> = (0..graph.blocks.len())
            .map(BlockId::of_index)
            .filter(|block| {
                let block = graph.block(*block);
                block.scope == ScopeId(1)
                    && block
                        .nodes
                        .iter()
                        .any(|node| matches!(node, Node::Overlay { .. }))
            })
            .collect();
        let [entry] = entries[..] else {
            panic!("one entry: {graph:#?}");
        };
        entry
    }

    /// The back edge to a loop's head that is an inlined scope's entry is
    /// its sender's: the poll there leaves the frame before the send, the
    /// scope's frame not laid on that pass.
    #[test]
    fn a_loop_headed_by_an_unguarded_inlined_send_polls_in_its_sender() {
        let tree = a_loop_headed_by_an_unguarded_send();
        let (plan, graph) = built(&tree);
        assert_eq!(
            plan.sites[&(ScopeId::ROOT, 1)].fall_through,
            FallThrough::Unguarded
        );
        let entry = entry_block(&graph);
        assert_eq!(graph.block(entry).taint_point, (ScopeId::ROOT, 1));
        let polls: Vec<BlockId> = (0..graph.blocks.len())
            .map(BlockId::of_index)
            .filter(|block| {
                crate::jit::optimizing::liveness::poll_target(&graph, *block) == Some(entry)
            })
            .collect();
        let [poll] = polls[..] else {
            panic!("one back edge to the entry: {graph:#?}");
        };
        assert_eq!(graph.block(poll).scope, ScopeId::ROOT);
    }

    /// The block on the way into an inlined scope's entry where two ways
    /// meet is its sender's, and it, like the entry's, reads its sender's
    /// leftover analysis at the send.
    #[test]
    fn an_edge_into_an_unguarded_entry_is_its_sender_s() {
        let tree = an_unguarded_send_where_two_ways_meet();
        let (_, graph) = built(&tree);
        let entry = entry_block(&graph);
        assert_eq!(graph.block(entry).taint_point, (ScopeId::ROOT, 2));
        let edges: Vec<&crate::jit::optimizing::graph::Block> = graph
            .block(entry)
            .predecessors
            .iter()
            .map(|predecessor| graph.block(*predecessor))
            .filter(|block| block.taint_point == (ScopeId::ROOT, 2))
            .collect();
        assert!(!edges.is_empty(), "{graph:#?}");
        for edge in edges {
            assert_eq!(edge.scope, ScopeId::ROOT);
            assert!(edge
                .nodes
                .iter()
                .all(|node| matches!(node, Node::FrameRead { .. })));
        }
    }

    /// A body for each kind of callee always inlined, each run once on the
    /// receivers it is inlined for: a slot's getter and setter and a lazy
    /// slot's getter, a quick method, `==`, `class` and an `Array`'s
    /// `at:put:`.
    const ALWAYS_INLINED: &str = indoc! {"
        class AlwaysInlined = () (
          class Point = (|
            public x ::= 3.
          |) (
            lazy public cached = 4.
          )
          public read: point = (
            ^point x
          )
          public write: point = (
            ^point x: 5
          )
          public cache: point = (
            ^point cached
          )
          public negate: flag = (
            ^flag not
          )
          public same: thing = (
            ^thing == nil
          )
          public classOfSelf = (
            ^self class
          )
          public put: elements = (
            ^elements at: 2 put: 7
          )
          public run = (
            | point = Point new. |
            self read: point.
            self write: point.
            self cache: point.
            self negate: true.
            self same: point.
            self classOfSelf.
            self put: (Array new: 3).
            ^0
          )
        )
    "};

    /// The graph of `selector`'s body after one run of `ALWAYS_INLINED`,
    /// the one send of `sent` in it, and the block its type-case's one arm
    /// enters.
    fn always_inlined(selector: &str, sent: &str) -> (SnapshotTree, Graph, u32, BlockId) {
        let tree = tree_of_run(ALWAYS_INLINED, selector);
        let (plan, graph) = built(&tree);
        let send = send_of(&tree.root, sent);
        let planned = &plan.sites[&(ScopeId::ROOT, send)];
        assert!(
            matches!(planned.arms[..], [(_, _, ArmPlan::Always(_))]),
            "{planned:?}"
        );
        let End::TypeCase { arms, .. } = type_case(&graph, ScopeId::ROOT, send) else {
            unreachable!();
        };
        let [(_, arm)] = arms[..] else {
            panic!("one arm: {graph:#?}");
        };
        (tree, graph, send, arm)
    }

    /// The type-case's receiver at the root's send at `program_counter`.
    fn tested(graph: &Graph, program_counter: u32) -> ValueId {
        let End::TypeCase { receiver, .. } = type_case(graph, ScopeId::ROOT, program_counter)
        else {
            unreachable!();
        };
        *receiver
    }

    /// Whether `value` is the type-case's receiver `tested`: the very
    /// value, or a read of the frame word it was read from, which a block
    /// of its own reads again.
    fn is_receiver(graph: &Graph, value: ValueId, tested: ValueId) -> bool {
        value == tested
            || matches!(
                (graph.definition(value), graph.definition(tested)),
                (
                    Definition::Node(Node::FrameRead { register, .. }),
                    Definition::Node(Node::FrameRead { register: other, .. }),
                ) if register == other
            )
    }

    /// The field the site of the root's send at `program_counter` cached
    /// its accessor at.
    fn cached_field(tree: &SnapshotTree, program_counter: u32) -> u32 {
        let site = tree.root.sites[program_counter as usize]
            .expect("a site")
            .site;
        match &tree.sites[&(tree.root.code_id, site)].arms[..] {
            [(_, CalleeFacts::Slot { field, .. })] => *field,
            other => panic!("one accessor: {other:?}"),
        }
    }

    #[test]
    fn an_accessor_arm_is_a_field_load() {
        let (tree, graph, send, arm) = always_inlined("read:", "x");
        let Definition::Node(Node::FieldLoad { object, field, .. }) =
            graph.definition(returned(&graph))
        else {
            panic!("the getter's load: {graph:#?}");
        };
        assert!(is_receiver(&graph, *object, tested(&graph, send)));
        assert_eq!(*field, cached_field(&tree, send));
        // The field is the arm's application's layout's, which the copy
        // relies on.
        let site = tree.root.sites[send as usize].expect("a site").site;
        let [(receiver, _)] = &tree.sites[&(tree.root.code_id, site)].arms[..] else {
            panic!("one arm");
        };
        assert_eq!(graph.arm_applications, vec![receiver.application]);
        // The arm is its sender's, at the send: no scope, no frame node.
        assert_eq!(graph.block(arm).scope, ScopeId::ROOT);
        assert_eq!(graph.scopes.len(), 1);
        assert!(graph
            .blocks
            .iter()
            .flat_map(|block| &block.nodes)
            .all(|node| !matches!(node, Node::Frame { .. })));
    }

    #[test]
    fn a_setter_arm_is_a_field_store_answering_its_receiver() {
        let (tree, graph, send, _) = always_inlined("write:", "x:");
        let stores: Vec<(ValueId, u32, ValueId)> = graph
            .blocks
            .iter()
            .flat_map(|block| &block.nodes)
            .filter_map(|node| match node {
                Node::FieldStore {
                    object,
                    field,
                    source,
                } => Some((*object, *field, *source)),
                _ => None,
            })
            .collect();
        let [(object, field, source)] = stores[..] else {
            panic!("one store: {graph:#?}");
        };
        assert!(is_receiver(&graph, object, tested(&graph, send)));
        assert_eq!(field, cached_field(&tree, send));
        assert!(matches!(
            graph.definition(source),
            Definition::Node(Node::Constant { word, .. })
                if *word == Immediate::small_integer(5).expect("small").bits()
        ));
        assert_eq!(returned(&graph), object, "a setter answers its receiver");
        assert!(graph
            .blocks
            .iter()
            .flat_map(|block| &block.nodes)
            .any(|node| node.calls_leaf()));
    }

    #[test]
    fn a_quick_method_arm_answers_its_constant() {
        let (_, graph, _, _) = always_inlined("negate:", "not");
        assert!(matches!(
            graph.definition(returned(&graph)),
            Definition::Node(Node::Constant { word, .. })
                if *word == Immediate::boolean(false).bits()
        ));
    }

    #[test]
    fn a_lazy_getter_arm_tests_and_loads() {
        let (_, graph, send, arm) = always_inlined("cache:", "cached");
        let value = returned(&graph);
        let Definition::Node(Node::FieldLoad { object, .. }) = graph.definition(value) else {
            panic!("the lazy slot's load: {graph:#?}");
        };
        assert!(is_receiver(&graph, *object, tested(&graph, send)));
        // An unfilled slot runs the getter again in the interpreter.
        assert!(graph.block(arm).nodes.iter().any(|node| matches!(
            node,
            Node::TrapIfNil { value: tested_value, program_counter }
                if *tested_value == value && *program_counter == send
        )));
    }

    #[test]
    fn identity_compare_answers_a_boolean_of_known_kind() {
        let (_, graph, send, _) = always_inlined("same:", "==");
        let value = returned(&graph);
        let Definition::Node(Node::IdentityCompare {
            left,
            program_counter,
            ..
        }) = graph.definition(value)
        else {
            panic!("the identity: {graph:#?}");
        };
        assert!(is_receiver(&graph, *left, tested(&graph, send)));
        assert_eq!(*program_counter, send);
        assert_eq!(graph.kind(value), ValueKind::Boolean);
    }

    #[test]
    fn a_class_arm_is_a_class_of_its_receiver() {
        let (_, graph, send, _) = always_inlined("classOfSelf", "class");
        let Definition::Node(Node::ClassOf { object, .. }) = graph.definition(returned(&graph))
        else {
            panic!("the class: {graph:#?}");
        };
        // `class` is protected, so its send is a lexical one to `self`, the
        // root's receiver, which the type-case reads from the header and
        // the arm reads again.
        for read in [*object, tested(&graph, send)] {
            assert!(
                matches!(
                    graph.definition(read),
                    Definition::Node(Node::Receiver { .. })
                ),
                "{graph:#?}"
            );
        }
    }

    #[test]
    fn an_array_at_put_arm_stores_once_its_checks_pass_and_traps_at_the_send() {
        let (_, graph, send, _) = always_inlined("put:", "at:put:");
        let stores: Vec<&Node> = graph
            .blocks
            .iter()
            .flat_map(|block| &block.nodes)
            .filter(|node| matches!(node, Node::IndexedStore { .. }))
            .collect();
        let [Node::IndexedStore {
            kind,
            object,
            source,
            program_counter,
            ..
        }] = stores[..]
        else {
            panic!("one store: {graph:#?}");
        };
        assert_eq!(*kind, IndexedKind::Array);
        assert!(is_receiver(&graph, *object, tested(&graph, send)));
        assert_eq!(*program_counter, send);
        assert_eq!(returned(&graph), *source, "`at:put:` answers the element");
    }

    /// A lexical send to `self`, whose receiver the copy knows, takes its
    /// accessor without a test, right where the send stands, at the field
    /// of the application the site cached.
    #[test]
    fn an_accessor_taken_without_a_test_is_a_field_load_where_the_send_stands() {
        let mut root = method(
            0,
            0,
            vec![send_to_self(14), Instruction::Return { register: 14 }],
        );
        ran_site(&mut root, 0, 0);
        let code_id = root.code_id;
        let mut tree = test_tree(
            root,
            vec![(
                code_id,
                0,
                vec![(
                    test_receiver(1, 1, 0),
                    CalleeFacts::Slot {
                        field: 3,
                        kind: crate::mixin::AccessorKind::Getter,
                        level: crate::heap::ApplicationIndex::from_raw(1),
                    },
                )],
            )],
            Vec::new(),
        );
        tree.root_receiver = Some(test_receiver(1, 1, 0));
        tree.root_entered = Some(test_receiver(1, 1, 0));
        let (plan, graph) = built(&tree);
        assert_eq!(
            plan.sites[&(ScopeId::ROOT, 0)].fall_through,
            FallThrough::Unguarded
        );
        assert!(graph
            .blocks
            .iter()
            .all(|block| !matches!(block.end, End::TypeCase { .. })));
        let Definition::Node(Node::FieldLoad { object, field, .. }) =
            graph.definition(returned(&graph))
        else {
            panic!("the getter's load: {graph:#?}");
        };
        assert_eq!(*field, 3);
        assert!(matches!(
            graph.definition(*object),
            Definition::Node(Node::Receiver { .. })
        ));
        assert_eq!(
            graph.arm_applications,
            vec![crate::heap::ApplicationIndex::from_raw(1)]
        );
    }

    /// A copy that speculates nothing never traps: no arm with a check is
    /// expressed, and its site is the send.
    #[test]
    fn without_speculation_an_arm_with_a_check_is_its_send() {
        let tree = tree_of_run(ALWAYS_INLINED, "put:");
        let plan = plan(&tree, Speculation::None);
        let graph = build_inlined(&tree, &plan, Speculation::None);
        assert_eq!(verify(&graph), Ok(()));
        assert!(graph
            .blocks
            .iter()
            .flat_map(|block| &block.nodes)
            .all(|node| node.trap().is_none()));
    }

    /// A site that trapped sends its arms with a check: the check failed
    /// there once, and would again.
    #[test]
    fn after_a_trap_an_arm_with_a_check_is_its_send() {
        let mut tree = tree_of_run(ALWAYS_INLINED, "cache:");
        let send = send_of(&tree.root, "cached");
        tree.root.sites[send as usize]
            .as_mut()
            .expect("a site")
            .trapped = true;
        let (_, graph) = built(&tree);
        assert!(graph
            .blocks
            .iter()
            .flat_map(|block| &block.nodes)
            .all(|node| !matches!(node, Node::TrapIfNil { .. } | Node::FieldLoad { .. })));
    }

    /// Every graph the inlining corpus's trees plan, speculating and not,
    /// verifies, every frame node's registers its own scope's.
    #[test]
    fn every_inlined_graph_verifies() {
        let mut inlined = 0;
        let mut type_cases = 0;
        // Each kind of node an arm always inlined becomes.
        let mut always = [0; 8];
        for tree in inlining_corpus() {
            for speculation in [Speculation::Traps, Speculation::None] {
                let plan = plan_with(&tree, &SWEEP_LIMITS, speculation);
                let graph = build_inlined(&tree, &plan, speculation);
                if let Err(error) = verify(&graph) {
                    panic!("{error:?} in {plan:#?}\n{graph:#?}");
                }
                inlined += graph
                    .blocks
                    .iter()
                    .filter(|block| block.scope != ScopeId::ROOT)
                    .count();
                type_cases += graph
                    .blocks
                    .iter()
                    .filter(|block| matches!(block.end, End::TypeCase { .. }))
                    .count();
                for block in &graph.blocks {
                    let snapshot = tree
                        .snapshot_of(graph.scope(block.scope).code_id)
                        .expect("every scope's body is snapshotted");
                    for node in &block.nodes {
                        let kind = match node {
                            Node::FieldLoad { .. } => 0,
                            Node::FieldStore { .. } => 1,
                            Node::ClassOf { .. } => 2,
                            Node::IdentityCompare { .. } => 3,
                            Node::IndexedStore { .. } => 4,
                            Node::IndexedLoad { .. } => 5,
                            Node::TrapIfNil { .. } => 6,
                            // An arm's arithmetic, at a send rather than at
                            // an `IntegerBinary` of the body's own.
                            Node::IntegerBinary {
                                program_counter, ..
                            } if snapshot.instructions[*program_counter as usize]
                                .argument_slots()
                                .is_some() =>
                            {
                                7
                            }
                            _ => continue,
                        };
                        always[kind] += 1;
                    }
                }
            }
        }
        assert!(inlined > 0 && type_cases > 0);
        assert!(always.iter().all(|count| *count > 0), "{always:?}");
    }
}
