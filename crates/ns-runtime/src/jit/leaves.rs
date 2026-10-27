//! The Rust functions machine code calls through the leaf shim, each an
//! instruction's own interpreter step that runs no Newspeak code, cannot
//! collect and reads no other activation (`docs/design/execution.md`,
//! "The foundation and the one extension point", the leaf call). A
//! template names one by its index in `table` (`Leaf::index`).

use crate::bytecode::{CompactInstruction, Instruction};
use crate::interpreter::locals::store_vector;
use crate::interpreter::Mutator;
use crate::vm::{load_literal_into, load_vector_into, make_array_into, make_vector_into};

use super::macro_assembler::LeafIndex;

/// The leaves, each by its index in `table`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Leaf {
    /// Runs one straight-line instruction through the interpreter's own
    /// effect function (`step`).
    Step = 0,
}

impl Leaf {
    /// The leaf's index in `table`, which a leaf call names.
    pub(crate) fn index(self) -> LeafIndex {
        LeafIndex(self as u32)
    }
}

/// The leaves, in `Leaf` order, each handed the mutator the code was
/// entered with.
pub(crate) fn table<'gc>() -> [ns_memory::Leaf<Mutator<'gc>>; 1] {
    [step]
}

/// What `step` answers when it ran the instruction.
const DONE: u64 = 0;
/// What `step` answers when it refused the instruction before changing
/// anything, so that the interpreter runs it again and raises the error
/// itself.
const REFUSED: u64 = 1;

/// The generic leaf: runs the straight-line instruction at
/// `arguments[1]`, the program counter, of the activation
/// `arguments[0]` names (`Heap::activation_word`) through the
/// interpreter's own effect function, the one `vm::run_stretch` calls, so
/// the two cannot drift. Answers `DONE`, or `REFUSED` for a vector access
/// the interpreter refuses -- which fails before it writes anything --
/// so that the exit after the call re-executes the instruction and the
/// interpreter raises the error, as no leaf can.
///
/// # Panics
///
/// On an instruction no lowering hands this leaf, which only a defect of
/// the JIT does; it aborts the process, as every panic in a leaf does.
fn step(mutator: &mut Mutator<'_>, arguments: [u64; 3]) -> u64 {
    let [activation, program_counter, _] = arguments;
    let activation = mutator.heap.machine_activation(activation);
    let register_base = mutator.heap.activation_register_base(activation);
    let code = mutator.heap.activation_code(activation);
    let Ok(program_counter) = usize::try_from(program_counter) else {
        unreachable!("internal: a program counter fits a usize");
    };
    let body = code.body();
    let done = match code.slot(program_counter) {
        CompactInstruction::LoadLiteral {
            destination,
            literal,
        } => load_literal_into(
            mutator,
            activation,
            register_base,
            u16::from(destination),
            body.literal(usize::from(literal)),
        )
        .is_ok(),
        CompactInstruction::MakeArray {
            destination,
            elements,
        } => {
            make_array_into(
                mutator,
                register_base,
                u16::from(destination),
                body.register_list(usize::from(elements)),
            );
            true
        }
        CompactInstruction::MakeVector { destination, size } => {
            make_vector_into(mutator, register_base, u16::from(destination), size);
            true
        }
        CompactInstruction::LoadVector {
            destination,
            vector,
            index,
        } => load_vector_into(
            mutator,
            register_base,
            u16::from(destination),
            u16::from(vector),
            index,
        )
        .is_ok(),
        CompactInstruction::StoreVector {
            vector,
            source,
            index,
        } => store_vector(
            &mut mutator.heap,
            register_base,
            u16::from(vector),
            index,
            u16::from(source),
        )
        .is_ok(),
        CompactInstruction::Wide { index } => match body.wide(index as usize) {
            Instruction::LoadLiteral {
                destination,
                literal,
            } => {
                load_literal_into(mutator, activation, register_base, *destination, literal).is_ok()
            }
            Instruction::MakeArray {
                destination,
                element_registers,
            } => {
                make_array_into(mutator, register_base, *destination, element_registers);
                true
            }
            Instruction::MakeVector { destination, size } => {
                make_vector_into(mutator, register_base, *destination, *size);
                true
            }
            Instruction::LoadVector {
                destination,
                vector,
                index,
            } => load_vector_into(mutator, register_base, *destination, *vector, *index).is_ok(),
            Instruction::StoreVector {
                vector,
                index,
                source,
            } => store_vector(&mut mutator.heap, register_base, *vector, *index, *source).is_ok(),
            other => unreachable!("internal: the generic leaf is handed {other:?}"),
        },
        other => unreachable!("internal: the generic leaf is handed {other:?}"),
    };
    if done {
        DONE
    } else {
        REFUSED
    }
}
