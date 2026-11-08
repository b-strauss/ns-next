//! Optimized code from the graph and its allocation
//! (`docs/design/execution.md`, "One tier, an SSA graph, the macro
//! assembler"): the baseline's prologue, then each block in the
//! allocation's order -- its moves, its simple nodes on the registers the
//! allocation chose, and each frame node, after the write-back of every
//! value its frame word does not hold yet, by the very template function
//! the baseline's arm for its instruction calls (`baseline::lower`). So at
//! every interrupt point the frame is the interpreter's word for word, and
//! a frame that leaves optimized code there goes on in baseline code or
//! the interpreter at its program counter.
//!
//! An inlined scope lies in the physical frame where the interpreter would
//! push its activation (`docs/design/execution.md`, "Inlining"): around
//! its frame nodes the frame register moves to the scope's register 0
//! (`MacroAssembler::shift_frame`), and the template runs there with the
//! scope's facts (`BodyAssembler::enter_scope`), so that it is the
//! template the scope's own baseline code runs. Every simple node, move
//! and write-back addresses the physical frame wherever the frame register
//! stands; every block is entered with it at the physical register 0, and
//! every way out of optimized code -- an exit, a trap, a poll's -- leaves
//! it at the register 0 of the innermost scope the interrupt point stops
//! in, whose frame the exit's walk and the leaves read. Every address
//! inside an inlined scope's code that a leaf or an exit is handed, and
//! every return address of its calls, is recorded with its interrupt
//! point (`MacroAssembler::set_site`), so that the walk finds the chain of
//! scopes it stands in (`jit::OptimizedMetadata`).
//!
//! A guard that fails traps (`MacroAssembler::trap`), running its
//! instruction again in the interpreter: its write-back, as a poll's,
//! lies on its way out alone, laid out after the body (`emit_traps`), so
//! that a guard that holds stores nothing; a send never executed traps
//! where it would be made. A value of a known kind (`ValueKind`) is tested
//! for nothing it is known to be.
//!
//! Between two frame nodes nothing is written into a register a value is
//! held in but that value's own: a simple node computes in the registers
//! no value is allocated to (`Roles::integer_temporaries`) and takes each
//! through `MacroAssembler::temporary`, which a debug build checks against
//! the registers `hold_values` names.

use std::collections::HashMap;

use ns_assembler::{Condition, Label, Memory, Register, Width};

use super::allocate::{Allocation, DirtyPoint, Location, Move, PointKind, Spot};
use super::graph::{
    BlockId, ClassStart, End, Graph, IndexedKind, Node, ScopeId, ValueId, ValueKind,
};
use super::liveness::poll_target;
use super::metadata::Metadata;
use super::snapshot::{Snapshot, SnapshotInstruction};
use crate::heap::{CodeId, Heap, Immediate};
use crate::interpreter::inline_cache::FeedbackBase;
use crate::jit::baseline::{lower, BodyAssembler, Lowering, ScopeFacts, TwinTransfer};
use crate::jit::macro_assembler::KnownSmallIntegers;
use crate::jit::stubs::ExitKind;
use crate::jit::{closures, sends, Refusal};

/// A body's optimized code, as `optimizing::compile` answers it for the
/// JIT to install beside the body's baseline code.
pub(crate) struct OptimizedBody {
    pub(crate) code: ns_assembler::Assembled,
    /// Each send's call's return address, a label bound right after the
    /// call, and the send's program counter in its own scope, as
    /// `call_send` records them.
    pub(crate) returns: Vec<(Label, u32)>,
    /// The offsets in the code of its first data word and of the byte past
    /// its last, if it has any (`baseline::BaselineBody::words`).
    pub(crate) words: Option<std::ops::Range<u32>>,
    /// The transfers into the baseline twin, one per `MakeClosure` of the
    /// body's own scope (`closures::transfer_to_twin`).
    pub(crate) twin_transfers: Vec<TwinTransfer>,
    /// Each interrupt point's description: the install keeps those of the
    /// points inside an inlined scope, whose scopes the exit's walk writes
    /// (`jit::materialize`).
    pub(crate) metadata: Metadata,
    /// Every address inside an inlined scope's code that a leaf or an exit
    /// is handed, and every return address of a call made there: its
    /// offset in the code and the index of its interrupt point in
    /// `metadata`.
    pub(crate) sites: Vec<(u32, u32)>,
    /// Each scope's body and the feedback base its sends' cells are found
    /// from, by its `ScopeId`, the body's own first; `None` for a scope
    /// the plan made that the graph lays out no code for, which is no
    /// scope of the copy's (`optimizing::laid_out_scopes`).
    pub(crate) scopes: Vec<Option<LaidOutScope>>,
    /// Every block the compile eliminated, by its `EliminatedId`: what the
    /// exit that finds it live makes it from (`jit::materialize`).
    pub(crate) eliminated: Vec<EliminatedBlock>,
    /// The most scopes inlined into one another below the body's own, and
    /// one past the highest physical register any scope's frame reaches
    /// (`plan::InliningPlan`).
    pub(crate) deepest_chain: u16,
    pub(crate) deepest_extent: u32,
    /// What the code relied on beyond its own body, which the install
    /// records as the copy's dependencies (`jit::CopyDependencies`).
    pub(crate) relied_on: ReliedOn,
    /// The offset in the code where the run loop's push enters, past the
    /// prologue a compiled call enters at: the body's first block.
    pub(crate) push_entry: u32,
    /// Each back edge's poll a frame goes back into the code at once the
    /// safepoint its exit stopped at is served.
    pub(crate) polls: Vec<PollEntry>,
    /// The `SmallInteger` tag tests the code holds, which the tests count.
    #[cfg(test)]
    pub(crate) small_integer_tag_tests: usize,
}

/// A back edge's poll a frame of the code goes back in at once the
/// safepoint its exit stopped at is served (`jit::resumed_at_poll`,
/// `docs/design/execution.md`, "Between the tiers"): the offsets in the
/// code of the address its exit hands over, which names the poll, and of
/// its entry, and the index of its interrupt point. The entry restores
/// every value live there from the frame's words a collection at the
/// safepoint kept current (`Emitter::poll_restores`), the frame register
/// at the physical frame's register 0, as the poll stood, and goes on
/// past the poll's test, so that a request standing still costs a turn
/// of the loop, never a turn without one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PollEntry {
    pub(crate) exit: u32,
    pub(crate) entry: u32,
    pub(crate) point: u32,
    /// Whether the entry writes a value back into a frame word the
    /// collector's pre-pass may have nilled, which the tests read.
    #[cfg(test)]
    pub(crate) rewrites_lost_words: bool,
}

/// A `PollEntry` as the emitter lays it out, before the code's offsets are
/// known.
pub(crate) struct EmittedPoll {
    pub(crate) exit: Label,
    pub(crate) entry: Label,
    pub(crate) point: u32,
    #[cfg(test)]
    pub(crate) rewrites_lost_words: bool,
}

/// A scope the copy laid out code for: its body, the feedback base its
/// sends' cells are found from, and for a block's scope the block the
/// compile eliminated whose `value` it runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LaidOutScope {
    pub(crate) code_id: CodeId,
    pub(crate) feedback_base: FeedbackBase,
    pub(crate) block: Option<super::plan::EliminatedId>,
}

/// A block the compile eliminated, as the exit makes it: its body, the
/// scope whose `MakeClosure` it is -- the frame it captures -- and the
/// registers of that scope its values are copied from, which the plan
/// keeps unwritten while it lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EliminatedBlock {
    pub(crate) code_id: CodeId,
    pub(crate) home: ScopeId,
    pub(crate) copied: Vec<u16>,
}

/// What optimized code relied on beyond its own body and its key: what an
/// edit, a full collection or a slide of the site ranges can change under
/// it (`docs/design/execution.md`, "Invalidation").
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct ReliedOn {
    /// The guards its type-cases test their arms by, each once.
    pub(crate) guards: Vec<crate::mixin::DispatchId>,
    /// The applications its inlined scopes were compiled for, each once:
    /// each scope's receiver's and the one it is entered at, whose lookup
    /// found the callee, whose layout its slot reads and whose outer
    /// structure its lexical sends walk; and the one each arm always
    /// inlined was decided for, whose layout an accessor's field is.
    pub(crate) applications: Vec<crate::heap::ApplicationIndex>,
    /// Each inlined body -- a method's or a block's -- and the feedback
    /// base its sends' cells are found from, in the order of its scopes,
    /// then each eliminated block's body, whose closure an exit makes,
    /// each once.
    pub(crate) bodies: Vec<(CodeId, FeedbackBase)>,
}

/// One scope of the graph as the emitter lowers it: the snapshot of its
/// body, whose sites name its traps', the facts its templates read, and
/// the physical register its register 0 lies at.
pub(crate) struct EmittedScope<'tree> {
    pub(crate) snapshot: &'tree Snapshot,
    pub(crate) facts: ScopeFacts,
    pub(crate) offset: u16,
}

/// Emits the optimized code of the graph `graph` over `scopes` -- the
/// body's own first -- allocated as `allocation` says and described by
/// `metadata`, into `body`, which `BodyAssembler::for_optimized` made;
/// answers the label the run loop's push enters at, past the prologue,
/// and each poll a frame goes back in at.
pub(in crate::jit) fn emit(
    scopes: &[EmittedScope<'_>],
    graph: &Graph,
    allocation: &Allocation,
    metadata: &Metadata,
    body: &mut BodyAssembler,
) -> Result<(Label, Vec<EmittedPoll>), Refusal> {
    let root = scopes
        .first()
        .expect("internal: a compile has the body's own scope");
    sends::prologue(body, &root.snapshot.facts);
    let mut emitter = Emitter {
        graph,
        allocation,
        metadata,
        scopes,
        labels: graph
            .blocks
            .iter()
            .map(|_| body.macro_assembler.assembler().label())
            .collect(),
        points: allocation
            .dirty_at
            .iter()
            .enumerate()
            .map(|(index, point)| (point.position, (index, point)))
            .collect(),
        polls: Vec::new(),
        poll_entries: Vec::new(),
        traps: Vec::new(),
        trampolines: Vec::new(),
    };
    let order = &allocation.positions.order;
    let first = *order.first().expect("internal: a graph has an entry block");
    // The push enters here, past the prologue, which falls through, and
    // before the first block, whose own label a back edge may jump to with
    // values in registers: the entry's countdown, if any, sees the frame
    // as the interpreter left it.
    let push_entry = body.macro_assembler.assembler().label();
    body.macro_assembler.assembler().bind(push_entry);
    let first_program_counter = graph.block(first).program_counter;
    body.program_counter = first_program_counter;
    body.macro_assembler
        .set_program_counter(first_program_counter);
    body.macro_assembler
        .count_down_at_entry(first_program_counter);
    for (index, block) in order.iter().enumerate() {
        emitter.block(body, *block, order.get(index + 1).copied());
    }
    emitter.emit_polls(body);
    emitter.emit_traps(body);
    emitter.emit_trampolines(body);
    body.macro_assembler.set_site(None);
    body.macro_assembler.hold_values(&[]);
    Ok((push_entry, emitter.poll_entries))
}

/// A back edge's poll, out of line: the label its test jumps to, the
/// stores it owes the frame there, its interrupt point, whose innermost
/// scope it exits in, at the loop head's program counter, its position,
/// and the label past its test, where a frame that goes back in at the
/// poll goes on.
struct Poll {
    label: Label,
    stores: Vec<(u16, Register)>,
    point: usize,
    position: u32,
    resume: Label,
}

/// A guard's trap, out of line: the label its failed test jumps to, the
/// shift of the frame register there, the stores it owes the frame, its
/// interrupt point, the instruction it runs again and the site its
/// speculation rested on.
struct Trap {
    label: Label,
    shift: i32,
    stores: Vec<(u16, Register)>,
    point: usize,
    program_counter: u32,
    site: u32,
}

struct Emitter<'graph> {
    graph: &'graph Graph,
    allocation: &'graph Allocation,
    metadata: &'graph Metadata,
    scopes: &'graph [EmittedScope<'graph>],
    /// Each block's label, by its index.
    labels: Vec<Label>,
    /// Each interrupt point's index and write-back, by its position.
    points: HashMap<u32, (usize, &'graph DirtyPoint)>,
    polls: Vec<Poll>,
    /// Each poll's exit and entry, once laid out (`emit_polls`).
    poll_entries: Vec<EmittedPoll>,
    traps: Vec<Trap>,
    /// The ways into a block from a template's jump made with the frame
    /// register at an inlined scope's register 0: the label the template
    /// jumps to, the shift there, and the block, entered with none.
    trampolines: Vec<(Label, i32, Label)>,
}

impl Emitter<'_> {
    /// The machine register `value` is in at `position`, which the
    /// allocation placed it in because the code there reads it there.
    fn register_of(&self, value: ValueId, position: u32) -> Register {
        match self.allocation.location_of(value, position) {
            Location::Register(register) => register,
            Location::FrameWord | Location::EliminatedBlock(_) => {
                panic!("internal: {value:?} is read in a register at {position}")
            }
        }
    }

    /// Tells a debug build which registers hold values where the code
    /// emitted next runs, at `position`.
    fn hold(&self, body: &mut BodyAssembler, position: u32) {
        #[cfg(debug_assertions)]
        body.macro_assembler
            .hold_values(&self.allocation.registers_held_at(position));
        #[cfg(not(debug_assertions))]
        let _ = (body, position);
    }

    /// Tells a debug build, before a store's barrier, whose remembering
    /// leaf keeps only the survivors, that only the values living past the
    /// node at `use_position` are held: the allocation moved those into
    /// the survivors, and the node's operands dying there are read no more.
    fn hold_past_leaf_call(&self, body: &mut BodyAssembler, use_position: u32) {
        self.hold(body, use_position + 1);
    }

    /// The address of the element the one-based index in `index` names of
    /// the `Array` or `ByteArray` `object` names, as the row's stub finds
    /// it (`MacroAssembler::load_element_address`, `load_byte_address`),
    /// in the idioms' temporaries: a jump to `failed` for every receiver,
    /// index and bound the row's leaf decides otherwise. `object` and
    /// `index` are kept.
    fn indexed_address(
        body: &mut BodyAssembler,
        kind: IndexedKind,
        object: Register,
        index: Register,
        failed: Label,
    ) -> Register {
        let macro_assembler = &mut body.macro_assembler;
        let temporaries = macro_assembler.roles().integer_temporaries();
        let (index_copy, address) = (
            macro_assembler.temporary(temporaries.right),
            macro_assembler.temporary(temporaries.remainder),
        );
        macro_assembler.move_register(index_copy, index);
        match kind {
            IndexedKind::Array => {
                macro_assembler.load_element_address(object, index_copy, address, failed);
            }
            IndexedKind::ByteArray => {
                let size = macro_assembler.temporary(temporaries.left);
                macro_assembler.load_byte_address(object, index_copy, address, size, failed);
            }
        }
        address
    }

    /// The scope `scope` names.
    fn scope(&self, scope: ScopeId) -> &EmittedScope<'_> {
        &self.scopes[scope.0 as usize]
    }

    /// The site the code of the interrupt point `point` records its
    /// addresses with: its index, for a point inside an inlined scope or
    /// where a block the compile eliminated is live, whose closure the exit
    /// makes; none for any other point of the body's own scope, whose
    /// addresses name the body.
    fn site_of(&self, point: usize) -> Option<u32> {
        let described = &self.metadata.points[point];
        (described.scopes.len() > 1 || described.holds_eliminated_blocks()).then(|| {
            u32::try_from(point).expect("internal: a body's points are numbered in 32 bits")
        })
    }

    /// Moves the frame register to the physical register `offset`.
    fn shift_to(body: &mut BodyAssembler, offset: u16) {
        let shift = i32::from(offset) - body.macro_assembler.frame_shift();
        body.macro_assembler.shift_frame(shift);
    }

    fn block(&mut self, body: &mut BodyAssembler, block_id: BlockId, next: Option<BlockId>) {
        let graph = self.graph;
        let positions = &self.allocation.positions;
        let block = graph.block(block_id);
        body.macro_assembler
            .assembler()
            .bind(self.labels[block_id.index()]);
        // Every way into a block brings the frame register at the physical
        // register 0.
        body.macro_assembler.assume_frame_shift(0);
        // A debug build's refusal of a dead register names the instruction
        // being run: the block's first, then each frame node's.
        body.program_counter = block.program_counter;
        body.macro_assembler
            .set_program_counter(block.program_counter);
        self.moves(body, positions.entry(block_id));
        self.moves(body, positions.phi_definition(block_id));
        for (index, node) in block.nodes.iter().enumerate() {
            let use_position = positions.node_use(block_id, index);
            self.hold(body, use_position);
            self.moves(body, use_position);
            self.node(body, block_id, index, node, use_position);
            self.moves(body, use_position + 1);
        }
        let end_use = positions.end_use(block_id);
        self.hold(body, end_use);
        self.moves(body, end_use);
        self.end(body, block_id, next, end_use);
    }

    /// The moves the allocation makes at `position`, in order.
    fn moves(&self, body: &mut BodyAssembler, position: u32) {
        let macro_assembler = &mut body.macro_assembler;
        let scratch = macro_assembler.roles().idiom_scratch;
        for each in self.allocation.moves_at(position) {
            match *each {
                Move::Store { from, register, .. } => {
                    macro_assembler.store_physical_register(register, from);
                }
                // The frame word was written by this code, or is one the
                // interpreter reads there: never a dead register's, which
                // a debug build's check would refuse.
                Move::Load { register, into, .. } => {
                    macro_assembler.load_physical_register(into, register);
                }
                Move::Transfer { from, into, .. } => {
                    let spot = |spot: Spot| match spot {
                        Spot::Machine(register) => register,
                        Spot::Scratch => scratch,
                    };
                    let target = match into {
                        Spot::Machine(register) => register,
                        Spot::Scratch => macro_assembler.temporary(scratch),
                    };
                    macro_assembler.move_register(target, spot(from));
                }
            }
        }
    }

    fn node(
        &mut self,
        body: &mut BodyAssembler,
        block_id: BlockId,
        index: usize,
        node: &Node,
        use_position: u32,
    ) {
        let definition = use_position + 1;
        let defined_in = |value: ValueId| match self.allocation.location_of(value, definition) {
            Location::Register(register) => Some(register),
            Location::FrameWord | Location::EliminatedBlock(_) => None,
        };
        match node {
            // A read only a frame node takes is not loaded: the frame
            // word holds it.
            Node::FrameRead { register, value } => {
                if let Some(target) = defined_in(*value) {
                    body.macro_assembler
                        .load_physical_register(target, *register);
                }
            }
            Node::Constant { value, word } => {
                if let Some(target) = defined_in(*value) {
                    body.macro_assembler.load_immediate_word(target, *word);
                }
            }
            // The block is made by the exit that finds it live; until then
            // every frame word bound to it holds `nil`, a word the
            // collector may trace.
            Node::EliminatedBlock { value, .. } => {
                if let Some(target) = defined_in(*value) {
                    body.macro_assembler
                        .load_immediate_word(target, Immediate::NIL.bits());
                }
            }
            // The body's own receiver: an inlined scope's is its header
            // binding's.
            Node::Receiver { value } => {
                if let Some(target) = defined_in(*value) {
                    let receiver = Heap::receiver_register(self.scopes[0].facts.argument_count);
                    body.macro_assembler.load_physical_header(target, receiver);
                }
            }
            // A binding, which the write-backs store.
            Node::Copy { .. } => {}
            Node::OwnSlotLoad {
                value,
                receiver,
                absolute_index,
            } => {
                let receiver = self.register_of(*receiver, use_position);
                if let Some(target) = defined_in(*value) {
                    body.macro_assembler
                        .load_field(target, receiver, *absolute_index);
                }
            }
            Node::IntegerBinary {
                operation,
                program_counter,
                value,
                left,
                right,
                fallback,
                ..
            } => {
                // A known `SmallInteger` operand is not tested again.
                let known = KnownSmallIntegers {
                    left: self.graph.kind(*left) == ValueKind::SmallInteger,
                    right: self.graph.kind(*right) == ValueKind::SmallInteger,
                };
                let (left, right) = (
                    self.register_of(*left, use_position),
                    self.register_of(*right, use_position),
                );
                // The fast path's failure goes to its fallback, entered
                // with the frame register at the physical register 0, or
                // else to its trap, whose write-back lies on that way
                // alone.
                let fallback = match fallback {
                    Some(fallback) => {
                        Self::shift_to(body, 0);
                        self.labels[fallback.index()]
                    }
                    None => {
                        let scope = self.graph.block(block_id).scope;
                        self.trap_out_of_line(body, use_position, *program_counter, scope, None)
                    }
                };
                let macro_assembler = &mut body.macro_assembler;
                let temporaries = macro_assembler.roles().integer_temporaries();
                for register in [
                    temporaries.left,
                    temporaries.right,
                    temporaries.remainder,
                    temporaries.first_spare,
                    temporaries.second_spare,
                ] {
                    macro_assembler.temporary(register);
                }
                // The operands stay where they are, which the fallback's
                // block is entered with.
                macro_assembler.move_register(temporaries.left, left);
                macro_assembler.move_register(temporaries.right, right);
                let result = macro_assembler.small_integer_operation(
                    *operation,
                    temporaries,
                    known,
                    &mut |macro_assembler, condition| {
                        macro_assembler.assembler().jump_if(condition, fallback);
                    },
                );
                if let Some(target) = defined_in(*value) {
                    macro_assembler.move_register(target, result);
                }
            }
            // Its registers are the callee's: what the sender held there
            // is never stored.
            Node::Overlay { .. } => {}
            Node::CurrentClass {
                value,
                from,
                start,
                steps,
            } => {
                let from = self.register_of(*from, use_position);
                self.current_class(body, block_id, defined_in(*value), from, *start, *steps);
            }
            Node::FieldLoad {
                value,
                object,
                field,
            } => {
                let object = self.register_of(*object, use_position);
                if let Some(target) = defined_in(*value) {
                    body.macro_assembler.load_field(target, object, *field);
                }
            }
            Node::FieldStore {
                object,
                field,
                source,
            } => {
                let (object, source) = (
                    self.register_of(*object, use_position),
                    self.register_of(*source, use_position),
                );
                let macro_assembler = &mut body.macro_assembler;
                let temporaries = macro_assembler.roles().integer_temporaries();
                let (holder, scratch) = (
                    macro_assembler.temporary(temporaries.left),
                    macro_assembler.temporary(temporaries.right),
                );
                macro_assembler.move_register(holder, object);
                self.hold_past_leaf_call(body, use_position);
                body.macro_assembler
                    .store_field_with_barrier(holder, *field, source, scratch);
            }
            Node::IdentityCompare {
                value,
                left,
                right,
                program_counter,
            } => {
                let (left, right) = (
                    self.register_of(*left, use_position),
                    self.register_of(*right, use_position),
                );
                let scope = self.graph.block(block_id).scope;
                let undecided =
                    self.trap_out_of_line(body, use_position, *program_counter, scope, None);
                let macro_assembler = &mut body.macro_assembler;
                let temporaries = macro_assembler.roles().integer_temporaries();
                let scratch = (
                    macro_assembler.temporary(temporaries.left),
                    macro_assembler.temporary(temporaries.right),
                );
                let (identical, done) = (
                    macro_assembler.assembler().label(),
                    macro_assembler.assembler().label(),
                );
                // One value on both sides -- `x == x` -- is identical, and
                // its one register is no pair the checks can take.
                if left == right {
                    macro_assembler.assembler().jump(identical);
                } else {
                    macro_assembler.assembler().compare(left, right);
                    macro_assembler
                        .assembler()
                        .jump_if(Condition::Equal, identical);
                    // Every check runs whether or not the answer is read: a
                    // future the leaf declines for is forced by the method.
                    macro_assembler.jump_if_identity_undecided((left, right), scratch, undecided);
                }
                match defined_in(*value) {
                    Some(target) => {
                        macro_assembler
                            .load_immediate_word(target, Immediate::boolean(false).bits());
                        macro_assembler.assembler().jump(done);
                        macro_assembler.assembler().bind(identical);
                        macro_assembler
                            .load_immediate_word(target, Immediate::boolean(true).bits());
                    }
                    None => macro_assembler.assembler().bind(identical),
                }
                macro_assembler.assembler().bind(done);
            }
            Node::ClassOf { value, object } => {
                let object = self.register_of(*object, use_position);
                if let Some(target) = defined_in(*value) {
                    let macro_assembler = &mut body.macro_assembler;
                    let class = macro_assembler
                        .temporary(macro_assembler.roles().integer_temporaries().left);
                    macro_assembler.load_class(class, object);
                    macro_assembler.move_register(target, class);
                }
            }
            Node::IndexedLoad {
                kind,
                value,
                object,
                index,
                program_counter,
            } => {
                let (object, index) = (
                    self.register_of(*object, use_position),
                    self.register_of(*index, use_position),
                );
                let scope = self.graph.block(block_id).scope;
                let failed =
                    self.trap_out_of_line(body, use_position, *program_counter, scope, None);
                let element = Self::indexed_address(body, *kind, object, index, failed);
                let macro_assembler = &mut body.macro_assembler;
                let loaded =
                    macro_assembler.temporary(macro_assembler.roles().integer_temporaries().left);
                let layout = Heap::machine_layout();
                let assembler = macro_assembler.assembler();
                match kind {
                    IndexedKind::Array => assembler.load(Width::Word64, loaded, word_at(element)),
                    IndexedKind::ByteArray => {
                        assembler.load(Width::Byte, loaded, word_at(element));
                        assembler.shift_left(loaded, layout.tag_bits);
                        assembler.or_immediate(loaded, layout.small_integer_tag);
                    }
                }
                if let Some(target) = defined_in(*value) {
                    macro_assembler.move_register(target, loaded);
                }
            }
            Node::IndexedStore {
                kind,
                object,
                index,
                source,
                program_counter,
            } => {
                let (object, index, source) = (
                    self.register_of(*object, use_position),
                    self.register_of(*index, use_position),
                    self.register_of(*source, use_position),
                );
                let scope = self.graph.block(block_id).scope;
                let failed =
                    self.trap_out_of_line(body, use_position, *program_counter, scope, None);
                let element = Self::indexed_address(body, *kind, object, index, failed);
                let macro_assembler = &mut body.macro_assembler;
                let temporaries = macro_assembler.roles().integer_temporaries();
                let layout = Heap::machine_layout();
                match kind {
                    IndexedKind::Array => {
                        let (holder, scratch) = (
                            macro_assembler.temporary(temporaries.left),
                            macro_assembler.temporary(temporaries.right),
                        );
                        macro_assembler.move_register(holder, object);
                        self.hold_past_leaf_call(body, use_position);
                        body.macro_assembler.store_with_barrier_at(
                            holder,
                            word_at(element),
                            source,
                            scratch,
                        );
                    }
                    IndexedKind::ByteArray => {
                        // A `SmallInteger` from 0 to 255: tagged, and its
                        // word below 256's, unsigned, which a negative
                        // one's is not.
                        let byte = macro_assembler.temporary(temporaries.left);
                        let assembler = macro_assembler.assembler();
                        assembler.test_immediate(source, layout.small_integer_tag);
                        assembler.jump_if(Condition::Equal, failed);
                        assembler.compare_immediate(source, BYTE_LIMIT << layout.tag_bits);
                        assembler.jump_if(Condition::AboveOrEqual, failed);
                        assembler.move_register(byte, source);
                        assembler.shift_right_logical(byte, layout.tag_bits);
                        assembler.store(Width::Byte, word_at(element), byte);
                    }
                }
            }
            Node::TrapIfNil {
                value,
                program_counter,
            } => {
                let value = self.register_of(*value, use_position);
                let scope = self.graph.block(block_id).scope;
                let unfilled =
                    self.trap_out_of_line(body, use_position, *program_counter, scope, None);
                let assembler = body.macro_assembler.assembler();
                assembler.compare_immediate(value, Heap::machine_layout().nil_word);
                assembler.jump_if(Condition::Equal, unfilled);
            }
            Node::Frame {
                scope,
                program_counter,
                instruction,
                ..
            } => self.frame_node(
                body,
                block_id,
                index,
                *scope,
                *program_counter,
                instruction,
                use_position,
            ),
        }
    }

    /// An inlined scope's current class into `target`, as a compiled call
    /// computes its callee's (`sends::send_public`): `steps` superclass
    /// steps up from the class of the value in `from`, or from the class
    /// `from` holds; and, where the scope's code addresses its own slots,
    /// its recorded guard set to none, since no push wrote it: the first
    /// own-slot access the scope's templates make then takes the cold
    /// check, which records it as the push would, and no guard a frame
    /// laid there before left vouches for this scope's entered level.
    fn current_class(
        &self,
        body: &mut BodyAssembler,
        block_id: BlockId,
        target: Option<Register>,
        from: Register,
        start: ClassStart,
        steps: u16,
    ) {
        let scope = self.scope(self.graph.block(block_id).scope);
        if scope.snapshot.facts.addresses_own_slots {
            let (guard, _) = Heap::own_slot_registers(scope.facts.argument_count);
            let (none_word, _) = Heap::machine_own_slot_words();
            let none_word =
                i32::try_from(none_word).expect("internal: the guard none is a small word");
            body.macro_assembler
                .store_physical_immediate(scope.offset + guard, none_word);
        }
        let Some(target) = target else {
            return;
        };
        let macro_assembler = &mut body.macro_assembler;
        let temporaries = macro_assembler.roles().integer_temporaries();
        let (class, level) = (
            macro_assembler.temporary(temporaries.left),
            macro_assembler.temporary(temporaries.right),
        );
        match start {
            ClassStart::ClassOf => macro_assembler.load_class(class, from),
            ClassStart::Class => macro_assembler.move_register(class, from),
        }
        if steps > 0 {
            // The receiver's guard, or the start the plan knows, says the
            // chain has the level: a walk that fails is a defect.
            let assembler = macro_assembler.assembler();
            let (failed, walked) = (assembler.label(), assembler.label());
            assembler.move_immediate(level, u64::from(steps));
            macro_assembler.walk_superclasses(class, level, failed);
            let assembler = macro_assembler.assembler();
            assembler.jump(walked);
            assembler.bind(failed);
            assembler.trap();
            assembler.bind(walked);
        }
        macro_assembler.move_register(target, class);
    }

    /// A frame node: the write-back the allocation owes the frame there,
    /// then the instruction by the template the baseline's arm for it
    /// emits, with nothing held, the frame register at the register 0 of
    /// the node's scope and the scope's facts.
    #[expect(
        clippy::too_many_arguments,
        reason = "the node's fields, as the block's walk hands them over"
    )]
    fn frame_node(
        &mut self,
        body: &mut BodyAssembler,
        block_id: BlockId,
        index: usize,
        scope: ScopeId,
        program_counter: u32,
        instruction: &SnapshotInstruction,
        use_position: u32,
    ) {
        let (point_index, point) = self.points[&use_position];
        debug_assert_eq!(
            (point.block, point.kind),
            (block_id, PointKind::Frame { node: index }),
            "internal: a frame node's write-back is its own"
        );
        for (register, _, location) in &point.stores {
            let Location::Register(from) = location else {
                panic!("internal: a value not in its frame word is in a register");
            };
            body.macro_assembler
                .store_physical_register(*register, *from);
        }
        body.macro_assembler.hold_values(&[]);
        let emitted = self.scope(scope);
        let (offset, facts, inlined) = (emitted.offset, emitted.facts, scope != ScopeId::ROOT);
        Self::shift_to(body, offset);
        body.macro_assembler.set_site(self.site_of(point_index));
        let own = body.enter_scope(&facts);
        body.program_counter = program_counter;
        body.macro_assembler.set_program_counter(program_counter);
        // Under the deopt stress mode the point may hand its instruction
        // to the interpreter here, after the write-back and before the
        // template, exactly where the template's own exits leave.
        body.macro_assembler
            .count_down_to_deoptimize(program_counter);
        // A template's own jump goes where the graph says: a mirror send's
        // failure to the block its edge enters, and a jump to the next
        // instruction -- a fast path declining into the send that follows
        // it -- to the code right after the template, where its block goes
        // on either way.
        let block = self.graph.block(block_id);
        let after = body.macro_assembler.assembler().label();
        if let Some(target) = instruction.jump_target() {
            let label = match block.end {
                End::FrameJump {
                    target: entered, ..
                } if index + 1 == block.nodes.len() => {
                    let entered = self.labels[entered.index()];
                    // A block is entered with the frame register at the
                    // physical register 0.
                    if offset == 0 {
                        entered
                    } else {
                        let trampoline = body.macro_assembler.assembler().label();
                        self.trampolines
                            .push((trampoline, i32::from(offset), entered));
                        trampoline
                    }
                }
                // Only the frame node a `FrameJump` ends on jumps elsewhere:
                // a mirror send's failure that some later change left
                // before another node would go on after the template
                // instead, a wrong way taken at exit status 0.
                _ => {
                    debug_assert_eq!(
                        target,
                        program_counter + 1,
                        "internal: a template's jump the block does not end on goes to the next instruction"
                    );
                    after
                }
            };
            body.set_start(target, label);
        }
        // The one instruction whose snapshot form lacks what its arm reads,
        // the block's code: optimized code never reifies its frame, so its
        // arm transfers into the twin, as a lean body's does -- the body's
        // own frame's twin, which no inlined scope has: in one the
        // interpreter makes the closure, once the exit has materialized
        // the scopes. A transfer writes nothing of the frame but its
        // header, so where a block the compile eliminated is live, whose
        // registers hold `nil` for it, the exit makes it instead.
        let eliminated = self.metadata.points[point_index].holds_eliminated_blocks();
        let lowering = match instruction {
            SnapshotInstruction::MakeClosure { .. } if inlined || eliminated => Lowering::Exit,
            SnapshotInstruction::MakeClosure { .. } => closures::transfer_to_twin(body),
            other => lower(
                &other
                    .unsnapshot()
                    .expect("internal: only a closure lacks its instruction"),
                body,
            ),
        };
        body.finish_lowering(lowering);
        body.macro_assembler.assembler().bind(after);
        body.enter_scope(&own);
        body.macro_assembler.set_site(None);
    }

    fn end(
        &mut self,
        body: &mut BodyAssembler,
        block_id: BlockId,
        next: Option<BlockId>,
        end_use: u32,
    ) {
        let graph = self.graph;
        let end_definition = self.allocation.positions.end_definition(block_id);
        let jump_unless_next = |body: &mut BodyAssembler, labels: &[Label], target: BlockId| {
            if next != Some(target) {
                body.macro_assembler
                    .assembler()
                    .jump(labels[target.index()]);
            }
        };
        let block = graph.block(block_id);
        // Every block is entered with the frame register at the physical
        // register 0; a trap leaves at its scope's, and an end that never
        // goes on leaves it where its frame node did.
        if !matches!(block.end, End::Trap { .. } | End::Frame) {
            Self::shift_to(body, 0);
        }
        match &block.end {
            End::Jump(target) => {
                if let Some(head) = poll_target(graph, block_id) {
                    self.back_edge(body, block_id, head, end_use);
                }
                self.moves(body, end_definition);
                jump_unless_next(body, &self.labels, *target);
            }
            End::Branch {
                program_counter,
                condition,
                if_true,
                if_false,
                not_boolean,
            } => {
                debug_assert!(
                    self.allocation.moves_at(end_definition).is_empty(),
                    "internal: a branch's ways need no moves"
                );
                let known_boolean = graph.kind(*condition) == ValueKind::Boolean;
                let condition = self.register_of(*condition, end_use);
                // Where control goes when no test holds: the third way, the
                // way a known Boolean's one test leaves, or the trap.
                let (tests, otherwise, otherwise_block): (Vec<(bool, BlockId)>, Label, _) =
                    match not_boolean {
                        Some(not_boolean) => (
                            [(true, *if_true), (false, *if_false)]
                                .into_iter()
                                .filter(|(_, target)| target != not_boolean)
                                .collect(),
                            self.labels[not_boolean.index()],
                            Some(*not_boolean),
                        ),
                        None if known_boolean => (
                            if if_true == if_false {
                                Vec::new()
                            } else {
                                vec![(true, *if_true)]
                            },
                            self.labels[if_false.index()],
                            Some(*if_false),
                        ),
                        None => (
                            vec![(true, *if_true), (false, *if_false)],
                            self.trap_out_of_line(
                                body,
                                end_use,
                                *program_counter,
                                block.scope,
                                None,
                            ),
                            None,
                        ),
                    };
                let assembler = body.macro_assembler.assembler();
                for (position, (boolean, target)) in tests.iter().enumerate() {
                    let word = i32::try_from(Immediate::boolean(*boolean).bits())
                        .expect("internal: a Boolean's word is a small constant");
                    assembler.compare_immediate(condition, word);
                    // The last test falls into the next block on whichever
                    // way leads there.
                    if position + 1 == tests.len() && next == Some(*target) {
                        assembler.jump_if(Condition::NotEqual, otherwise);
                        return;
                    }
                    assembler.jump_if(Condition::Equal, self.labels[target.index()]);
                }
                if otherwise_block.is_none() || otherwise_block != next {
                    assembler.jump(otherwise);
                }
            }
            End::Trap {
                program_counter,
                site,
            } => {
                let (point, _) = self.points[&end_use];
                let stores = self.trap_stores(end_use);
                for (register, from) in stores {
                    body.macro_assembler.store_physical_register(register, from);
                }
                self.leave_at(body, point);
                body.macro_assembler.trap(*program_counter, *site);
                body.macro_assembler.set_site(None);
            }
            End::Return { value } => {
                let value = self.register_of(*value, end_use);
                let macro_assembler = &mut body.macro_assembler;
                let leaf_result = macro_assembler.temporary(macro_assembler.roles().leaf_result);
                macro_assembler.move_register(leaf_result, value);
                sends::return_loaded(body);
            }
            // The return is laid out before the way on, so that the moves
            // owed `next` are made on that way alone.
            End::ReturnUnlessNil {
                value,
                next: initializer,
            } => {
                let value = self.register_of(*value, end_use);
                let nil = crate::heap::Heap::machine_layout().nil_word;
                let macro_assembler = &mut body.macro_assembler;
                let unfilled = macro_assembler.assembler().label();
                macro_assembler.assembler().compare_immediate(value, nil);
                macro_assembler
                    .assembler()
                    .jump_if(Condition::Equal, unfilled);
                // No value lives on the way that returns.
                macro_assembler.hold_values(&[]);
                let leaf_result = macro_assembler.temporary(macro_assembler.roles().leaf_result);
                macro_assembler.move_register(leaf_result, value);
                sends::return_loaded(body);
                body.macro_assembler.assembler().bind(unfilled);
                self.hold(body, end_use);
                self.moves(body, end_definition);
                jump_unless_next(body, &self.labels, *initializer);
            }
            End::Frame => {}
            End::FrameJump {
                next: fall_through, ..
            } => {
                // The frame node it ends with holds no value past it, so no
                // way out owes a move: one set of moves here would serve
                // the template's jump and the fall-through alike.
                debug_assert!(
                    self.allocation.moves_at(end_definition).is_empty(),
                    "internal: a frame jump's ways out move nothing"
                );
                self.moves(body, end_definition);
                jump_unless_next(body, &self.labels, *fall_through);
            }
            End::TypeCase {
                program_counter,
                scope,
                receiver,
                arms,
                fall_through,
                site,
            } => {
                debug_assert!(
                    self.allocation.moves_at(end_definition).is_empty(),
                    "internal: a type-case's ways need no moves"
                );
                // The receiver's guard, as a cell's compare loads it,
                // tested against each arm's in the cache's order.
                let receiver = self.register_of(*receiver, end_use);
                let otherwise = match fall_through {
                    Some(send) => self.labels[send.index()],
                    None => {
                        self.trap_out_of_line(body, end_use, *program_counter, *scope, Some(*site))
                    }
                };
                let macro_assembler = &mut body.macro_assembler;
                let temporaries = macro_assembler.roles().integer_temporaries();
                let guard = macro_assembler.temporary(temporaries.left);
                let expected = macro_assembler.temporary(temporaries.right);
                macro_assembler.load_guard(guard, receiver);
                let assembler = macro_assembler.assembler();
                for (dispatch_id, arm) in arms {
                    match i32::try_from(dispatch_id.bits()) {
                        Ok(bits) => assembler.compare_immediate(guard, bits),
                        Err(_) => {
                            assembler.move_immediate(expected, u64::from(dispatch_id.bits()));
                            assembler.compare(guard, expected);
                        }
                    }
                    assembler.jump_if(Condition::Equal, self.labels[arm.index()]);
                }
                if fall_through.is_none() || *fall_through != next {
                    assembler.jump(otherwise);
                }
            }
        }
    }

    /// Moves the frame register to the register 0 of the innermost scope
    /// the interrupt point `point` stops in, and records the addresses of
    /// the exit that follows with the point: the frame the exit's walk
    /// materializes the scopes above.
    fn leave_at(&self, body: &mut BodyAssembler, point: usize) {
        let innermost = &self.metadata.points[point].scopes[0];
        Self::shift_to(body, innermost.offset);
        body.macro_assembler.set_site(self.site_of(point));
    }

    /// A back edge's count and poll, whose write-back lies on the way to
    /// its exit alone: a loop that finds no safepoint requested stores
    /// nothing.
    fn back_edge(
        &mut self,
        body: &mut BodyAssembler,
        block_id: BlockId,
        head: BlockId,
        end_use: u32,
    ) {
        let (point_index, point) = self.points[&end_use];
        debug_assert_eq!(
            (point.block, point.kind),
            (block_id, PointKind::Poll),
            "internal: a back edge's write-back is its poll's"
        );
        debug_assert!(
            self.metadata.points[point_index].scopes[0].program_counter
                == self.graph.block(head).taint_point.1,
            "internal: a poll exits at its loop head"
        );
        let stores = point
            .stores
            .iter()
            .map(|(register, _, location)| match location {
                Location::Register(from) => (*register, *from),
                Location::FrameWord | Location::EliminatedBlock(_) => {
                    panic!("internal: a value not in its frame word is in a register")
                }
            })
            .collect();
        let label = body.macro_assembler.assembler().label();
        body.macro_assembler.count_back_edge();
        // The deopt stress mode's exit at a back edge is the poll's own,
        // whose write-back lies on its way alone.
        body.macro_assembler.count_down_to(label);
        body.macro_assembler.poll_to(label);
        let resume = body.macro_assembler.assembler().label();
        body.macro_assembler.assembler().bind(resume);
        self.polls.push(Poll {
            label,
            stores,
            point: point_index,
            position: end_use,
            resume,
        });
    }

    /// The stores the trap at `position` owes the frame, each from the
    /// register its value is in.
    fn trap_stores(&self, position: u32) -> Vec<(u16, Register)> {
        let (_, point) = self.points[&position];
        debug_assert!(
            matches!(point.kind, PointKind::Trap { .. }),
            "internal: a trap's write-back is its own"
        );
        point
            .stores
            .iter()
            .map(|(register, _, location)| match location {
                Location::Register(from) => (*register, *from),
                Location::FrameWord | Location::EliminatedBlock(_) => {
                    panic!("internal: a value not in its frame word is in a register")
                }
            })
            .collect()
    }

    /// The label of the trap at `position`, which runs the instruction at
    /// `program_counter` of `scope` again, its write-back and exit laid
    /// out after the body (`emit_traps`), so that a guard that holds
    /// stores nothing. Its site is `site`, or the one the scope's own
    /// snapshot records at the instruction.
    fn trap_out_of_line(
        &mut self,
        body: &mut BodyAssembler,
        position: u32,
        program_counter: u32,
        scope: ScopeId,
        site: Option<u32>,
    ) -> Label {
        let label = body.macro_assembler.assembler().label();
        let site = site.unwrap_or_else(|| {
            let index = usize::try_from(program_counter).expect("internal: a program counter fits");
            self.scope(scope).snapshot.sites[index]
                .expect("internal: a trap rests on a site")
                .site
        });
        let (point, _) = self.points[&position];
        self.traps.push(Trap {
            label,
            shift: body.macro_assembler.frame_shift(),
            stores: self.trap_stores(position),
            point,
            program_counter,
            site,
        });
        label
    }

    /// Each guard's way to its trap: the write-back, then the trap, which
    /// runs the instruction again in the interpreter.
    fn emit_traps(&mut self, body: &mut BodyAssembler) {
        for trap in std::mem::take(&mut self.traps) {
            body.macro_assembler.assembler().bind(trap.label);
            body.macro_assembler.assume_frame_shift(trap.shift);
            for (register, from) in trap.stores {
                body.macro_assembler.store_physical_register(register, from);
            }
            self.leave_at(body, trap.point);
            body.macro_assembler.trap(trap.program_counter, trap.site);
            body.macro_assembler.set_site(None);
        }
    }

    /// Each back edge's way to its exit: the write-back, then the exit at
    /// the loop's head, where the interpreter takes the safepoint; and
    /// the poll's entry, where the frame goes back into the code once the
    /// safepoint is served (`PollEntry`), for a poll whose every live
    /// value it can restore (`poll_restores`).
    fn emit_polls(&mut self, body: &mut BodyAssembler) {
        for poll in std::mem::take(&mut self.polls) {
            body.macro_assembler.assembler().bind(poll.label);
            body.macro_assembler.assume_frame_shift(0);
            for (register, from) in poll.stores {
                body.macro_assembler.store_physical_register(register, from);
            }
            self.leave_at(body, poll.point);
            let program_counter = self.metadata.points[poll.point].scopes[0].program_counter;
            let exit = body
                .macro_assembler
                .exit_naming_its_address(ExitKind::Safepoint, program_counter);
            body.macro_assembler.set_site(None);
            // A test's way to the exit of a poll without an entry, which
            // only a value no restore rebuilds makes otherwise.
            #[cfg(test)]
            if body.macro_assembler.refuses_poll_entries() {
                continue;
            }
            let Some(restores) = self.poll_restores(poll.point, poll.position) else {
                continue;
            };
            let entry = body.macro_assembler.assembler().label();
            body.macro_assembler.assembler().bind(entry);
            // The run loop enters the physical frame at its register 0,
            // where the frame register stood at the poll.
            body.macro_assembler.assume_frame_shift(0);
            self.hold(body, poll.position);
            #[cfg(test)]
            let rewrites_lost_words = restores.iter().any(|restore| !restore.lost.is_empty());
            for restore in restores {
                Self::restore(body, restore);
            }
            body.macro_assembler.assembler().jump(poll.resume);
            self.poll_entries.push(EmittedPoll {
                exit,
                entry,
                point: u32::try_from(poll.point)
                    .expect("internal: a body's points are numbered in 32 bits"),
                #[cfg(test)]
                rewrites_lost_words,
            });
        }
    }

    /// What a frame going back into the code at the poll of the interrupt
    /// point `point`, at `position`, restores: every value live there in
    /// the machine register the code holds it in, and in every frame word
    /// the code after the poll reads it back from -- each from a frame
    /// word the frame kept, one of a register the leftover analysis leaves
    /// to the frame there, which the collector kept current meanwhile, or
    /// for a constant from its word, since the collector's pre-pass nils
    /// every other (`Heap::trace_stack`). A value the code past the poll
    /// never reads -- live only for the stores of its dirty bindings, as a
    /// load of a slot the loop then stores is -- is read back from a kept
    /// word one of them names, which the poll's write-back stored, so that
    /// the merge's stores write it again, or else is `nil`, written only
    /// into words nothing reads. `None` where a value the code reads on is
    /// in no such word and no constant: no frame goes back in there, and
    /// the poll's exit is any interrupt point's
    /// (`JitStatistics::polls_without_an_entry`).
    fn poll_restores(&self, point: usize, position: u32) -> Option<Vec<Restore>> {
        let kept: Vec<u16> = self.metadata.points[point]
            .scopes
            .iter()
            .flat_map(|scope| {
                scope
                    .locations
                    .iter()
                    .filter(|(_, location)| {
                        matches!(location, Location::FrameWord | Location::EliminatedBlock(_))
                    })
                    .map(move |(own, _)| scope.offset + own)
            })
            .collect();
        let mut restores = Vec::new();
        for polled in self.allocation.poll_values.get(&position)? {
            // A value read on: every clean binding the code may read it
            // back from; one read by nothing but the stores of its dirty
            // bindings: none, its machine register alone.
            let lost: Vec<u16> = if polled.read_on {
                polled
                    .clean
                    .iter()
                    .copied()
                    .filter(|register| !kept.contains(register))
                    .collect()
            } else {
                Vec::new()
            };
            if polled.machine.is_none() && lost.is_empty() {
                continue;
            }
            let source = match polled.bound.iter().find(|register| kept.contains(register)) {
                // A dirty binding the frame keeps, which the poll's
                // write-back stored, is a store the merge makes again.
                Some(register) => Source::Frame(*register),
                None => match self.graph.constant_word(polled.value) {
                    Some(word) => Source::Word(word),
                    None if !polled.read_on => Source::Word(Immediate::NIL.bits()),
                    None => return None,
                },
            };
            restores.push(Restore {
                source,
                machine: polled.machine,
                lost,
            });
        }
        Some(restores)
    }

    /// Emits `restore`: its value into its machine register, if it has
    /// one, then into each frame word lost.
    fn restore(body: &mut BodyAssembler, restore: Restore) {
        let macro_assembler = &mut body.macro_assembler;
        let held = match restore.machine {
            Some(register) => register,
            None => macro_assembler.temporary(macro_assembler.roles().integer_temporaries().left),
        };
        match restore.source {
            Source::Frame(register) => macro_assembler.load_physical_register(held, register),
            Source::Word(word) => macro_assembler.assembler().move_immediate(held, word),
        }
        for register in restore.lost {
            macro_assembler.store_physical_register(register, held);
        }
    }

    /// Each template's jump into a block made with the frame register at
    /// an inlined scope's register 0: the frame register moved back, then
    /// the block.
    fn emit_trampolines(&mut self, body: &mut BodyAssembler) {
        for (label, shift, block) in std::mem::take(&mut self.trampolines) {
            body.macro_assembler.assembler().bind(label);
            body.macro_assembler.assume_frame_shift(shift);
            body.macro_assembler.shift_frame(-shift);
            body.macro_assembler.assembler().jump(block);
        }
    }
}

/// Where a poll's entry reads a value back from (`Emitter::poll_restores`):
/// a frame word the frame kept, or a constant's word.
#[derive(Clone, Copy)]
enum Source {
    Frame(u16),
    Word(u64),
}

/// One value a poll's entry restores: from where, into the machine
/// register the code holds it in, if any, and into each frame word the
/// collector's pre-pass may have nilled that the code reads it back from.
struct Restore {
    source: Source,
    machine: Option<Register>,
    lost: Vec<u16>,
}

/// The word at `address`.
fn word_at(address: Register) -> Memory {
    Memory {
        base: address,
        displacement: 0,
    }
}

/// One past the largest byte, which `at:put:` stores into a `ByteArray`.
const BYTE_LIMIT: i32 = 256;

#[cfg(test)]
mod tests {
    use crate::bytecode::{Instruction, IntegerOperation, Literal};
    use crate::heap::{Immediate, SMALL_INTEGER_MAX, SMALL_INTEGER_MIN};
    use crate::interpreter::inline_cache::FeedbackBase;
    use crate::jit::baseline::BodyRole;
    use crate::jit::macro_assembler::{Abi, MacroAssembler};
    use crate::jit::optimizing::snapshot::{Customization, Snapshot, SnapshotInstruction};
    use crate::jit::optimizing::{
        compile, corpus_of_bodies, shipped_bodies, test_send, test_snapshot_with_arguments,
        unrun_every_site, unrun_site, Speculation,
    };
    use crate::jit::stubs::testing::Machine;
    use crate::jit::stubs::{BlockWord, MachineExit};

    fn small_integer(value: i64) -> u64 {
        Immediate::small_integer(value)
            .expect("a SmallInteger")
            .bits()
    }

    fn boolean(value: bool) -> u64 {
        Immediate::boolean(value).bits()
    }

    fn literal(destination: u16, value: i64) -> Instruction {
        Instruction::LoadLiteral {
            destination,
            literal: Literal::SmallInteger(value),
        }
    }

    /// `snapshot` compiled for the host, its polls exiting at every one
    /// when `exit_at_every_poll`, installed and run from the push's entry
    /// on `frame`.
    fn run(snapshot: &Snapshot, frame: &mut [u64], exit_at_every_poll: bool) -> MachineExit {
        let mut macro_assembler = MacroAssembler::new(Abi::host());
        macro_assembler.set_exit_at_every_poll(exit_at_every_poll);
        let optimized = match compile(
            snapshot,
            macro_assembler,
            BodyRole::Method,
            false,
            Speculation::Traps,
        ) {
            Ok(optimized) => optimized,
            Err(refusal) => panic!("refused: {refusal:?}"),
        };
        let mut machine = Machine::new();
        let start = machine.install(&optimized.code);
        let entry = ns_memory::CodeOffset(start.0 + optimized.push_entry);
        machine.run(entry, frame, 0, false, &mut (), &[]).0
    }

    /// `r13 := 0; loop: r13 := r13 + 1; r14 := r13 < 1000; BranchIfTrue
    /// r14 loop; Return r13`, each fast path's fallback an uncached send.
    /// Registers 13 on are a method's locals past its header.
    fn counting_loop() -> Snapshot {
        test_snapshot_with_arguments(
            vec![
                literal(13, 0),
                literal(15, 1),
                literal(16, 1000),
                Instruction::IntegerBinary {
                    operation: IntegerOperation::Add,
                    destination: 13,
                    left: 13,
                    right: 15,
                    target: 7,
                },
                Instruction::IntegerBinary {
                    operation: IntegerOperation::Less,
                    destination: 14,
                    left: 13,
                    right: 16,
                    target: 9,
                },
                Instruction::BranchIfTrue {
                    register: 14,
                    target: 3,
                },
                Instruction::Return { register: 13 },
                test_send(13, 15, 13),
                Instruction::Jump { target: 4 },
                test_send(13, 16, 14),
                Instruction::Jump { target: 5 },
            ],
            0,
        )
    }

    #[test]
    fn an_optimized_counting_loop_answers_the_count() {
        let mut frame = [0u64; 64];
        assert_eq!(
            run(&counting_loop(), &mut frame, false),
            MachineExit::Returned {
                value: small_integer(1000)
            }
        );
    }

    #[test]
    fn an_optimized_loop_writes_its_counter_back_before_its_poll_exits() {
        let mut frame = [0u64; 64];
        assert_eq!(
            run(&counting_loop(), &mut frame, true),
            MachineExit::Safepoint { program_counter: 3 }
        );
        assert_eq!(frame[13], small_integer(1), "written back");
        assert_eq!(frame[14], boolean(true), "written back");
    }

    #[test]
    fn an_overflow_takes_the_fallback_send_with_the_operands_in_their_frame_words() {
        let snapshot = test_snapshot_with_arguments(
            vec![
                literal(13, SMALL_INTEGER_MAX),
                Instruction::IntegerBinary {
                    operation: IntegerOperation::Add,
                    destination: 14,
                    left: 13,
                    right: 13,
                    target: 3,
                },
                Instruction::Return { register: 14 },
                test_send(13, 13, 14),
                Instruction::Return { register: 14 },
            ],
            0,
        );
        let mut frame = [0u64; 64];
        frame[14] = 0x5A5A;
        assert_eq!(
            run(&snapshot, &mut frame, false),
            MachineExit::Reexecute { program_counter: 3 }
        );
        assert_eq!(frame[13], small_integer(SMALL_INTEGER_MAX));
        assert_eq!(frame[14], 0x5A5A, "the fast path wrote nothing");
    }

    /// An operation whose fallback is the way the program takes is its
    /// send alone: on operands its fast path would answer, the code goes
    /// to the send at the fallback's own program counter, the operand
    /// written back and the destination untouched, as the interpreter's
    /// failing instruction leaves the frame there.
    #[test]
    fn a_taken_fallback_is_sent_at_its_own_program_counter_without_the_fast_path() {
        let mut snapshot = test_snapshot_with_arguments(
            vec![
                literal(13, 1),
                Instruction::IntegerBinary {
                    operation: IntegerOperation::Add,
                    destination: 14,
                    left: 13,
                    right: 13,
                    target: 3,
                },
                Instruction::Return { register: 14 },
                test_send(13, 13, 14),
                Instruction::Return { register: 14 },
            ],
            0,
        );
        snapshot.fallback_taken[1] = true;
        let mut frame = [0u64; 64];
        frame[14] = 0x5A5A;
        assert_eq!(
            run(&snapshot, &mut frame, false),
            MachineExit::Reexecute { program_counter: 3 }
        );
        assert_eq!(frame[13], small_integer(1), "written back for the send");
        assert_eq!(frame[14], 0x5A5A, "no fast path wrote it");
    }

    #[cfg(debug_assertions)]
    #[test]
    fn an_optimized_body_refuses_a_dead_register() {
        let snapshot = test_snapshot_with_arguments(vec![Instruction::Return { register: 0 }], 1);
        let mut poisoned_frame = [0u64; 64];
        poisoned_frame[0] = crate::heap::Word::DEAD_REGISTER_BITS;
        assert!(matches!(
            run(&snapshot, &mut poisoned_frame, false),
            MachineExit::DeadRegister { register: 0, .. }
        ));
    }

    /// The words the baseline's fast path is held to (`baseline`'s
    /// `operands`): the edges of every operation, and words that are no
    /// `SmallInteger`.
    fn operands() -> Vec<u64> {
        let integers = [
            0,
            1,
            -1,
            2,
            -2,
            3,
            -3,
            7,
            -7,
            8,
            1 << 40,
            -(1 << 40) - 3,
            SMALL_INTEGER_MAX,
            SMALL_INTEGER_MIN,
            SMALL_INTEGER_MAX - 1,
            SMALL_INTEGER_MIN + 1,
        ];
        let mut words: Vec<u64> = integers.into_iter().map(small_integer).collect();
        words.extend([Immediate::NIL.bits(), boolean(true), 0x40]);
        words
    }

    /// Every operation on registers the allocation chose answers what the
    /// baseline's template answers on the same words, and declines into
    /// the fallback where it does.
    #[test]
    fn an_optimized_fast_path_answers_what_the_baseline_answers() {
        let operations = [
            IntegerOperation::Add,
            IntegerOperation::Subtract,
            IntegerOperation::Multiply,
            IntegerOperation::FloorDivide,
            IntegerOperation::Less,
            IntegerOperation::Greater,
            IntegerOperation::LessOrEqual,
            IntegerOperation::GreaterOrEqual,
            IntegerOperation::Equal,
        ];
        for operation in operations {
            let instructions = vec![
                Instruction::IntegerBinary {
                    operation,
                    destination: 15,
                    left: 0,
                    right: 1,
                    target: 2,
                },
                Instruction::Return { register: 15 },
                test_send(0, 1, 15),
                Instruction::Return { register: 15 },
            ];
            let optimized = test_snapshot_with_arguments(instructions.clone(), 2);
            let baseline = crate::bytecode::CompiledCode::new(
                crate::bytecode::CodeKind::Method,
                instructions,
                2,
                30,
                0,
                Vec::new(),
                ns_syntax::Span::new(0, 0),
            );
            let baseline_body = crate::jit::baseline::compile(
                &baseline,
                MacroAssembler::new(Abi::host()),
                FeedbackBase::NOT_ASSIGNED,
                BodyRole::Method,
                false,
                crate::jit::baseline::BodyCopy::Lean,
            )
            .unwrap_or_else(|refusal| panic!("refused: {refusal:?}"));
            let mut machine = Machine::new();
            let baseline_start = machine.install(&baseline_body.code);
            let baseline_entry = ns_memory::CodeOffset(
                baseline_start.0 + baseline_body.map.machine_offset(0).expect("mapped"),
            );
            for &left in &operands() {
                for &right in &operands() {
                    let what = format!("{operation:?} {left:#x} {right:#x}");
                    let mut frame = [0u64; 64];
                    frame[0] = left;
                    frame[1] = right;
                    let optimized_exit = run(&optimized, &mut frame, false);
                    let mut baseline_frame = [0u64; 64];
                    baseline_frame[0] = left;
                    baseline_frame[1] = right;
                    let (baseline_exit, _) =
                        machine.run(baseline_entry, &mut baseline_frame, 0, false, &mut (), &[]);
                    assert_eq!(optimized_exit, baseline_exit, "{what}");
                    assert_eq!((frame[0], frame[1]), (left, right), "{what}: operands kept");
                }
            }
        }
    }

    /// A lazy slot's getter of own slot 1, its initializer a send, as a
    /// copy customized at an application whose own slots start at field
    /// 3: its slot is field 4.
    fn customized_lazy_getter() -> Snapshot {
        let mut snapshot = test_snapshot_with_arguments(
            vec![
                Instruction::ReturnSlotIfFilled { own_index: 1 },
                Instruction::LoadSelf { destination: 13 },
                test_send(13, 13, 14),
                Instruction::InitializeSlot {
                    source: 14,
                    own_index: 1,
                },
                Instruction::Return { register: 14 },
            ],
            0,
        );
        let id = crate::mixin::DispatchId::from_bits(7);
        snapshot.customization = Some(Customization {
            key: crate::jit::CustomizationKey {
                receiver: id,
                entered: id,
            },
            own_slot_base: 3,
        });
        snapshot
    }

    /// `customized_lazy_getter` run on a receiver whose field 4 holds
    /// `slot`, in an object space of its own.
    fn run_lazy_getter(slot: u64) -> (MachineExit, [u64; 64]) {
        let optimized = compile(
            &customized_lazy_getter(),
            MacroAssembler::new(Abi::host()),
            BodyRole::Method,
            false,
            Speculation::Traps,
        )
        .unwrap_or_else(|refusal| panic!("refused: {refusal:?}"));
        let fields = crate::heap::Heap::machine_layout().fields_displacement;
        let receiver: usize = 64;
        let mut object_space = vec![Immediate::NIL.bits(); 64];
        let field = (receiver + usize::try_from(fields).expect("positive")) / 8 + 4;
        object_space[field] = slot;
        let mut machine = Machine::new();
        machine.set_block_word(
            BlockWord::ObjectSpaceBase,
            object_space.as_mut_ptr().expose_provenance() as u64,
        );
        let start = machine.install(&optimized.code);
        let entry = ns_memory::CodeOffset(start.0 + optimized.push_entry);
        let mut frame = [0u64; 64];
        frame[usize::from(crate::heap::Heap::receiver_register(0))] =
            u64::try_from(receiver).expect("small");
        let exit = machine.run(entry, &mut frame, 0, false, &mut (), &[]).0;
        (exit, frame)
    }

    /// A copy's lazy getter reads its slot at the fixed field its key's
    /// entered level gives, with no guard: a filled slot is returned, and
    /// an empty one goes on to the initializer, whose send exits here.
    #[test]
    fn a_copy_s_lazy_getter_returns_its_filled_slot_and_computes_an_empty_one() {
        assert_eq!(
            run_lazy_getter(small_integer(42)).0,
            MachineExit::Returned {
                value: small_integer(42)
            }
        );
        let (exit, frame) = run_lazy_getter(Immediate::NIL.bits());
        assert_eq!(exit, MachineExit::Reexecute { program_counter: 2 });
        assert_eq!(frame[13], 64, "the receiver, written back for the send");
    }

    /// Every body of the corpus and of the shipped library that reads an
    /// own slot compiles as a customized copy under every ABI.
    #[test]
    fn a_customized_copy_assembles_under_every_abi() {
        let customization = customized_lazy_getter().customization;
        let mut compiled = 0;
        for mut snapshot in corpus_of_bodies()
            .into_iter()
            .chain(shipped_bodies())
            .chain([customized_lazy_getter()])
        {
            let reads_a_slot = snapshot.instructions.iter().any(|instruction| {
                matches!(
                    instruction,
                    SnapshotInstruction::LoadOwnSlot { .. }
                        | SnapshotInstruction::ReturnSlotIfFilled { .. }
                )
            });
            if !reads_a_slot {
                continue;
            }
            snapshot.feedback_base = FeedbackBase::from_raw(0);
            snapshot.customization = customization;
            for abi in [Abi::Aapcs64, Abi::Windows64, Abi::SystemV] {
                compile(
                    &snapshot,
                    MacroAssembler::new(abi),
                    role_of(&snapshot),
                    false,
                    Speculation::Traps,
                )
                .unwrap_or_else(|refusal| panic!("refused under {abi:?}: {refusal:?}"));
            }
            compiled += 1;
        }
        assert!(compiled > 1, "the library's lazy getters among them");
    }

    /// The role a body of `snapshot`'s kind is compiled as.
    fn role_of(snapshot: &Snapshot) -> BodyRole {
        match snapshot.facts.kind {
            crate::bytecode::CodeKind::Method => BodyRole::Method,
            crate::bytecode::CodeKind::Block => BodyRole::Block,
            crate::bytecode::CodeKind::Initializer => BodyRole::Initializer,
            crate::bytecode::CodeKind::SuperclassClause => BodyRole::EnteredOnly,
        }
    }

    /// Every body of the corpus and of the shipped library, its sends
    /// cached, compiles under every ABI: AArch64's, which no test here
    /// runs, included.
    #[test]
    fn an_optimized_body_assembles_for_aapcs64() {
        for mut snapshot in corpus_of_bodies().into_iter().chain(shipped_bodies()) {
            snapshot.feedback_base = FeedbackBase::from_raw(0);
            let closures = snapshot
                .instructions
                .iter()
                .filter(|instruction| {
                    matches!(instruction, SnapshotInstruction::MakeClosure { .. })
                })
                .count();
            for abi in [Abi::Aapcs64, Abi::Windows64, Abi::SystemV] {
                let role = role_of(&snapshot);
                let optimized = compile(
                    &snapshot,
                    MacroAssembler::new(abi),
                    role,
                    false,
                    Speculation::Traps,
                )
                .unwrap_or_else(|refusal| panic!("refused under {abi:?}: {refusal:?}"));
                let length = u32::try_from(optimized.code.bytes.len()).expect("small");
                for (label, program_counter) in &optimized.returns {
                    let offset = optimized.code.label_offsets[label.index()].expect("bound");
                    assert!(offset <= length);
                    assert!(
                        snapshot.instructions[*program_counter as usize]
                            .argument_slots()
                            .is_some()
                            || matches!(
                                snapshot.instructions[*program_counter as usize],
                                SnapshotInstruction::InitializeInstance { .. }
                                    | SnapshotInstruction::ResolveFuture { .. }
                            ),
                        "a return address is a call's"
                    );
                }
                if let Some(words) = &optimized.words {
                    assert!(words.start < words.end && words.end <= length);
                }
                assert!(optimized.push_entry < length);
                assert!(
                    optimized.twin_transfers.len() <= closures,
                    "a transfer per closure at most, an unreachable one making none"
                );
                assert!(optimized.metadata.points.iter().all(|point| {
                    (point.scopes[0].program_counter as usize) < snapshot.instructions.len()
                }));
            }
        }
    }

    /// Every tree of the inlining corpus compiles with its inlined scopes
    /// -- their type-cases, entries, returns and frame nodes at their
    /// scopes' offsets -- under AArch64's convention, which no test here
    /// runs, and every address recorded with an interrupt point lies in
    /// the code and names a point inside an inlined scope or one where a
    /// block the copy eliminated is live; and the corpus eliminates more
    /// than ten blocks.
    #[test]
    fn an_inlined_body_assembles_for_aapcs64() {
        let (mut inlined, mut eliminated) = (0, 0);
        for tree in crate::jit::optimizing::inlining_corpus() {
            let optimized = crate::jit::optimizing::compile_tree(
                &tree,
                MacroAssembler::new(Abi::Aapcs64),
                role_of(&tree.root),
                false,
                Speculation::Traps,
            )
            .unwrap_or_else(|refusal| panic!("refused: {refusal:?}"));
            let length = u32::try_from(optimized.code.bytes.len()).expect("small");
            for (offset, point) in &optimized.sites {
                assert!(*offset <= length);
                let described = &optimized.metadata.points[*point as usize];
                assert!(described.scopes.len() > 1 || described.holds_eliminated_blocks());
            }
            if optimized.deepest_chain > 0 {
                inlined += 1;
            }
            eliminated += optimized.eliminated.len();
        }
        assert!(inlined > 10, "{inlined} trees inline");
        assert!(eliminated > 10, "{eliminated} blocks eliminated");
    }

    /// `r15 := r0 + r1. r16 := r15 < r2. ^r16`, each operation's fallback
    /// an uncached send, whose site `unrun` records as never run.
    fn add_then_compare(unrun: bool) -> Snapshot {
        let mut snapshot = test_snapshot_with_arguments(
            vec![
                Instruction::IntegerBinary {
                    operation: IntegerOperation::Add,
                    destination: 15,
                    left: 0,
                    right: 1,
                    target: 3,
                },
                Instruction::IntegerBinary {
                    operation: IntegerOperation::Less,
                    destination: 16,
                    left: 15,
                    right: 2,
                    target: 5,
                },
                Instruction::Return { register: 16 },
                test_send(0, 1, 15),
                Instruction::Jump { target: 1 },
                test_send(15, 2, 16),
                Instruction::Return { register: 16 },
            ],
            3,
        );
        if unrun {
            unrun_site(&mut snapshot, 0, 0);
            unrun_site(&mut snapshot, 1, 1);
        }
        snapshot
    }

    /// The tag tests `snapshot`'s optimized code holds under `speculation`.
    fn tag_tests(snapshot: &Snapshot, speculation: Speculation) -> usize {
        compile(
            snapshot,
            MacroAssembler::new(Abi::host()),
            BodyRole::Method,
            false,
            speculation,
        )
        .unwrap_or_else(|refusal| panic!("refused: {refusal:?}"))
        .small_integer_tag_tests
    }

    /// The sum's fast path, whose failure traps, answers a
    /// `SmallInteger` wherever control goes on past it, so the comparison
    /// tests only `r2`'s tag; with a fallback, or with no speculation, the
    /// sum merges with its send's answer and is tested again.
    #[test]
    fn a_result_known_to_be_a_small_integer_is_not_tested_again() {
        assert_eq!(tag_tests(&add_then_compare(true), Speculation::Traps), 3);
        assert_eq!(tag_tests(&add_then_compare(false), Speculation::Traps), 4);
        assert_eq!(tag_tests(&add_then_compare(true), Speculation::None), 4);
    }

    /// A trap goes where the fallback would: the frame written back, the
    /// destination untouched, and the exit naming the instruction to run
    /// again and the site the speculation rested on.
    #[test]
    fn an_overflow_whose_fallback_never_ran_traps_with_the_frame_written_back() {
        let mut snapshot = test_snapshot_with_arguments(
            vec![
                literal(13, SMALL_INTEGER_MAX),
                Instruction::LoadSelf { destination: 14 },
                Instruction::IntegerBinary {
                    operation: IntegerOperation::Add,
                    destination: 15,
                    left: 13,
                    right: 13,
                    target: 4,
                },
                Instruction::Return { register: 15 },
                test_send(13, 13, 15),
                Instruction::Return { register: 15 },
            ],
            0,
        );
        unrun_site(&mut snapshot, 2, 5);
        let mut frame = [0u64; 64];
        let receiver = usize::from(crate::heap::Heap::receiver_register(0));
        frame[receiver] = 0x40;
        frame[15] = 0x5A5A;
        assert_eq!(
            run(&snapshot, &mut frame, false),
            MachineExit::Trap {
                program_counter: 2,
                site: 5
            }
        );
        assert_eq!(frame[13], small_integer(SMALL_INTEGER_MAX), "written back");
        assert_eq!(frame[14], 0x40, "written back");
        assert_eq!(frame[15], 0x5A5A, "the fast path wrote nothing");
        // On operands it answers, it answers.
        snapshot.instructions[0] =
            SnapshotInstruction::of_unclosed(&literal(13, 20)).expect("a literal");
        let mut frame = [0u64; 64];
        assert_eq!(
            run(&snapshot, &mut frame, false),
            MachineExit::Returned {
                value: small_integer(40)
            }
        );
    }

    /// `^r0 ifTrue: [1] ifFalse: [2]`, whose non-Boolean way never ran,
    /// answers on a Boolean and traps at the conditional on anything else.
    #[test]
    fn a_non_boolean_whose_way_never_ran_traps_at_the_conditional() {
        let mut snapshot = test_snapshot_with_arguments(
            vec![
                Instruction::JumpIfNotBoolean {
                    register: 0,
                    target: 6,
                },
                Instruction::BranchIfFalse {
                    register: 0,
                    target: 4,
                },
                literal(15, 1),
                Instruction::Return { register: 15 },
                literal(15, 2),
                Instruction::Return { register: 15 },
                test_send(0, 0, 15),
                Instruction::Return { register: 15 },
            ],
            1,
        );
        unrun_site(&mut snapshot, 0, 3);
        for (condition, answer) in [(true, 1), (false, 2)] {
            let mut frame = [0u64; 64];
            frame[0] = boolean(condition);
            assert_eq!(
                run(&snapshot, &mut frame, false),
                MachineExit::Returned {
                    value: small_integer(answer)
                }
            );
        }
        let mut frame = [0u64; 64];
        frame[0] = Immediate::NIL.bits();
        assert_eq!(
            run(&snapshot, &mut frame, false),
            MachineExit::Trap {
                program_counter: 0,
                site: 3
            }
        );
    }

    /// A send never executed is not made: its block writes the frame back
    /// and traps at it.
    #[test]
    fn a_send_never_executed_traps_with_its_arguments_written_back() {
        let mut snapshot = test_snapshot_with_arguments(
            vec![
                literal(13, 7),
                test_send(13, 13, 14),
                Instruction::Return { register: 14 },
            ],
            0,
        );
        unrun_site(&mut snapshot, 1, 2);
        let mut frame = [0u64; 64];
        assert_eq!(
            run(&snapshot, &mut frame, false),
            MachineExit::Trap {
                program_counter: 1,
                site: 2
            }
        );
        assert_eq!(frame[13], small_integer(7), "written back");
    }

    /// Every body of the corpus and of the shipped library, every site
    /// recorded as never run, compiles with its traps under every ABI.
    #[test]
    fn a_speculating_body_assembles_under_every_abi() {
        for mut snapshot in corpus_of_bodies().into_iter().chain(shipped_bodies()) {
            snapshot.feedback_base = FeedbackBase::from_raw(0);
            unrun_every_site(&mut snapshot);
            for abi in [Abi::Aapcs64, Abi::Windows64, Abi::SystemV] {
                compile(
                    &snapshot,
                    MacroAssembler::new(abi),
                    role_of(&snapshot),
                    false,
                    Speculation::Traps,
                )
                .unwrap_or_else(|refusal| panic!("refused under {abi:?}: {refusal:?}"));
            }
        }
    }

    /// A tree whose root, taking two arguments, sends `selector` to its
    /// argument 0 with `arguments` of its own, its site 0 cached for each
    /// of `cached`: a guard number of `test_receiver` and the callee it
    /// reached, an arm always inlined.
    fn always_inlined_tree(
        selector: &str,
        arguments: Vec<u16>,
        cached: Vec<(u32, crate::jit::optimizing::tree::CalleeFacts)>,
    ) -> crate::jit::optimizing::tree::SnapshotTree {
        let mut root = test_snapshot_with_arguments(
            vec![
                Instruction::SendPublic {
                    selector: crate::Symbol::intern(selector),
                    receiver_register: 0,
                    argument_base: 20,
                    argument_sources: arguments,
                    destination: 14,
                    call_site: crate::bytecode::CallSite { number: 0 },
                },
                Instruction::Return { register: 14 },
            ],
            2,
        );
        crate::jit::optimizing::ran_site(&mut root, 0, 0);
        let code_id = root.code_id;
        crate::jit::optimizing::test_tree(
            root,
            vec![(
                code_id,
                0,
                cached
                    .into_iter()
                    .map(|(guard, callee)| {
                        (crate::jit::optimizing::test_receiver(guard, 1, 0), callee)
                    })
                    .collect(),
            )],
            Vec::new(),
        )
    }

    /// The dispatch id `test_receiver(guard, ..)` names, as a guard word.
    fn guard_word(guard: u32) -> u64 {
        u64::from(guard + crate::mixin::DispatchId::FIRST_MINTED)
    }

    /// `tree` compiled for the host and run from the push's entry on a
    /// frame whose arguments are `first` and `second`, every immediate of
    /// `immediates` -- its index and the guard and class words the machine
    /// words hold for it -- set.
    fn run_tree(
        tree: &crate::jit::optimizing::tree::SnapshotTree,
        (first, second): (u64, u64),
        immediates: &[(u64, u64, u64)],
    ) -> MachineExit {
        use crate::heap::machine_words::MachineWord;
        let optimized = crate::jit::optimizing::compile_tree(
            tree,
            MacroAssembler::new(Abi::host()),
            BodyRole::Method,
            false,
            Speculation::Traps,
        )
        .unwrap_or_else(|refusal| panic!("refused: {refusal:?}"));
        let mut machine = Machine::new();
        for (index, guard, class) in immediates {
            let index = usize::try_from(*index).expect("small");
            machine.set_machine_word(MachineWord::ImmediateGuard(index), *guard);
            machine.set_machine_word(MachineWord::ImmediateClass(index), *class);
        }
        let start = machine.install(&optimized.code);
        let entry = ns_memory::CodeOffset(start.0 + optimized.push_entry);
        let mut frame = [0u64; 64];
        frame[0] = first;
        frame[1] = second;
        machine.run(entry, &mut frame, 0, false, &mut (), &[]).0
    }

    /// A `SmallInteger`'s `+` always inlined at a send that is no fast
    /// path's fallback -- which no lowering of this compiler emits, only an
    /// image's code can hold -- answers the sum, and traps at the send,
    /// running it again in the interpreter, on an overflow and on an
    /// argument that is no `SmallInteger`.
    #[test]
    fn an_inlined_integer_arm_answers_and_traps_at_its_send() {
        use crate::builtins::primitive::{IntegerPrimitive, Primitive};
        let tree = always_inlined_tree(
            "+",
            vec![1],
            vec![(
                1,
                crate::jit::optimizing::tree::CalleeFacts::Primitive(Primitive::Integer(
                    IntegerPrimitive::Add,
                )),
            )],
        );
        let index = crate::heap::Heap::machine_layout().small_integer_index;
        let integers = [(index, guard_word(1), 0)];
        assert_eq!(
            run_tree(&tree, (small_integer(37), small_integer(5)), &integers),
            MachineExit::Returned {
                value: small_integer(42)
            }
        );
        let trap = MachineExit::Trap {
            program_counter: 0,
            site: 0,
        };
        assert_eq!(
            run_tree(
                &tree,
                (small_integer(SMALL_INTEGER_MAX), small_integer(1)),
                &integers
            ),
            trap
        );
        assert_eq!(
            run_tree(&tree, (small_integer(1), Immediate::NIL.bits()), &integers),
            trap
        );
    }

    /// `class` always inlined answers an immediate's class from the machine
    /// words, a `SmallInteger`'s and a `SmallFloat`'s each its own -- the
    /// immediates this VM has besides the constants -- with no leaf.
    #[test]
    fn an_inlined_class_of_an_immediate_reads_the_machine_words() {
        use crate::builtins::primitive::{ObjectPrimitive, Primitive};
        let class = || {
            crate::jit::optimizing::tree::CalleeFacts::Primitive(Primitive::Object(
                ObjectPrimitive::Class,
            ))
        };
        let tree = always_inlined_tree("class", Vec::new(), vec![(1, class()), (2, class())]);
        let layout = crate::heap::Heap::machine_layout();
        let immediates = [
            (layout.small_integer_index, guard_word(1), 0x1110),
            (layout.small_float_index, guard_word(2), 0x2220),
        ];
        assert_eq!(
            run_tree(&tree, (small_integer(3), 0), &immediates),
            MachineExit::Returned { value: 0x1110 }
        );
        let float = Immediate::small_float(1.5).expect("a SmallFloat").bits();
        assert_eq!(
            run_tree(&tree, (float, 0), &immediates),
            MachineExit::Returned { value: 0x2220 }
        );
    }
}
