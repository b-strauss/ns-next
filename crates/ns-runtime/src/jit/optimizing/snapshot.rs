//! The optimizing compiler's input: a snapshot of a body, taken once at the
//! run loop's safepoint (`docs/design/execution.md`, "Planned: the
//! optimizing JIT"). Everything the compiler reads of the body is here --
//! the body's facts and its instructions, a closure's block named by its
//! code id rather than held as an `Rc` -- so the snapshot is `Send` and a
//! compile that runs beside the interpreter reads nothing the interpreter
//! can change.
//!
//! The baseline reads `BodyFacts` in `sends::prologue`; its
//! `sends::call_slots` reads the encoded body's cached send first and the
//! snapshot form (`sends::call_slots_of`) only for the instructions that
//! are not one, a test holding the two paths to the same answer.

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
                .expect("internal: a body holds at most INSTRUCTION_LIMIT instructions"),
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

    /// The instruction as the interpreter's own form, which the register
    /// queries of `bytecode.rs` answer for; `None` for a `MakeClosure`,
    /// whose block the snapshot holds by id only.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the optimizing compiler's liveness, built on it, is its first reader"
        )
    )]
    fn unsnapshot(&self) -> Option<Instruction> {
        Some(match self {
            SnapshotInstruction::MakeClosure { .. } => return None,
            SnapshotInstruction::LoadLiteral {
                destination,
                literal,
            } => Instruction::LoadLiteral {
                destination: *destination,
                literal: literal.clone(),
            },
            SnapshotInstruction::LoadSelf { destination } => Instruction::LoadSelf {
                destination: *destination,
            },
            SnapshotInstruction::LoadEnclosingObject { destination, depth } => {
                Instruction::LoadEnclosingObject {
                    destination: *destination,
                    depth: *depth,
                }
            }
            SnapshotInstruction::Move {
                destination,
                source,
            } => Instruction::Move {
                destination: *destination,
                source: *source,
            },
            SnapshotInstruction::StoreLocal { source, slot } => Instruction::StoreLocal {
                source: *source,
                slot: *slot,
            },
            SnapshotInstruction::LoadOuterLocal {
                destination,
                depth,
                index,
            } => Instruction::LoadOuterLocal {
                destination: *destination,
                depth: *depth,
                index: *index,
            },
            SnapshotInstruction::StoreOuterLocal {
                source,
                depth,
                index,
            } => Instruction::StoreOuterLocal {
                source: *source,
                depth: *depth,
                index: *index,
            },
            SnapshotInstruction::SendLexical {
                depth,
                selector,
                argument_base,
                argument_sources,
                destination,
                call_site,
            } => Instruction::SendLexical {
                depth: *depth,
                selector: *selector,
                argument_base: *argument_base,
                argument_sources: argument_sources.clone(),
                destination: *destination,
                call_site: *call_site,
            },
            SnapshotInstruction::SendPublic {
                selector,
                receiver_register,
                argument_base,
                argument_sources,
                destination,
                call_site,
            } => Instruction::SendPublic {
                selector: *selector,
                receiver_register: *receiver_register,
                argument_base: *argument_base,
                argument_sources: argument_sources.clone(),
                destination: *destination,
                call_site: *call_site,
            },
            SnapshotInstruction::SendInlinable {
                selector,
                receiver_register,
                argument_base,
                argument_sources,
                destination,
                call_site,
            } => Instruction::SendInlinable {
                selector: *selector,
                receiver_register: *receiver_register,
                argument_base: *argument_base,
                argument_sources: argument_sources.clone(),
                destination: *destination,
                call_site: *call_site,
            },
            SnapshotInstruction::SendSuper {
                selector,
                argument_base,
                argument_sources,
                destination,
                call_site,
            } => Instruction::SendSuper {
                selector: *selector,
                argument_base: *argument_base,
                argument_sources: argument_sources.clone(),
                destination: *destination,
                call_site: *call_site,
            },
            SnapshotInstruction::SendMirror {
                primitive,
                mirror_register,
                subject_register,
                argument_base,
                argument_sources,
                destination,
                failure,
            } => Instruction::SendMirror {
                primitive: *primitive,
                mirror_register: *mirror_register,
                subject_register: *subject_register,
                argument_base: *argument_base,
                argument_sources: argument_sources.clone(),
                destination: *destination,
                failure: *failure,
            },
            SnapshotInstruction::MakeArray {
                destination,
                element_registers,
            } => Instruction::MakeArray {
                destination: *destination,
                element_registers: element_registers.clone(),
            },
            SnapshotInstruction::MakeVector { destination, size } => Instruction::MakeVector {
                destination: *destination,
                size: *size,
            },
            SnapshotInstruction::LoadVector {
                destination,
                vector,
                index,
            } => Instruction::LoadVector {
                destination: *destination,
                vector: *vector,
                index: *index,
            },
            SnapshotInstruction::StoreVector {
                vector,
                index,
                source,
            } => Instruction::StoreVector {
                vector: *vector,
                index: *index,
                source: *source,
            },
            SnapshotInstruction::InitializeSlot { source, own_index } => {
                Instruction::InitializeSlot {
                    source: *source,
                    own_index: *own_index,
                }
            }
            SnapshotInstruction::ReturnSlotIfFilled { own_index } => {
                Instruction::ReturnSlotIfFilled {
                    own_index: *own_index,
                }
            }
            SnapshotInstruction::MakeFuture {
                destination,
                closure,
            } => Instruction::MakeFuture {
                destination: *destination,
                closure: *closure,
            },
            SnapshotInstruction::ResolveFuture {
                destination,
                source,
            } => Instruction::ResolveFuture {
                destination: *destination,
                source: *source,
            },
            SnapshotInstruction::LoadOwnSlot {
                destination,
                own_index,
            } => Instruction::LoadOwnSlot {
                destination: *destination,
                own_index: *own_index,
            },
            SnapshotInstruction::InitializeSlotProvisionally { source, own_index } => {
                Instruction::InitializeSlotProvisionally {
                    source: *source,
                    own_index: *own_index,
                }
            }
            SnapshotInstruction::Return { register } => Instruction::Return {
                register: *register,
            },
            SnapshotInstruction::ImplicitReturn { register } => Instruction::ImplicitReturn {
                register: *register,
            },
            SnapshotInstruction::NonLocalReturn { register } => Instruction::NonLocalReturn {
                register: *register,
            },
            SnapshotInstruction::SignalNonBoolean { register } => Instruction::SignalNonBoolean {
                register: *register,
            },
            SnapshotInstruction::Jump { target } => Instruction::Jump { target: *target },
            SnapshotInstruction::JumpIfNotBoolean { register, target } => {
                Instruction::JumpIfNotBoolean {
                    register: *register,
                    target: *target,
                }
            }
            SnapshotInstruction::BranchIfFalse { register, target } => Instruction::BranchIfFalse {
                register: *register,
                target: *target,
            },
            SnapshotInstruction::BranchIfTrue { register, target } => Instruction::BranchIfTrue {
                register: *register,
                target: *target,
            },
            SnapshotInstruction::IntegerBinary {
                operation,
                destination,
                left,
                right,
                target,
            } => Instruction::IntegerBinary {
                operation: *operation,
                destination: *destination,
                left: *left,
                right: *right,
                target: *target,
            },
            SnapshotInstruction::AllocateInstance { destination } => {
                Instruction::AllocateInstance {
                    destination: *destination,
                }
            }
            SnapshotInstruction::InitializeInstance {
                instance,
                destination,
            } => Instruction::InitializeInstance {
                instance: *instance,
                destination: *destination,
            },
            SnapshotInstruction::CheckAppliedClause { steps, selector } => {
                Instruction::CheckAppliedClause {
                    steps: *steps,
                    selector: *selector,
                }
            }
            SnapshotInstruction::InitializeSuperclass {
                steps,
                selector,
                argument_base,
                argument_sources,
                destination,
            } => Instruction::InitializeSuperclass {
                steps: *steps,
                selector: *selector,
                argument_base: *argument_base,
                argument_sources: argument_sources.clone(),
                destination: *destination,
            },
            SnapshotInstruction::InitializeAppliedLevel {
                steps,
                argument_base,
                argument_sources,
                destination,
            } => Instruction::InitializeAppliedLevel {
                steps: *steps,
                argument_base: *argument_base,
                argument_sources: argument_sources.clone(),
                destination: *destination,
            },
        })
    }

    /// The registers the instruction reads (`bytecode::registers_read`).
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the optimizing compiler's liveness, built on it, is its first reader"
        )
    )]
    pub(crate) fn registers_read(&self) -> Vec<u16> {
        match self {
            SnapshotInstruction::MakeClosure { copied, .. } => copied.clone(),
            _ => crate::bytecode::registers_read(&self.unsnapshot().expect("not a closure"))
                .collect(),
        }
    }

    /// The register the instruction writes (`bytecode::register_written`).
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the optimizing compiler's liveness, built on it, is its first reader"
        )
    )]
    pub(crate) fn register_written(&self) -> Option<u16> {
        match self {
            SnapshotInstruction::MakeClosure { destination, .. } => Some(*destination),
            _ => crate::bytecode::register_written(&self.unsnapshot().expect("not a closure")),
        }
    }

    /// Where the instruction jumps to, if it jumps (`bytecode::jump_target`).
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the optimizing compiler's liveness, built on it, is its first reader"
        )
    )]
    pub(crate) fn jump_target(&self) -> Option<u32> {
        crate::bytecode::jump_target(&self.unsnapshot()?)
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

    /// One of every `Instruction`, each field a distinct number, so that a
    /// swap of two same-typed fields shows.
    fn every_instruction() -> Vec<Instruction> {
        use crate::builtins::primitive::MirrorPrimitive;
        use crate::bytecode::{CallSite, Literal};
        let site = CallSite { number: 3 };
        vec![
            Instruction::LoadLiteral {
                destination: 21,
                literal: Literal::SmallInteger(7),
            },
            Instruction::LoadSelf { destination: 22 },
            Instruction::LoadEnclosingObject {
                destination: 23,
                depth: 2,
            },
            Instruction::Move {
                destination: 24,
                source: 25,
            },
            Instruction::StoreLocal {
                source: 26,
                slot: 27,
            },
            Instruction::LoadOuterLocal {
                destination: 28,
                depth: 1,
                index: 29,
            },
            Instruction::StoreOuterLocal {
                source: 30,
                depth: 1,
                index: 31,
            },
            Instruction::SendLexical {
                depth: 2,
                selector: Symbol::NEW,
                argument_base: 32,
                argument_sources: vec![33, 34],
                destination: 35,
                call_site: site,
            },
            Instruction::SendPublic {
                selector: Symbol::RESOLVE,
                receiver_register: 36,
                argument_base: 37,
                argument_sources: vec![38],
                destination: 39,
                call_site: site,
            },
            Instruction::SendInlinable {
                selector: Symbol::NEW,
                receiver_register: 40,
                argument_base: 41,
                argument_sources: vec![42],
                destination: 43,
                call_site: site,
            },
            Instruction::SendSuper {
                selector: Symbol::RESOLVE,
                argument_base: 44,
                argument_sources: vec![45, 46],
                destination: 47,
                call_site: site,
            },
            Instruction::SendMirror {
                primitive: MirrorPrimitive::from_index(0),
                mirror_register: 48,
                subject_register: 49,
                argument_base: 50,
                argument_sources: vec![51],
                destination: 52,
                failure: 9,
            },
            Instruction::MakeArray {
                destination: 53,
                element_registers: vec![54, 55],
            },
            Instruction::MakeVector {
                destination: 56,
                size: 4,
            },
            Instruction::LoadVector {
                destination: 57,
                vector: 58,
                index: 59,
            },
            Instruction::StoreVector {
                vector: 60,
                index: 61,
                source: 62,
            },
            Instruction::InitializeSlot {
                source: 63,
                own_index: 5,
            },
            Instruction::ReturnSlotIfFilled { own_index: 6 },
            Instruction::MakeFuture {
                destination: 64,
                closure: 65,
            },
            Instruction::ResolveFuture {
                destination: 66,
                source: 67,
            },
            Instruction::LoadOwnSlot {
                destination: 68,
                own_index: 7,
            },
            Instruction::InitializeSlotProvisionally {
                source: 69,
                own_index: 8,
            },
            Instruction::Return { register: 70 },
            Instruction::ImplicitReturn { register: 71 },
            Instruction::NonLocalReturn { register: 72 },
            Instruction::SignalNonBoolean { register: 73 },
            Instruction::Jump { target: 10 },
            Instruction::JumpIfNotBoolean {
                register: 74,
                target: 11,
            },
            Instruction::BranchIfFalse {
                register: 75,
                target: 12,
            },
            Instruction::BranchIfTrue {
                register: 76,
                target: 13,
            },
            Instruction::IntegerBinary {
                operation: IntegerOperation::Subtract,
                destination: 77,
                left: 78,
                right: 79,
                target: 14,
            },
            Instruction::AllocateInstance { destination: 80 },
            Instruction::InitializeInstance {
                instance: 81,
                destination: 82,
            },
            Instruction::CheckAppliedClause {
                steps: 3,
                selector: Symbol::NEW,
            },
            Instruction::InitializeSuperclass {
                steps: 2,
                selector: Symbol::RESOLVE,
                argument_base: 83,
                argument_sources: vec![84, 85],
                destination: 86,
            },
            Instruction::InitializeAppliedLevel {
                steps: 1,
                argument_base: 87,
                argument_sources: vec![88],
                destination: 89,
            },
            Instruction::MakeClosure {
                destination: 90,
                parameters: Rc::new(vec![Symbol::NEW, Symbol::RESOLVE]),
                code: body(
                    CodeKind::Block,
                    vec![Instruction::ImplicitReturn { register: 0 }],
                    2,
                ),
                copied: vec![91, 92],
            },
        ]
    }

    #[test]
    fn the_list_holds_every_instruction_once() {
        let kinds: std::collections::HashSet<_> = every_instruction()
            .iter()
            .map(std::mem::discriminant)
            .collect();
        assert_eq!(
            kinds.len(),
            37,
            "the list names each variant of Instruction"
        );
    }

    #[test]
    fn a_snapshot_instruction_answers_what_its_instruction_does() {
        let mut interpreter = Interpreter::bootstrap_with(Lowering::Optimized);
        interpreter.enter(|mutator| {
            for instruction in every_instruction() {
                let snapshot = SnapshotInstruction::of(&instruction, &mut mutator.heap);
                assert_eq!(
                    snapshot.registers_read(),
                    crate::bytecode::registers_read(&instruction).collect::<Vec<_>>(),
                    "{instruction:?}"
                );
                assert_eq!(
                    snapshot.register_written(),
                    crate::bytecode::register_written(&instruction),
                    "{instruction:?}"
                );
                assert_eq!(
                    snapshot.jump_target(),
                    crate::bytecode::jump_target(&instruction),
                    "{instruction:?}"
                );
                assert_eq!(
                    snapshot.argument_slots(),
                    instruction.argument_slots(),
                    "{instruction:?}"
                );
                // The conversion loses nothing: back in the interpreter's
                // form it is the instruction, but for a closure's block.
                match snapshot.unsnapshot() {
                    Some(back) => assert_eq!(back, instruction),
                    None => assert!(matches!(instruction, Instruction::MakeClosure { .. })),
                }
            }
        });
    }

    #[test]
    fn the_two_ways_to_a_call_s_slots_agree() {
        use crate::bytecode::CallSite;
        use crate::jit::sends::{call_slots, call_slots_of};
        // A register past a byte takes the wide form of a send.
        let wide = |register: u16| register + 300;
        let mut instructions = every_instruction();
        instructions.push(Instruction::SendPublic {
            selector: Symbol::NEW,
            receiver_register: wide(1),
            argument_base: wide(2),
            argument_sources: vec![wide(3), wide(4)],
            destination: wide(5),
            call_site: CallSite { number: 4 },
        });
        instructions.push(Instruction::SendLexical {
            depth: 1,
            selector: Symbol::RESOLVE,
            argument_base: wide(6),
            argument_sources: Vec::new(),
            destination: wide(7),
            call_site: CallSite { number: 5 },
        });
        instructions.push(Instruction::SendSuper {
            selector: Symbol::NEW,
            argument_base: wide(8),
            argument_sources: vec![wide(9)],
            destination: wide(10),
            call_site: CallSite { number: 6 },
        });
        let (mut seen_wide, mut seen_narrow) = (false, false);
        for instruction in instructions {
            let code = CompiledCode::new(
                CodeKind::Method,
                vec![instruction.clone()],
                2,
                500,
                0,
                Vec::new(),
                Span::new(0, 0),
            );
            if code.cached_send(0).is_some() {
                if code.is_wide(0) {
                    seen_wide = true;
                } else {
                    seen_narrow = true;
                }
            }
            // `call_slots` reads a cached send off its slot and decodes
            // anything else; the snapshot form must name the same call.
            let encoded = call_slots(&code, 0)
                .map(|slots| (slots.argument_base, slots.argument_count, slots.selector));
            let snapshot = SnapshotInstruction::of_unclosed(&instruction).and_then(|snapshot| {
                call_slots_of(&super::BodyFacts::of(&code), &snapshot)
                    .map(|slots| (slots.argument_base, slots.argument_count, slots.selector))
            });
            assert_eq!(encoded, snapshot, "{instruction:?}");
        }
        assert!(
            seen_wide && seen_narrow,
            "a send of each form is among them"
        );
    }
}
