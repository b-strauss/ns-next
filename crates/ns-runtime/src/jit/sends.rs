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

use ns_assembler::{Condition, Label, Memory, Register, Width};

use crate::bytecode::{CallSite, CompiledCode, HEADER_REGISTERS};
use crate::heap::machine_words::MachineWord;
use crate::heap::{Heap, Immediate};
use crate::interpreter::inline_cache::cells::{ChainStart, CELL_BYTES, CELL_WORDS};
use crate::interpreter::inline_cache::{rule_tag, site_index, CacheRule, RULE_TAG_BITS};
use crate::mixin::DispatchId;
use crate::Symbol;

use super::baseline::{BodyAssembler, Lowering};
use super::macro_assembler::MacroAssembler;
use super::stubs::{BlockWord, ExitKind};

/// The bytes of a frame register.
const REGISTER_BYTES: i32 = 8;

/// A cell's words' displacements from its start (`Cell`'s order).
const CELL_GUARD: i32 = 0;
const CELL_ENTRY: i32 = REGISTER_BYTES;
const CELL_LEVEL: i32 = 2 * REGISTER_BYTES;
const CELL_CHAIN_START: i32 = 3 * REGISTER_BYTES;

const _: () = assert!(CELL_WORDS == 4 && CELL_BYTES == 32);

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
    let (depth, scratch, end) = (roles.depth_count, roles.frame_extent, roles.stack_end);
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
    // A debug build refuses a real request alone, never the hold of a stress
    // mode, which the polls of the frames it lets nest take
    // (`Heap::set_safepoint_requested`).
    #[cfg(debug_assertions)]
    {
        assembler.compare_immediate(scratch, 1);
        macro_assembler.exit_if(Condition::Equal, ExitKind::PushRefused, 0);
    }
    #[cfg(not(debug_assertions))]
    {
        assembler.compare_immediate(scratch, 0);
        macro_assembler.exit_if(Condition::NotEqual, ExitKind::PushRefused, 0);
    }
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
    // Only now, every check passed: the receiver and the current class the
    // call handed over.
    let receiver_place = Heap::receiver_register(code.argument_count);
    assembler.store(
        Width::Word64,
        frame_slot(roles.frame, receiver_place),
        roles.callee_receiver(),
    );
    assembler.store(
        Width::Word64,
        frame_slot(
            roles.frame,
            Heap::current_class_register(code.argument_count),
        ),
        roles.callee_class(),
    );
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
    let poison = roles.poison;
    let assembler = macro_assembler.assembler();
    assembler.move_immediate(poison, crate::heap::Word::DEAD_REGISTER_BITS);
    for register in first_scratch..window_end {
        assembler.store(Width::Word64, frame_slot(roles.frame, register), poison);
    }
    macro_assembler.store_frame_generation(code.argument_count);
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
    let count = roles.depth_count;
    assembler.load(Width::Word64, count, depth);
    assembler.add_immediate(count, -1);
    assembler.store(Width::Word64, depth, count);
    assembler.return_();
    Lowering::Transfer
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

/// Whom a send through its site's cell sends to, which decides how its
/// receiver is found and what its guard compares.
#[derive(Clone, Copy)]
enum Receiver {
    /// `SendPublic`'s, in a frame register: its guard compared.
    Register(u16),
    /// `SendLexical`'s, the enclosing object at the lexical depth: its
    /// guard compared at a site its cell says is not private, N's
    /// application word at a private one (`ChainStart::LexicalClass`).
    Lexical(u16),
    /// `SendSuper`'s, `self`: S's application word compared, S the
    /// superclass of the frame's current class.
    Super,
}

/// `SendPublic` through its site's cell
/// (`docs/design/execution.md`, "Sends in machine code"): the receiver's
/// guard compared with the cell's; on a match the callee's current class,
/// `level` superclass steps up the receiver's class, then a call of the
/// cell's entry address; otherwise a call of the polymorphic stub
/// (`BlockWord::PolymorphicStub`), which finds the site's entry for the
/// guard in its block or the global cache's mirror, or has the miss leaf
/// decide the send (`jit::targets`). The
/// frame register moves to the callee's register 0 on the send's slots
/// for the call and back after it, the receiver and the current class
/// travelling in the roles' `callee_receiver` and `callee_class`, and the
/// value the callee returns is stored into `destination`. A send whose
/// site is uncached -- the reference lowering's, or past its family's
/// range -- exits, and so does one whose arguments cannot be placed
/// without a register to spare.
pub(super) fn send_public(
    body: &mut BodyAssembler,
    selector: Symbol,
    receiver_register: u16,
    argument_base: u16,
    argument_sources: &[u16],
    destination: u16,
    call_site: CallSite,
) -> Lowering {
    send_through_cell(
        body,
        selector,
        Receiver::Register(receiver_register),
        argument_base,
        argument_sources,
        destination,
        call_site,
    )
}

/// `SendLexical` through its site's cell, as `inline_cache::send_lexical`
/// sends it (§5.7-§5.9): the receiver the enclosing object at `depth`
/// (`MacroAssembler::load_enclosing_object`), and the guard the cell's
/// `chain_start` chooses -- N's application word at a private site, N
/// found by `MacroAssembler::load_lexical_class`, and the enclosing
/// object's guard at every other, a vacant one included, whose guard
/// nothing matches, so that the miss leaf decides its first send as
/// `send_lexical_first` does. On a match the callee's current class is
/// `level` steps up from N, or from the enclosing object's class. A
/// body-less declaration's own code walks out from its declaration's
/// scope and runs its lexical sends uncached, so they exit, as does an
/// uncached site.
pub(super) fn send_lexical(
    body: &mut BodyAssembler,
    selector: Symbol,
    depth: u16,
    argument_base: u16,
    argument_sources: &[u16],
    destination: u16,
    call_site: CallSite,
) -> Lowering {
    if body.declaration_code {
        return Lowering::Exit;
    }
    send_through_cell(
        body,
        selector,
        Receiver::Lexical(depth),
        argument_base,
        argument_sources,
        destination,
        call_site,
    )
}

/// `SendSuper` through its site's cell, as `inline_cache::send_super`
/// sends it (§5.10): S, the superclass of the frame's current class
/// (`interpreter::sends::super_start`), its application word compared
/// with the cell's guard, and on a match the callee's current class
/// `level` steps up from S, the receiver `self`. A frame with no current
/// class, or whose current class has no superclass, exits before
/// anything is written, so that the interpreter raises the error; so does
/// an uncached site.
pub(super) fn send_super(
    body: &mut BodyAssembler,
    selector: Symbol,
    argument_base: u16,
    argument_sources: &[u16],
    destination: u16,
    call_site: CallSite,
) -> Lowering {
    send_through_cell(
        body,
        selector,
        Receiver::Super,
        argument_base,
        argument_sources,
        destination,
        call_site,
    )
}

/// What a send's miss hands the polymorphic stub besides the cell and the
/// guard (`jit::targets`): its key past the guard -- the selector's index
/// above the rule's tag -- in `send_key`, and in `callee_class` N or S.
#[derive(Clone, Copy)]
enum MissBlock {
    /// The key alone: `callee_class` already holds what the stub needs --
    /// S, a private site's N -- or nothing it reads, at a public site.
    Key(u64),
    /// A lexical site that is not private: its key, and N at `depth`,
    /// which the global cache's `Lexical` rule carries and which only the
    /// miss needs, so that a hit never computes it; `nil` where N does
    /// not exist.
    Lexical { depth: u16, key: u64 },
    /// A private lexical site whose N does not exist: `nil` for the guard,
    /// which no cell holds, and for N, and the key.
    NoLexicalClass(u64),
}

/// The send through its site's cell that `send_public`, `send_lexical`
/// and `send_super` lower: the receiver found -- and for `super` S --
/// before any argument is placed, so that an exit leaves the frame as it
/// was; the arguments placed; the guard `receiver` says compared with the
/// cell's; then the call of the cell's entry on a match whose level
/// exists, of the polymorphic stub otherwise, entered with what it needs
/// to find the send's entry itself (`MissBlock`): the cell's address, the
/// guard compared, the selector and the rule's tag, and N or S. A level
/// that does not exist is the miss of the site's own entry, whose cell
/// holds a guard, so that the stub goes to the miss leaf before it reads
/// anything but the cell.
fn send_through_cell(
    body: &mut BodyAssembler,
    selector: Symbol,
    receiver: Receiver,
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
    let writes_receiver =
        matches!(receiver, Receiver::Register(register) if placement.writes(register));
    let placement_registers = body.macro_assembler.roles().placement.len();
    if writes_receiver || (placement.moves.len() > placement_registers && placement.overlaps()) {
        return Lowering::Exit;
    }
    let (argument_count, program_counter) = (body.argument_count, body.program_counter);
    let macro_assembler = &mut body.macro_assembler;
    let roles = macro_assembler.roles();
    let (value, class) = (roles.callee_receiver(), roles.callee_class());
    let (guard, cells, level, entry, key) = (
        roles.send_guard,
        roles.send_cells,
        roles.send_level,
        roles.send_entry,
        roles.send_key,
    );
    let key_of = |rule: CacheRule| ((selector.index() as u64) << RULE_TAG_BITS) | rule_tag(rule);
    match receiver {
        Receiver::Register(register) => macro_assembler.load_frame_register(value, register),
        Receiver::Lexical(depth) => {
            macro_assembler.load_enclosing_object(value, argument_count, depth);
        }
        Receiver::Super => {
            macro_assembler.load_superclass(class, argument_count, program_counter);
            macro_assembler.load_receiver(value, argument_count);
        }
    }
    place(macro_assembler, &placement);
    let cells_word = macro_assembler.block_word(BlockWord::Cells);
    let polymorphic_stub = macro_assembler.block_word(BlockWord::PolymorphicStub);
    let cell_word = |displacement: i32| Memory {
        base: cells,
        displacement: cell + displacement,
    };
    let assembler = macro_assembler.assembler();
    let (call, hit, missed) = (assembler.label(), assembler.label(), assembler.label());
    assembler.load(Width::Word64, cells, cells_word);
    // The miss blocks, laid out after the hit.
    let mut misses: Vec<(Label, MissBlock)> = Vec::new();
    match receiver {
        Receiver::Register(_) => {
            let public = macro_assembler.assembler().label();
            misses.push((public, MissBlock::Key(key_of(CacheRule::Public))));
            macro_assembler.compare_receiver_guard(guard, value, cell_word(CELL_GUARD), public);
            macro_assembler.load_class(class, value);
        }
        Receiver::Lexical(depth) => {
            let assembler = macro_assembler.assembler();
            let (private, lexical, private_missed, no_lexical_class) = (
                assembler.label(),
                assembler.label(),
                assembler.label(),
                assembler.label(),
            );
            misses.push((
                lexical,
                MissBlock::Lexical {
                    depth,
                    key: key_of(CacheRule::Lexical(DispatchId::NONE)),
                },
            ));
            misses.push((
                private_missed,
                MissBlock::Key(key_of(CacheRule::LexicalPrivate)),
            ));
            misses.push((
                no_lexical_class,
                MissBlock::NoLexicalClass(key_of(CacheRule::LexicalPrivate)),
            ));
            assembler.load(Width::Word64, level, cell_word(CELL_CHAIN_START));
            assembler.compare_immediate(level, ChainStart::LexicalClass as i32);
            assembler.jump_if(Condition::Equal, private);
            macro_assembler.compare_receiver_guard(guard, value, cell_word(CELL_GUARD), lexical);
            macro_assembler.load_class(class, value);
            let assembler = macro_assembler.assembler();
            assembler.jump(hit);
            assembler.bind(private);
            macro_assembler.load_lexical_class(class, argument_count, depth, no_lexical_class);
            macro_assembler.compare_application_word(
                guard,
                class,
                cell_word(CELL_GUARD),
                private_missed,
            );
        }
        Receiver::Super => {
            let superclass = macro_assembler.assembler().label();
            misses.push((superclass, MissBlock::Key(key_of(CacheRule::Super))));
            macro_assembler.compare_application_word(
                guard,
                class,
                cell_word(CELL_GUARD),
                superclass,
            );
        }
    }
    let assembler = macro_assembler.assembler();
    let missed_own = assembler.label();
    assembler.bind(hit);
    assembler.load(Width::Word64, level, cell_word(CELL_LEVEL));
    macro_assembler.walk_superclasses(class, level, missed_own);
    let assembler = macro_assembler.assembler();
    assembler.load(Width::Word64, entry, cell_word(CELL_ENTRY));
    // The callee is entered with its site's own cell, which the leaf
    // stub's leaf runs a slot accessor straight from.
    assembler.add_immediate(cells, cell);
    macro_assembler.count_compiled_call();
    macro_assembler.assembler().jump(call);
    let nil = Immediate::NIL.bits();
    for (label, block) in misses {
        macro_assembler.assembler().bind(label);
        match block {
            MissBlock::Key(word) => macro_assembler.assembler().move_immediate(key, word),
            MissBlock::Lexical { depth, key: word } => {
                let (absent, found) = {
                    let assembler = macro_assembler.assembler();
                    (assembler.label(), assembler.label())
                };
                macro_assembler.load_lexical_class(class, argument_count, depth, absent);
                let assembler = macro_assembler.assembler();
                assembler.jump(found);
                assembler.bind(absent);
                assembler.move_immediate(class, nil);
                assembler.bind(found);
                assembler.move_immediate(key, word);
            }
            MissBlock::NoLexicalClass(word) => {
                let assembler = macro_assembler.assembler();
                assembler.move_immediate(guard, nil);
                assembler.move_immediate(class, nil);
                assembler.move_immediate(key, word);
            }
        }
        macro_assembler.assembler().jump(missed);
    }
    let assembler = macro_assembler.assembler();
    // A level the chain of the site's own entry lacks: that entry's cell
    // holds a guard, so the stub goes to the miss leaf before it reads
    // the key.
    assembler.bind(missed_own);
    assembler.move_immediate(key, 0);
    assembler.bind(missed);
    assembler.add_immediate(cells, cell);
    assembler.load(Width::Word64, entry, polymorphic_stub);
    let slots = i32::from(argument_base) * REGISTER_BYTES;
    assembler.bind(call);
    assembler.add_immediate(roles.frame, slots);
    body.call_send(entry);
    let macro_assembler = &mut body.macro_assembler;
    macro_assembler
        .assembler()
        .add_immediate(roles.frame, -slots);
    #[cfg(debug_assertions)]
    give_back_dead_registers(body, argument_base + count);
    #[cfg(not(debug_assertions))]
    let _ = count;
    let macro_assembler = &mut body.macro_assembler;
    macro_assembler.store_frame_register(destination, roles.leaf_result);
    Lowering::Transfer
}

/// A debug build's poison over the registers a callee that lay on the
/// send's slots gives back, its header's register on: the dead register's
/// word, as the interpreter's pop writes it (`Heap::pop_activation`'s
/// `FrameStack::give_back_dead_registers`, from the callee's header to
/// the top it was pushed at), so that a read of one is refused. The
/// sender's own window is what its code can read; past it lie the
/// registers of a frame below, which its own callee's return, compiled
/// or popped by the loop, poisons in turn before that frame runs again.
/// The value register is left alone.
#[cfg(debug_assertions)]
fn give_back_dead_registers(body: &mut BodyAssembler, callee_header: u16) {
    let window_end = body.window_end;
    let macro_assembler = &mut body.macro_assembler;
    let roles = macro_assembler.roles();
    let poison = roles.poison;
    let assembler = macro_assembler.assembler();
    assembler.move_immediate(poison, crate::heap::Word::DEAD_REGISTER_BITS);
    for register in callee_header..window_end {
        assembler.store(Width::Word64, frame_slot(roles.frame, register), poison);
    }
}

/// Moves each source of `placement` into its slot: every source loaded
/// before any slot is stored when the moves fit the placement registers,
/// one after another otherwise, which `send_public` allows only where no
/// slot written is a source still to be read.
fn place(macro_assembler: &mut MacroAssembler, placement: &Placement) {
    let registers = macro_assembler.roles().placement;
    if placement.moves.len() <= registers.len() {
        for ((source, _), register) in placement.moves.iter().zip(registers) {
            macro_assembler.load_frame_register(register, *source);
        }
        for ((_, slot), register) in placement.moves.iter().zip(registers) {
            macro_assembler.store_frame_register(*slot, register);
        }
    } else {
        let [register, ..] = registers;
        for (source, slot) in &placement.moves {
            macro_assembler.load_frame_register(register, *source);
            macro_assembler.store_frame_register(*slot, register);
        }
    }
}
