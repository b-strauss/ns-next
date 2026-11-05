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

use super::graph::{Block, BlockId, End, Graph, Node, Phi, ValueId};
use super::snapshot::{Snapshot, SnapshotInstruction};
use super::verify::immediate_dominators;

/// The input of a loop head's phi whose back edge is not seen yet.
const UNFILLED: ValueId = ValueId(u32::MAX);

/// The graph of `snapshot`'s body. Only the blocks reachable from the
/// body's first instruction are built: an unreachable block has no
/// register state to start from -- nothing defines its registers -- and
/// never runs, so keeping it would mean inventing one. A body whose first
/// instruction heads a loop is given an entry block of its own, with no
/// instruction, so that the head's phis have an input for the entry.
pub(crate) fn build(snapshot: &Snapshot) -> Graph {
    let graph = Builder::new(snapshot).build();
    debug_assert!(
        super::verify::verify(&graph).is_ok(),
        "internal: the built graph is refused: {:?}",
        super::verify::verify(&graph)
    );
    graph
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
    },
    Return {
        register: u16,
    },
    Jump,
    /// A branch instruction, which goes to its target for one outcome and
    /// to the next instruction for the other two.
    Branch {
        register: u16,
        taken: Outcome,
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
fn kind_of(instruction: &SnapshotInstruction, program_counter: usize) -> Kind {
    match instruction {
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
        },
        SnapshotInstruction::BranchIfFalse { register, .. } => Kind::Branch {
            register: *register,
            taken: Outcome::False,
        },
        SnapshotInstruction::BranchIfTrue { register, .. } => Kind::Branch {
            register: *register,
            taken: Outcome::True,
        },
        _ => Kind::Frame,
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

/// The register state a block leaves with: `None` for a register whose
/// value is its frame word.
type State = Vec<Option<ValueId>>;

struct Builder<'snapshot> {
    instructions: &'snapshot [SnapshotInstruction],
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
    /// For each value, the register it is a frame read of.
    frame_read_of: Vec<Option<u16>>,
}

impl<'snapshot> Builder<'snapshot> {
    fn new(snapshot: &'snapshot Snapshot) -> Builder<'snapshot> {
        let instructions = snapshot.instructions.as_slice();
        assert!(
            !instructions.is_empty(),
            "internal: a body holds at least one instruction"
        );
        let reads: Vec<Vec<u16>> = instructions
            .iter()
            .map(SnapshotInstruction::registers_read)
            .collect();
        let written: Vec<Option<u16>> = instructions
            .iter()
            .map(SnapshotInstruction::register_written)
            .collect();
        let register_count = snapshot.register_count();
        let shapes = shapes_of(instructions);
        let block_count = shapes.len();
        let mut builder = Builder {
            instructions,
            reads,
            written,
            register_count,
            shapes,
            blocks: Vec::with_capacity(block_count),
            live_in: Vec::new(),
            loops: Vec::new(),
            exits: vec![Vec::new(); block_count],
            fallback_exits: vec![None; block_count],
            head_entries: vec![None; block_count],
            frame_read_of: Vec::new(),
        };
        builder.lay_out_blocks();
        builder.live_in = builder.liveness();
        builder.loops = builder.loops();
        builder
    }

    fn build(mut self) -> Graph {
        for block in 0..self.blocks.len() {
            self.build_block(block);
        }
        debug_assert!(
            self.blocks
                .iter()
                .all(|block| block.phis.iter().all(|phi| !phi.inputs.contains(&UNFILLED))),
            "internal: every back edge filled its head's phis"
        );
        Graph::from_blocks(self.blocks)
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
            let mut facts = LoopFacts {
                written: RegisterSet::empty(self.register_count),
                frame_node: false,
            };
            if back_edges
                .iter()
                .all(|predecessor| dominates(head, *predecessor))
            {
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
                        if matches!(kind_of(&self.instructions[index], index), Kind::Frame) {
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

    fn new_value(&mut self, frame_read: Option<u16>) -> ValueId {
        let value = ValueId(
            u32::try_from(self.frame_read_of.len())
                .expect("internal: a body defines fewer than 2^32 values"),
        );
        self.frame_read_of.push(frame_read);
        value
    }

    /// Whether `binding` of `register` says what its frame word holds: no
    /// value, or a read of that very word, which nothing has written
    /// since -- a frame node would have unbound the register, and a write
    /// back stores the value it is bound to.
    fn is_frame_word(&self, binding: Option<ValueId>, register: usize) -> bool {
        binding.is_none_or(|value| {
            self.frame_read_of[value.index()].map(usize::from) == Some(register)
        })
    }

    /// What `register` is bound to where control leaves `predecessor`
    /// for `block`.
    fn edge_binding(&self, predecessor: usize, block: usize, register: usize) -> Option<ValueId> {
        match &self.fallback_exits[predecessor] {
            Some(fallback) if self.shapes[predecessor].target == Some(block) => fallback[register],
            _ => self.exits[predecessor][register],
        }
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
        let value = self.new_value(Some(register_number));
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
        let mut state: State = vec![None; self.register_count];
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
        let live: Vec<usize> = self.live_in[block].registers().collect();
        for register in live {
            let bindings: Vec<Option<ValueId>> = forward
                .iter()
                .map(|predecessor| self.edge_binding(*predecessor, block, register))
                .collect();
            // Around a loop, a register changes if the loop writes it, or
            // if a frame node in it leaves the register to its frame word
            // and the register came in bound to anything else.
            let loop_changes = self.loops[block].as_ref().is_some_and(|facts| {
                facts.written.contains(register)
                    || (facts.frame_node
                        && !bindings
                            .iter()
                            .all(|binding| self.is_frame_word(*binding, register)))
            });
            if !loop_changes {
                // A register that is its frame word on every way in is
                // read again at its next use: a read made before the
                // block does not survive a frame node on a later pass
                // around a loop through it, nor any other way that reaches
                // its use through one.
                if !bindings.is_empty()
                    && bindings
                        .iter()
                        .all(|binding| self.is_frame_word(*binding, register))
                {
                    continue;
                }
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
        if cfg!(debug_assertions) {
            // A live register the head took no phi for is one no pass
            // around the loop changes: it entered bound to a value no
            // frame node in the loop can reach, which the back edge
            // brings again, or it entered as its frame word, which the
            // back edge may bring as a read of that word.
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
                        || (entry[register].is_none() && self.is_frame_word(binding, register))
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
        let value = self.new_value(Some(register));
        self.blocks[block]
            .nodes
            .push(Node::FrameRead { register, value });
        state[index] = Some(value);
        value
    }

    fn build_block(&mut self, block: usize) {
        let mut state = if block == 0 {
            vec![None; self.register_count]
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
        match kind_of(instruction, index) {
            Kind::Constant { destination, word } => {
                let value = self.new_value(None);
                let nodes = &mut self.blocks[block].nodes;
                nodes.push(Node::Constant { value, word });
                nodes.push(Node::Copy {
                    destination_register: destination,
                    source: value,
                });
                state[usize::from(destination)] = Some(value);
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
                state[usize::from(destination)] = Some(value);
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
                state[usize::from(destination)] = Some(value);
                None
            }
            Kind::IntegerBinary {
                operation,
                destination,
                left,
                right,
            } => {
                let left = self.read(block, state, left);
                let right = self.read(block, state, right);
                self.fallback_exits[block] = Some(state.clone());
                let value = self.new_value(None);
                self.blocks[block].nodes.push(Node::IntegerBinary {
                    operation,
                    destination_register: destination,
                    value,
                    left,
                    right,
                    fallback: target.expect("internal: a fast path has its fallback"),
                });
                state[usize::from(destination)] = Some(value);
                None
            }
            Kind::Return { register } => {
                let value = self.read(block, state, register);
                Some(End::Return { value })
            }
            Kind::Jump => Some(End::Jump(target.expect("internal: a jump has its target"))),
            Kind::Branch { register, taken } => {
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
                    condition,
                    if_true: pick(Outcome::True),
                    if_false: pick(Outcome::False),
                    not_boolean: pick(Outcome::NotBoolean),
                })
            }
            Kind::Frame => {
                let registers = self.reads[index].clone();
                let reads = registers
                    .into_iter()
                    .map(|register| (register, self.read(block, state, register)))
                    .collect();
                let instruction = self.instructions[index].clone();
                let falls_through = instruction.falls_through();
                self.blocks[block].nodes.push(Node::Frame {
                    program_counter: u32::try_from(index)
                        .expect("internal: an instruction index fits a u32"),
                    instruction,
                    reads,
                    written: self.written[index],
                });
                // What the instruction ran may have written any of the
                // frame's registers by reference, or moved what a value
                // names: every register is its frame word again.
                state.fill(None);
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
fn shapes_of(instructions: &[SnapshotInstruction]) -> Vec<Shape> {
    let count = instructions.len();
    let index_of = |target: u32| {
        let target = usize::try_from(target).expect("internal: a jump target fits a usize");
        assert!(target < count, "internal: a jump target lies in its body");
        target
    };
    let mut starts = vec![false; count];
    starts[0] = true;
    for (index, instruction) in instructions.iter().enumerate() {
        let target = instruction.jump_target();
        if let Some(target) = target {
            starts[index_of(target)] = true;
        }
        if (target.is_some() || !instruction.falls_through()) && index + 1 < count {
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
        let instruction = &instructions[last];
        let target = instruction
            .jump_target()
            .map(|target| run_of[index_of(target)]);
        let next = if instruction.falls_through() {
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

    use super::build;
    use crate::bytecode::{
        CallSite, CodeKind, CompiledCode, Instruction, IntegerOperation, Literal,
    };
    use crate::heap::CodeId;
    use crate::interpreter::inline_cache::FeedbackBase;
    use crate::jit::optimizing::graph::{BlockId, Definition, End, Node};
    use crate::jit::optimizing::snapshot::{
        tainted_before_each, BodyFacts, Snapshot, SnapshotInstruction,
    };
    use crate::jit::optimizing::verify::verify;
    use crate::jit::optimizing::{corpus_of_bodies, shipped_bodies};
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
        assert_eq!(not_boolean, if_true);
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
}
