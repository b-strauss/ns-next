//! The Rust functions machine code calls through the leaf shim, none of
//! which runs Newspeak code, collects, reads another activation or
//! pushes one (`docs/design/execution.md`, "The foundation and the one
//! extension point", the leaf call): an instruction's own interpreter
//! step, the miss of a send's cell -- the one leaf that may compile and
//! install a body -- the leaf stub's run of a target that is no method
//! with code, a leaf-safe mirror send, and the exit's materialization of
//! the frames machine code pushed. A template or a stub names one by its index in `table`
//! (`Leaf::index`).

use crate::builtins::primitive::MirrorPrimitive;
use crate::bytecode::{CompactInstruction, Instruction};
use crate::heap::{register_number, ActivationReference, ArgumentSlots, Word};
use crate::interpreter::inline_cache::{self, MissAnswer};
use crate::interpreter::locals::store_vector;
use crate::interpreter::mirror::{self, MachineMirrorAnswer};
use crate::interpreter::{Arguments, Mutator};
use crate::types::Value;
use crate::vm::{
    argument_window, load_literal_into, load_vector_into, make_array_into, make_vector_into,
};

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
    /// Runs the target a send's cell named the leaf stub for, as `act`
    /// runs it (`act`).
    Act = 4,
    /// Runs a `SendMirror` of a leaf-safe row (`mirror`).
    Mirror = 5,
    /// Stores a setter's argument into its receiver's field
    /// (`store_field`).
    StoreField = 6,
    /// Runs a method whose fast path is a leaf-safe mirror row
    /// (`mirror_method`).
    MirrorMethod = 7,
}

impl Leaf {
    /// The leaf's index in `table`, which a leaf call names.
    pub(crate) fn index(self) -> LeafIndex {
        LeafIndex(self as u32)
    }
}

/// The leaves, in `Leaf` order, each handed the mutator the code was
/// entered with.
pub(crate) fn table<'gc>() -> [ns_memory::Leaf<Mutator<'gc>>; 8] {
    // Each cast on its own: an array's elements meet at one type only by
    // the first element's, and a fn item generic over the mutator's brand
    // does not meet the pointer of one brand that way.
    [
        step as ns_memory::Leaf<Mutator<'gc>>,
        miss as ns_memory::Leaf<Mutator<'gc>>,
        materialize_frame as ns_memory::Leaf<Mutator<'gc>>,
        materialize_innermost as ns_memory::Leaf<Mutator<'gc>>,
        act as ns_memory::Leaf<Mutator<'gc>>,
        mirror as ns_memory::Leaf<Mutator<'gc>>,
        store_field as ns_memory::Leaf<Mutator<'gc>>,
        mirror_method as ns_memory::Leaf<Mutator<'gc>>,
    ]
}

/// What `step` answers when it ran the instruction.
const DONE: u64 = 0;
/// What `step` answers when it refused the instruction before changing
/// anything, so that the interpreter runs it again and raises the error
/// itself.
const REFUSED: u64 = 1;
/// What `mirror` answers when the row's leaf declined, so that the send
/// goes on at its failure block.
pub(super) const FAILED: u64 = 2;

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

/// The leaf stub's leaf (`targets::assemble_accessor_stubs`), which a send
/// whose cell names the leaf stub calls: the callee's register 0 at
/// `arguments[0]`, the send's slots, the send's return address
/// `arguments[1]`, which names the send and so the sending frame, as the
/// miss leaf's do, and the address of the cell the send hit,
/// `arguments[2]`, its receiver handed over in the machine words. Runs the
/// target the send's cell recorded as the interpreter's `act` runs it and
/// answers its value, or `Word::NO_VALUE_BITS`, at which the stub refuses
/// the call with nothing changed, so that the interpreter runs the send:
/// a slot accessor its site's own cell records straight from the entry
/// (`InlineCaches::site_slot_hit`), every other target as the send's
/// decision finds it (`inline_cache::act_at`).
fn act(mutator: &mut Mutator<'_>, arguments: [u64; 3]) -> u64 {
    let [callee_address, return_address, cell_address] = arguments;
    mutator.jit.statistics.act_leaf_calls += 1;
    if let Some(answer) = act_on_a_site_slot(mutator, callee_address, return_address, cell_address)
    {
        return answer;
    }
    let Some((sender_address, program_counter)) =
        mutator.jit.sender_of(callee_address, return_address)
    else {
        unreachable!("internal: a send's call returns into an installed body");
    };
    let frame = materialize::leaf_frame(mutator, sender_address, return_address, program_counter);
    let answer = inline_cache::act_at(mutator, frame.activation, program_counter)
        .map_or(Word::NO_VALUE_BITS, |value| {
            mutator.heap.machine_word_of(value)
        });
    materialize::release_leaf_frame(mutator, frame);
    answer
}

/// `act` for a slot accessor the send's site's own cell recorded: the
/// accessor run on the receiver handed over in the machine words and, for
/// a setter, the value in the send's one slot, as `act` runs it -- its
/// guard's check for a private or `super` one included -- without a frame
/// for the sender or a decision of the send. `None` for any other hit.
fn act_on_a_site_slot(
    mutator: &mut Mutator<'_>,
    callee_address: u64,
    return_address: u64,
    cell_address: u64,
) -> Option<u64> {
    let site = mutator.jit.call_site(return_address)?;
    let resolved = mutator
        .caches
        .site_slot_hit(site.cache_index?, cell_address)?;
    let receiver = mutator.heap.machine_callee_receiver();
    // A getter's call places no argument, so its slot is read only for a
    // setter's.
    let slot;
    let given: &[Value<'_>] = if site.argument_count == 0 {
        &[]
    } else {
        slot = [mutator.heap.machine_frame_value(callee_address)];
        &slot
    };
    let answer = mutator
        .act_for_machine_code(resolved, receiver, site.selector, Arguments::Values(&given))
        .map_or(Word::NO_VALUE_BITS, |value| {
            mutator.heap.machine_word_of(value)
        });
    Some(answer)
}

/// The setter stub's leaf (`targets::assemble_accessor_stubs`): stores
/// the value `arguments[2]` into field `arguments[1]` of the receiver
/// `arguments[0]` through the heap's store accessor and its write barrier
/// (`Heap::instance_set_field`), and answers the receiver, as the setter
/// `Mutator::decide_slot_accessor` runs does; or, for a receiver without
/// that field, `Word::NO_VALUE_BITS`, having written nothing, so that the
/// stub refuses the call and the interpreter raises the error. A leaf of
/// its own, which reads nothing of the send: the cell decided the target,
/// and the stub's entry names the field.
fn store_field(mutator: &mut Mutator<'_>, arguments: [u64; 3]) -> u64 {
    let [receiver_bits, field, value_bits] = arguments;
    mutator.jit.statistics.act_leaf_calls += 1;
    let receiver = mutator.heap.machine_value(receiver_bits);
    let value = mutator.heap.machine_value(value_bits);
    let stored = receiver.as_object().is_some_and(|object| {
        usize::try_from(field)
            .is_ok_and(|field| mutator.heap.instance_set_field(object, field, value))
    });
    if stored {
        receiver_bits
    } else {
        Word::NO_VALUE_BITS
    }
}

/// The mirror method stub's leaf (`targets::assemble_accessor_stubs`),
/// which a send whose cell names the stub of mirror row `arguments[1]`
/// calls: the callee's register 0 at `arguments[0]`, the send's slots,
/// and the send's return address `arguments[2]`, which names the send, its
/// receiver handed over in the machine words. Runs the row's leaf as
/// `Mutator::act_for_machine_code` runs a method whose fast path it is
/// (`mirror::execute_method_for_machine_code`) and answers its value, or
/// `Word::NO_VALUE_BITS` for a decline, an error or anything else, having
/// changed nothing, so that the stub refuses the call and the interpreter
/// runs the send. Its own leaf rather than the leaf stub's, since the
/// cell already decided the target: nothing of the send's lookup is made
/// again.
fn mirror_method(mutator: &mut Mutator<'_>, arguments: [u64; 3]) -> u64 {
    let [callee_address, row, return_address] = arguments;
    mutator.jit.statistics.act_leaf_calls += 1;
    let Some(site) = mutator.jit.call_site(return_address) else {
        unreachable!("internal: a send's call returns into an installed body");
    };
    let Some(sender_address) = callee_address.checked_sub(u64::from(site.argument_base) * 8) else {
        unreachable!("internal: a callee lies on its send's argument slots");
    };
    let Ok(row) = u16::try_from(row) else {
        unreachable!("internal: a mirror row is numbered in 16 bits");
    };
    let row = MirrorPrimitive::from_index(row);
    let receiver = mutator.heap.machine_callee_receiver();
    // A send of a few arguments hands them over as values, read off its
    // slots, so that no frame is written for the sender.
    if usize::from(site.argument_count) <= HANDED_ARGUMENTS {
        let mut values = [Value::nil(); HANDED_ARGUMENTS];
        for (position, value) in (0..u64::from(site.argument_count)).zip(values.iter_mut()) {
            *value = mutator
                .heap
                .machine_frame_value(callee_address + position * 8);
        }
        let given: &[Value<'_>] = &values[..usize::from(site.argument_count)];
        return mirror::execute_method_for_machine_code(
            mutator,
            row,
            receiver,
            site.selector,
            Arguments::Values(&given),
        )
        .map_or(Word::NO_VALUE_BITS, |value| {
            mutator.heap.machine_word_of(value)
        });
    }
    let frame = materialize::leaf_frame(
        mutator,
        sender_address,
        return_address,
        site.program_counter,
    );
    let register_base = mutator.heap.activation_register_base(frame.activation);
    let slots = Arguments::Slots(ArgumentSlots::new(
        register_base,
        site.argument_base,
        site.argument_count,
    ));
    let answer =
        mirror::execute_method_for_machine_code(mutator, row, receiver, site.selector, slots)
            .map_or(Word::NO_VALUE_BITS, |value| {
                mutator.heap.machine_word_of(value)
            });
    materialize::release_leaf_frame(mutator, frame);
    answer
}

/// How many arguments the mirror method leaf reads off a send's slots as
/// values; a send of more lays a frame for its sender.
const HANDED_ARGUMENTS: usize = 4;

/// The leaf of a `SendMirror` of a leaf-safe row: the instruction at
/// `arguments[1]`, the program counter, of the frame `arguments[0]` and
/// `arguments[2]` name, as for `step`, run as the interpreter's
/// `vm::send_mirror` runs it -- its arguments placed by `argument_window`,
/// the leaf run by `mirror::send_inlined_for_machine_code` -- answering
/// `DONE` with the leaf's value in the instruction's destination,
/// `FAILED` with nothing written when the leaf declined, so that the code
/// goes on at the failure block, and `REFUSED` for anything else, the
/// destination unwritten, so that the exit after the call re-executes the
/// instruction and the interpreter runs it.
fn mirror(mutator: &mut Mutator<'_>, arguments: [u64; 3]) -> u64 {
    let [frame_address, program_counter, code_address] = arguments;
    let Ok(program_counter) = u32::try_from(program_counter) else {
        unreachable!("internal: a program counter fits 32 bits");
    };
    let frame = materialize::leaf_frame(mutator, frame_address, code_address, program_counter);
    let answer = mirror_in(mutator, frame.activation, program_counter as usize);
    materialize::release_leaf_frame(mutator, frame);
    answer
}

/// `mirror`, in `activation`.
fn mirror_in(
    mutator: &mut Mutator<'_>,
    activation: ActivationReference,
    program_counter: usize,
) -> u64 {
    let register_base = mutator.heap.activation_register_base(activation);
    let code = mutator.heap.activation_code(activation);
    let body = code.body();
    let (primitive, mirror, subject, argument_base, sources, destination) =
        match code.slot(program_counter) {
            CompactInstruction::SendMirror {
                destination,
                mirror,
                subject,
                primitive,
                site,
            } => {
                let site = body.mirror_site(usize::from(site));
                (
                    primitive,
                    u16::from(mirror),
                    u16::from(subject),
                    site.argument_base(),
                    site.arguments(),
                    u16::from(destination),
                )
            }
            CompactInstruction::Wide { index } => match body.wide(index as usize) {
                Instruction::SendMirror {
                    primitive,
                    mirror_register,
                    subject_register,
                    argument_base,
                    argument_sources,
                    destination,
                    ..
                } => (
                    *primitive,
                    *mirror_register,
                    *subject_register,
                    *argument_base,
                    argument_sources.as_slice(),
                    *destination,
                ),
                other => unreachable!("internal: the mirror leaf is handed {other:?}"),
            },
            other => unreachable!("internal: the mirror leaf is handed {other:?}"),
        };
    let mirror = mutator.heap.register(register_base, mirror);
    let subject = mutator.heap.register(register_base, subject);
    let window = argument_window(mutator, register_base, argument_base, sources);
    let arguments = Arguments::Slots(ArgumentSlots::new(
        register_base,
        window,
        register_number(sources.len()),
    ));
    match mirror::send_inlined_for_machine_code(mutator, primitive, mirror, subject, arguments) {
        MachineMirrorAnswer::Value(value) => {
            mutator.heap.set_register(register_base, destination, value);
            DONE
        }
        MachineMirrorAnswer::Failed => FAILED,
        MachineMirrorAnswer::Exit => REFUSED,
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
