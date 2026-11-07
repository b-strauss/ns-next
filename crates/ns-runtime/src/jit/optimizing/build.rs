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

use std::ops::Range;

use crate::bytecode::IntegerOperation;

use super::graph::{Block, BlockId, End, Graph, Node, Phi, ValueId, ValueKind};
use super::snapshot::{Snapshot, SnapshotInstruction};
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
/// of no known kind.
pub(crate) fn build(snapshot: &Snapshot, speculation: Speculation) -> Graph {
    let count = snapshot.instructions.len();
    let mut forced = None;
    // The conditionals built without their third way, their condition
    // being a known Boolean; and those a build refuted, which keep it.
    let mut known_boolean = vec![false; count];
    let mut refuted_boolean = vec![false; count];
    let graph = loop {
        let mut builder = Builder::new(snapshot, speculation, &known_boolean);
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
        let (more, refuted) = drop_boolean_third_ways(&mut graph, &known_boolean, &guards);
        let more: Vec<usize> = more
            .into_iter()
            .filter(|program_counter| !refuted_boolean[*program_counter])
            .collect();
        if more.is_empty() && refuted.is_empty() {
            break graph;
        }
        for program_counter in more {
            known_boolean[program_counter] = true;
        }
        for program_counter in refuted {
            known_boolean[program_counter] = false;
            refuted_boolean[program_counter] = true;
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
/// leaves the edges as they are. Answers the program counters of the
/// `JumpIfNotBoolean`s whose third way is a block of its own, which the
/// next build lays out without it, and of those `known_boolean` built
/// without one whose condition is no known Boolean after all and which no
/// guard of theirs (`guards`) traps for, which the next build gives it
/// back: a known Boolean seen in one build may not survive the blocks the
/// next lays out differently.
fn drop_boolean_third_ways(
    graph: &mut Graph,
    known_boolean: &[bool],
    guards: &[Guard],
) -> (Vec<usize>, Vec<usize>) {
    let kinds: Vec<ValueKind> = graph.values.iter().map(|value| value.kind).collect();
    let mut more = Vec::new();
    let mut refuted = Vec::new();
    for block in &mut graph.blocks {
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
        let index =
            usize::try_from(*program_counter).expect("internal: a program counter fits a usize");
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
#[derive(Clone, Copy, PartialEq, Eq)]
enum Guard {
    /// The instruction keeps every way stage 1 gives it.
    None,
    /// An integer fast path whose fallback never ran: its failure traps.
    Fallback,
    /// A `JumpIfNotBoolean` whose non-Boolean way never ran: any other
    /// value traps.
    NotBoolean,
    /// A send never executed: the send itself traps.
    Send { site: u32 },
}

/// What an instruction becomes in the graph.
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
}

/// The outcome of a Boolean test a branch instruction jumps for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Outcome {
    True,
    False,
    NotBoolean,
}

/// What `instruction`, at `program_counter`, becomes. Every instruction
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
    program_counter: usize,
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
        } if usize::try_from(*target).ok() != program_counter.checked_add(1) => {
            Kind::IntegerBinary {
                operation: *operation,
                destination: *destination,
                left: *left,
                right: *right,
                trap: choice.guard == Guard::Fallback,
            }
        }
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

/// Whether `instruction`'s kind ends its block, whether or not it falls
/// through or has a jump target the graph keeps: a customized copy's
/// `ReturnSlotIfFilled`, whose block ends in the slot's return
/// (`End::ReturnUnlessNil`), and every branch, whose block ends in its
/// test -- what decides for a `JumpIfNotBoolean` built without its third
/// way, which keeps no jump target, while any other branch ends its block
/// by its target as well.
fn ends_block(
    instruction: &SnapshotInstruction,
    choice: Choice,
    own_slot_base: Option<u32>,
) -> bool {
    matches!(
        kind_of(instruction, 0, choice, own_slot_base),
        Kind::ReturnUnlessNil { .. } | Kind::Branch { .. }
    )
}

/// What decides an instruction's kind beside the instruction itself.
#[derive(Clone, Copy)]
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
fn guard_of(snapshot: &Snapshot, index: usize, send_only: bool, speculation: Speculation) -> Guard {
    if speculation == Speculation::None || send_only {
        return Guard::None;
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

/// A block before its nodes are built: the instructions it holds, empty
/// for an entry block or an edge block, and where control goes after it.
struct Shape {
    program_counter: u32,
    instructions: Range<usize>,
    /// The block the last instruction falls through to; for a block with
    /// no instruction, the block it leads to.
    next: Option<usize>,
    /// The block the last instruction's jump target is.
    target: Option<usize>,
}

impl Shape {
    fn successors(&self) -> Vec<usize> {
        let mut successors = Vec::with_capacity(2);
        for block in [self.next, self.target].into_iter().flatten() {
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

struct Builder<'snapshot> {
    instructions: &'snapshot [SnapshotInstruction],
    /// The entered application's own slot base of a customized copy
    /// (`Snapshot::customization`).
    own_slot_base: Option<u32>,
    /// What decides each instruction's kind beside it (`kind_of`).
    choices: Vec<Choice>,
    /// Each instruction's guard, as `choices` holds it.
    guards: Vec<Guard>,
    /// The registers the leftover analysis taints before each instruction
    /// (`Snapshot::tainted_before`).
    tainted_before: &'snapshot [Vec<Range<u16>>],
    reads: Vec<Vec<u16>>,
    written: Vec<Option<u16>>,
    register_count: usize,
    shapes: Vec<Shape>,
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

impl<'snapshot> Builder<'snapshot> {
    fn new(
        snapshot: &'snapshot Snapshot,
        speculation: Speculation,
        known_boolean: &[bool],
    ) -> Builder<'snapshot> {
        let instructions = snapshot.instructions.as_slice();
        assert!(
            !instructions.is_empty(),
            "internal: a body holds at least one instruction"
        );
        let choices: Vec<Choice> = instructions
            .iter()
            .enumerate()
            .map(|(index, instruction)| {
                let send_only = matches!(instruction, SnapshotInstruction::IntegerBinary { .. })
                    && snapshot.fallback_taken.get(index).copied().unwrap_or(false);
                Choice {
                    send_only,
                    guard: guard_of(snapshot, index, send_only, speculation),
                    // Only a `JumpIfNotBoolean`'s third way is a jump target.
                    known_boolean: matches!(
                        instruction,
                        SnapshotInstruction::JumpIfNotBoolean { .. }
                    ) && known_boolean.get(index).copied().unwrap_or(false),
                }
            })
            .collect();
        // A jump to the fallback reads nothing and writes nothing: the
        // fallback send reads the operands. A send that traps writes
        // nothing either; it reads what the interpreter's send, run again,
        // reads.
        let reads: Vec<Vec<u16>> = instructions
            .iter()
            .zip(&choices)
            .map(|(instruction, choice)| {
                if choice.send_only {
                    Vec::new()
                } else {
                    instruction.registers_read()
                }
            })
            .collect();
        let written: Vec<Option<u16>> = instructions
            .iter()
            .zip(&choices)
            .map(|(instruction, choice)| {
                if choice.may_fall_through() {
                    instruction.register_written()
                } else {
                    None
                }
            })
            .collect();
        let register_count = snapshot.register_count();
        let own_slot_base = snapshot
            .customization
            .map(|customization| customization.own_slot_base);
        let shapes = shapes_of(instructions, &choices, own_slot_base);
        let block_count = shapes.len();
        let mut builder = Builder {
            instructions,
            own_slot_base,
            guards: choices.iter().map(|choice| choice.guard).collect(),
            choices,
            tainted_before: &snapshot.tainted_before,
            reads,
            written,
            register_count,
            shapes,
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
        Ok(Graph::from_blocks(self.blocks))
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
                program_counter: shape.program_counter,
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
                        if let Some(written) = self.written[last] {
                            live.remove(usize::from(written));
                        }
                        if let Some(target) = shape.target {
                            live.union_with(&live_in[target]);
                        }
                        self.add_reads(&mut live, last);
                        for index in instructions {
                            if let Some(written) = self.written[index] {
                                live.remove(usize::from(written));
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
                        if matches!(
                            kind_of(
                                &self.instructions[index],
                                index,
                                self.choices[index],
                                self.own_slot_base
                            ),
                            Kind::Frame
                        ) {
                            facts.frame_node = true;
                        }
                        if let Some(written) = self.written[index] {
                            facts.written.insert(usize::from(written));
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
            let tainted = self
                .tainted_before
                .get(self.blocks[block].program_counter as usize)
                .is_some_and(|ranges| {
                    u16::try_from(register)
                        .is_ok_and(|register| ranges.iter().any(|range| range.contains(&register)))
                });
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

    /// The receiver's field at `absolute_index`, read by a new
    /// `Node::OwnSlotLoad` at the end of `block`.
    fn own_slot_load(&mut self, block: usize, absolute_index: u32) -> ValueId {
        let receiver = self.new_value(None);
        let value = self.new_value(None);
        let nodes = &mut self.blocks[block].nodes;
        nodes.push(Node::Receiver { value: receiver });
        nodes.push(Node::OwnSlotLoad {
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

    /// Lowers the instruction at `index`, the last of its block only if it
    /// answers an end other than a fall into `next`.
    fn lower(
        &mut self,
        block: usize,
        index: usize,
        state: &mut State,
        next: Option<BlockId>,
        target: Option<BlockId>,
    ) -> Option<End> {
        let instruction = &self.instructions[index];
        let program_counter =
            u32::try_from(index).expect("internal: an instruction index fits a u32");
        match kind_of(instruction, index, self.choices[index], self.own_slot_base) {
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
                let value = self.new_value(None);
                let nodes = &mut self.blocks[block].nodes;
                nodes.push(Node::Receiver { value });
                nodes.push(Node::Copy {
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
                let value = self.own_slot_load(block, absolute_index);
                self.blocks[block].nodes.push(Node::Copy {
                    destination_register: destination,
                    source: value,
                });
                self.bind(state, destination, value);
                None
            }
            Kind::ReturnUnlessNil { absolute_index } => {
                let value = self.own_slot_load(block, absolute_index);
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
                let registers = self.reads[index].clone();
                let reads = registers
                    .into_iter()
                    .map(|register| (register, self.read(block, state, register)))
                    .collect();
                let instruction = self.instructions[index].clone();
                let falls_through = instruction.falls_through();
                self.blocks[block].nodes.push(Node::Frame {
                    program_counter,
                    instruction,
                    reads,
                    written: self.written[index],
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
        }
    }
}

/// The blocks of `instructions`: one for every run that starts at the
/// first instruction, a jump target or after an instruction that jumps or
/// stops and is reachable from the first, in order; an entry block of
/// its own before them when the first instruction heads a loop; and an
/// edge block, with no instruction, for each way out of a block with two
/// ways out -- a branch, an integer fast path, a mirror send -- into a
/// block other ways enter too, or back to a block at or before it. Only
/// such a block's top can take a phi, and a phi's input may need a read
/// of a frame word placed on its way alone: on the other way the register
/// may be dead, its word one a callee left, and a mirror send's template
/// jumps for its failure itself, leaving no room after it on one way only.
/// A customized copy's `ReturnSlotIfFilled` ends its run too
/// (`ends_block`), its block ending in the slot's return.
fn shapes_of(
    instructions: &[SnapshotInstruction],
    choices: &[Choice],
    own_slot_base: Option<u32>,
) -> Vec<Shape> {
    let count = instructions.len();
    let index_of = |target: u32| {
        let target = usize::try_from(target).expect("internal: a jump target fits a usize");
        assert!(target < count, "internal: a jump target lies in its body");
        target
    };
    // A jump to the fallback does not fall through, nor a send that
    // traps; and a fast path whose failure traps, or a conditional with no
    // third way, has no jump target (`kind_of`).
    let falls_through =
        |index: usize| choices[index].may_fall_through() && instructions[index].falls_through();
    let jump_target = |index: usize| {
        instructions[index]
            .jump_target()
            .filter(|_| choices[index].keeps_target())
    };
    let mut starts = vec![false; count];
    starts[0] = true;
    for (index, instruction) in instructions.iter().enumerate() {
        let target = jump_target(index);
        if let Some(target) = target {
            starts[index_of(target)] = true;
        }
        if (target.is_some()
            || !falls_through(index)
            || ends_block(instruction, choices[index], own_slot_base))
            && index + 1 < count
        {
            starts[index + 1] = true;
        }
    }
    // The runs, each with the runs it may go to.
    let mut run_of = vec![usize::MAX; count];
    let mut runs: Vec<(Range<usize>, Option<usize>, Option<usize>)> = Vec::new();
    for index in 0..count {
        if starts[index] {
            runs.push((index..index + 1, None, None));
        } else {
            runs.last_mut()
                .expect("internal: the first instruction starts a run")
                .0
                .end = index + 1;
        }
        run_of[index] = runs.len() - 1;
    }
    for run in &mut runs {
        let last = run.0.end - 1;
        let target = jump_target(last).map(|target| run_of[index_of(target)]);
        let next = if falls_through(last) {
            assert!(
                last + 1 < count,
                "internal: a body does not run off its end"
            );
            Some(run_of[last + 1])
        } else {
            None
        };
        // An instruction whose target is the next instruction goes on
        // there either way.
        run.1 = next;
        run.2 = if target == next { None } else { target };
    }
    // The runs reachable from the first.
    let mut reachable = vec![false; runs.len()];
    let mut pending = vec![0usize];
    while let Some(run) = pending.pop() {
        if std::mem::replace(&mut reachable[run], true) {
            continue;
        }
        pending.extend([runs[run].1, runs[run].2].into_iter().flatten());
    }
    let mut entered = vec![0usize; runs.len()];
    for (run, (_, next, target)) in runs.iter().enumerate() {
        if reachable[run] {
            for successor in [*next, *target].into_iter().flatten() {
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
    for (run, (_, next, target)) in runs.iter().enumerate() {
        if !reachable[run] {
            continue;
        }
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
    let mut shapes = Vec::with_capacity(block_count);
    if entry_block {
        shapes.push(Shape {
            program_counter: 0,
            instructions: 0..0,
            next: Some(block_of_run[0]),
            target: None,
        });
    }
    let mut edge_blocks = edge_blocks.into_iter().peekable();
    for (run, (range, next, target)) in runs.iter().enumerate() {
        if !reachable[run] {
            continue;
        }
        let program_counter = |run: usize| {
            u32::try_from(runs[run].0.start).expect("internal: an instruction index fits a u32")
        };
        let mut shape = Shape {
            program_counter: program_counter(run),
            instructions: range.clone(),
            next: next.map(|next| block_of_run[next]),
            target: target.map(|target| block_of_run[target]),
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
                    edges.push(Shape {
                        program_counter: program_counter(to_run),
                        instructions: range.end..range.end,
                        next: Some(block_of_run[to_run]),
                        target: None,
                    });
                    *successor = Some(following);
                    following += 1;
                }
            }
        }
        shapes.push(shape);
        shapes.extend(edges);
    }
    shapes
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
