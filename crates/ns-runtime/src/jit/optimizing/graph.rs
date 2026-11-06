//! The optimizing compiler's intermediate form (`docs/design/execution.md`,
//! "One tier, an SSA graph, the macro assembler"): a control-flow graph in
//! SSA form over one body's instructions, built by `build::build` in one
//! pass and checked by `verify::verify`.
//!
//! A value lives in a machine register only between two frame nodes: a
//! frame node may run Newspeak code, which can write this frame's
//! registers by reference, move every object a value names, or reify the
//! frame, so after one every register is read again from its frame word
//! (`Node::FrameRead`). Every node but a frame node is register
//! arithmetic that emits no call.
use crate::bytecode::IntegerOperation;

use super::snapshot::SnapshotInstruction;

/// A block of the graph: its index in `Graph::blocks`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct BlockId(pub(crate) u32);

impl BlockId {
    pub(crate) fn index(self) -> usize {
        usize::try_from(self.0).expect("internal: a block id fits a usize")
    }

    pub(crate) fn of_index(index: usize) -> BlockId {
        BlockId(u32::try_from(index).expect("internal: a body has fewer than 2^32 blocks"))
    }
}

/// A value of the graph, defined once, by a node or a phi: its index in
/// `Graph::values`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct ValueId(pub(crate) u32);

impl ValueId {
    pub(crate) fn index(self) -> usize {
        usize::try_from(self.0).expect("internal: a value id fits a usize")
    }
}

/// A body's graph: its blocks, block 0 the entry, and where each value is
/// defined.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Graph {
    pub(crate) blocks: Vec<Block>,
    pub(crate) values: Vec<ValueDefinition>,
}

/// Where a value is defined: a phi or a node of a block, by its index in
/// the block's `phis` or `nodes`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ValueDefinition {
    pub(crate) block: BlockId,
    pub(crate) place: Place,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Place {
    Phi(usize),
    Node(usize),
}

/// What defines a value, as `Graph::definition` answers it.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Definition<'graph> {
    Phi(&'graph Phi),
    Node(&'graph Node),
}

/// A straight run of nodes, entered only at its top.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Block {
    /// The index of the block's first instruction. The entry block a body
    /// whose first instruction is a loop's head is given holds no
    /// instruction and names 0 as well.
    pub(crate) program_counter: u32,
    pub(crate) phis: Vec<Phi>,
    pub(crate) nodes: Vec<Node>,
    pub(crate) end: End,
    /// Every block with an edge to this one, once each, in the order the
    /// phis' inputs follow.
    pub(crate) predecessors: Vec<BlockId>,
    /// Whether an edge from this block or one after it leads here: the
    /// head of a loop, whose phis were made before their back edges were
    /// seen.
    pub(crate) loop_head: bool,
}

/// The value a register holds where control from several blocks meets:
/// one input per predecessor, in the order of `Block::predecessors`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Phi {
    pub(crate) register: u16,
    pub(crate) value: ValueId,
    pub(crate) inputs: Vec<ValueId>,
}

/// One node of a block.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Node {
    /// The register's value as its frame word holds it: at entry, and
    /// after a frame node.
    FrameRead { register: u16, value: ValueId },
    /// An immediate literal's word (`jit::immediate_literal`). `self` is
    /// not a constant: the receiver is a word of the frame, which a
    /// collection may move.
    Constant { value: ValueId, word: u64 },
    /// The receiver, read from the frame's header.
    Receiver { value: ValueId },
    /// Binds the register to the value; emits nothing, but the register's
    /// frame word must take the value before the next interrupt point.
    Copy {
        destination_register: u16,
        source: ValueId,
    },
    /// The `SmallInteger` fast path of `IntegerBinary`, its result bound
    /// to `destination_register`; `fallback` is the block holding the
    /// instruction's fallback send, where control goes with the register
    /// still unwritten when the fast path does not apply.
    IntegerBinary {
        operation: IntegerOperation,
        destination_register: u16,
        value: ValueId,
        left: ValueId,
        right: ValueId,
        fallback: BlockId,
    },
    /// A customized copy's read of the receiver's field at
    /// `absolute_index` (`LoadOwnSlot`, `ReturnSlotIfFilled`): the
    /// `own_index` offset by the entered application's own slot base,
    /// which the copy's key fixes. It checks no guard: the receiver was
    /// verified at the copy's entry, and whatever could replace it -- a
    /// forward, an install -- runs after an exit, past which the frame goes
    /// on in baseline code (`docs/design/execution.md`, "Customization").
    /// Nor does it check the field exists: every object of a program class
    /// is an ordinary instance of exactly its application's slot count,
    /// which the allocation makes and the image decoder holds a loaded one
    /// to (`image::decode`'s `check_program_instances`, and its checks of
    /// a shipped class's records as they are read).
    OwnSlotLoad {
        value: ValueId,
        receiver: ValueId,
        absolute_index: u32,
    },
    /// Every other instruction, emitted by the baseline's template, its
    /// result in its frame word. Before it, every register binding not yet
    /// stored -- each `Copy`, fast path and phi whose value its register's
    /// frame word does not hold yet, the operands' and every other
    /// register's alike -- is written back, since the instruction may run
    /// code that reads any of the frame's registers by reference, and the
    /// collector traces the frame as the interpreter left it. After it, no
    /// value is read again: every register is its frame word. `reads`
    /// pairs each register `registers_read` names with the value it holds;
    /// `written` is the register `register_written` names.
    Frame {
        program_counter: u32,
        instruction: SnapshotInstruction,
        reads: Vec<(u16, ValueId)>,
        written: Option<u16>,
    },
}

/// How a block ends. A `Jump` or `Branch` edge to a block at or before
/// its own is a back edge, which polls.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum End {
    Jump(BlockId),
    /// A Boolean test: `if_true` for `true`, `if_false` for `false`,
    /// `not_boolean` for any other value. The interpreter's three branch
    /// instructions each decide one of them and go on at the next
    /// instruction otherwise, so two of the three are the same block.
    Branch {
        condition: ValueId,
        if_true: BlockId,
        if_false: BlockId,
        not_boolean: BlockId,
    },
    Return {
        value: ValueId,
    },
    /// A customized copy's `ReturnSlotIfFilled`: returns `value`, the
    /// slot it read, unless it is `nil`, a lazy slot not yet computed
    /// (§6.3.3), and goes on at `next`, the slot's initializer, if it is.
    ReturnUnlessNil {
        value: ValueId,
        next: BlockId,
    },
    /// The block ends in a frame node that does not fall through: a
    /// non-local return, a signal, a load that always raises.
    Frame,
    /// The block ends in a frame node that goes on at `next` or jumps to
    /// `target`: a mirror send, whose failure code is `target`.
    FrameJump {
        next: BlockId,
        target: BlockId,
    },
}

impl Node {
    /// The value the node defines, if it defines one.
    pub(crate) fn defined(&self) -> Option<ValueId> {
        match self {
            Node::FrameRead { value, .. }
            | Node::Constant { value, .. }
            | Node::Receiver { value }
            | Node::IntegerBinary { value, .. }
            | Node::OwnSlotLoad { value, .. } => Some(*value),
            Node::Copy { .. } | Node::Frame { .. } => None,
        }
    }

    /// Whether the node is a simple node whose code calls a Rust leaf,
    /// across which only the registers `Roles::survive_leaf_calls` counts
    /// keep their values. No simple node does yet; a frame node is an
    /// interrupt point, not a leaf call, after which no value holds.
    pub(crate) fn calls_leaf(&self) -> bool {
        match self {
            Node::FrameRead { .. }
            | Node::Constant { .. }
            | Node::Receiver { .. }
            | Node::Copy { .. }
            | Node::IntegerBinary { .. }
            | Node::OwnSlotLoad { .. }
            | Node::Frame { .. } => false,
        }
    }

    /// The values the node reads, in operand order.
    pub(crate) fn uses(&self) -> Vec<ValueId> {
        match self {
            Node::FrameRead { .. } | Node::Constant { .. } | Node::Receiver { .. } => Vec::new(),
            Node::Copy { source, .. } => vec![*source],
            Node::OwnSlotLoad { receiver, .. } => vec![*receiver],
            Node::IntegerBinary { left, right, .. } => vec![*left, *right],
            Node::Frame { reads, .. } => reads.iter().map(|(_, value)| *value).collect(),
        }
    }
}

impl End {
    /// The blocks the end leads to, each once.
    pub(crate) fn successors(&self) -> Vec<BlockId> {
        let listed = match self {
            End::Jump(target) | End::ReturnUnlessNil { next: target, .. } => vec![*target],
            End::Branch {
                if_true,
                if_false,
                not_boolean,
                ..
            } => vec![*if_true, *if_false, *not_boolean],
            End::Return { .. } | End::Frame => Vec::new(),
            End::FrameJump { next, target } => vec![*next, *target],
        };
        let mut successors = Vec::with_capacity(listed.len());
        for block in listed {
            if !successors.contains(&block) {
                successors.push(block);
            }
        }
        successors
    }

    /// The value the end reads, if it reads one.
    pub(crate) fn uses(&self) -> Option<ValueId> {
        match self {
            End::Branch { condition, .. } => Some(*condition),
            End::Return { value } | End::ReturnUnlessNil { value, .. } => Some(*value),
            End::Jump(_) | End::Frame | End::FrameJump { .. } => None,
        }
    }
}

impl Block {
    /// The blocks control goes to from this one, each once: the end's,
    /// then an integer fast path's fallback.
    pub(crate) fn successors(&self) -> Vec<BlockId> {
        let mut successors = self.end.successors();
        for node in &self.nodes {
            if let Node::IntegerBinary { fallback, .. } = node {
                if !successors.contains(fallback) {
                    successors.push(*fallback);
                }
            }
        }
        successors
    }

    /// Where in this block control leaves for `successor`: past every
    /// node when the end leads there, before the fast path when only an
    /// integer fast path's fallback does, since the fallback is taken
    /// with the fast path's result unbound. `None` if no edge leads there.
    pub(crate) fn exit_position(&self, successor: BlockId) -> Option<usize> {
        if self.end.successors().contains(&successor) {
            return Some(self.nodes.len());
        }
        self.nodes.iter().position(
            |node| matches!(node, Node::IntegerBinary { fallback, .. } if *fallback == successor),
        )
    }
}

impl Graph {
    /// The graph of `blocks`, its value table found from their phis and
    /// nodes; a value defined twice is recorded at its first definition,
    /// which `verify` then refuses.
    pub(crate) fn from_blocks(blocks: Vec<Block>) -> Graph {
        let mut found: Vec<Option<ValueDefinition>> = Vec::new();
        let mut record = |value: ValueId, definition: ValueDefinition| {
            let index = value.index();
            if found.len() <= index {
                found.resize(index + 1, None);
            }
            found[index].get_or_insert(definition);
        };
        for (block_index, block) in blocks.iter().enumerate() {
            let block_id = BlockId::of_index(block_index);
            for (index, phi) in block.phis.iter().enumerate() {
                record(
                    phi.value,
                    ValueDefinition {
                        block: block_id,
                        place: Place::Phi(index),
                    },
                );
            }
            for (index, node) in block.nodes.iter().enumerate() {
                if let Some(value) = node.defined() {
                    record(
                        value,
                        ValueDefinition {
                            block: block_id,
                            place: Place::Node(index),
                        },
                    );
                }
            }
        }
        let values = found
            .into_iter()
            .map(|definition| {
                definition.expect("internal: every value id up to the last is defined")
            })
            .collect();
        Graph { blocks, values }
    }

    pub(crate) fn block(&self, block: BlockId) -> &Block {
        &self.blocks[block.index()]
    }

    /// The phi or node that defines `value`.
    #[cfg(test)]
    pub(crate) fn definition(&self, value: ValueId) -> Definition<'_> {
        let ValueDefinition { block, place } = self.values[value.index()];
        let block = self.block(block);
        match place {
            Place::Phi(index) => Definition::Phi(&block.phis[index]),
            Place::Node(index) => Definition::Node(&block.nodes[index]),
        }
    }

    /// The blocks that head a loop, in block order.
    #[cfg(test)]
    pub(crate) fn loop_heads(&self) -> impl Iterator<Item = BlockId> + '_ {
        self.blocks
            .iter()
            .enumerate()
            .filter(|(_, block)| block.loop_head)
            .map(|(index, _)| BlockId::of_index(index))
    }
}
