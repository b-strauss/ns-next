//! The Rust functions machine code calls through the leaf shim, none of
//! which runs Newspeak code, collects, reads another activation or
//! pushes one (`docs/design/execution.md`, "The foundation and the one
//! extension point", the leaf call): an instruction's own interpreter
//! step, the miss of a send's cell -- the one leaf that may compile and
//! install a body -- and the exit's materialization of the frames machine
//! code pushed. A template or a stub names one by its index in `table`
//! (`Leaf::index`).

use crate::bytecode::{CompactInstruction, Instruction};
use crate::heap::ActivationReference;
use crate::interpreter::inline_cache::{self, MissAnswer};
use crate::interpreter::locals::store_vector;
use crate::interpreter::Mutator;
use crate::vm::{load_literal_into, load_vector_into, make_array_into, make_vector_into};

use super::macro_assembler::LeafIndex;
use super::materialize;

/// The leaves, each by its index in `table`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Leaf {
    /// Runs one straight-line instruction through the interpreter's own
    /// effect function (`step`).
    Step = 0,
    /// Decides a send machine code's cell did not (`miss`).
    Miss = 1,
    /// Materializes the frame a return address lies in
    /// (`materialize::materialize_frame`).
    MaterializeFrame = 2,
    /// Materializes the frame an exit left, and makes the walk's frames
    /// the stack's (`materialize::materialize_innermost`).
    MaterializeInnermost = 3,
}

impl Leaf {
    /// The leaf's index in `table`, which a leaf call names.
    pub(crate) fn index(self) -> LeafIndex {
        LeafIndex(self as u32)
    }
}

/// The leaves, in `Leaf` order, each handed the mutator the code was
/// entered with.
pub(crate) fn table<'gc>() -> [ns_memory::Leaf<Mutator<'gc>>; 4] {
    // Each cast on its own: an array's elements meet at one type only by
    // the first element's, and a fn item generic over the mutator's brand
    // does not meet the pointer of one brand that way.
    [
        step as ns_memory::Leaf<Mutator<'gc>>,
        miss as ns_memory::Leaf<Mutator<'gc>>,
        materialize_frame as ns_memory::Leaf<Mutator<'gc>>,
        materialize_innermost as ns_memory::Leaf<Mutator<'gc>>,
    ]
}

/// What `step` answers when it ran the instruction.
const DONE: u64 = 0;
/// What `step` answers when it refused the instruction before changing
/// anything, so that the interpreter runs it again and raises the error
/// itself.
const REFUSED: u64 = 1;

/// The generic leaf: runs the straight-line instruction at
/// `arguments[1]`, the program counter, of the frame whose register 0
/// machine code holds at `arguments[0]` and whose body `arguments[2]`, an
/// address inside it, names (`materialize::leaf_frame`), through the
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
    let [frame_address, program_counter, code_address] = arguments;
    let Ok(program_counter) = u32::try_from(program_counter) else {
        unreachable!("internal: a program counter fits 32 bits");
    };
    let frame = materialize::leaf_frame(mutator, frame_address, code_address, program_counter);
    let answer = step_in(mutator, frame.activation, program_counter as usize);
    materialize::release_leaf_frame(mutator, frame);
    answer
}

/// `step`, in `activation`.
fn step_in(
    mutator: &mut Mutator<'_>,
    activation: ActivationReference,
    program_counter: usize,
) -> u64 {
    let register_base = mutator.heap.activation_register_base(activation);
    let code = mutator.heap.activation_code(activation);
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

/// The miss leaf, which the miss stub calls for a send whose call reached
/// it: the callee's register 0 at `arguments[0]`, where the send's frame
/// register points, and the send's return address `arguments[1]`, which
/// names the send and so the sending frame. Decides the send as the
/// interpreter's miss does, filling its cache, without acting on it
/// (`inline_cache::decide_at`), and answers the callee's machine entry to
/// jump to -- compiling the callee first if it is due -- or 0, at which the
/// stub refuses the call so that the interpreter runs the send.
fn miss(mutator: &mut Mutator<'_>, arguments: [u64; 3]) -> u64 {
    let [callee_address, return_address, _] = arguments;
    mutator.jit.statistics.miss_leaf_calls += 1;
    let Some((sender_address, program_counter)) =
        mutator.jit.sender_of(callee_address, return_address)
    else {
        unreachable!("internal: a send's call returns into an installed body");
    };
    let frame = materialize::leaf_frame(mutator, sender_address, return_address, program_counter);
    let answer = inline_cache::decide_at(mutator, frame.activation, program_counter);
    materialize::release_leaf_frame(mutator, frame);
    match answer {
        MissAnswer::Enter(address) => address,
        MissAnswer::Refuse => 0,
    }
}

/// The exit stub's call for each return address of a compiled frame,
/// `arguments[0]`, outermost first (`materialize::materialize_frame`).
fn materialize_frame(mutator: &mut Mutator<'_>, arguments: [u64; 3]) -> u64 {
    materialize::materialize_frame(mutator, arguments[0]);
    0
}

/// The exit stub's last call of its walk: the exit's code address, kind
/// and program counter (`materialize::materialize_innermost`).
fn materialize_innermost(mutator: &mut Mutator<'_>, arguments: [u64; 3]) -> u64 {
    let [code_address, kind, program_counter] = arguments;
    materialize::materialize_innermost(mutator, code_address, kind, program_counter);
    0
}
