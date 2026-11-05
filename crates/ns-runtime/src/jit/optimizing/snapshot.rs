//! The optimizing compiler's input: a snapshot of a body, taken once at the
//! run loop's safepoint (`docs/design/execution.md`, "Planned: the
//! optimizing JIT"). Everything the compiler reads of the body is here --
//! the body's facts and its instructions, a closure's block named by its
//! code id rather than held as an `Rc` -- so the snapshot is `Send` and a
//! compile that runs beside the interpreter reads nothing the interpreter
//! can change.
//!
//! The baseline reads the same two forms where it asks what the body
//! lays on the stack (`sends::prologue`, `sends::call_slots`), so that
//! both compilers answer those questions from one place.

use std::rc::Rc;

use crate::builtins::primitive::MirrorPrimitive;
use crate::bytecode::{
    CallSite, CodeKind, CompiledCode, Instruction, IntegerOperation, Literal, HEADER_REGISTERS,
};
use crate::heap::{CodeId, Heap};
use crate::interpreter::inline_cache::FeedbackBase;
use crate::Symbol;

/// What a body says of itself apart from its instructions: the numbers
/// the push and the prologue lay a frame out by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BodyFacts {
    pub(crate) argument_count: u16,
    pub(crate) local_count: u16,
    pub(crate) register_count: u16,
    pub(crate) kind: CodeKind,
    pub(crate) family_size: u32,
    pub(crate) copied_count: u16,
    pub(crate) addresses_own_slots: bool,
    pub(crate) declaration_code: bool,
    pub(crate) unwind_marker: bool,
    pub(crate) frame_register_count: usize,
    pub(crate) instruction_count: u32,
}

impl BodyFacts {
    pub(crate) fn of(code: &CompiledCode) -> BodyFacts {
        BodyFacts {
            argument_count: code.argument_count,
            local_count: code.local_count,
            register_count: code.register_count,
            kind: code.kind,
            family_size: code.family_size,
            copied_count: code.copied_count,
            addresses_own_slots: code.addresses_own_slots,
            declaration_code: code.is_declaration_code(),
            unwind_marker: code.unwind_marker.is_some(),
            frame_register_count: code.frame_register_count(),
            // A body past `INSTRUCTION_LIMIT` compiles to a single
            // instruction, so the count always fits.
            instruction_count: u32::try_from(code.instruction_count())
                .expect("internal: a body holds at most 2^20 instructions"),
        }
    }

    /// How many registers an activation has above its header
    /// (`CompiledCode::registers_above_header`).
    pub(crate) fn registers_above_header(&self) -> usize {
        usize::from(self.register_count)
            .saturating_sub(usize::from(self.argument_count) + usize::from(HEADER_REGISTERS))
    }

    /// One past the last register of a frame running the body: its
    /// window's end, above which a callee no send lays on the slots is
    /// pushed.
    pub(crate) fn window_end(&self) -> u16 {
        self.register_count
            .max(self.argument_count.saturating_add(HEADER_REGISTERS))
    }
}

/// One instruction of a body, in the form the optimizing compiler reads:
/// an `Instruction` with the same fields but for `MakeClosure`, whose
/// block is named by its code id (and its parameters by their count, the
/// only part of them a compile reads) instead of held by `Rc`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SnapshotInstruction {
    LoadLiteral {
        destination: u16,
        literal: Literal,
    },
    LoadSelf {
        destination: u16,
    },
    LoadEnclosingObject {
        destination: u16,
        depth: u16,
    },
    Move {
        destination: u16,
        source: u16,
    },
    StoreLocal {
        source: u16,
        slot: u16,
    },
    LoadOuterLocal {
        destination: u16,
        depth: u16,
        index: u16,
    },
    StoreOuterLocal {
        source: u16,
        depth: u16,
        index: u16,
    },
    SendLexical {
        depth: u16,
        selector: Symbol,
        argument_base: u16,
        argument_sources: Vec<u16>,
        destination: u16,
        call_site: CallSite,
    },
    SendPublic {
        selector: Symbol,
        receiver_register: u16,
        argument_base: u16,
        argument_sources: Vec<u16>,
        destination: u16,
        call_site: CallSite,
    },
    SendInlinable {
        selector: Symbol,
        receiver_register: u16,
        argument_base: u16,
        argument_sources: Vec<u16>,
        destination: u16,
        call_site: CallSite,
    },
    SendSuper {
        selector: Symbol,
        argument_base: u16,
        argument_sources: Vec<u16>,
        destination: u16,
        call_site: CallSite,
    },
    SendMirror {
        primitive: MirrorPrimitive,
        mirror_register: u16,
        subject_register: u16,
        argument_base: u16,
        argument_sources: Vec<u16>,
        destination: u16,
        failure: u32,
    },
    MakeArray {
        destination: u16,
        element_registers: Vec<u16>,
    },
    MakeVector {
        destination: u16,
        size: u16,
    },
    LoadVector {
        destination: u16,
        vector: u16,
        index: u16,
    },
    StoreVector {
        vector: u16,
        index: u16,
        source: u16,
    },
    InitializeSlot {
        source: u16,
        own_index: u32,
    },
    ReturnSlotIfFilled {
        own_index: u32,
    },
    MakeFuture {
        destination: u16,
        closure: u16,
    },
    ResolveFuture {
        destination: u16,
        source: u16,
    },
    LoadOwnSlot {
        destination: u16,
        own_index: u32,
    },
    InitializeSlotProvisionally {
        source: u16,
        own_index: u32,
    },
    Return {
        register: u16,
    },
    ImplicitReturn {
        register: u16,
    },
    NonLocalReturn {
        register: u16,
    },
    SignalNonBoolean {
        register: u16,
    },
    Jump {
        target: u32,
    },
    JumpIfNotBoolean {
        register: u16,
        target: u32,
    },
    BranchIfFalse {
        register: u16,
        target: u32,
    },
    BranchIfTrue {
        register: u16,
        target: u32,
    },
    IntegerBinary {
        operation: IntegerOperation,
        destination: u16,
        left: u16,
        right: u16,
        target: u32,
    },
    AllocateInstance {
        destination: u16,
    },
    InitializeInstance {
        instance: u16,
        destination: u16,
    },
    CheckAppliedClause {
        steps: u16,
        selector: Symbol,
    },
    InitializeSuperclass {
        steps: u16,
        selector: Symbol,
        argument_base: u16,
        argument_sources: Vec<u16>,
        destination: u16,
    },
    InitializeAppliedLevel {
        steps: u16,
        argument_base: u16,
        argument_sources: Vec<u16>,
        destination: u16,
    },
    MakeClosure {
        destination: u16,
        block: CodeId,
        parameter_count: u16,
        copied: Vec<u16>,
    },
}

impl SnapshotInstruction {
    /// The snapshot form of `instruction`. A `MakeClosure`'s block is
    /// registered with `heap` if it was not yet, which gives it the id the
    /// closure the instruction makes will carry.
    pub(crate) fn of(instruction: &Instruction, heap: &mut Heap) -> SnapshotInstruction {
        convert(instruction, &mut |block| Some(heap.code_id(block)))
            .expect("internal: every block is registered when the heap is given")
    }

    /// The snapshot form of an instruction that is not a `MakeClosure`,
    /// for a reader that asks what a call lays and needs no heap: `None`
    /// for a `MakeClosure`, which calls nothing.
    pub(crate) fn of_unclosed(instruction: &Instruction) -> Option<SnapshotInstruction> {
        convert(instruction, &mut |_| None)
    }

    /// The slots the instruction lays a callee's arguments on, as
    /// `Instruction::argument_slots` answers them.
    pub(crate) fn argument_slots(&self) -> Option<(u16, &[u16])> {
        match self {
            SnapshotInstruction::SendLexical {
                argument_base,
                argument_sources,
                ..
            }
            | SnapshotInstruction::SendPublic {
                argument_base,
                argument_sources,
                ..
            }
            | SnapshotInstruction::SendInlinable {
                argument_base,
                argument_sources,
                ..
            }
            | SnapshotInstruction::SendSuper {
                argument_base,
                argument_sources,
                ..
            }
            | SnapshotInstruction::SendMirror {
                argument_base,
                argument_sources,
                ..
            }
            | SnapshotInstruction::InitializeSuperclass {
                argument_base,
                argument_sources,
                ..
            }
            | SnapshotInstruction::InitializeAppliedLevel {
                argument_base,
                argument_sources,
                ..
            } => Some((*argument_base, argument_sources)),
            _ => None,
        }
    }
}

/// `SnapshotInstruction::of` with a `MakeClosure`'s block id asked of
/// `block_id`, which answers `None` where there is no heap to ask.
fn convert(
    instruction: &Instruction,
    block_id: &mut dyn FnMut(&Rc<CompiledCode>) -> Option<CodeId>,
) -> Option<SnapshotInstruction> {
    Some(match instruction {
        Instruction::MakeClosure {
            destination,
            parameters,
            code,
            copied,
        } => SnapshotInstruction::MakeClosure {
            destination: *destination,
            block: block_id(code)?,
            parameter_count: u16::try_from(parameters.len())
                .expect("internal: a block takes at most 255 parameters"),
            copied: copied.clone(),
        },
        Instruction::LoadLiteral {
            destination,
            literal,
        } => SnapshotInstruction::LoadLiteral {
            destination: *destination,
            literal: literal.clone(),
        },
        Instruction::LoadSelf { destination } => SnapshotInstruction::LoadSelf {
            destination: *destination,
        },
        Instruction::LoadEnclosingObject { destination, depth } => {
            SnapshotInstruction::LoadEnclosingObject {
                destination: *destination,
                depth: *depth,
            }
        }
        Instruction::Move {
            destination,
            source,
        } => SnapshotInstruction::Move {
            destination: *destination,
            source: *source,
        },
        Instruction::StoreLocal { source, slot } => SnapshotInstruction::StoreLocal {
            source: *source,
            slot: *slot,
        },
        Instruction::LoadOuterLocal {
            destination,
            depth,
            index,
        } => SnapshotInstruction::LoadOuterLocal {
            destination: *destination,
            depth: *depth,
            index: *index,
        },
        Instruction::StoreOuterLocal {
            source,
            depth,
            index,
        } => SnapshotInstruction::StoreOuterLocal {
            source: *source,
            depth: *depth,
            index: *index,
        },
        Instruction::SendLexical {
            depth,
            selector,
            argument_base,
            argument_sources,
            destination,
            call_site,
        } => SnapshotInstruction::SendLexical {
            depth: *depth,
            selector: *selector,
            argument_base: *argument_base,
            argument_sources: argument_sources.clone(),
            destination: *destination,
            call_site: *call_site,
        },
        Instruction::SendPublic {
            selector,
            receiver_register,
            argument_base,
            argument_sources,
            destination,
            call_site,
        } => SnapshotInstruction::SendPublic {
            selector: *selector,
            receiver_register: *receiver_register,
            argument_base: *argument_base,
            argument_sources: argument_sources.clone(),
            destination: *destination,
            call_site: *call_site,
        },
        Instruction::SendInlinable {
            selector,
            receiver_register,
            argument_base,
            argument_sources,
            destination,
            call_site,
        } => SnapshotInstruction::SendInlinable {
            selector: *selector,
            receiver_register: *receiver_register,
            argument_base: *argument_base,
            argument_sources: argument_sources.clone(),
            destination: *destination,
            call_site: *call_site,
        },
        Instruction::SendSuper {
            selector,
            argument_base,
            argument_sources,
            destination,
            call_site,
        } => SnapshotInstruction::SendSuper {
            selector: *selector,
            argument_base: *argument_base,
            argument_sources: argument_sources.clone(),
            destination: *destination,
            call_site: *call_site,
        },
        Instruction::SendMirror {
            primitive,
            mirror_register,
            subject_register,
            argument_base,
            argument_sources,
            destination,
            failure,
        } => SnapshotInstruction::SendMirror {
            primitive: *primitive,
            mirror_register: *mirror_register,
            subject_register: *subject_register,
            argument_base: *argument_base,
            argument_sources: argument_sources.clone(),
            destination: *destination,
            failure: *failure,
        },
        Instruction::MakeArray {
            destination,
            element_registers,
        } => SnapshotInstruction::MakeArray {
            destination: *destination,
            element_registers: element_registers.clone(),
        },
        Instruction::MakeVector { destination, size } => SnapshotInstruction::MakeVector {
            destination: *destination,
            size: *size,
        },
        Instruction::LoadVector {
            destination,
            vector,
            index,
        } => SnapshotInstruction::LoadVector {
            destination: *destination,
            vector: *vector,
            index: *index,
        },
        Instruction::StoreVector {
            vector,
            index,
            source,
        } => SnapshotInstruction::StoreVector {
            vector: *vector,
            index: *index,
            source: *source,
        },
        Instruction::InitializeSlot { source, own_index } => SnapshotInstruction::InitializeSlot {
            source: *source,
            own_index: *own_index,
        },
        Instruction::ReturnSlotIfFilled { own_index } => SnapshotInstruction::ReturnSlotIfFilled {
            own_index: *own_index,
        },
        Instruction::MakeFuture {
            destination,
            closure,
        } => SnapshotInstruction::MakeFuture {
            destination: *destination,
            closure: *closure,
        },
        Instruction::ResolveFuture {
            destination,
            source,
        } => SnapshotInstruction::ResolveFuture {
            destination: *destination,
            source: *source,
        },
        Instruction::LoadOwnSlot {
            destination,
            own_index,
        } => SnapshotInstruction::LoadOwnSlot {
            destination: *destination,
            own_index: *own_index,
        },
        Instruction::InitializeSlotProvisionally { source, own_index } => {
            SnapshotInstruction::InitializeSlotProvisionally {
                source: *source,
                own_index: *own_index,
            }
        }
        Instruction::Return { register } => SnapshotInstruction::Return {
            register: *register,
        },
        Instruction::ImplicitReturn { register } => SnapshotInstruction::ImplicitReturn {
            register: *register,
        },
        Instruction::NonLocalReturn { register } => SnapshotInstruction::NonLocalReturn {
            register: *register,
        },
        Instruction::SignalNonBoolean { register } => SnapshotInstruction::SignalNonBoolean {
            register: *register,
        },
        Instruction::Jump { target } => SnapshotInstruction::Jump { target: *target },
        Instruction::JumpIfNotBoolean { register, target } => {
            SnapshotInstruction::JumpIfNotBoolean {
                register: *register,
                target: *target,
            }
        }
        Instruction::BranchIfFalse { register, target } => SnapshotInstruction::BranchIfFalse {
            register: *register,
            target: *target,
        },
        Instruction::BranchIfTrue { register, target } => SnapshotInstruction::BranchIfTrue {
            register: *register,
            target: *target,
        },
        Instruction::IntegerBinary {
            operation,
            destination,
            left,
            right,
            target,
        } => SnapshotInstruction::IntegerBinary {
            operation: *operation,
            destination: *destination,
            left: *left,
            right: *right,
            target: *target,
        },
        Instruction::AllocateInstance { destination } => SnapshotInstruction::AllocateInstance {
            destination: *destination,
        },
        Instruction::InitializeInstance {
            instance,
            destination,
        } => SnapshotInstruction::InitializeInstance {
            instance: *instance,
            destination: *destination,
        },
        Instruction::CheckAppliedClause { steps, selector } => {
            SnapshotInstruction::CheckAppliedClause {
                steps: *steps,
                selector: *selector,
            }
        }
        Instruction::InitializeSuperclass {
            steps,
            selector,
            argument_base,
            argument_sources,
            destination,
        } => SnapshotInstruction::InitializeSuperclass {
            steps: *steps,
            selector: *selector,
            argument_base: *argument_base,
            argument_sources: argument_sources.clone(),
            destination: *destination,
        },
        Instruction::InitializeAppliedLevel {
            steps,
            argument_base,
            argument_sources,
            destination,
        } => SnapshotInstruction::InitializeAppliedLevel {
            steps: *steps,
            argument_base: *argument_base,
            argument_sources: argument_sources.clone(),
            destination: *destination,
        },
    })
}

/// A body as the optimizing compiler reads it, taken at a safepoint.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Snapshot {
    pub(crate) code_id: CodeId,
    pub(crate) facts: BodyFacts,
    pub(crate) feedback_base: FeedbackBase,
    pub(crate) instructions: Vec<SnapshotInstruction>,
}

impl Snapshot {
    /// The snapshot of `code`, its sends' cells found from
    /// `feedback_base`; the body and its blocks are registered with `heap`
    /// if they were not yet.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the optimizing compiler, whose first caller this is, is built on it"
        )
    )]
    pub(crate) fn take(
        code: &Rc<CompiledCode>,
        heap: &mut Heap,
        feedback_base: FeedbackBase,
    ) -> Snapshot {
        Snapshot {
            code_id: heap.code_id(code),
            facts: BodyFacts::of(code),
            feedback_base,
            instructions: code
                .instructions()
                .iter()
                .map(|instruction| SnapshotInstruction::of(instruction, heap))
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use ns_syntax::Span;

    use super::{Snapshot, SnapshotInstruction};
    use crate::bytecode::{CodeKind, CompiledCode, Instruction, IntegerOperation};
    use crate::interpreter::inline_cache::FeedbackBase;
    use crate::interpreter::Interpreter;
    use crate::{Lowering, Symbol};

    fn body(kind: CodeKind, instructions: Vec<Instruction>, arguments: u16) -> Rc<CompiledCode> {
        Rc::new(CompiledCode::new(
            kind,
            instructions,
            arguments,
            arguments + 20,
            0,
            Vec::new(),
            Span::new(0, 0),
        ))
    }

    #[test]
    fn a_snapshot_crosses_threads() {
        fn assert_send<T: Send + 'static>() {}
        assert_send::<Snapshot>();
    }

    #[test]
    fn a_snapshot_holds_the_body_s_instructions_in_order() {
        let code = body(
            CodeKind::Method,
            vec![
                Instruction::LoadSelf { destination: 14 },
                Instruction::IntegerBinary {
                    operation: IntegerOperation::Add,
                    destination: 14,
                    left: 14,
                    right: 14,
                    target: 3,
                },
                Instruction::Return { register: 14 },
            ],
            0,
        );
        let mut interpreter = Interpreter::bootstrap_with(Lowering::Optimized);
        interpreter.enter(|mutator| {
            let heap = &mut mutator.heap;
            let snapshot = Snapshot::take(&code, heap, FeedbackBase::NOT_ASSIGNED);
            assert_eq!(snapshot.instructions.len(), code.instruction_count());
            assert!(matches!(
                snapshot.instructions[1],
                SnapshotInstruction::IntegerBinary {
                    operation: IntegerOperation::Add,
                    ..
                }
            ));
            assert_eq!(snapshot.facts.local_count, code.local_count);
            assert_eq!(snapshot.facts.instruction_count, 3);
            assert_eq!(snapshot.code_id, heap.code_id(&code));
        });
    }

    #[test]
    fn a_snapshot_names_a_closure_s_block_by_its_code_id() {
        let block = body(
            CodeKind::Block,
            vec![Instruction::ImplicitReturn { register: 0 }],
            1,
        );
        let code = body(
            CodeKind::Method,
            vec![
                Instruction::LoadSelf { destination: 14 },
                Instruction::MakeClosure {
                    destination: 15,
                    parameters: Rc::new(vec![Symbol::NEW]),
                    code: block.clone(),
                    copied: vec![14],
                },
                Instruction::Return { register: 15 },
            ],
            0,
        );
        let mut interpreter = Interpreter::bootstrap_with(Lowering::Optimized);
        interpreter.enter(|mutator| {
            let heap = &mut mutator.heap;
            let snapshot = Snapshot::take(&code, heap, FeedbackBase::NOT_ASSIGNED);
            let SnapshotInstruction::MakeClosure {
                block: block_id,
                parameter_count,
                copied,
                ..
            } = &snapshot.instructions[1]
            else {
                panic!("the second instruction makes a closure");
            };
            assert_eq!(*block_id, heap.code_id(&block));
            assert_eq!(*parameter_count, 1);
            assert_eq!(copied, &vec![14]);
        });
    }
}
