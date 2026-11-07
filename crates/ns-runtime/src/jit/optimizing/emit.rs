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

use ns_assembler::{Condition, Label, Register};

use super::allocate::{Allocation, DirtyPoint, Location, Move, PointKind, Spot};
use super::graph::{BlockId, End, Graph, Node, ValueId, ValueKind};
use super::liveness::poll_target;
use super::metadata::Metadata;
use super::snapshot::{Snapshot, SnapshotInstruction};
use crate::heap::Immediate;
use crate::jit::baseline::{lower, BodyAssembler, TwinTransfer};
use crate::jit::macro_assembler::KnownSmallIntegers;
use crate::jit::stubs::ExitKind;
use crate::jit::{closures, sends, Refusal};

/// A body's optimized code, as `optimizing::compile` answers it for the
/// JIT to install beside the body's baseline code.
pub(crate) struct OptimizedBody {
    pub(crate) code: ns_assembler::Assembled,
    /// Each send's call's return address, a label bound right after the
    /// call, and the send's program counter, as `call_send` records them.
    pub(crate) returns: Vec<(Label, u32)>,
    /// The offsets in the code of its first data word and of the byte past
    /// its last, if it has any (`baseline::BaselineBody::words`).
    pub(crate) words: Option<std::ops::Range<u32>>,
    /// The transfers into the baseline twin, one per `MakeClosure`
    /// (`closures::transfer_to_twin`).
    pub(crate) twin_transfers: Vec<TwinTransfer>,
    /// Each interrupt point's description, which the install drops: no
    /// frame is rebuilt from it until optimized code holds a value outside
    /// its frame word.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "read by the deoptimization of frames whose values live outside their words"
        )
    )]
    pub(crate) metadata: Metadata,
    /// The offset in the code where the run loop's push enters, past the
    /// prologue a compiled call enters at: the body's first block.
    pub(crate) push_entry: u32,
    /// The `SmallInteger` tag tests the code holds, which the tests count.
    #[cfg(test)]
    pub(crate) small_integer_tag_tests: usize,
}

/// Emits `snapshot`'s optimized code, its `graph` allocated as
/// `allocation` says, into `body`, which `BodyAssembler::for_optimized`
/// made; answers the label the run loop's push enters at, past the
/// prologue.
pub(in crate::jit) fn emit(
    snapshot: &Snapshot,
    graph: &Graph,
    allocation: &Allocation,
    body: &mut BodyAssembler,
) -> Result<Label, Refusal> {
    sends::prologue(body, &snapshot.facts);
    let mut emitter = Emitter {
        graph,
        allocation,
        labels: graph
            .blocks
            .iter()
            .map(|_| body.macro_assembler.assembler().label())
            .collect(),
        points: allocation
            .dirty_at
            .iter()
            .map(|point| (point.position, point))
            .collect(),
        polls: Vec::new(),
        traps: Vec::new(),
        snapshot,
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
    body.macro_assembler.hold_values(&[]);
    Ok(push_entry)
}

/// A back edge's poll, out of line: the label its test jumps to, the
/// stores it owes the frame there, and the loop head it exits at.
struct Poll {
    label: Label,
    stores: Vec<(u16, Register)>,
    program_counter: u32,
}

/// A guard's trap, out of line: the label its failed test jumps to, the
/// stores it owes the frame there, the instruction it runs again and the
/// site its speculation rested on.
struct Trap {
    label: Label,
    stores: Vec<(u16, Register)>,
    program_counter: u32,
    site: u32,
}

struct Emitter<'graph> {
    graph: &'graph Graph,
    allocation: &'graph Allocation,
    /// Each block's label, by its index.
    labels: Vec<Label>,
    /// Each interrupt point's write-back, by its position.
    points: HashMap<u32, &'graph DirtyPoint>,
    polls: Vec<Poll>,
    traps: Vec<Trap>,
    /// The snapshot, whose sites name each trap's.
    snapshot: &'graph Snapshot,
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

    fn block(&mut self, body: &mut BodyAssembler, block_id: BlockId, next: Option<BlockId>) {
        let graph = self.graph;
        let positions = &self.allocation.positions;
        let block = graph.block(block_id);
        body.macro_assembler
            .assembler()
            .bind(self.labels[block_id.index()]);
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
                    macro_assembler.store_frame_register(register, from);
                }
                // The frame word was written by this code, or is one the
                // interpreter reads there: never a dead register's, which
                // a debug build's check would refuse.
                Move::Load { register, into, .. } => {
                    macro_assembler.load_frame_register(into, register);
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
                    body.macro_assembler.load_frame_register(target, *register);
                }
            }
            Node::Constant { value, word } => {
                if let Some(target) = defined_in(*value) {
                    body.macro_assembler.load_immediate_word(target, *word);
                }
            }
            Node::Receiver { value } => {
                if let Some(target) = defined_in(*value) {
                    let argument_count = body.argument_count;
                    body.macro_assembler.load_receiver(target, argument_count);
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
                // The fast path's failure goes to its fallback, or else to
                // its trap, whose write-back lies on that way alone.
                let fallback = match fallback {
                    Some(fallback) => self.labels[fallback.index()],
                    None => self.trap_out_of_line(body, use_position, *program_counter),
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
            Node::CurrentClass { .. } => {
                panic!("internal: an optimized compile builds no inlined scope")
            }
            Node::Frame {
                program_counter,
                instruction,
                ..
            } => self.frame_node(
                body,
                block_id,
                index,
                *program_counter,
                instruction,
                use_position,
            ),
        }
    }

    /// A frame node: the write-back the allocation owes the frame there,
    /// then the instruction by the template the baseline's arm for it
    /// emits, with nothing held.
    fn frame_node(
        &mut self,
        body: &mut BodyAssembler,
        block_id: BlockId,
        index: usize,
        program_counter: u32,
        instruction: &SnapshotInstruction,
        use_position: u32,
    ) {
        let point = self.points[&use_position];
        debug_assert_eq!(
            (point.block, point.kind),
            (block_id, PointKind::Frame { node: index }),
            "internal: a frame node's write-back is its own"
        );
        for (register, _, location) in &point.stores {
            let Location::Register(from) = location else {
                panic!("internal: a value not in its frame word is in a register");
            };
            body.macro_assembler.store_frame_register(*register, *from);
        }
        body.macro_assembler.hold_values(&[]);
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
                } if index + 1 == block.nodes.len() => self.labels[entered.index()],
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
        // arm transfers into the twin, as a lean body's does.
        let lowering = match instruction {
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
        match &graph.block(block_id).end {
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
                            self.trap_out_of_line(body, end_use, *program_counter),
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
                let stores = self.trap_stores(end_use);
                let macro_assembler = &mut body.macro_assembler;
                for (register, from) in stores {
                    macro_assembler.store_frame_register(register, from);
                }
                macro_assembler.trap(*program_counter, *site);
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
            End::TypeCase { .. } => {
                panic!("internal: an optimized compile builds no inlined send")
            }
        }
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
        let point = self.points[&end_use];
        debug_assert_eq!(
            (point.block, point.kind),
            (block_id, PointKind::Poll),
            "internal: a back edge's write-back is its poll's"
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
        self.polls.push(Poll {
            label,
            stores,
            program_counter: self.graph.block(head).program_counter,
        });
    }

    /// The stores the trap at `position` owes the frame, each from the
    /// register its value is in.
    fn trap_stores(&self, position: u32) -> Vec<(u16, Register)> {
        let point = self.points[&position];
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
    /// `program_counter` again, its write-back and exit laid out after the
    /// body (`emit_traps`), so that a guard that holds stores nothing.
    fn trap_out_of_line(
        &mut self,
        body: &mut BodyAssembler,
        position: u32,
        program_counter: u32,
    ) -> Label {
        let label = body.macro_assembler.assembler().label();
        let index = usize::try_from(program_counter).expect("internal: a program counter fits");
        let site = self.snapshot.sites[index]
            .expect("internal: a trap rests on a site")
            .site;
        self.traps.push(Trap {
            label,
            stores: self.trap_stores(position),
            program_counter,
            site,
        });
        label
    }

    /// Each guard's way to its trap: the write-back, then the trap, which
    /// runs the instruction again in the interpreter.
    fn emit_traps(&mut self, body: &mut BodyAssembler) {
        let macro_assembler = &mut body.macro_assembler;
        for trap in std::mem::take(&mut self.traps) {
            macro_assembler.assembler().bind(trap.label);
            for (register, from) in trap.stores {
                macro_assembler.store_frame_register(register, from);
            }
            macro_assembler.trap(trap.program_counter, trap.site);
        }
    }

    /// Each back edge's way to its exit: the write-back, then the exit at
    /// the loop's head, where the interpreter takes the safepoint.
    fn emit_polls(&mut self, body: &mut BodyAssembler) {
        let macro_assembler = &mut body.macro_assembler;
        for poll in std::mem::take(&mut self.polls) {
            macro_assembler.assembler().bind(poll.label);
            for (register, from) in poll.stores {
                macro_assembler.store_frame_register(register, from);
            }
            macro_assembler.exit(ExitKind::Safepoint, poll.program_counter);
        }
    }
}

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
}
