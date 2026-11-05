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
//! Between two frame nodes nothing is written into a register a value is
//! held in but that value's own: a simple node computes in the registers
//! no value is allocated to (`Roles::integer_temporaries`) and takes each
//! through `MacroAssembler::temporary`, which a debug build checks against
//! the registers `hold_values` names.
#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the JIT's installation of optimized code, Task 6's, is the emitter's first caller"
    )
)]

use std::collections::HashMap;

use ns_assembler::{Condition, Label, Register};

use super::allocate::{Allocation, DirtyPoint, Location, Move, PointKind, Spot};
use super::graph::{BlockId, End, Graph, Node, ValueId};
use super::liveness::poll_target;
use super::metadata::Metadata;
use super::snapshot::{Snapshot, SnapshotInstruction};
use crate::heap::Immediate;
use crate::jit::baseline::{lower, BodyAssembler, TwinTransfer};
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
    pub(crate) metadata: Metadata,
    /// The offset in the code where the run loop's push enters, past the
    /// prologue a compiled call enters at: the body's first block.
    pub(crate) push_entry: u32,
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
    };
    let order = &allocation.positions.order;
    let push_entry = emitter.labels[order
        .first()
        .expect("internal: a graph has an entry block")
        .index()];
    for (index, block) in order.iter().enumerate() {
        emitter.block(body, *block, order.get(index + 1).copied());
    }
    emitter.emit_polls(body);
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

struct Emitter<'graph> {
    graph: &'graph Graph,
    allocation: &'graph Allocation,
    /// Each block's label, by its index.
    labels: Vec<Label>,
    /// Each interrupt point's write-back, by its position.
    points: HashMap<u32, &'graph DirtyPoint>,
    polls: Vec<Poll>,
}

impl Emitter<'_> {
    /// The machine register `value` is in at `position`, which the
    /// allocation placed it in because the code there reads it there.
    fn register_of(&self, value: ValueId, position: u32) -> Register {
        match self.allocation.location_of(value, position) {
            Location::Register(register) => register,
            Location::FrameWord => {
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
            Location::FrameWord => None,
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
            Node::IntegerBinary {
                operation,
                value,
                left,
                right,
                fallback,
                ..
            } => {
                let (left, right) = (
                    self.register_of(*left, use_position),
                    self.register_of(*right, use_position),
                );
                let fallback = self.labels[fallback.index()];
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
                    &mut |macro_assembler, condition| {
                        macro_assembler.assembler().jump_if(condition, fallback);
                    },
                );
                if let Some(target) = defined_in(*value) {
                    macro_assembler.move_register(target, result);
                }
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
                _ => after,
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
                condition,
                if_true,
                if_false,
                not_boolean,
            } => {
                debug_assert!(
                    self.allocation.moves_at(end_definition).is_empty(),
                    "internal: a branch's ways need no moves"
                );
                let condition = self.register_of(*condition, end_use);
                let tests: Vec<(bool, BlockId)> = [(true, *if_true), (false, *if_false)]
                    .into_iter()
                    .filter(|(_, target)| target != not_boolean)
                    .collect();
                let assembler = body.macro_assembler.assembler();
                for (position, (boolean, target)) in tests.iter().enumerate() {
                    let word = i32::try_from(Immediate::boolean(*boolean).bits())
                        .expect("internal: a Boolean's word is a small constant");
                    assembler.compare_immediate(condition, word);
                    // The last test falls into the next block on whichever
                    // way leads there.
                    if position + 1 == tests.len() && next == Some(*target) {
                        assembler.jump_if(Condition::NotEqual, self.labels[not_boolean.index()]);
                        return;
                    }
                    assembler.jump_if(Condition::Equal, self.labels[target.index()]);
                }
                jump_unless_next(body, &self.labels, *not_boolean);
            }
            End::Return { value } => {
                let value = self.register_of(*value, end_use);
                let macro_assembler = &mut body.macro_assembler;
                let leaf_result = macro_assembler.temporary(macro_assembler.roles().leaf_result);
                macro_assembler.move_register(leaf_result, value);
                sends::return_loaded(body);
            }
            End::Frame => {}
            End::FrameJump {
                next: fall_through, ..
            } => {
                self.moves(body, end_definition);
                jump_unless_next(body, &self.labels, *fall_through);
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
                Location::FrameWord => {
                    panic!("internal: a value not in its frame word is in a register")
                }
            })
            .collect();
        let label = body.macro_assembler.assembler().label();
        body.macro_assembler.count_back_edge();
        body.macro_assembler.poll_to(label);
        self.polls.push(Poll {
            label,
            stores,
            program_counter: self.graph.block(head).program_counter,
        });
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
    use crate::jit::optimizing::snapshot::{Snapshot, SnapshotInstruction};
    use crate::jit::optimizing::{
        compile, corpus_of_bodies, shipped_bodies, test_send, test_snapshot_with_arguments,
    };
    use crate::jit::stubs::testing::Machine;
    use crate::jit::stubs::MachineExit;

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
        let optimized = match compile(snapshot, macro_assembler, BodyRole::Method) {
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
                let optimized = compile(&snapshot, MacroAssembler::new(abi), role)
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
                    (point.program_counter as usize) < snapshot.instructions.len()
                }));
            }
        }
    }
}
