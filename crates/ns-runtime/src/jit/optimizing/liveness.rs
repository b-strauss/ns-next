//! What register allocation reads of a graph (`docs/design/execution.md`,
//! "One tier, an SSA graph, the macro assembler"): an order of its blocks,
//! a linear numbering of their nodes, each value's live ranges over it,
//! and which bytecode register is bound to which value where.
//!
//! **Positions.** Every block, in `block_order`, takes a run of slots: one
//! for its top, where its phis are defined, one per node, and one for its
//! end. A slot is two positions, the use and then the definition: a
//! node's operands are read at its use position, its value written at its
//! definition position. The end's use position reads the end's operand
//! and takes a back edge's poll; its definition position holds the moves
//! that carry the block's values into its successor's.
//!
//! **Bindings.** A value is bound to a bytecode register by a frame read,
//! a `Copy`, a fast path's result or a phi; the binding is *dirty* from
//! then until its value is stored into the register's frame word, and
//! decision 2 of the optimizing JIT leaves no store out: before every
//! interrupt point -- a frame node, a back edge's poll -- every dirty
//! binding is written back, and where control from several blocks meets
//! and a register's binding does not carry on -- the register is dead
//! there, or bound differently without a phi -- the binding is written
//! back on its way in, since no later write-back knows its value, unless
//! the leftover analysis taints the register there (`Binding::STALE`).
//! Each write-back is a use of its value, so the value lives until it.
use std::ops::Range;

use super::graph::{Block, BlockId, End, Graph, Node, ValueId};

/// The blocks reachable from block 0 in reverse postorder: every block
/// after each predecessor it has that is not reached through it, so that
/// a block but a loop's head is reached only after every way into it.
pub(crate) fn block_order(graph: &Graph) -> Vec<BlockId> {
    let count = graph.blocks.len();
    if count == 0 {
        return Vec::new();
    }
    let successors: Vec<Vec<BlockId>> = graph.blocks.iter().map(Block::successors).collect();
    let mut visited = vec![false; count];
    let mut postorder = Vec::with_capacity(count);
    // Each block on the walk's path with the index of its next successor.
    let mut path: Vec<(usize, usize)> = vec![(0, 0)];
    visited[0] = true;
    while let Some(&mut (block, ref mut next)) = path.last_mut() {
        if let Some(successor) = successors[block].get(*next) {
            *next += 1;
            let successor = successor.index();
            if !std::mem::replace(&mut visited[successor], true) {
                path.push((successor, 0));
            }
        } else {
            postorder.push(BlockId::of_index(block));
            path.pop();
        }
    }
    postorder.reverse();
    postorder
}

/// The linear numbering of a graph's nodes in a block order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Positions {
    pub(crate) order: Vec<BlockId>,
    /// Each block's first slot, `u32::MAX` for a block not in the order.
    first_slot: Vec<u32>,
    /// Each block's end slot.
    end_slot: Vec<u32>,
}

impl Positions {
    pub(crate) fn of(graph: &Graph, order: &[BlockId]) -> Positions {
        let mut first_slot = vec![u32::MAX; graph.blocks.len()];
        let mut end_slot = vec![u32::MAX; graph.blocks.len()];
        let mut slot: u32 = 0;
        for block in order {
            first_slot[block.index()] = slot;
            let nodes = u32::try_from(graph.block(*block).nodes.len())
                .expect("internal: a block holds fewer than 2^32 nodes");
            end_slot[block.index()] = slot + 1 + nodes;
            slot = slot
                .checked_add(nodes + 2)
                .expect("internal: a body holds fewer than 2^31 slots");
        }
        assert!(
            slot < u32::MAX / 2,
            "internal: a body's positions fit a u32"
        );
        Positions {
            order: order.to_vec(),
            first_slot,
            end_slot,
        }
    }

    /// The use position of `block`'s top, where its live values enter it.
    pub(crate) fn entry(&self, block: BlockId) -> u32 {
        2 * self.first_slot[block.index()]
    }

    /// Where `block`'s phis are defined.
    pub(crate) fn phi_definition(&self, block: BlockId) -> u32 {
        self.entry(block) + 1
    }

    pub(crate) fn node_use(&self, block: BlockId, index: usize) -> u32 {
        let index = u32::try_from(index).expect("internal: a node index fits a u32");
        2 * (self.first_slot[block.index()] + 1 + index)
    }

    pub(crate) fn node_definition(&self, block: BlockId, index: usize) -> u32 {
        self.node_use(block, index) + 1
    }

    pub(crate) fn end_use(&self, block: BlockId) -> u32 {
        2 * self.end_slot[block.index()]
    }

    /// Where the moves into `block`'s successor are made.
    pub(crate) fn end_definition(&self, block: BlockId) -> u32 {
        self.end_use(block) + 1
    }

    /// The position whose state control leaves `block` for `successor`
    /// with: the end's definition, or the use of the fast path whose
    /// fallback it is, taken before the fast path writes its result.
    pub(crate) fn exit(&self, graph: &Graph, block: BlockId, successor: BlockId) -> u32 {
        let leaving = graph
            .block(block)
            .exit_position(successor)
            .expect("internal: an exit to a successor");
        if leaving == graph.block(block).nodes.len() {
            self.end_definition(block)
        } else {
            self.node_use(block, leaving)
        }
    }
}

/// What a bytecode register holds: the value bound to it, or `None` for
/// what its frame word holds, and whether the frame word holds the value
/// -- always for `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Binding {
    pub(crate) value: Option<ValueId>,
    pub(crate) clean: bool,
}

impl Binding {
    pub(crate) const FRAME_WORD: Binding = Binding {
        value: None,
        clean: true,
    };

    /// A register whose frame word holds what no one may read: one the
    /// leftover analysis taints where ways in that bind it differently
    /// meet, whose bindings are therefore not written back -- the
    /// collector's pre-pass nils the word at every interrupt point until
    /// the register is written again, and no instruction reads it before.
    pub(crate) const STALE: Binding = Binding {
        value: None,
        clean: false,
    };
}

/// The block a back edge's poll in `block` goes to, if `block` ends in
/// one: a jump to a block at or before its own. The builder gives every
/// other way back a block of its own that ends so.
pub(crate) fn poll_target(graph: &Graph, block: BlockId) -> Option<BlockId> {
    match graph.block(block).end {
        End::Jump(target) if target <= block => Some(target),
        _ => None,
    }
}

/// How many bytecode registers a graph names: one past the highest.
pub(crate) fn named_registers(graph: &Graph) -> usize {
    let mut highest = 0;
    let mut note = |register: u16| highest = highest.max(usize::from(register) + 1);
    for block in &graph.blocks {
        for phi in &block.phis {
            note(phi.register);
        }
        for node in &block.nodes {
            match node {
                Node::FrameRead { register, .. } => note(*register),
                Node::Copy {
                    destination_register,
                    ..
                }
                | Node::IntegerBinary {
                    destination_register,
                    ..
                } => note(*destination_register),
                Node::Frame { reads, written, .. } => {
                    for (register, _) in reads {
                        note(*register);
                    }
                    if let Some(written) = written {
                        note(*written);
                    }
                }
                Node::Constant { .. } | Node::Receiver { .. } | Node::OwnSlotLoad { .. } => {}
            }
        }
    }
    highest
}

/// What `node` does to the bindings, past its code: the frame node's
/// write-back and its leaving every register to its frame word, a frame
/// read's clean binding, a `Copy`'s and a fast path's dirty one.
pub(crate) fn bind(bindings: &mut [Binding], node: &Node) {
    match node {
        Node::FrameRead { register, value } => {
            bindings[usize::from(*register)] = Binding {
                value: Some(*value),
                clean: true,
            };
        }
        Node::Copy {
            destination_register,
            source,
        } => {
            let binding = &mut bindings[usize::from(*destination_register)];
            if binding.value != Some(*source) {
                *binding = Binding {
                    value: Some(*source),
                    clean: false,
                };
            }
        }
        Node::IntegerBinary {
            destination_register,
            value,
            ..
        } => {
            bindings[usize::from(*destination_register)] = Binding {
                value: Some(*value),
                clean: false,
            };
        }
        Node::Frame { .. } => bindings.fill(Binding::FRAME_WORD),
        Node::Constant { .. } | Node::Receiver { .. } | Node::OwnSlotLoad { .. } => {}
    }
}

/// The bindings at the top of `successor` from those its predecessors
/// leave it with, `None` for a predecessor not yet seen: a phi's register
/// is bound to the phi, dirty unless every way in brings its input
/// clean; a register every way in binds to one value stays bound to it;
/// any other is its frame word, every dirty binding of it written back on
/// its way in -- or, if `tainted` (the registers the leftover analysis
/// taints at `successor`'s top) holds it, `Binding::STALE`, and nothing is
/// written back.
pub(crate) fn join(
    graph: &Graph,
    successor: BlockId,
    incoming: &[Option<&[Binding]>],
    register_count: usize,
    tainted: &[Range<u16>],
) -> Vec<Binding> {
    let block = graph.block(successor);
    let mut joined = vec![Binding::FRAME_WORD; register_count];
    for (register, slot) in joined.iter_mut().enumerate() {
        if let Some(phi) = block
            .phis
            .iter()
            .find(|phi| usize::from(phi.register) == register)
        {
            let clean = incoming.iter().zip(&phi.inputs).all(|(bindings, input)| {
                bindings.is_none_or(|bindings| {
                    bindings[register]
                        == Binding {
                            value: Some(*input),
                            clean: true,
                        }
                })
            });
            *slot = Binding {
                value: Some(phi.value),
                clean,
            };
            continue;
        }
        let mut seen = incoming.iter().flatten().map(|bindings| bindings[register]);
        let Some(first) = seen.next() else {
            continue;
        };
        let mut agreed = first;
        let mut agree = true;
        for binding in seen {
            if binding.value != agreed.value {
                agree = false;
                break;
            }
            agreed.clean &= binding.clean;
        }
        if agree {
            *slot = agreed;
        } else if u16::try_from(register)
            .is_ok_and(|register| tainted.iter().any(|range| range.contains(&register)))
        {
            *slot = Binding::STALE;
        }
    }
    joined
}

/// The bindings at each block's top, `None` for a block not in the
/// order, found by iterating the joins to their fixed point: a loop's
/// head is first joined from its ways in from before it. `tainted_before`
/// is the snapshot's, if known.
pub(crate) fn entry_bindings(
    graph: &Graph,
    positions: &Positions,
    register_count: usize,
    tainted_before: Option<&[Vec<Range<u16>>]>,
) -> Vec<Option<Vec<Binding>>> {
    let count = graph.blocks.len();
    let mut entries: Vec<Option<Vec<Binding>>> = vec![None; count];
    // For each block, what it leaves each successor with.
    let mut exits: Vec<Vec<(BlockId, Vec<Binding>)>> = vec![Vec::new(); count];
    loop {
        let mut changed = false;
        for block in &positions.order {
            let entry = if block.index() == 0 {
                vec![Binding::FRAME_WORD; register_count]
            } else {
                let incoming: Vec<Option<&[Binding]>> = graph
                    .block(*block)
                    .predecessors
                    .iter()
                    .map(|predecessor| {
                        exits[predecessor.index()]
                            .iter()
                            .find(|(successor, _)| successor == block)
                            .map(|(_, bindings)| bindings.as_slice())
                    })
                    .collect();
                let tainted = tainted_before
                    .and_then(|sets| {
                        sets.get(usize::try_from(graph.block(*block).program_counter).ok()?)
                    })
                    .map_or(&[][..], Vec::as_slice);
                join(graph, *block, &incoming, register_count, tainted)
            };
            if entries[block.index()].as_ref() != Some(&entry) {
                changed = true;
            }
            exits[block.index()] = block_exits(graph, *block, &entry);
            entries[block.index()] = Some(entry);
        }
        if !changed {
            return entries;
        }
    }
}

/// What `block`, entered with `entry`, leaves each of its successors
/// with.
fn block_exits(graph: &Graph, block: BlockId, entry: &[Binding]) -> Vec<(BlockId, Vec<Binding>)> {
    let nodes = &graph.block(block).nodes;
    let mut bindings = entry.to_vec();
    let mut exits = Vec::new();
    for (index, node) in nodes.iter().enumerate() {
        if let Node::IntegerBinary { fallback, .. } = node {
            if graph.block(block).exit_position(*fallback) == Some(index) {
                exits.push((*fallback, bindings.clone()));
            }
        }
        bind(&mut bindings, node);
    }
    for successor in graph.block(block).end.successors() {
        exits.push((successor, bindings.clone()));
    }
    exits
}

/// A set of values, one bit each.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ValueSet {
    words: Vec<u64>,
}

impl ValueSet {
    fn empty(count: usize) -> ValueSet {
        ValueSet {
            words: vec![0; count.div_ceil(64)],
        }
    }

    fn insert(&mut self, value: ValueId) {
        self.words[value.index() / 64] |= 1 << (value.index() % 64);
    }

    fn values(&self) -> impl Iterator<Item = ValueId> + '_ {
        self.words.iter().enumerate().flat_map(|(index, word)| {
            (0..64)
                .filter(move |bit| word & (1 << bit) != 0)
                .map(move |bit| {
                    ValueId(
                        u32::try_from(index * 64 + bit).expect("internal: a value id fits a u32"),
                    )
                })
        })
    }
}

/// How a position reads a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Need {
    /// In a machine register: an operand of a simple node or of an end.
    Register,
    /// Wherever it is: a write-back or a phi's input, which a move takes
    /// from a register or from a frame word alike.
    Anywhere,
}

/// Each value's live ranges and uses, and the positions register
/// allocation must know of.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Intervals {
    pub(crate) positions: Positions,
    /// For each value, the half-open ranges of positions it is live in,
    /// sorted and apart: from its definition, or a block's top, to just
    /// past the last position that reads it there.
    pub(crate) ranges: Vec<Vec<(u32, u32)>>,
    /// For each value, the positions that read it in a machine register,
    /// ascending.
    pub(crate) register_uses: Vec<Vec<u32>>,
    /// The use position of every frame node.
    pub(crate) frames: Vec<u32>,
    /// The use position of every simple node that calls a leaf
    /// (`Node::calls_leaf`).
    pub(crate) leaf_calls: Vec<u32>,
    /// The end's use position of every block that polls.
    pub(crate) polls: Vec<u32>,
    /// For each block in the order, the values live at its top that it
    /// does not define: those its predecessors hand it, its phis apart.
    pub(crate) live_in: Vec<Vec<ValueId>>,
    /// The bindings at each block's top (`entry_bindings`).
    pub(crate) entry_bindings: Vec<Option<Vec<Binding>>>,
    /// How many bytecode registers the graph binds (`named_registers`).
    pub(crate) register_count: usize,
}

impl Intervals {
    /// Whether `value` is live at `position`.
    pub(crate) fn live_at(&self, value: ValueId, position: u32) -> bool {
        let ranges = &self.ranges[value.index()];
        let after = ranges.partition_point(|(start, _)| *start <= position);
        after > 0 && position < ranges[after - 1].1
    }

    /// The first position at or after `position` that reads `value` in a
    /// machine register, `u32::MAX` if none does.
    pub(crate) fn next_register_use(&self, value: ValueId, position: u32) -> u32 {
        let uses = &self.register_uses[value.index()];
        uses.get(uses.partition_point(|used| *used < position))
            .copied()
            .unwrap_or(u32::MAX)
    }
}

/// The live ranges of `graph`'s values over its blocks in `order`, which
/// `block_order` gives, knowing nothing of the leftover analysis: every
/// binding a merge does not carry on is written back on its way in.
#[cfg(test)]
pub(crate) fn intervals(graph: &Graph, order: &[BlockId]) -> Intervals {
    intervals_knowing(graph, order, None)
}

/// `intervals`, with the registers the leftover analysis taints before
/// each instruction, a snapshot's `tainted_before`: a binding a merge
/// does not carry on is not written back if the merge's top taints it.
pub(crate) fn intervals_knowing(
    graph: &Graph,
    order: &[BlockId],
    tainted_before: Option<&[Vec<Range<u16>>]>,
) -> Intervals {
    let positions = Positions::of(graph, order);
    let register_count = named_registers(graph);
    let entry_bindings = entry_bindings(graph, &positions, register_count, tainted_before);
    let value_count = graph.values.len();

    // Every read of a value in each block, write-backs included, and the
    // definitions.
    let mut reads: Vec<Vec<(u32, ValueId, Need)>> = vec![Vec::new(); graph.blocks.len()];
    let mut frames = Vec::new();
    let mut leaf_calls = Vec::new();
    let mut polls = Vec::new();
    for block in order {
        let entry = entry_bindings[block.index()]
            .as_ref()
            .expect("internal: every block in the order has its bindings");
        let mut bindings = entry.clone();
        let reads = &mut reads[block.index()];
        let write_back =
            |bindings: &[Binding], position: u32, reads: &mut Vec<(u32, ValueId, Need)>| {
                for binding in bindings {
                    if let Binding {
                        value: Some(value),
                        clean: false,
                    } = binding
                    {
                        reads.push((position, *value, Need::Anywhere));
                    }
                }
            };
        let nodes = &graph.block(*block).nodes;
        for (index, node) in nodes.iter().enumerate() {
            let position = positions.node_use(*block, index);
            match node {
                Node::Frame { .. } => {
                    frames.push(position);
                    write_back(&bindings, position, reads);
                }
                Node::Copy { source, .. }
                | Node::OwnSlotLoad {
                    receiver: source, ..
                } => reads.push((position, *source, Need::Register)),
                Node::IntegerBinary { left, right, .. } => {
                    reads.push((position, *left, Need::Register));
                    reads.push((position, *right, Need::Register));
                }
                Node::FrameRead { .. } | Node::Constant { .. } | Node::Receiver { .. } => {}
            }
            if node.calls_leaf() {
                leaf_calls.push(position);
            }
            bind(&mut bindings, node);
        }
        let end_use = positions.end_use(*block);
        if let Some(value) = graph.block(*block).end.uses() {
            reads.push((end_use, value, Need::Register));
        }
        if poll_target(graph, *block).is_some() {
            polls.push(end_use);
            write_back(&bindings, end_use, reads);
        }
        // A merge's way in: the phis' inputs, and every dirty binding the
        // merge does not carry on.
        let end_definition = positions.end_definition(*block);
        for successor in graph.block(*block).end.successors() {
            let target = graph.block(successor);
            if target.predecessors.len() < 2 {
                continue;
            }
            let way = target
                .predecessors
                .iter()
                .position(|predecessor| predecessor == block)
                .expect("internal: a block is its successor's predecessor");
            for phi in &target.phis {
                reads.push((end_definition, phi.inputs[way], Need::Anywhere));
            }
            let merged = entry_bindings[successor.index()]
                .as_ref()
                .expect("internal: a successor is in the order");
            for (binding, joined) in bindings.iter().zip(merged) {
                if let Binding {
                    value: Some(value),
                    clean: false,
                } = binding
                {
                    if *joined == Binding::FRAME_WORD {
                        reads.push((end_definition, *value, Need::Anywhere));
                    }
                }
            }
        }
    }

    // Where each value is defined.
    let mut definition = vec![(BlockId(u32::MAX), 0u32); value_count];
    for block in order {
        for phi in &graph.block(*block).phis {
            definition[phi.value.index()] = (*block, positions.phi_definition(*block));
        }
        for (index, node) in graph.block(*block).nodes.iter().enumerate() {
            if let Some(value) = node.defined() {
                definition[value.index()] = (*block, positions.node_definition(*block, index));
            }
        }
    }

    // The values live at each block's top, to the fixed point: those it
    // reads, or hands a successor, that it does not define.
    let mut live_in: Vec<ValueSet> = vec![ValueSet::empty(value_count); graph.blocks.len()];
    let handed = |block: BlockId, live_in: &[ValueSet]| -> Vec<(u32, ValueId)> {
        let mut handed = Vec::new();
        for successor in graph.block(block).successors() {
            let exit = positions.exit(graph, block, successor);
            for value in live_in[successor.index()].values() {
                handed.push((exit, value));
            }
        }
        handed
    };
    loop {
        let mut changed = false;
        for block in order.iter().rev() {
            let mut live = ValueSet::empty(value_count);
            let read = reads[block.index()].iter().map(|(_, value, _)| *value);
            let handed_on = handed(*block, &live_in);
            for value in read.chain(handed_on.into_iter().map(|(_, value)| value)) {
                if definition[value.index()].0 != *block {
                    live.insert(value);
                }
            }
            if live != live_in[block.index()] {
                live_in[block.index()] = live;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    // The ranges: in each block, from the definition or the top to past
    // the last position that needs the value.
    let mut ranges: Vec<Vec<(u32, u32)>> = vec![Vec::new(); value_count];
    let mut register_uses: Vec<Vec<u32>> = vec![Vec::new(); value_count];
    for block in order {
        let mut last: std::collections::BTreeMap<ValueId, u32> = std::collections::BTreeMap::new();
        let mut need = |value: ValueId, position: u32| {
            let entry = last.entry(value).or_insert(position);
            *entry = (*entry).max(position);
        };
        for (position, value, kind) in &reads[block.index()] {
            need(*value, *position);
            if *kind == Need::Register {
                register_uses[value.index()].push(*position);
            }
        }
        for (exit, value) in handed(*block, &live_in) {
            need(value, exit);
        }
        for value in live_in[block.index()].values() {
            need(value, positions.entry(*block));
        }
        for phi in &graph.block(*block).phis {
            need(phi.value, positions.phi_definition(*block));
        }
        for (index, node) in graph.block(*block).nodes.iter().enumerate() {
            if let Some(value) = node.defined() {
                need(value, positions.node_definition(*block, index));
            }
        }
        for (value, end) in last {
            let (defining, defined_at) = definition[value.index()];
            let start = if defining == *block {
                defined_at
            } else {
                positions.entry(*block)
            };
            ranges[value.index()].push((start, end + 1));
        }
    }
    for ranges in &mut ranges {
        ranges.sort_unstable();
        let mut merged: Vec<(u32, u32)> = Vec::with_capacity(ranges.len());
        for (start, end) in ranges.drain(..) {
            match merged.last_mut() {
                Some(previous) if previous.1 >= start => previous.1 = previous.1.max(end),
                _ => merged.push((start, end)),
            }
        }
        *ranges = merged;
    }
    for uses in &mut register_uses {
        uses.sort_unstable();
        uses.dedup();
    }

    Intervals {
        positions,
        ranges,
        register_uses,
        frames,
        leaf_calls,
        polls,
        live_in: live_in.iter().map(|set| set.values().collect()).collect(),
        entry_bindings,
        register_count,
    }
}

#[cfg(test)]
mod tests {
    use super::{block_order, intervals};
    use crate::bytecode::{Instruction, Literal};
    use crate::jit::optimizing::build::build;
    use crate::jit::optimizing::graph::{BlockId, Node};
    use crate::jit::optimizing::{corpus_of_bodies, test_snapshot};

    #[test]
    fn the_block_order_puts_every_block_after_its_ways_in_but_a_loop_s_back_edge() {
        for snapshot in corpus_of_bodies() {
            let graph = build(&snapshot);
            let order = block_order(&graph);
            assert_eq!(order[0], BlockId(0));
            assert_eq!(
                order.len(),
                graph.blocks.len(),
                "every built block is reachable"
            );
            let place = |block: BlockId| order.iter().position(|each| *each == block);
            for block in &order {
                for predecessor in &graph.block(*block).predecessors {
                    assert!(
                        place(*predecessor) < place(*block) || graph.block(*block).loop_head,
                        "a way into a block that heads no loop comes before it"
                    );
                }
            }
        }
    }

    #[test]
    fn a_value_lives_from_its_definition_to_its_last_use() {
        // r3 := self. r4 := r3. ^r4
        let graph = build(&test_snapshot(vec![
            Instruction::LoadSelf { destination: 3 },
            Instruction::LoadLiteral {
                destination: 5,
                literal: Literal::SmallInteger(1),
            },
            Instruction::Move {
                destination: 4,
                source: 3,
            },
            Instruction::Return { register: 4 },
        ]));
        let order = block_order(&graph);
        let intervals = intervals(&graph, &order);
        let Node::Receiver { value } = graph.blocks[0].nodes[0] else {
            panic!("the receiver first");
        };
        let positions = &intervals.positions;
        let defined = positions.node_definition(BlockId(0), 0);
        assert_eq!(
            intervals.ranges[value.index()],
            vec![(defined, positions.end_use(BlockId(0)) + 1)]
        );
        assert!(intervals.live_at(value, defined));
        assert!(!intervals.live_at(value, defined - 1));
        assert_eq!(
            intervals.next_register_use(value, defined),
            positions.node_use(BlockId(0), 1),
            "its first use is the Copy into r3"
        );
    }

    #[test]
    fn a_dirty_binding_lives_until_the_send_writes_it_back() {
        // r3 := self. r6 := r5 foo: r5 -- the send writes r3 back.
        let graph = build(&test_snapshot(vec![
            Instruction::LoadSelf { destination: 3 },
            crate::jit::optimizing::test_send(5, 5, 6),
            Instruction::Return { register: 6 },
        ]));
        let intervals = intervals(&graph, &block_order(&graph));
        let Node::Receiver { value } = graph.blocks[0].nodes[0] else {
            panic!("the receiver first");
        };
        let send = intervals.frames[0];
        assert!(intervals.live_at(value, send));
        assert!(!intervals.live_at(value, send + 1));
    }
}
