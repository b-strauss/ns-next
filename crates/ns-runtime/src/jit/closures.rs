//! `MakeClosure` in machine code (`docs/design/execution.md`, "Stage 4:
//! closures, non-local return, allocation and the barrier"): the frame
//! the closure keeps reified inline, as Cog marries a frame in machine
//! code, and the closure allocated and filled after it, as
//! `vm::make_closure_into` makes both.
//!
//! The frame object's and the closure's code ids are the heap's, which a
//! full collection frees once nothing names them and mints again for
//! other bodies, while the machine code that stores them lives on with its
//! mixin. So no code id is a constant of the code: each `MakeClosure` has
//! a closure site of its own, which records the two ids with the epoch of
//! the code table they were asked in, and its template compares that epoch
//! with the heap's (`MachineWord::CodeTableEpoch`) before it reads them,
//! calling the closure site leaf to ask them afresh whenever the two differ
//! -- at the site's first run, and after a full collection that freed a
//! code id.
//!
//! A site is three of its body's data words
//! (`MacroAssembler::take_body_words`), in the zone right after the body's
//! code, as Cog keeps a method's data with its code: it is installed with
//! the body, zeroed, stays wherever the body's code stays and takes its
//! room from the zone's, and the template reaches it relative to its own
//! instruction, through no word of the entry block. The leaf writes it
//! through the zone's one writer.

use ns_assembler::{Condition, Memory, Register, Width};

use crate::bytecode::{CompiledCode, Instruction};
use crate::heap::machine_words::MachineWord;
use crate::heap::Heap;
use crate::interpreter::Mutator;

use super::baseline::{BodyAssembler, BodyRole, Lowering};
use super::leaves::Leaf;
use super::macro_assembler::{MacroAssembler, ObjectSize, Operand};
use super::sends::{frame_slot, store_program_counter};
use super::stubs::BlockWord;

/// The words of a closure site, in order: the epoch of the code table its
/// ids were asked in, 0 for a site never filled -- no heap's epoch is 0 --
/// then the `SmallInteger` words of the code ids of the body the
/// `MakeClosure` lies in and of its block.
const SITE_EPOCH: usize = 0;
const SITE_FRAME_CODE: usize = 1;
const SITE_BLOCK_CODE: usize = 2;
const SITE_WORDS: usize = 3;

/// `Leaf::ClosureSite`: writes the closure site at `site_address`, of the
/// `MakeClosure` at `program_counter` of the body `code_address` lies in,
/// afresh -- the code table's current epoch, and the code ids of the body,
/// registered with its method when it is one's body, as the frame it runs
/// in names it (`materialize::code_id`), and of the block, as a closure of
/// it names it -- each registered if a full collection freed it.
pub(crate) fn refill_site(
    mutator: &mut Mutator<'_>,
    site_address: u64,
    program_counter: u64,
    code_address: u64,
) {
    let (Ok(site_address), Ok(program_counter), Ok(code_address)) = (
        usize::try_from(site_address),
        usize::try_from(program_counter),
        usize::try_from(code_address),
    ) else {
        unreachable!("internal: an address and a program counter fit the pointer width");
    };
    let Some(installed) = mutator.jit.body_of(code_address) else {
        unreachable!("internal: a closure site's leaf is called from an installed body");
    };
    let Instruction::MakeClosure { code: block, .. } = installed.code.instruction(program_counter)
    else {
        unreachable!("internal: a closure site's leaf is called at a `MakeClosure`");
    };
    let frame_code = super::materialize::code_id(
        &mut mutator.heap,
        &installed.code,
        installed.method.as_ref(),
    );
    let block_code = mutator.heap.code_id(&block);
    let words = [
        mutator.heap.code_table_epoch(),
        Heap::machine_count_word(frame_code.index()),
        Heap::machine_count_word(block_code.index()),
    ];
    mutator
        .jit
        .write_body_words(code_address, site_address, &words);
}

/// A word that fits a 32-bit immediate: a tag, `nil`, a Boolean, a small
/// count.
fn small(word: u64) -> i32 {
    i32::try_from(word).expect("internal: the word is a small constant")
}

/// The scratch registers the template takes, by role.
struct ClosureRegisters {
    /// The object allocated: the frame object, then the closure.
    object: Register,
    header: Register,
    class_word: Register,
    /// The object's address, which its fields are stored through.
    address: Register,
    value: Register,
    spare: Register,
}

impl ClosureRegisters {
    fn of(macro_assembler: &MacroAssembler) -> ClosureRegisters {
        let scratch = macro_assembler.roles().scratch;
        ClosureRegisters {
            object: scratch[0],
            header: scratch[1],
            class_word: scratch[2],
            address: scratch[3],
            value: scratch[4],
            spare: scratch[5],
        }
    }
}

/// `MakeClosure`: `vm::make_closure_into`'s work, with no exit. Its
/// program counter stored, as the interpreter stores it first; the frame
/// reified unless its reified word names a frame object already: a frame
/// object bump-allocated -- or placed by the allocation leaf past the
/// bump's limit -- and filled as `Heap::reify` fills it, from the frame's
/// header and the body's constants, its code id from the site, every
/// register nil, then the reified word; `retains_registers` set when the
/// block reads its creator's registers by reference; then the closure,
/// allocated the same way, of the block's code id, the frame object, the
/// body's feedback base and the copied values, and its word into
/// `destination`. Every store into the two objects is a store into an
/// object the template has just allocated, which takes no barrier
/// (`allocate_closure_from_registers`, `allocate_frame`); the reified
/// word and the flags are the frame's own. A debug build first checks
/// every copied register as the interpreter's read does, which exits
/// before anything is written.
pub(super) fn make_closure(
    body: &mut BodyAssembler,
    destination: u16,
    block: &CompiledCode,
    copied: &[u16],
) -> Lowering {
    let site = body.macro_assembler.take_body_words(SITE_WORDS);
    let registers = ClosureRegisters::of(&body.macro_assembler);
    #[cfg(debug_assertions)]
    for register in copied {
        body.macro_assembler
            .load_frame_register(registers.value, *register);
    }
    store_program_counter(body);
    check_site(body, site, &registers);
    reify_frame(body, site, &registers);
    if block.outer_reach > 0 {
        retain_registers(body, &registers);
    }
    allocate_closure(body, site, destination, copied, &registers);
    Lowering::Template
}

/// The site's check, the site the body words from `site` on: its epoch
/// against the heap's, and the closure site leaf's call out of line, after
/// which the check is made again, when the two differ.
fn check_site(body: &mut BodyAssembler, site: usize, registers: &ClosureRegisters) {
    let program_counter = body.program_counter;
    let macro_assembler = &mut body.macro_assembler;
    let leaf_result = macro_assembler.roles().leaf_result;
    let (epoch, machine_words) = (registers.value, registers.spare);
    let assembler = macro_assembler.assembler();
    let (check, refill) = (assembler.label(), assembler.label());
    assembler.bind(check);
    macro_assembler.load_body_word(epoch, site + SITE_EPOCH);
    macro_assembler.load_machine_words(machine_words);
    let assembler = macro_assembler.assembler();
    assembler.compare_memory(
        epoch,
        MacroAssembler::machine_word(machine_words, MachineWord::CodeTableEpoch),
    );
    assembler.jump_if(Condition::NotEqual, refill);
    macro_assembler.call_leaf_out_of_line(
        refill,
        Leaf::ClosureSite.index(),
        [
            Operand::BodyWord(site),
            Operand::Immediate(u64::from(program_counter)),
            Operand::CodeAddress,
        ],
        leaf_result,
        check,
    );
}

/// `Heap::reify` for the frame unless its reified word names a frame
/// object already.
fn reify_frame(body: &mut BodyAssembler, site: usize, registers: &ClosureRegisters) {
    let layout = Heap::machine_layout();
    let divorce = Heap::machine_divorce(body.argument_count);
    let frame = body.macro_assembler.roles().frame;
    let word_count = 1 + layout.frame_fixed_fields + body.frame_register_count;
    let ClosureRegisters {
        object,
        header,
        class_word,
        ..
    } = *registers;
    let macro_assembler = &mut body.macro_assembler;
    let assembler = macro_assembler.assembler();
    let (reified, slow, placed) = (assembler.label(), assembler.label(), assembler.label());
    assembler.load(
        Width::Word64,
        object,
        frame_slot(frame, divorce.reified_register),
    );
    assembler.compare_immediate(object, layout.nil_word);
    assembler.jump_if(Condition::NotEqual, reified);
    macro_assembler.load_header_and_class(
        (header, MachineWord::FrameHeader),
        (class_word, MachineWord::FrameClass),
        word_count,
    );
    macro_assembler.bump_allocate(
        object,
        ObjectSize::Constant(word_count),
        header,
        class_word,
        slow,
    );
    macro_assembler.assembler().bind(placed);
    fill_frame_fields(body, site, registers);
    let frame_register_count = body.frame_register_count;
    let macro_assembler = &mut body.macro_assembler;
    macro_assembler.nil_fill(object, layout.frame_fixed_fields, frame_register_count);
    macro_assembler.assembler().store(
        Width::Word64,
        frame_slot(frame, divorce.reified_register),
        object,
    );
    macro_assembler.assembler().bind(reified);
    macro_assembler.call_leaf_out_of_line(
        slow,
        Leaf::Allocate.index(),
        [
            Operand::Immediate(word_count as u64),
            Operand::Register(header),
            Operand::Register(class_word),
        ],
        object,
        placed,
    );
}

/// The fixed fields of the frame object in `registers.object`, as
/// `Heap::frame_fixed_words` writes a running activation's: no sender; a
/// block's frame's home, its closure's (`home_of_operand`), and none for
/// any other; the receiver and the current class from the header; the
/// program counter just stored; the body's code id; the kind and its
/// operand, as the frame's machine kind says them (`MachineKind`) -- a
/// method's, a factory's and a superclass clause's constants, an
/// initializer's from its flags word, a factory's and an initializer's
/// operand the current class, a block's and a clause's the header's
/// operand; continuable; married to the frame's own offset.
fn fill_frame_fields(body: &mut BodyAssembler, site: usize, registers: &ClosureRegisters) {
    let layout = Heap::machine_layout();
    let kinds = Heap::machine_frame_kinds();
    let argument_count = body.argument_count;
    let program_counter = body.program_counter;
    let role = body.role;
    let ClosureRegisters {
        object,
        address,
        value,
        spare,
        ..
    } = *registers;
    let macro_assembler = &mut body.macro_assembler;
    let roles = macro_assembler.roles();
    let frame = roles.frame;
    let field = |displacement: i32| Memory {
        base: address,
        displacement,
    };
    let assembler = macro_assembler.assembler();
    assembler.move_register(address, object);
    assembler.add(address, roles.object_space);
    assembler.store_immediate32(field(layout.frame_sender_displacement), layout.nil_word);
    let block_frame = role == BodyRole::Block;
    if block_frame {
        home_of_operand(macro_assembler, argument_count, value, spare);
        macro_assembler.assembler().store(
            Width::Word64,
            field(layout.frame_home_displacement),
            value,
        );
    } else {
        macro_assembler
            .assembler()
            .store_immediate32(field(layout.frame_home_displacement), layout.nil_word);
    }
    let receiver = frame_slot(frame, Heap::receiver_register(argument_count));
    let current_class = frame_slot(frame, Heap::current_class_register(argument_count));
    let operand = frame_slot(frame, Heap::operand_register(argument_count));
    let assembler = macro_assembler.assembler();
    assembler.load(Width::Word64, value, receiver);
    assembler.store(
        Width::Word64,
        field(layout.frame_receiver_displacement),
        value,
    );
    assembler.load(Width::Word64, value, current_class);
    assembler.store(
        Width::Word64,
        field(layout.frame_current_class_displacement),
        value,
    );
    assembler.store_immediate32(
        field(layout.frame_program_counter_displacement),
        Heap::machine_program_counter_word(program_counter),
    );
    macro_assembler.load_body_word(spare, site + SITE_FRAME_CODE);
    macro_assembler.assembler().store(
        Width::Word64,
        field(layout.frame_code_id_displacement),
        spare,
    );
    // The kind word into `value`, then where the operand is read from.
    let operand_source = match role {
        BodyRole::Method => None,
        BodyRole::Factory => {
            macro_assembler
                .assembler()
                .move_immediate(value, kinds.factory);
            Some(current_class)
        }
        BodyRole::Initializer => {
            initializer_kind(macro_assembler, argument_count, value, spare);
            Some(current_class)
        }
        BodyRole::Block => {
            macro_assembler
                .assembler()
                .move_immediate(value, kinds.closure);
            Some(operand)
        }
        BodyRole::EnteredOnly => {
            macro_assembler
                .assembler()
                .move_immediate(value, kinds.superclass_clause);
            Some(operand)
        }
    };
    let assembler = macro_assembler.assembler();
    let kind = field(layout.frame_kind_displacement);
    let kind_operand = field(layout.frame_kind_operand_displacement);
    match operand_source {
        None => {
            assembler.store_immediate32(kind, small(kinds.method));
            assembler.store_immediate32(kind_operand, layout.nil_word);
        }
        Some(source) => {
            assembler.store(Width::Word64, kind, value);
            assembler.load(Width::Word64, value, source);
            assembler.store(Width::Word64, kind_operand, value);
        }
    }
    assembler.store_immediate32(
        field(layout.frame_uncontinuable_displacement),
        kinds.false_word,
    );
    // Married: the `SmallInteger` of the header's word offset from the
    // activation stack's base, the frame's register 0's plus its
    // arguments.
    let stack_base = macro_assembler.block_word(BlockWord::StackBase);
    let assembler = macro_assembler.assembler();
    assembler.move_register(value, frame);
    assembler.load(Width::Word64, spare, stack_base);
    assembler.subtract(value, spare);
    assembler.shift_right_logical(value, WORD_SHIFT);
    assembler.add_immediate(value, i32::from(argument_count));
    assembler.shift_left(value, layout.tag_bits);
    assembler.or_immediate(value, layout.small_integer_tag);
    assembler.store(
        Width::Word64,
        field(layout.frame_married_displacement),
        value,
    );
}

/// How far a byte count is shifted to be a word count.
const WORD_SHIFT: u8 = 3;

/// `home :=` the home of the closure a block's frame holds as its operand,
/// as `Heap::closure_home` finds it: the frame the closure captured, or
/// that frame's own home when it is a block's and has one; nil for a
/// closure that captured none. Clobbers the flags and `spare`.
fn home_of_operand(
    macro_assembler: &mut MacroAssembler,
    argument_count: u16,
    home: Register,
    spare: Register,
) {
    let layout = Heap::machine_layout();
    let kinds = Heap::machine_frame_kinds();
    let roles = macro_assembler.roles();
    let memory = |base: Register, displacement: i32| Memory { base, displacement };
    let assembler = macro_assembler.assembler();
    let found = assembler.label();
    assembler.load(
        Width::Word64,
        home,
        frame_slot(roles.frame, Heap::operand_register(argument_count)),
    );
    assembler.add(home, roles.object_space);
    assembler.load(
        Width::Word64,
        home,
        memory(home, layout.closure_captured_frame_displacement),
    );
    assembler.compare_immediate(home, layout.nil_word);
    assembler.jump_if(Condition::Equal, found);
    assembler.move_register(spare, home);
    assembler.add(spare, roles.object_space);
    assembler.load(
        Width::Word64,
        spare,
        memory(spare, layout.frame_kind_displacement),
    );
    assembler.compare_immediate(spare, small(kinds.closure));
    assembler.jump_if(Condition::NotEqual, found);
    assembler.move_register(spare, home);
    assembler.add(spare, roles.object_space);
    assembler.load(
        Width::Word64,
        spare,
        memory(spare, layout.frame_home_displacement),
    );
    assembler.compare_immediate(spare, layout.nil_word);
    assembler.jump_if(Condition::Equal, found);
    assembler.move_register(home, spare);
    assembler.bind(found);
}

/// `kind :=` the kind word of an initializer's frame, from the tag its
/// flags word holds for the body it runs (`MachineKind::initializer`).
/// Clobbers the flags and `tag`.
fn initializer_kind(
    macro_assembler: &mut MacroAssembler,
    argument_count: u16,
    kind: Register,
    tag: Register,
) {
    let kinds = Heap::machine_frame_kinds();
    let flags = Heap::machine_divorce(argument_count).flags_register;
    let frame = macro_assembler.roles().frame;
    let assembler = macro_assembler.assembler();
    assembler.load(Width::Word64, tag, frame_slot(frame, flags));
    assembler.shift_right_logical(tag, kinds.kind_tag_shift);
    assembler.and_immediate(tag, kinds.kind_tag_mask);
    let [(_, full), others @ ..] = kinds.initializers;
    assembler.move_immediate(kind, full);
    for (body_tag, word) in others {
        let other = assembler.label();
        assembler.compare_immediate(tag, i32::from(body_tag));
        assembler.jump_if(Condition::NotEqual, other);
        assembler.move_immediate(kind, word);
        assembler.bind(other);
    }
}

/// `Heap::retain_registers`: the frame's flags word with
/// `retains_registers` set, for a block that reads its creator's registers
/// by reference, which makes them outlive the frame.
fn retain_registers(body: &mut BodyAssembler, registers: &ClosureRegisters) {
    let divorce = Heap::machine_divorce(body.argument_count);
    let frame = body.macro_assembler.roles().frame;
    let flags = frame_slot(frame, divorce.flags_register);
    let assembler = body.macro_assembler.assembler();
    assembler.load(Width::Word64, registers.value, flags);
    assembler.or_immediate(registers.value, divorce.retains_registers_bit);
    assembler.store(Width::Word64, flags, registers.value);
}

/// The closure, as `Heap::allocate_closure_from_registers` makes it, its
/// word into the frame's `destination`.
fn allocate_closure(
    body: &mut BodyAssembler,
    site: usize,
    destination: u16,
    copied: &[u16],
    registers: &ClosureRegisters,
) {
    let layout = Heap::machine_layout();
    let divorce = Heap::machine_divorce(body.argument_count);
    let word_count = 1 + layout.closure_fixed_fields + copied.len();
    let feedback_base = Heap::machine_count_word(body.feedback_base.raw() as usize);
    let ClosureRegisters {
        object,
        header,
        class_word,
        address,
        value,
        spare,
    } = *registers;
    let macro_assembler = &mut body.macro_assembler;
    let roles = macro_assembler.roles();
    let frame = roles.frame;
    let field = |displacement: i32| Memory {
        base: address,
        displacement,
    };
    let (slow, placed) = (
        macro_assembler.assembler().label(),
        macro_assembler.assembler().label(),
    );
    macro_assembler.load_header_and_class(
        (header, MachineWord::ClosureHeader),
        (class_word, MachineWord::ClosureClass),
        word_count,
    );
    macro_assembler.bump_allocate(
        object,
        ObjectSize::Constant(word_count),
        header,
        class_word,
        slow,
    );
    macro_assembler.assembler().bind(placed);
    macro_assembler.load_body_word(spare, site + SITE_BLOCK_CODE);
    let assembler = macro_assembler.assembler();
    assembler.move_register(address, object);
    assembler.add(address, roles.object_space);
    assembler.store(
        Width::Word64,
        field(layout.closure_code_id_displacement),
        spare,
    );
    assembler.load(
        Width::Word64,
        value,
        frame_slot(frame, divorce.reified_register),
    );
    assembler.store(
        Width::Word64,
        field(layout.closure_captured_frame_displacement),
        value,
    );
    assembler.move_immediate(value, feedback_base);
    assembler.store(
        Width::Word64,
        field(layout.closure_feedback_base_displacement),
        value,
    );
    let first_copied = i32::try_from(layout.closure_fixed_fields * 8)
        .expect("internal: a closure's fixed fields are few")
        + layout.fields_displacement;
    for (position, register) in copied.iter().enumerate() {
        let displacement = i32::try_from(position)
            .ok()
            .and_then(|position| position.checked_mul(8))
            .and_then(|bytes| bytes.checked_add(first_copied))
            .expect("internal: a closure's copied values lie within 2 GiB of its start");
        // Read as `allocate_closure_from_registers` reads them, the words
        // themselves: a debug build checked each before anything was
        // written (`make_closure`).
        assembler.load(Width::Word64, value, frame_slot(frame, *register));
        assembler.store(Width::Word64, field(displacement), value);
    }
    macro_assembler.store_frame_register(destination, object);
    macro_assembler.call_leaf_out_of_line(
        slow,
        Leaf::Allocate.index(),
        [
            Operand::Immediate(word_count as u64),
            Operand::Register(header),
            Operand::Register(class_word),
        ],
        object,
        placed,
    );
}
