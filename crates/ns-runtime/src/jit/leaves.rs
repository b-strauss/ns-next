//! The Rust functions machine code calls through the leaf shim, none of
//! which runs Newspeak code, collects, reads another activation or
//! pushes one (`docs/design/execution.md`, "The foundation and the one
//! extension point", the leaf call): an instruction's own interpreter
//! step, the miss of a send's cell and the decision of an initializer
//! invocation -- the two leaves that may compile and install a body --
//! the leaf stub's run of a target that is no method with code, a
//! leaf-safe mirror send, an allocation past the inline bump's limit, the
//! write barrier's remembering of an old object, the cold check of an
//! own-slot instruction, and the exit's materialization of the frames
//! machine code pushed. A
//! template or a stub names one by its index in `table` (`Leaf::index`),
//! and every call through the table is counted once
//! (`JitStatistics::leaf_calls`).

use crate::builtins::primitive::MirrorPrimitive;
use crate::bytecode::{CompactInstruction, Instruction};
use crate::heap::{register_number, ActivationReference, ArgumentSlots, Word};
use crate::interpreter::inline_cache::{self, MissAnswer};
use crate::interpreter::locals::receiver_keeps_entered_level;
use crate::interpreter::mirror::{self, MachineMirrorAnswer};
use crate::interpreter::{Arguments, Mutator, Step, StepArguments};
use crate::jit;
use crate::mixin::DispatchId;
use crate::types::Value;
use crate::vm::{
    allocate_instance_into, argument_window, initialize_slot, load_enclosing_object_into,
    load_literal_into,
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
    /// Runs a method whose fast path is a leaf-safe mirror row
    /// (`mirror_method`).
    MirrorMethod = 6,
    /// Allocates an object the inline bump found no room for, as the
    /// interpreter's cold path does (`allocate`).
    Allocate = 7,
    /// Remembers an old object machine code stored a young one into, as
    /// the interpreter's write barrier does (`remember`).
    Remember = 8,
    /// The cold check of an own-slot instruction whose receiver's guard
    /// is not the one its frame recorded (`own_slots`).
    OwnSlots = 9,
    /// Decides the body an initializer invocation runs (`initializer`).
    Initializer = 10,
}

impl Leaf {
    /// The leaf's index in `table`, which a leaf call names.
    pub(crate) fn index(self) -> LeafIndex {
        LeafIndex(self as u32)
    }
}

/// `leaf`, behind the count every call through the table takes
/// (`JitStatistics::leaf_calls`): a function of its own per leaf, so that
/// the count is made in one place and a new leaf cannot miss it, and a
/// constant cast, so that the table is still promoted to a static.
macro_rules! counted {
    ($leaf:ident) => {{
        fn counted(mutator: &mut Mutator<'_>, arguments: [u64; 3]) -> u64 {
            mutator.jit.statistics.leaf_calls += 1;
            $leaf(mutator, arguments)
        }
        counted as ns_memory::Leaf<Mutator<'gc>>
    }};
}

/// The leaves, in `Leaf` order, each counted and handed the mutator the
/// code was entered with: one table for the process, a constant the
/// compiler promotes to a static, so that no entry builds it.
pub(crate) fn table<'gc>() -> &'gc [ns_memory::Leaf<Mutator<'gc>>; 11] {
    // Each cast on its own: an array's elements meet at one type only by
    // the first element's, and a fn item generic over the mutator's brand
    // does not meet the pointer of one brand that way.
    &[
        counted!(step),
        counted!(miss),
        counted!(materialize_frame),
        counted!(materialize_innermost),
        counted!(act),
        counted!(mirror),
        counted!(mirror_method),
        counted!(allocate),
        counted!(remember),
        counted!(own_slots),
        counted!(initializer),
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
/// the two cannot drift: an allocated literal's load, and an
/// `InitializeSlot` whose value §3.1.1's check of a value's slot
/// must look at (`jit::baseline`'s own-slot templates), an
/// `AllocateInstance` the inline bump does not make, a `CheckAppliedClause`,
/// and a body-less declaration's own `LoadEnclosingObject`. Answers `DONE`, or
/// `REFUSED` for an instruction the interpreter refuses -- which fails
/// before it writes anything -- so that the exit after the call
/// re-executes the instruction and the interpreter raises the error, or
/// forces the future a value's slot is handed (`vm`'s
/// `initialize_slot_refused`), as no leaf can.
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
        CompactInstruction::InitializeSlot { source, own_index } => initialize_slot(
            mutator,
            activation,
            register_base,
            u16::from(source),
            own_index,
        )
        .is_ok(),
        CompactInstruction::AllocateInstance { destination } => {
            allocate_instance_into(mutator, activation, register_base, u16::from(destination))
                .is_ok()
        }
        CompactInstruction::CheckAppliedClause { steps, selector } => mutator
            .check_applied_clause(activation, steps, selector)
            .is_ok(),
        CompactInstruction::LoadEnclosingObject { destination, depth } => {
            load_enclosing_object_into(
                mutator,
                activation,
                register_base,
                u16::from(destination),
                depth,
                code.is_declaration_code(),
            );
            true
        }
        CompactInstruction::Wide { index } => match body.wide(index as usize) {
            Instruction::LoadLiteral {
                destination,
                literal,
            } => {
                load_literal_into(mutator, activation, register_base, *destination, literal).is_ok()
            }
            Instruction::InitializeSlot { source, own_index } => {
                initialize_slot(mutator, activation, register_base, *source, *own_index).is_ok()
            }
            Instruction::AllocateInstance { destination } => {
                allocate_instance_into(mutator, activation, register_base, *destination).is_ok()
            }
            Instruction::CheckAppliedClause { steps, selector } => mutator
                .check_applied_clause(activation, *steps, *selector)
                .is_ok(),
            Instruction::LoadEnclosingObject { destination, depth } => {
                load_enclosing_object_into(
                    mutator,
                    activation,
                    register_base,
                    *destination,
                    *depth,
                    code.is_declaration_code(),
                );
                true
            }
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

/// `Leaf::OwnSlots`: the cold half of `check_own_slots`
/// (`interpreter::locals`) for the own-slot instruction at
/// `arguments[1]`, the program counter, of the frame whose register 0
/// machine code holds at `arguments[0]` and whose body `arguments[2]`
/// names, as `step` finds it: whether its receiver, an object, still has
/// the slots of the level the frame was entered at, in which case the
/// receiver's guard is recorded in the frame, so that the accesses after
/// it take the template's hot path, and the leaf answers `DONE`. A
/// receiver without the level, an immediate -- whose own-slot access the
/// interpreter answers without a field -- and an object whose guard is
/// `NONE`, which the hot path never matches, change nothing and answer
/// `REFUSED`, so that the exit after the call re-executes the instruction
/// and the interpreter raises the error or answers as it does.
fn own_slots(mutator: &mut Mutator<'_>, arguments: [u64; 3]) -> u64 {
    let [frame_address, program_counter, code_address] = arguments;
    let Ok(program_counter) = u32::try_from(program_counter) else {
        unreachable!("internal: a program counter fits 32 bits");
    };
    let frame = materialize::leaf_frame(mutator, frame_address, code_address, program_counter);
    let receiver = mutator.heap.activation_receiver(frame.activation);
    // A receiver whose guard is `NONE` would take the cold check again at
    // the template's second compare, which never holds for it, and again.
    let kept = receiver.as_object().is_some()
        && mutator.heap.dispatch_id(receiver) != DispatchId::NONE
        && receiver_keeps_entered_level(mutator, frame.activation, receiver);
    materialize::release_leaf_frame(mutator, frame);
    if kept {
        DONE
    } else {
        REFUSED
    }
}

/// The initializer leaf, which an initializer invocation's template calls
/// (`jit::sends::initialize`): decides the instruction at `arguments[1]`,
/// the program counter, of the frame `arguments[0]` and `arguments[2]`
/// name, as for `step` -- `InitializeInstance`, `InitializeSuperclass` or
/// `InitializeAppliedLevel` -- through the interpreter's own decision, the
/// one `vm.rs`'s arm hands `send`, on the arguments the template placed
/// where the callee lies, and acts on nothing. For a body to run, it
/// counts the callee's entry as the miss leaf does, compiling and
/// installing it at its second (`jit::note_entry_by_call`), and answers
/// its machine entry, the receiver, the level and the body's kind handed
/// over in the machine words (`Heap::hand_machine_initializer`); for a
/// decision that runs no code, the stub answering its receiver, the
/// instance handed over as the receiver; and 0, having handed nothing
/// over, for an error, a body without machine code and a body taking
/// other arguments than the template placed, so that the exit after the
/// call re-executes the instruction and the interpreter raises the error
/// or pushes the body.
fn initializer(mutator: &mut Mutator<'_>, arguments: [u64; 3]) -> u64 {
    let [frame_address, program_counter, code_address] = arguments;
    let Ok(program_counter) = u32::try_from(program_counter) else {
        unreachable!("internal: a program counter fits 32 bits");
    };
    let frame = materialize::leaf_frame(mutator, frame_address, code_address, program_counter);
    let answer = initializer_in(mutator, frame.activation, program_counter as usize);
    materialize::release_leaf_frame(mutator, frame);
    answer
}

/// `initializer`, in `activation`.
fn initializer_in(
    mutator: &mut Mutator<'_>,
    activation: ActivationReference,
    program_counter: usize,
) -> u64 {
    let register_base = mutator.heap.activation_register_base(activation);
    let code = mutator.heap.activation_code(activation);
    let slots = |argument_base: u16, sources: &[u16]| {
        ArgumentSlots::new(register_base, argument_base, register_number(sources.len()))
    };
    let (decided, placed) = match code.instruction(program_counter) {
        Instruction::InitializeInstance { instance, .. } => {
            let instance = mutator.heap.register(register_base, instance);
            let arguments = ArgumentSlots::new(register_base, 0, code.argument_count);
            (
                mutator.initialize_instance(activation, instance, Arguments::Slots(arguments)),
                code.argument_count,
            )
        }
        Instruction::InitializeSuperclass {
            steps,
            selector,
            argument_base,
            argument_sources,
            ..
        } => {
            let arguments = slots(argument_base, &argument_sources);
            (
                mutator.decide_superclass_initializer(
                    activation,
                    steps,
                    selector,
                    Arguments::Slots(arguments),
                ),
                arguments.count(),
            )
        }
        Instruction::InitializeAppliedLevel {
            steps,
            argument_base,
            argument_sources,
            ..
        } => {
            let arguments = slots(argument_base, &argument_sources);
            (
                mutator.decide_applied_level_initializer(
                    activation,
                    steps,
                    Arguments::Slots(arguments),
                ),
                arguments.count(),
            )
        }
        other => unreachable!("internal: the initializer leaf is handed {other:?}"),
    };
    match decided {
        Ok(Step::Value(instance)) => {
            mutator.heap.hand_machine_receiver(instance);
            mutator.jit.receiver_stub_address()
        }
        Ok(Step::Initializer {
            receiver,
            level,
            arguments,
            body,
            code,
        }) => {
            // A passed level's initializer takes no arguments, whatever the
            // invocation placed: its frame lies on the slots all the same.
            let taken = match &arguments {
                StepArguments::Send => placed,
                StepArguments::Owned(values) if values.is_empty() => 0,
                StepArguments::Owned(_) | StepArguments::ControlFlow => return 0,
            };
            if taken != code.argument_count {
                return 0;
            }
            let base = mutator.caches.base_for(&code);
            jit::note_entry_by_call(&mut mutator.jit, &code, base, None);
            let Some(address) = jit::machine_entry_address(&mutator.jit, &code, base) else {
                return 0;
            };
            mutator.heap.hand_machine_initializer(receiver, level, body);
            address
        }
        _ => 0,
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
/// miss leaf's do, and `arguments[2]`, the address of the send's site's
/// own cell when the send hit it directly, a block's cell or a global
/// row when a stub found the entry, 0 after the miss leaf
/// (`Roles::send_cells`), its receiver handed over in the machine words.
/// Runs the
/// target the send's cell recorded as the interpreter's `act` runs it and
/// answers its value, or `Word::NO_VALUE_BITS`, at which the stub refuses
/// the call with nothing changed, so that the interpreter runs the send:
/// a slot accessor its site's own cell records straight from the entry
/// (`InlineCaches::site_slot_hit`), every other target as the send's
/// decision finds it (`inline_cache::act_at`).
fn act(mutator: &mut Mutator<'_>, arguments: [u64; 3]) -> u64 {
    let [callee_address, return_address, cell_address] = arguments;
    mutator.jit.statistics.act_leaf_calls += 1;
    if let Some(answer) = act_on_a_site_slot(mutator, callee_address, cell_address) {
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
/// a setter, the value in the send's one slot, at `callee_address`, as
/// `act` runs it -- its guard's check for a private or `super` one
/// included (`Mutator::slot_for_machine_code`) -- without a frame for the
/// sender, a look at the send or a decision of it. `None` for any other
/// hit.
fn act_on_a_site_slot(
    mutator: &mut Mutator<'_>,
    callee_address: u64,
    cell_address: u64,
) -> Option<u64> {
    let resolved = mutator.caches.site_slot_hit(cell_address)?;
    let receiver = mutator.heap.machine_callee_receiver();
    let setter = matches!(
        resolved,
        crate::interpreter::resolve::Resolved::Slot {
            kind: crate::mixin::AccessorKind::Setter,
            ..
        }
    );
    // A getter's call places no argument, so its slot is read only for a
    // setter's.
    let slot;
    let given: &[Value<'_>] = if setter {
        let Some(value) = mutator.heap.machine_frame_value(callee_address) else {
            return Some(Word::NO_VALUE_BITS);
        };
        slot = [value];
        &slot
    } else {
        &[]
    };
    let answer = match mutator.slot_for_machine_code(&resolved, receiver, Arguments::Values(&given))
    {
        Some(value) => {
            mutator.jit.statistics.site_slot_hits += 1;
            mutator.heap.machine_word_of(value)
        }
        None => Word::NO_VALUE_BITS,
    };
    Some(answer)
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
            let Some(read) = mutator
                .heap
                .machine_frame_value(callee_address + position * 8)
            else {
                return Word::NO_VALUE_BITS;
            };
            *value = read;
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
/// instruction and the interpreter runs it. Arguments that are not
/// consecutive registers are placed into the send's argument slots
/// before the row's leaf runs, as the interpreter places them, so a
/// decline or a refusal may leave those slots written: nothing reads a
/// send's slots after it but the send itself, which the interpreter
/// re-executes, so the refusal still changed nothing the program sees.
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

/// The allocation leaf, which a template's slow path calls when the
/// inline bump found eden's cursor at its limit
/// (`MacroAssembler::bump_allocate`): places an object of `arguments[0]`
/// words after its header, its header word `arguments[1]` and its class
/// word `arguments[2]`, as the interpreter's cold path places one
/// (`Heap::allocate_for_machine_code`) -- in eden up to its end, asking
/// for the scavenge, or old and remembered once eden is spent -- and
/// answers its word, for the template to fill its fields. It never
/// collects: a scavenge it asks for runs at the next poll's exit, as the
/// interpreter's runs at its next safepoint.
fn allocate(mutator: &mut Mutator<'_>, arguments: [u64; 3]) -> u64 {
    let [word_count, header, class_word] = arguments;
    mutator
        .heap
        .allocate_for_machine_code(word_count, header, class_word)
}

/// The write barrier's leaf (`MacroAssembler::store_with_barrier`),
/// which a store calls only once it has found that the old object whose
/// word is `arguments[0]` came to name a young one and is not remembered
/// yet: adds it to the remembered set as the interpreter's barrier does
/// (`Heap::remember`), asking for the scavenge that tenures everything
/// once the set is past its limit, which runs at the next poll's exit.
/// Answers the holder's word again, so that the store finds its holder
/// where it left it on either path: a setter answers it.
fn remember(mutator: &mut Mutator<'_>, arguments: [u64; 3]) -> u64 {
    let [holder_word, _, _] = arguments;
    mutator.heap.remember_for_machine_code(holder_word);
    holder_word
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
