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
use crate::heap::Immediate;

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
/// the block's `phis` or `nodes`; and what kind of value it is known to
/// be wherever it is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ValueDefinition {
    pub(crate) block: BlockId,
    pub(crate) place: Place,
    pub(crate) kind: ValueKind,
}

/// What a value is known to be, so that what it is is not tested again
/// (`docs/design/execution.md`, "Speculation", "Guards and traps"): a value
/// is of a kind only where its definition guarantees it -- an integer
/// fast path's result, which exists only on the way its tests passed, a
/// constant's word, or a phi whose every input is of the kind -- and the
/// graph's values are never written, so the kind holds at every read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ValueKind {
    Unknown,
    SmallInteger,
    Boolean,
}

impl ValueKind {
    /// The kind of an `IntegerBinary` fast path's result: a
    /// `SmallInteger` for arithmetic, which declines what leaves 61 bits,
    /// and a Boolean for a comparison.
    pub(crate) fn of_operation(operation: IntegerOperation) -> ValueKind {
        match operation {
            IntegerOperation::Add
            | IntegerOperation::Subtract
            | IntegerOperation::Multiply
            | IntegerOperation::FloorDivide => ValueKind::SmallInteger,
            IntegerOperation::Less
            | IntegerOperation::Greater
            | IntegerOperation::LessOrEqual
            | IntegerOperation::GreaterOrEqual
            | IntegerOperation::Equal => ValueKind::Boolean,
        }
    }

    /// The kind of an immediate word.
    pub(crate) fn of_word(word: u64) -> ValueKind {
        if Immediate::is_small_integer_word(word) {
            ValueKind::SmallInteger
        } else if word == Immediate::boolean(true).bits()
            || word == Immediate::boolean(false).bits()
        {
            ValueKind::Boolean
        } else {
            ValueKind::Unknown
        }
    }
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
    /// The `SmallInteger` fast path of `IntegerBinary`, the instruction
    /// at `program_counter`, its result bound to `destination_register`;
    /// `fallback` is the block holding the instruction's fallback send,
    /// where control goes with the register still unwritten when the fast
    /// path does not apply -- or `None`, where the feedback says the
    /// fallback never ran: the fast path's failure is then a trap, which
    /// writes the frame back and exits to run the instruction again
    /// (`docs/design/execution.md`, "Speculation", "Guards and traps"), so
    /// that past the node the result is certain and no merge with the
    /// fallback's way costs a phi.
    IntegerBinary {
        operation: IntegerOperation,
        program_counter: u32,
        destination_register: u16,
        value: ValueId,
        left: ValueId,
        right: ValueId,
        fallback: Option<BlockId>,
    },
    /// A customized copy's read of the receiver's field at
    /// `absolute_index` (`LoadOwnSlot`, `ReturnSlotIfFilled`): the
    /// `own_index` offset by the entered application's own slot base,
    /// which the copy's key fixes. It checks no guard: the receiver was
    /// verified at the copy's entry, and whatever could replace it -- a
    /// forward, an install -- runs after an exit, past which the frame goes
    /// on in baseline code (`docs/design/execution.md`, "Speculation", "Own
    /// slots at fixed offsets").
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
    /// A Boolean test, the instruction at `program_counter`: `if_true`
    /// for `true`, `if_false` for `false`, `not_boolean` for any other
    /// value. The interpreter's three branch instructions each decide one
    /// of them and go on at the next instruction otherwise, so two of the
    /// three are the same block. `not_boolean` is `None` where there is no
    /// third way: the condition is known to be a Boolean
    /// (`ValueKind::Boolean`), and nothing is tested for it; or else the
    /// feedback says it never ran, and any other value traps, running the
    /// instruction again (`Graph::end_trap`).
    Branch {
        program_counter: u32,
        condition: ValueId,
        if_true: BlockId,
        if_false: BlockId,
        not_boolean: Option<BlockId>,
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
    /// The send at `program_counter`, whose call site `site` the feedback
    /// says never ran: the block writes the frame back and exits to run
    /// it in the interpreter, which fills the site's cache. A trap does
    /// not fall through, so nothing after the send is built on this way.
    Trap {
        program_counter: u32,
        site: u32,
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

    /// What the value the node defines is known to be, by the node alone.
    pub(crate) fn kind(&self) -> ValueKind {
        match self {
            Node::Constant { word, .. } => ValueKind::of_word(*word),
            Node::IntegerBinary { operation, .. } => ValueKind::of_operation(*operation),
            Node::FrameRead { .. }
            | Node::Receiver { .. }
            | Node::Copy { .. }
            | Node::OwnSlotLoad { .. }
            | Node::Frame { .. } => ValueKind::Unknown,
        }
    }

    /// The program counter of the trap the node takes when its fast path
    /// fails, if it takes one: an integer fast path with no fallback.
    pub(crate) fn trap(&self) -> Option<u32> {
        match self {
            Node::IntegerBinary {
                program_counter,
                fallback: None,
                ..
            } => Some(*program_counter),
            _ => None,
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
            } => [Some(*if_true), Some(*if_false), *not_boolean]
                .into_iter()
                .flatten()
                .collect(),
            End::Return { .. } | End::Frame | End::Trap { .. } => Vec::new(),
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
            End::Jump(_) | End::Frame | End::FrameJump { .. } | End::Trap { .. } => None,
        }
    }
}

impl Block {
    /// The blocks control goes to from this one, each once: the end's,
    /// then an integer fast path's fallback.
    pub(crate) fn successors(&self) -> Vec<BlockId> {
        let mut successors = self.end.successors();
        for node in &self.nodes {
            if let Node::IntegerBinary {
                fallback: Some(fallback),
                ..
            } = node
            {
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
            |node| matches!(node, Node::IntegerBinary { fallback: Some(fallback), .. } if *fallback == successor),
        )
    }
}

impl Graph {
    /// The graph of `blocks`, its value table found from their phis and
    /// nodes, with each value's kind (`kinds_of`); a value defined twice is
    /// recorded at its first definition, which `verify` then refuses.
    pub(crate) fn from_blocks(blocks: Vec<Block>) -> Graph {
        let mut found: Vec<Option<ValueDefinition>> = Vec::new();
        let mut record = |value: ValueId, block: BlockId, place: Place, kind: ValueKind| {
            let index = value.index();
            if found.len() <= index {
                found.resize(index + 1, None);
            }
            found[index].get_or_insert(ValueDefinition { block, place, kind });
        };
        for (block_index, block) in blocks.iter().enumerate() {
            let block_id = BlockId::of_index(block_index);
            for (index, phi) in block.phis.iter().enumerate() {
                record(phi.value, block_id, Place::Phi(index), ValueKind::Unknown);
            }
            for (index, node) in block.nodes.iter().enumerate() {
                if let Some(value) = node.defined() {
                    record(value, block_id, Place::Node(index), node.kind());
                }
            }
        }
        let values = found
            .into_iter()
            .map(|definition| {
                definition.expect("internal: every value id up to the last is defined")
            })
            .collect();
        let mut graph = Graph { blocks, values };
        graph.find_phi_kinds();
        graph
    }

    /// Each phi's kind: the one its every input has, found optimistically
    /// -- a phi not yet decided counts as agreeing with any kind -- so
    /// that a loop's phi whose inputs are its entry's `SmallInteger` and
    /// the fast path that adds to it is a `SmallInteger` too. A phi's kind
    /// only falls, from undecided to a kind to `Unknown`, so the search
    /// ends; one still undecided, whose inputs are only phis like it,
    /// stays `Unknown`.
    fn find_phi_kinds(&mut self) {
        let mut decided: Vec<Option<ValueKind>> = self
            .values
            .iter()
            .map(|definition| match definition.place {
                Place::Phi(_) => None,
                Place::Node(_) => Some(definition.kind),
            })
            .collect();
        let mut changed = true;
        while changed {
            changed = false;
            for block in &self.blocks {
                for phi in &block.phis {
                    let mut met: Option<ValueKind> = None;
                    for input in &phi.inputs {
                        // An input the table does not hold is one `verify`
                        // refuses: it says nothing of the phi.
                        let kind = decided
                            .get(input.index())
                            .copied()
                            .unwrap_or(Some(ValueKind::Unknown));
                        met = match (met, kind) {
                            (met, None) => met,
                            (None, Some(kind)) => Some(kind),
                            (Some(met), Some(kind)) if met == kind => Some(met),
                            (Some(_), Some(_)) => Some(ValueKind::Unknown),
                        };
                    }
                    let slot = &mut decided[phi.value.index()];
                    if met.is_some() && *slot != met {
                        *slot = met;
                        changed = true;
                    }
                }
            }
        }
        for (definition, decided) in self.values.iter_mut().zip(decided) {
            if matches!(definition.place, Place::Phi(_)) {
                definition.kind = decided.unwrap_or(ValueKind::Unknown);
            }
        }
    }

    /// Forgets every value's kind: a compile without speculation builds
    /// stage 1's code, which tests every operand and every condition.
    pub(crate) fn forget_kinds(&mut self) {
        for definition in &mut self.values {
            definition.kind = ValueKind::Unknown;
        }
    }

    /// What `value` is known to be.
    pub(crate) fn kind(&self, value: ValueId) -> ValueKind {
        self.values[value.index()].kind
    }

    /// The program counter of the trap `block`'s end takes, if it takes
    /// one: a send never executed, or a conditional with no third way
    /// whose condition is not known to be a Boolean, which any other value
    /// fails.
    pub(crate) fn end_trap(&self, block: BlockId) -> Option<u32> {
        match self.block(block).end {
            End::Trap {
                program_counter, ..
            } => Some(program_counter),
            End::Branch {
                program_counter,
                condition,
                not_boolean: None,
                ..
            } if self.kind(condition) != ValueKind::Boolean => Some(program_counter),
            _ => None,
        }
    }

    pub(crate) fn block(&self, block: BlockId) -> &Block {
        &self.blocks[block.index()]
    }

    /// The phi or node that defines `value`.
    #[cfg(test)]
    pub(crate) fn definition(&self, value: ValueId) -> Definition<'_> {
        let ValueDefinition { block, place, .. } = self.values[value.index()];
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
