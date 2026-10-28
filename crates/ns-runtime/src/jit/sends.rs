//! A send in machine code, and what a compiled call enters and leaves
//! through (`docs/design/execution.md`, "Sends in machine code"): the
//! call through a site's cell, the callee's prologue -- its machine entry,
//! the push's checks and the words of its frame the push writes -- and the
//! return by `ret`.
//!
//! Every value stays in its frame register across a send, so a compiled
//! frame's registers are the interpreter's at every send, and the callee
//! lies on the send's argument slots as the interpreter's push lays it:
//! its arguments are those slots, its header follows them, and the call
//! writes the header's first two words, the receiver and the current
//! class, which nothing can derive later. Every other header word is
//! written when an exit materializes the frame (`jit::materialize`).

use ns_assembler::x64::{R10, R12, R8, R9, RAX, RCX, RDI, RDX, RSI};
use ns_assembler::{Condition, Memory, Register, Width};

use crate::bytecode::{CallSite, CompiledCode, HEADER_REGISTERS};
use crate::heap::machine_words::MachineWord;
use crate::heap::Immediate;
use crate::interpreter::inline_cache::cells::{CELL_BYTES, CELL_WORDS};
use crate::interpreter::inline_cache::site_index;

use super::baseline::{BodyAssembler, Lowering};
use super::macro_assembler::MacroAssembler;
use super::stubs::{BlockWord, ExitKind};

/// The bytes of a frame register.
const REGISTER_BYTES: i32 = 8;

/// A cell's words' displacements from its start (`Cell`'s order).
const CELL_GUARD: i32 = 0;
const CELL_ENTRY: i32 = REGISTER_BYTES;
const CELL_LEVEL: i32 = 2 * REGISTER_BYTES;

const _: () = assert!(CELL_WORDS == 4 && CELL_BYTES == 32);

/// The registers an argument placement holds the moved values in, all
/// loaded before any is stored, so that no slot is written before every
/// source is read.
const PLACEMENT_REGISTERS: [Register; 6] = [RAX, RDX, R8, R9, R10, R12];

/// The callee's prologue, at its machine entry -- the body's first byte --
/// where a compiled call enters it, the frame register at the callee's
/// register 0 on the send's slots and the call's return address on top of
/// the native stack: the checks the interpreter's push makes, each of
/// which, failing, refuses the call before anything is written, so that
/// the caller re-executes the send and the interpreter decides what the
/// push does -- the depth limit's error, a growth of the activation
/// stack, the safepoint (`Mutator::push_on_slots`); then the count, and
/// the words of the frame the push writes but the header: the named
/// locals nil, and in a debug build the dead register's word in the
/// scratch registers and the flags word with the push's generation, the
/// rest of the header the exit's walk's (`jit::materialize`). The run
/// loop's push writes all of it itself and enters the body past the
/// prologue, at its first instruction.
///
/// The depth limit word is a bound, not the limit: at or past it the
/// interpreter decides, as it does while a `doesNotUnderstand:` handler
/// holds the reserve the word does not count (`MachineWord::DepthLimit`).
pub(super) fn prologue(body: &mut BodyAssembler, code: &CompiledCode) {
    let macro_assembler = &mut body.macro_assembler;
    let roles = macro_assembler.roles();
    let machine_words = roles.idiom_scratch;
    let (depth, scratch, end) = (RAX, RCX, RDX);
    let word = |name| MacroAssembler::machine_word(machine_words, name);
    let frame_end_bytes = (i32::from(code.argument_count)
        + i32::from(HEADER_REGISTERS)
        + register_number(code.registers_above_header()))
        * REGISTER_BYTES;
    let stack_base = macro_assembler.block_word(BlockWord::StackBase);
    macro_assembler.load_machine_words(machine_words);
    let assembler = macro_assembler.assembler();
    assembler.load(Width::Word64, depth, word(MachineWord::DepthCount));
    assembler.compare_memory(depth, word(MachineWord::DepthLimit));
    macro_assembler.exit_if(Condition::AboveOrEqual, ExitKind::PushRefused, 0);
    let assembler = macro_assembler.assembler();
    assembler.load(
        Width::Word64,
        scratch,
        word(MachineWord::SafepointRequested),
    );
    assembler.compare_immediate(scratch, 0);
    macro_assembler.exit_if(Condition::NotEqual, ExitKind::PushRefused, 0);
    let assembler = macro_assembler.assembler();
    assembler.move_register(scratch, roles.frame);
    assembler.load(Width::Word64, end, stack_base);
    assembler.subtract(scratch, end);
    assembler.add_immediate(scratch, frame_end_bytes);
    assembler.load(Width::Word64, end, word(MachineWord::OrdinaryStackEnd));
    assembler.shift_left(end, WORD_SHIFT);
    assembler.compare(scratch, end);
    macro_assembler.exit_if(Condition::Above, ExitKind::PushRefused, 0);
    let assembler = macro_assembler.assembler();
    assembler.add_immediate(depth, 1);
    assembler.store(Width::Word64, word(MachineWord::DepthCount), depth);
    let header_end = code.argument_count + HEADER_REGISTERS;
    for register in header_end..code.local_count.max(header_end) {
        let nil = i32::try_from(Immediate::NIL.bits()).expect("internal: nil is a small word");
        assembler.store_immediate32(frame_slot(roles.frame, register), nil);
    }
    #[cfg(debug_assertions)]
    debug_frame_words(body, code);
}

/// A debug build's words of a pushed frame the interpreter's push writes
/// and the walk does not: the dead register's word in every scratch
/// register (`heap.rs`'s `fill_temporaries`), and the flags word with the
/// generation the push takes from the machine words, so that a handle to
/// a frame since returned and pushed again is refused.
#[cfg(debug_assertions)]
fn debug_frame_words(body: &mut BodyAssembler, code: &CompiledCode) {
    let macro_assembler = &mut body.macro_assembler;
    let roles = macro_assembler.roles();
    let header_end = code.argument_count + HEADER_REGISTERS;
    let first_scratch = code.local_count.max(header_end);
    let window_end = header_end + u16::try_from(code.registers_above_header()).unwrap_or(u16::MAX);
    let assembler = macro_assembler.assembler();
    assembler.move_immediate(RAX, crate::heap::Word::DEAD_REGISTER_BITS);
    for register in first_scratch..window_end {
        assembler.store(Width::Word64, frame_slot(roles.frame, register), RAX);
    }
    let (generation, constant) = crate::heap::Heap::machine_flags(code.argument_count);
    let machine_words = roles.idiom_scratch;
    macro_assembler.load_machine_words(machine_words);
    let next = MacroAssembler::machine_word(machine_words, MachineWord::NextGeneration);
    let assembler = macro_assembler.assembler();
    assembler.load(Width::Word64, RAX, next);
    assembler.add_immediate(RAX, 1);
    assembler.and_immediate(RAX, generation.mask);
    assembler.store(Width::Word64, next, RAX);
    assembler.shift_left(RAX, generation.shift);
    assembler.move_immediate(RCX, constant);
    assembler.add(RAX, RCX);
    assembler.store(
        Width::Word64,
        frame_slot(roles.frame, code.argument_count + generation.flags_register),
        RAX,
    );
}

/// How far an index into the machine words is shifted to be a byte
/// offset.
const WORD_SHIFT: u8 = 3;

/// A register count as the `i32` a displacement is computed in: every
/// count is numbered in 16 bits.
fn register_number(count: usize) -> i32 {
    i32::try_from(count).expect("internal: a body's registers are numbered in 16 bits")
}

/// The frame's register `register`.
fn frame_slot(frame: Register, register: u16) -> Memory {
    Memory {
        base: frame,
        displacement: i32::from(register) * REGISTER_BYTES,
    }
}

/// `Return` and `ImplicitReturn`: the frame's `register` into the value
/// register, the count taken back, and `ret` to the caller's send, which
/// stores the value -- or to the entry stub, for the frame the run loop
/// entered, whose pop the run loop makes (`stubs::assemble_entry_stub`).
/// A frame pushed by compiled code since the last exit delivers its value
/// and was reified by nothing, since only its own code could have and no
/// template reifies a frame, so the return tests neither.
pub(super) fn return_from(body: &mut BodyAssembler, register: u16) -> Lowering {
    let macro_assembler = &mut body.macro_assembler;
    let roles = macro_assembler.roles();
    macro_assembler.load_frame_register(roles.leaf_result, register);
    let machine_words = roles.idiom_scratch;
    macro_assembler.load_machine_words(machine_words);
    let depth = MacroAssembler::machine_word(machine_words, MachineWord::DepthCount);
    let assembler = macro_assembler.assembler();
    assembler.load(Width::Word64, RCX, depth);
    assembler.add_immediate(RCX, -1);
    assembler.store(Width::Word64, depth, RCX);
    assembler.return_();
    Lowering::Template
}

/// The argument placement of a send whose sources are not all its slots:
/// each source moved into its slot.
struct Placement {
    moves: Vec<(u16, u16)>,
}

impl Placement {
    /// The moves of `sources` into the slots from `argument_base`.
    fn of(argument_base: u16, sources: &[u16]) -> Placement {
        let moves = (argument_base..)
            .zip(sources)
            .filter(|(slot, source)| *slot != **source)
            .map(|(slot, source)| (*source, slot))
            .collect();
        Placement { moves }
    }

    /// Whether a slot a move writes is a source another move reads, so
    /// that the moves cannot be made one after another.
    fn overlaps(&self) -> bool {
        self.moves.iter().any(|(_, slot)| {
            self.moves
                .iter()
                .any(|(source, other_slot)| source == slot && other_slot != slot)
        })
    }

    /// Whether a move writes `register`.
    fn writes(&self, register: u16) -> bool {
        self.moves.iter().any(|(_, slot)| *slot == register)
    }
}

/// `SendPublic` through its site's cell
/// (`docs/design/execution.md`, "Sends in machine code"): the receiver's
/// guard compared with the cell's; on a match the callee's current class,
/// `level` superclass steps up the receiver's class, and the receiver
/// written into the callee's header, then a call of the cell's entry
/// address; otherwise a call of the polymorphic stub
/// (`BlockWord::PolymorphicStub`), which decides the send itself. The
/// frame register moves to the callee's register 0 on the send's slots
/// for the call and back after it, and the value the callee returns is
/// stored into `destination`. A send whose site is uncached -- the
/// reference lowering's, or past its family's range -- exits, and so does
/// one whose arguments cannot be placed without a register to spare.
pub(super) fn send_public(
    body: &mut BodyAssembler,
    receiver_register: u16,
    argument_base: u16,
    argument_sources: &[u16],
    destination: u16,
    call_site: CallSite,
) -> Lowering {
    let Some(index) = site_index(body.feedback_base, body.family_size, call_site) else {
        return Lowering::Exit;
    };
    let Ok(cell) = index
        .checked_mul(CELL_BYTES)
        .map_or(Err(()), |bytes| i32::try_from(bytes).map_err(|_| ()))
    else {
        return Lowering::Exit;
    };
    let Ok(count) = u16::try_from(argument_sources.len()) else {
        return Lowering::Exit;
    };
    let placement = Placement::of(argument_base, argument_sources);
    if placement.writes(receiver_register)
        || (placement.moves.len() > PLACEMENT_REGISTERS.len() && placement.overlaps())
    {
        return Lowering::Exit;
    }
    let (value, guard, cells, class, level, entry) = (RSI, RDI, R8, R9, R10, RCX);
    let callee_header = argument_base + count;
    let macro_assembler = &mut body.macro_assembler;
    let roles = macro_assembler.roles();
    macro_assembler.load_frame_register(value, receiver_register);
    place(macro_assembler, &placement);
    macro_assembler.load_guard(guard, value);
    let cells_word = macro_assembler.block_word(BlockWord::Cells);
    let polymorphic_stub = macro_assembler.block_word(BlockWord::PolymorphicStub);
    let cell_word = |displacement: i32| Memory {
        base: cells,
        displacement: cell + displacement,
    };
    let assembler = macro_assembler.assembler();
    let call = assembler.label();
    assembler.load(Width::Word64, cells, cells_word);
    assembler.load(Width::Word64, entry, polymorphic_stub);
    assembler.compare_memory(guard, cell_word(CELL_GUARD));
    assembler.jump_if(Condition::NotEqual, call);
    macro_assembler.load_class(class, value);
    macro_assembler
        .assembler()
        .load(Width::Word64, level, cell_word(CELL_LEVEL));
    macro_assembler.walk_superclasses(class, level, call);
    let assembler = macro_assembler.assembler();
    assembler.store(Width::Word64, frame_slot(roles.frame, callee_header), value);
    assembler.store(
        Width::Word64,
        frame_slot(roles.frame, callee_header + 1),
        class,
    );
    assembler.load(Width::Word64, entry, cell_word(CELL_ENTRY));
    macro_assembler.count_compiled_call();
    let slots = i32::from(argument_base) * REGISTER_BYTES;
    let assembler = macro_assembler.assembler();
    assembler.bind(call);
    assembler.add_immediate(roles.frame, slots);
    body.call_send(entry);
    let macro_assembler = &mut body.macro_assembler;
    macro_assembler
        .assembler()
        .add_immediate(roles.frame, -slots);
    macro_assembler.store_frame_register(destination, roles.leaf_result);
    Lowering::Template
}

/// Moves each source of `placement` into its slot: every source loaded
/// before any slot is stored when the moves fit the placement registers,
/// one after another otherwise, which `send_public` allows only where no
/// slot written is a source still to be read.
fn place(macro_assembler: &mut MacroAssembler, placement: &Placement) {
    if placement.moves.len() <= PLACEMENT_REGISTERS.len() {
        for ((source, _), register) in placement.moves.iter().zip(PLACEMENT_REGISTERS) {
            macro_assembler.load_frame_register(register, *source);
        }
        for ((_, slot), register) in placement.moves.iter().zip(PLACEMENT_REGISTERS) {
            macro_assembler.store_frame_register(*slot, register);
        }
    } else {
        for (source, slot) in &placement.moves {
            macro_assembler.load_frame_register(RAX, *source);
            macro_assembler.store_frame_register(*slot, RAX);
        }
    }
}
