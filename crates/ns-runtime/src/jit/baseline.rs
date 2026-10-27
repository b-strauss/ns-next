//! The baseline compiler: a body's instructions, one after another, each
//! lowered on its own through the one exhaustive `match` over
//! `Instruction` (`lower`), the extension point every instruction is
//! added through (`docs/design/execution.md`, "The foundation and the one
//! extension point"). A baseline frame's registers are the interpreter's
//! frame words, so each instruction's code starts and ends with every
//! value in its slot, and any instruction can hand the frame to the
//! interpreter at its own program counter.

use crate::bytecode::{CompiledCode, Instruction};

use super::macro_assembler::{LeafIndex, MacroAssembler, Operand};
use super::stubs::ExitKind;
use super::{ProgramCounterMap, Refusal};

/// A compiled body: its machine code, and where each instruction's code
/// starts in it.
pub(crate) struct BaselineBody {
    pub(crate) code: ns_assembler::Assembled,
    pub(crate) map: ProgramCounterMap,
    /// Whether the first instruction lowers to an exit, so that entering
    /// the code would only leave it again at once (`jit::note_entry`).
    pub(crate) exits_at_entry: bool,
}

/// What `lower` did with one instruction, and what `compile` still emits
/// for it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Lowering {
    /// The arm emitted the instruction's template itself.
    #[expect(
        dead_code,
        reason = "the instructions without a send take their templates in the next stage"
    )]
    Template,
    /// A call of `leaf` with `arguments`, the instruction's own
    /// interpreter step.
    #[expect(
        dead_code,
        reason = "the instructions without a send take their leaves in the next stage"
    )]
    Leaf {
        leaf: LeafIndex,
        arguments: [Operand; 3],
    },
    /// An exit that hands the frame to the interpreter, which runs the
    /// instruction.
    Exit,
}

/// Compiles `code` with `macro_assembler`: each instruction in turn, at a
/// label whose offset the program-counter map records.
pub(crate) fn compile(
    code: &CompiledCode,
    mut macro_assembler: MacroAssembler,
) -> Result<BaselineBody, Refusal> {
    // A program counter of the map, and an exit's, is a `u32`; the bound
    // is the zone's layout's, which a later stage's tables index by
    // `u16`.
    let count =
        u16::try_from(code.instruction_count()).map_err(|_| Refusal::TooManyInstructions)?;
    let mut starts = Vec::with_capacity(usize::from(count));
    let mut exits_at_entry = false;
    for program_counter in 0..u32::from(count) {
        let start = macro_assembler.assembler().label();
        macro_assembler.assembler().bind(start);
        starts.push(start);
        macro_assembler.set_program_counter(program_counter);
        let instruction = code.instruction(program_counter as usize);
        // What every lowering ends with, not a second dispatch on the
        // instruction: the instruction was decided by `lower`'s arm.
        match lower(&instruction, &mut macro_assembler) {
            Lowering::Template => {}
            Lowering::Leaf { leaf, arguments } => macro_assembler.call_leaf(leaf, arguments),
            Lowering::Exit => {
                exits_at_entry |= program_counter == 0;
                macro_assembler.exit(ExitKind::Reexecute, program_counter);
            }
        }
    }
    let assembled = macro_assembler.finish().map_err(|_| Refusal::Unassembled)?;
    let starts = starts
        .into_iter()
        .map(|label| {
            assembled.label_offsets[label.index()]
                .expect("internal: every instruction's label is bound")
        })
        .collect();
    Ok(BaselineBody {
        code: assembled,
        map: ProgramCounterMap::new(starts),
        exits_at_entry,
    })
}

/// The lowering of `instruction`. One of three, by what the instruction
/// may do (`docs/design/execution.md`, "The foundation and the one
/// extension point"):
///
/// 1. **A template** -- machine code written against the macro
///    assembler's idioms and the register cache's operand interface,
///    never against an encoder -- for an instruction whose work machine
///    code does itself.
/// 2. **A leaf call** -- the instruction's own interpreter step, a Rust
///    function shared with `vm::run`, called through the leaf stub with
///    the frame's registers flushed -- only for an instruction that runs
///    no Newspeak code, cannot collect and reads no other activation.
/// 3. **An exit that re-executes it** -- the frame handed to the
///    interpreter at that instruction -- for every other one. Always
///    correct, so every instruction has a lowering from the day it is
///    added.
///
/// The match has an arm per instruction and no `_` arm, so a new
/// instruction cannot compile until it chooses one, and it is the only
/// dispatch on the instruction: a template's arm emits the template
/// through `macro_assembler` and answers `Lowering::Template`, a leaf's
/// answers the leaf and its operands, an exit's emits nothing.
fn lower(instruction: &Instruction, macro_assembler: &mut MacroAssembler) -> Lowering {
    // Read by the arms with a template, which come with the next stage.
    let _ = &macro_assembler;
    match instruction {
        Instruction::LoadLiteral { .. } => Lowering::Exit,
        Instruction::LoadSelf { .. } => Lowering::Exit,
        Instruction::LoadEnclosingObject { .. } => Lowering::Exit,
        Instruction::Move { .. } => Lowering::Exit,
        Instruction::StoreLocal { .. } => Lowering::Exit,
        Instruction::LoadOuterLocal { .. } => Lowering::Exit,
        Instruction::StoreOuterLocal { .. } => Lowering::Exit,
        Instruction::SendLexical { .. } => Lowering::Exit,
        Instruction::SendPublic { .. } => Lowering::Exit,
        Instruction::SendInlinable { .. } => Lowering::Exit,
        Instruction::SendSuper { .. } => Lowering::Exit,
        Instruction::SendMirror { .. } => Lowering::Exit,
        Instruction::MakeClosure { .. } => Lowering::Exit,
        Instruction::MakeArray { .. } => Lowering::Exit,
        Instruction::MakeVector { .. } => Lowering::Exit,
        Instruction::LoadVector { .. } => Lowering::Exit,
        Instruction::StoreVector { .. } => Lowering::Exit,
        Instruction::InitializeSlot { .. } => Lowering::Exit,
        Instruction::ReturnSlotIfFilled { .. } => Lowering::Exit,
        Instruction::MakeFuture { .. } => Lowering::Exit,
        Instruction::ResolveFuture { .. } => Lowering::Exit,
        Instruction::LoadOwnSlot { .. } => Lowering::Exit,
        Instruction::InitializeSlotProvisionally { .. } => Lowering::Exit,
        Instruction::Return { .. } => Lowering::Exit,
        Instruction::ImplicitReturn { .. } => Lowering::Exit,
        Instruction::NonLocalReturn { .. } => Lowering::Exit,
        Instruction::SignalNonBoolean { .. } => Lowering::Exit,
        Instruction::Jump { .. } => Lowering::Exit,
        Instruction::JumpIfNotBoolean { .. } => Lowering::Exit,
        Instruction::BranchIfFalse { .. } => Lowering::Exit,
        Instruction::BranchIfTrue { .. } => Lowering::Exit,
        Instruction::IntegerBinary { .. } => Lowering::Exit,
        Instruction::AllocateInstance { .. } => Lowering::Exit,
        Instruction::InitializeInstance { .. } => Lowering::Exit,
        Instruction::CheckAppliedClause { .. } => Lowering::Exit,
        Instruction::InitializeSuperclass { .. } => Lowering::Exit,
        Instruction::InitializeAppliedLevel { .. } => Lowering::Exit,
    }
}

#[cfg(test)]
mod tests {
    use crate::bytecode::{CodeKind, CompiledCode, Instruction, Literal};
    use crate::jit::macro_assembler::{Abi, MacroAssembler};
    use crate::jit::stubs::testing::Machine;
    use crate::jit::stubs::MachineExit;

    fn three_instructions() -> CompiledCode {
        CompiledCode::new(
            CodeKind::Method,
            vec![
                Instruction::LoadLiteral {
                    destination: 13,
                    literal: Literal::SmallInteger(3),
                },
                Instruction::Move {
                    destination: 14,
                    source: 13,
                },
                Instruction::Return { register: 14 },
            ],
            0,
            15,
            0,
            Vec::new(),
            ns_syntax::Span::new(0, 0),
        )
    }

    #[test]
    fn every_instruction_starts_at_its_own_offset_in_order() {
        let body = super::compile(&three_instructions(), MacroAssembler::new(Abi::host()))
            .expect("compiled");
        let starts: Vec<u32> = (0..3)
            .map(|program_counter| body.map.machine_offset(program_counter).expect("mapped"))
            .collect();
        assert_eq!(starts[0], 0);
        assert!(starts[0] < starts[1] && starts[1] < starts[2]);
        assert!(starts[2] < u32::try_from(body.code.bytes.len()).expect("small"));
        for (program_counter, start) in (0..).zip(&starts) {
            assert_eq!(body.map.program_counter_at(*start), Some(program_counter));
        }
    }

    #[test]
    fn a_compiled_body_exits_at_its_first_instruction() {
        let body = super::compile(&three_instructions(), MacroAssembler::new(Abi::host()))
            .expect("compiled");
        let mut machine = Machine::new();
        let entry = machine.install(&body.code);
        let mut frame = [0u64; 16];
        let (exit, _) = machine.run(entry, &mut frame, 0, &false, &mut (), &[]);
        assert_eq!(exit, MachineExit::Reexecute { program_counter: 0 });
        assert_eq!(frame, [0u64; 16], "the exit wrote nothing");
    }
}
