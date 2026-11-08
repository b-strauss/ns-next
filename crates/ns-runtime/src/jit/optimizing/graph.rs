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
//! arithmetic that runs no Newspeak code: none emits a call but a store's
//! out of line, through the write barrier into its remembering leaf
//! (`Node::calls_leaf`).
use std::ops::Range;

use crate::bytecode::{IntegerOperation, HEADER_REGISTERS};
use crate::heap::{ApplicationIndex, CodeId, Immediate};
use crate::mixin::DispatchId;

pub(crate) use super::plan::{EliminatedId, ScopeId};
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

/// A body's graph: its blocks, block 0 the entry, where each value is
/// defined, and the scopes its blocks belong to -- the body's own, and
/// each body inlined into it -- `ScopeId::ROOT` the body's; empty for a
/// graph made by hand, which says nothing of the leftover analysis.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Graph {
    pub(crate) blocks: Vec<Block>,
    pub(crate) values: Vec<ValueDefinition>,
    pub(crate) scopes: Vec<GraphScope>,
    /// The applications the arms always inlined that the graph lays out
    /// were decided for, each once: a field an accessor's node addresses
    /// is that application's layout's, which the copy then relies on
    /// (`jit::CopyDependencies`).
    pub(crate) arm_applications: Vec<ApplicationIndex>,
}

/// One scope of a graph, where it lies in the physical frame
/// (`docs/design/execution.md`, "Inlining"): its register `r` is the
/// physical frame's register `offset + r`, so that every frame word is the
/// one the interpreter's activation of it would hold, its header among
/// them. An inlined scope's receiver and current class are the bindings of
/// its header's first two words (`HeaderBinding`), which every interrupt
/// point inside it writes back like any register's; its other header
/// words are the exit's to write, and no node reads or writes them.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct GraphScope {
    pub(crate) code_id: CodeId,
    /// The scope that sends it and the program counter of the send;
    /// `None` for the root.
    pub(crate) parent: Option<(ScopeId, u32)>,
    pub(crate) offset: u16,
    pub(crate) argument_count: u16,
    /// How many registers the body names (`Snapshot::register_count`).
    pub(crate) register_count: u16,
    /// The registers the leftover analysis taints before each of the
    /// body's instructions, in its own numbering
    /// (`Snapshot::tainted_before`).
    pub(crate) tainted_before: Vec<Vec<Range<u16>>>,
    /// For a block's scope, the eliminated block whose `value` it runs:
    /// its activation's closure is that block, made by the exit.
    pub(crate) block: Option<EliminatedId>,
    /// The registers that hold an eliminated block before each of the
    /// body's instructions, in its own numbering, and the block
    /// (`plan::ScopePlan::eliminated_before`): where an exit stopped there
    /// writes the closure it makes of it.
    pub(crate) eliminated_before: Vec<Vec<(u16, EliminatedId)>>,
}

/// Why a physical register number fits a `u16`: the plan inlines a scope
/// only where its every register does (`plan::Planner::take`).
pub(crate) const FRAME_BOUND: &str =
    "internal: an inlined scope's registers lie below u16::MAX, which the plan keeps";

/// A header word of an inlined scope that is a binding of the allocation
/// like a register's: written back before every interrupt point inside
/// the scope, since the exit writes the header's other words only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HeaderBinding {
    Receiver,
    CurrentClass,
}

impl GraphScope {
    /// The physical register of the header word `binding` names.
    pub(crate) fn header_register(&self, binding: HeaderBinding) -> u16 {
        let word = match binding {
            HeaderBinding::Receiver => 0,
            HeaderBinding::CurrentClass => 1,
        };
        self.offset
            .checked_add(self.argument_count + word)
            .expect(FRAME_BOUND)
    }

    /// The physical registers of the header words the exit writes, past
    /// the two that are bindings: none for the root, whose header is its
    /// frame's own.
    pub(crate) fn exit_words(&self) -> Range<u16> {
        if self.parent.is_none() {
            return 0..0;
        }
        let header = self.header_register(HeaderBinding::Receiver);
        header + 2..header.checked_add(HEADER_REGISTERS).expect(FRAME_BOUND)
    }

    /// One past the highest physical register the scope's body names.
    pub(crate) fn top(&self) -> u16 {
        self.offset.saturating_add(self.register_count)
    }
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
    /// The scope the block's instructions belong to: a block holds one
    /// scope's alone.
    pub(crate) scope: ScopeId,
    /// The index of the block's first instruction in its scope's body.
    /// The entry block a body whose first instruction is a loop's head is
    /// given holds no instruction and names 0 as well.
    pub(crate) program_counter: u32,
    /// The scope and the instruction whose leftover analysis holds at the
    /// block's top, which is also where a frame goes on that leaves at a
    /// back edge to it: the block's own first instruction, but for the
    /// block an inlined scope's entry begins -- the scope's frame is not
    /// laid there yet -- whose point is its sender's send.
    pub(crate) taint_point: (ScopeId, u32),
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
    /// Every other instruction, of the scope `scope`, emitted by the
    /// baseline's template with the frame register at the scope's
    /// register 0, its result in its frame word. `instruction` is as the
    /// scope's body holds it, numbered from the scope's register 0, and
    /// `program_counter` its index there; `reads` and `written` name
    /// physical registers. Before it, every register binding not yet
    /// stored -- each `Copy`, fast path and phi whose value its register's
    /// frame word does not hold yet, the operands' and every other
    /// register's alike -- is written back, since the instruction may run
    /// code that reads any of the frame's registers by reference, and the
    /// collector traces the frame as the interpreter left it. After it, no
    /// value is read again: every register is its frame word. `reads`
    /// pairs each register `registers_read` names with the value it holds;
    /// `written` is the register `register_written` names.
    Frame {
        scope: ScopeId,
        program_counter: u32,
        instruction: SnapshotInstruction,
        reads: Vec<(u16, ValueId)>,
        written: Option<u16>,
    },
    /// The current class of an inlined scope, as a compiled call computes
    /// its callee's (`sends::send_public`): `steps` superclass steps up
    /// from the class `start` names of `from`. Emits no call.
    CurrentClass {
        value: ValueId,
        from: ValueId,
        start: ClassStart,
        steps: u16,
    },
    /// The physical registers an inlined callee's frame is laid over, at
    /// its entry, or that it leaves, at its return: their bindings end
    /// unwritten, every one a register the leftover analysis taints there
    /// -- the callee's scratch, its sender's registers past its slots and
    /// the header words the exit writes -- so that no value the sender
    /// held there is ever written into the callee's frame, nor read after
    /// the callee returns. Emits nothing.
    Overlay { ranges: Vec<Range<u16>> },
    /// An always-inlined getter, or a lazy slot's getter: field `field` of
    /// the object `object` names, the absolute index the lookup resolved
    /// the accessor to for the arm's receiver application, which the
    /// type-case's guard -- or, taken without a test, the scope's known
    /// receiver -- vouches the object is of. It checks the field exists no
    /// more than `OwnSlotLoad` does, for the same reason, and no guard:
    /// what could replace the object runs after an exit.
    FieldLoad {
        value: ValueId,
        object: ValueId,
        field: u32,
    },
    /// An always-inlined setter: `source` stored into field `field` of the
    /// object `object` names, as `FieldLoad` addresses it, through the
    /// inline write barrier, whose remembering leaf it may call. The
    /// setter answers its receiver, which the builder binds the send's
    /// destination to.
    FieldStore {
        object: ValueId,
        field: u32,
        source: ValueId,
    },
    /// `==` inlined: `true` where the two words are the same, `false`
    /// where they differ and the words alone decide it -- as
    /// `object_class::identical` and its decline for a future decide it
    /// -- and a trap, running the send at `program_counter` again, where
    /// they do not: either a future, for which the leaf declines. No
    /// number reaches it on its left, whose bits would decide: every
    /// number is a `Value`, whose `==` is `=`.
    IdentityCompare {
        value: ValueId,
        left: ValueId,
        right: ValueId,
        program_counter: u32,
    },
    /// `class` inlined: the class object of `object`, as
    /// `Mutator::class_of` finds it -- an object's class word, an
    /// immediate's class from the machine words -- which never needs the
    /// heap's tables, so it is a simple node.
    ClassOf { value: ValueId, object: ValueId },
    /// `at:` of an `Array` or a `ByteArray` inlined: the element the
    /// one-based `index` names of the object `object` names. Every check
    /// the row's leaf makes -- the object's kind, the index a
    /// `SmallInteger` within its bounds -- is a trap that runs the send at
    /// `program_counter` again, so that the interpreter decides, and
    /// raises, what the leaf would.
    IndexedLoad {
        kind: IndexedKind,
        value: ValueId,
        object: ValueId,
        index: ValueId,
        program_counter: u32,
    },
    /// `at:put:` inlined, as `IndexedLoad` checks it, a `ByteArray`'s
    /// byte also checked to be a `SmallInteger` from 0 to 255: `source`
    /// stored, into an `Array` through the inline write barrier, only once
    /// every check passed. It answers `source`, which the builder binds
    /// the send's destination to.
    IndexedStore {
        kind: IndexedKind,
        object: ValueId,
        index: ValueId,
        source: ValueId,
        program_counter: u32,
    },
    /// Traps, running the send at `program_counter` again, where `value`
    /// is `nil`: a lazy slot's getter inlined finds its slot unfilled, and
    /// the interpreter runs the getter, its initializer and all.
    TrapIfNil {
        value: ValueId,
        program_counter: u32,
    },
    /// A block the compile never creates (`plan::BlockPlan::Eliminated`):
    /// a value no machine code reads -- every use of it is an inlined
    /// `value` send of it or a move -- whose frame words hold `nil` until
    /// an exit that finds it live makes the closure and writes it into
    /// every register the interrupt point says holds it
    /// (`jit::materialize`). Emitted as `nil`.
    EliminatedBlock { value: ValueId, block: EliminatedId },
}

/// What an inlined indexed access reads or writes (`Node::IndexedLoad`,
/// `Node::IndexedStore`): an `Array`'s elements, or a `ByteArray`'s bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IndexedKind {
    Array,
    ByteArray,
}

/// Where a `Node::CurrentClass` starts its steps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClassStart {
    /// The class of the object `from`: a public or a lexical send's
    /// receiver.
    ClassOf,
    /// The class `from` is: the sender's current class, at which a
    /// private lexical send's lookup starts and one step below which a
    /// `super` send's does.
    Class,
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
    /// The send at `program_counter` of `scope`, its call site `site`,
    /// lowered with its receivers' callees inlined: `receiver`'s guard
    /// tested against each arm's in the cache's order, each arm's block the
    /// entry of its callee's scope; for any other receiver `fall_through`,
    /// the block holding the send itself, or, for `None`, a trap that runs
    /// the send again in the interpreter.
    TypeCase {
        program_counter: u32,
        scope: ScopeId,
        receiver: ValueId,
        arms: Vec<(DispatchId, BlockId)>,
        fall_through: Option<BlockId>,
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
            | Node::OwnSlotLoad { value, .. }
            | Node::CurrentClass { value, .. }
            | Node::FieldLoad { value, .. }
            | Node::IdentityCompare { value, .. }
            | Node::ClassOf { value, .. }
            | Node::IndexedLoad { value, .. }
            | Node::EliminatedBlock { value, .. } => Some(*value),
            Node::Copy { .. }
            | Node::Frame { .. }
            | Node::Overlay { .. }
            | Node::FieldStore { .. }
            | Node::IndexedStore { .. }
            | Node::TrapIfNil { .. } => None,
        }
    }

    /// What the value the node defines is known to be, by the node alone:
    /// a byte read is a `SmallInteger`, and an identity a Boolean.
    pub(crate) fn kind(&self) -> ValueKind {
        match self {
            Node::Constant { word, .. } => ValueKind::of_word(*word),
            Node::IntegerBinary { operation, .. } => ValueKind::of_operation(*operation),
            Node::IdentityCompare { .. } => ValueKind::Boolean,
            Node::IndexedLoad {
                kind: IndexedKind::ByteArray,
                ..
            } => ValueKind::SmallInteger,
            Node::FrameRead { .. }
            | Node::Receiver { .. }
            | Node::Copy { .. }
            | Node::OwnSlotLoad { .. }
            | Node::Frame { .. }
            | Node::CurrentClass { .. }
            | Node::Overlay { .. }
            | Node::FieldLoad { .. }
            | Node::FieldStore { .. }
            | Node::ClassOf { .. }
            | Node::IndexedLoad {
                kind: IndexedKind::Array,
                ..
            }
            | Node::IndexedStore { .. }
            | Node::TrapIfNil { .. }
            | Node::EliminatedBlock { .. } => ValueKind::Unknown,
        }
    }

    /// The program counter of the trap the node takes when its fast path
    /// fails, if it takes one: an integer fast path with no fallback, and
    /// every check of a callee always inlined, each of which runs the send
    /// again.
    pub(crate) fn trap(&self) -> Option<u32> {
        match self {
            Node::IntegerBinary {
                program_counter,
                fallback: None,
                ..
            }
            | Node::IdentityCompare {
                program_counter, ..
            }
            | Node::IndexedLoad {
                program_counter, ..
            }
            | Node::IndexedStore {
                program_counter, ..
            }
            | Node::TrapIfNil {
                program_counter, ..
            } => Some(*program_counter),
            _ => None,
        }
    }

    /// Whether the node is a simple node whose code calls a Rust leaf,
    /// across which only the registers `Roles::survive_leaf_calls` counts
    /// keep their values: a store into an object, whose inline barrier
    /// calls the remembering leaf for an old object it stores a young
    /// one into. A frame node is an interrupt point, not a leaf call,
    /// after which no value holds.
    pub(crate) fn calls_leaf(&self) -> bool {
        match self {
            Node::FieldStore { .. }
            | Node::IndexedStore {
                kind: IndexedKind::Array,
                ..
            } => true,
            Node::FrameRead { .. }
            | Node::Constant { .. }
            | Node::Receiver { .. }
            | Node::Copy { .. }
            | Node::IntegerBinary { .. }
            | Node::OwnSlotLoad { .. }
            | Node::Frame { .. }
            | Node::CurrentClass { .. }
            | Node::Overlay { .. }
            | Node::FieldLoad { .. }
            | Node::IdentityCompare { .. }
            | Node::ClassOf { .. }
            | Node::IndexedLoad { .. }
            | Node::IndexedStore {
                kind: IndexedKind::ByteArray,
                ..
            }
            | Node::TrapIfNil { .. }
            | Node::EliminatedBlock { .. } => false,
        }
    }

    /// The values the node reads, in operand order.
    pub(crate) fn uses(&self) -> Vec<ValueId> {
        match self {
            Node::FrameRead { .. }
            | Node::Constant { .. }
            | Node::Receiver { .. }
            | Node::Overlay { .. }
            | Node::EliminatedBlock { .. } => Vec::new(),
            Node::Copy { source, .. }
            | Node::CurrentClass { from: source, .. }
            | Node::FieldLoad { object: source, .. }
            | Node::ClassOf { object: source, .. }
            | Node::TrapIfNil { value: source, .. } => vec![*source],
            Node::OwnSlotLoad { receiver, .. } => vec![*receiver],
            Node::IntegerBinary { left, right, .. } | Node::IdentityCompare { left, right, .. } => {
                vec![*left, *right]
            }
            Node::FieldStore { object, source, .. } => vec![*object, *source],
            Node::IndexedLoad { object, index, .. } => vec![*object, *index],
            Node::IndexedStore {
                object,
                index,
                source,
                ..
            } => vec![*object, *index, *source],
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
            End::TypeCase {
                arms, fall_through, ..
            } => arms
                .iter()
                .map(|(_, block)| *block)
                .chain(*fall_through)
                .collect(),
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
            End::TypeCase { receiver, .. } => Some(*receiver),
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
        let mut graph = Graph {
            blocks,
            values,
            scopes: Vec::new(),
            arm_applications: Vec::new(),
        };
        graph.find_phi_kinds();
        graph
    }

    /// `from_blocks`, its blocks belonging to `scopes`.
    pub(crate) fn with_scopes(blocks: Vec<Block>, scopes: Vec<GraphScope>) -> Graph {
        let mut graph = Graph::from_blocks(blocks);
        graph.scopes = scopes;
        graph
    }

    /// The scope `id` names.
    pub(crate) fn scope(&self, id: ScopeId) -> &GraphScope {
        &self.scopes[id.0 as usize]
    }

    /// The physical registers the leftover analysis taints before the
    /// instruction at `program_counter` of `scope` (`tainted_at`); none
    /// for a graph that knows no scope.
    pub(crate) fn tainted_at(&self, scope: ScopeId, program_counter: u32) -> Vec<Range<u16>> {
        if self.scopes.is_empty() {
            return Vec::new();
        }
        tainted_at(&self.scopes, scope, program_counter)
    }

    /// The physical registers the leftover analysis taints at the top of
    /// `block`, at its `taint_point`.
    pub(crate) fn tainted_at_top(&self, block: BlockId) -> Vec<Range<u16>> {
        if self.scopes.is_empty() {
            return Vec::new();
        }
        let (scope, program_counter) = self.block(block).taint_point;
        tainted_at(&self.scopes, scope, program_counter)
    }

    /// `scope` and every scope it is inlined into, innermost first.
    #[cfg(test)]
    pub(crate) fn chain(&self, scope: ScopeId) -> Vec<ScopeId> {
        let mut chain = vec![scope];
        let mut next = self.scope(scope).parent;
        while let Some((parent, _)) = next {
            chain.push(parent);
            next = self.scope(parent).parent;
        }
        chain
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
            End::TypeCase {
                program_counter,
                fall_through: None,
                ..
            } => Some(program_counter),
            _ => None,
        }
    }

    pub(crate) fn block(&self, block: BlockId) -> &Block {
        &self.blocks[block.index()]
    }

    /// The word of `value` when a constant defines it.
    pub(crate) fn constant_word(&self, value: ValueId) -> Option<u64> {
        let ValueDefinition { block, place, .. } = self.values[value.index()];
        match place {
            Place::Node(index) => match self.block(block).nodes[index] {
                Node::Constant { word, .. } => Some(word),
                _ => None,
            },
            Place::Phi(_) => None,
        }
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

/// The physical registers the leftover analysis taints before the
/// instruction at `program_counter` of `scope`, one of `scopes`: the
/// scope's own there, and every register past the highest it names,
/// where no frame lies -- what a callee inlined there left, past the
/// sender's own; for each scope it is inlined
/// into, that scope's own at the send it is inlined at, among the
/// registers below the slots the send lays it on, the rest being the
/// callee's; and the header words the exit writes of every inlined scope
/// of the chain, which no instruction reads.
pub(crate) fn tainted_at(
    scopes: &[GraphScope],
    scope: ScopeId,
    program_counter: u32,
) -> Vec<Range<u16>> {
    let shifted = |owner: &GraphScope, program_counter: u32, limit: u16| {
        owner
            .tainted_before
            .get(program_counter as usize)
            .into_iter()
            .flatten()
            .map(move |range| {
                range.start.saturating_add(owner.offset)
                    ..range.end.saturating_add(owner.offset).min(limit)
            })
            .filter(|range| !range.is_empty())
            .collect::<Vec<_>>()
    };
    let innermost = &scopes[scope.0 as usize];
    let mut tainted = shifted(innermost, program_counter, u16::MAX);
    tainted.push(innermost.top()..u16::MAX);
    tainted.push(innermost.exit_words());
    let mut child = innermost;
    while let Some((parent, call)) = child.parent {
        let owner = &scopes[parent.0 as usize];
        tainted.extend(shifted(owner, call, child.offset));
        tainted.push(owner.exit_words());
        child = owner;
    }
    tainted.retain(|range| !range.is_empty());
    tainted
}
