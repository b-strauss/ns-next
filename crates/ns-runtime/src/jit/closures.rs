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
//!
//! `LoadOuterLocal` and `StoreOuterLocal` are here too (`outer_local`):
//! the registers a block reaches by reference in the frames its closures
//! captured, on the activation stack while a frame is married and in its
//! frame object once divorced, the argument count that places a divorced
//! frame's register read from an outer-local site kept as a closure site
//! is.

use ns_assembler::{Condition, Label, Memory, Register, Width};

use crate::bytecode::{CompiledCode, Instruction};
use crate::heap::machine_words::MachineWord;
use crate::heap::Heap;
use crate::interpreter::Mutator;

use super::baseline::{BodyAssembler, BodyRole, Lowering};
use super::leaves::Leaf;
use super::macro_assembler::{MacroAssembler, ObjectSize, Operand};
use super::sends::{frame_slot, store_program_counter};
use super::stubs::{BlockWord, ExitKind};

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

/// The words of an outer-local site, in order: the epoch of the code
/// table it was filled in, 0 for a site never filled -- no heap's epoch
/// is 0; the code id word of the divorced frame object it was filled
/// from; and the argument count of the body that code id names, a plain
/// integer.
const OUTER_SITE_EPOCH: usize = 0;
const OUTER_SITE_CODE: usize = 1;
const OUTER_SITE_ARGUMENTS: usize = 2;
const OUTER_SITE_WORDS: usize = 3;

/// `Leaf::OuterLocalSite`: writes the outer-local site at `site_address`
/// among the data words of the body `code_address` lies in afresh, from
/// the divorced frame object `frame_word` names: the code table's current
/// epoch, the frame's code id word and the argument count of the body it
/// names (`Heap::machine_frame_code_arguments`).
pub(crate) fn refill_outer_site(
    mutator: &mut Mutator<'_>,
    site_address: u64,
    frame_word: u64,
    code_address: u64,
) {
    let (Ok(site_address), Ok(code_address)) =
        (usize::try_from(site_address), usize::try_from(code_address))
    else {
        unreachable!("internal: an address fits the pointer width");
    };
    let (code_word, argument_count) = mutator.heap.machine_frame_code_arguments(frame_word);
    let words = [mutator.heap.code_table_epoch(), code_word, argument_count];
    mutator
        .jit
        .write_body_words(code_address, site_address, &words);
}

/// What an outer-local instruction does with the register it reaches.
#[derive(Clone, Copy)]
pub(super) enum OuterAccess {
    /// `LoadOuterLocal`: the register into the frame's `destination`.
    Load { destination: u16 },
    /// `StoreOuterLocal`: the frame's `source` into the register.
    Store { source: u16 },
}

/// The scratch registers the outer-local template takes, by role.
#[derive(Clone, Copy)]
struct OuterRegisters {
    /// The word naming the frame object reached.
    frame: Register,
    /// The value read or written.
    value: Register,
    /// The frame object's address, then a married frame's register 0's.
    address: Register,
    spare: Register,
    other: Register,
    /// The hops still to take past the first, then the machine words.
    hops: Register,
}

impl OuterRegisters {
    fn of(macro_assembler: &MacroAssembler) -> OuterRegisters {
        let scratch = macro_assembler.roles().scratch;
        OuterRegisters {
            frame: scratch[0],
            value: scratch[1],
            address: scratch[2],
            spare: scratch[3],
            other: scratch[4],
            hops: scratch[5],
        }
    }
}

/// `LoadOuterLocal` and `StoreOuterLocal`: `interpreter::locals`'s
/// `read_outer_local` and `write_outer_local`, register `index` of the
/// frame `depth` closure hops out, with no exit but a debug build's
/// refusal of a dead register.
///
/// The walk is `frame_at_depth`'s: a body that is no block's, or depth 0,
/// reaches its own frame; otherwise the first hop takes the frame the
/// running closure captured -- the own frame when it captured none -- and
/// every further hop the frame a closure frame's closure captured,
/// staying where it is at any other frame, as the interpreter's walk
/// stays.
///
/// At the frame object reached, a married frame -- its married field a
/// `SmallInteger`, its activation's header offset -- has the register
/// read or written on the activation stack, below the header by the
/// argument count its flags word holds, exactly as `Heap::register_slot`
/// finds it, wherever the activation was laid; a store there takes no
/// barrier, since the stack is no object. A divorced one -- returned, or
/// suspended into its frame object -- has the register in its frame
/// object's register field, which `Heap::frame_field_of_register` names
/// from the frame's argument count: its body's, which the instruction's
/// site records beside the code id it read it for and the code table's
/// epoch (`refill_outer_site`), bounded by the frame's registers as
/// `Heap::frame_argument_count` bounds it. A store there goes through the
/// barrier, since the frame object may be old and the value young; a
/// register the frame object has no field for reads `nil` and is not
/// written, as `frame_local` and `frame_set_local` treat it.
///
/// A site that does not match is filled by `Leaf::OuterLocalSite` out of
/// line, before anything is written, and the template starts over.
pub(super) fn outer_local(
    body: &mut BodyAssembler,
    depth: u16,
    index: u16,
    access: OuterAccess,
) -> Lowering {
    if depth == 0 || body.role != BodyRole::Block {
        let value = OuterRegisters::of(&body.macro_assembler).value;
        if let OuterAccess::Store { source } = access {
            body.macro_assembler.load_frame_register(value, source);
        }
        own_local(&mut body.macro_assembler, index, access, value);
        return Lowering::Template;
    }
    let site = body.macro_assembler.take_body_words(OUTER_SITE_WORDS);
    let registers = OuterRegisters::of(&body.macro_assembler);
    let layout = Heap::machine_layout();
    let argument_count = body.argument_count;
    let program_counter = body.program_counter;
    let macro_assembler = &mut body.macro_assembler;
    let roles = macro_assembler.roles();
    let OuterRegisters {
        frame,
        value,
        address,
        spare,
        other,
        ..
    } = registers;
    let assembler = macro_assembler.assembler();
    let (start, own, found, divorced, done) = (
        assembler.label(),
        assembler.label(),
        assembler.label(),
        assembler.label(),
        assembler.label(),
    );
    assembler.bind(start);
    if let OuterAccess::Store { source } = access {
        macro_assembler.load_frame_register(value, source);
    }
    // The first hop: the frame the running closure captured.
    let assembler = macro_assembler.assembler();
    assembler.load(
        Width::Word64,
        frame,
        frame_slot(roles.frame, Heap::operand_register(argument_count)),
    );
    assembler.add(frame, roles.object_space);
    assembler.load(
        Width::Word64,
        frame,
        Memory {
            base: frame,
            displacement: layout.closure_captured_frame_displacement,
        },
    );
    macro_assembler.jump_unless_object_of_format(frame, layout.frame_format, spare, own);
    if depth > 1 {
        further_hops(macro_assembler, depth - 1, registers, found);
    }
    // Married or divorced.
    let assembler = macro_assembler.assembler();
    assembler.bind(found);
    assembler.move_register(address, frame);
    assembler.add(address, roles.object_space);
    assembler.load(
        Width::Word64,
        other,
        Memory {
            base: address,
            displacement: layout.frame_married_displacement,
        },
    );
    assembler.move_register(spare, other);
    assembler.and_immediate(spare, layout.tag_mask);
    assembler.compare_immediate(spare, layout.small_integer_tag);
    assembler.jump_if(Condition::NotEqual, divorced);
    married_local(macro_assembler, index, access, registers, program_counter);
    let assembler = macro_assembler.assembler();
    assembler.jump(done);
    assembler.bind(divorced);
    divorced_local(
        macro_assembler,
        site,
        index,
        access,
        registers,
        (start, done),
    );
    macro_assembler.assembler().bind(own);
    own_local(macro_assembler, index, access, value);
    macro_assembler.assembler().bind(done);
    Lowering::Template
}

/// The frame's own register `index`, read into `destination` or written
/// from `value`, which holds a store's source: the walk's answer at depth
/// 0, for a body that is no block's, and for a closure that captured no
/// frame.
fn own_local(
    macro_assembler: &mut MacroAssembler,
    index: u16,
    access: OuterAccess,
    value: Register,
) {
    match access {
        OuterAccess::Load { destination } => {
            macro_assembler.load_frame_register(value, index);
            macro_assembler.store_frame_register(destination, value);
        }
        OuterAccess::Store { .. } => macro_assembler.store_frame_register(index, value),
    }
}

/// The walk's hops past the first, `hops` of them, from the frame object
/// `registers.frame` names: each takes the frame a closure frame's
/// closure captured, and jumps to `found` -- the frame where it is -- at
/// a frame that is no closure's, or whose operand names no closure, or
/// whose closure names no frame, as `Heap::frame_captured` answers `None`
/// for each. Clobbers the flags and every register but `value`.
fn further_hops(
    macro_assembler: &mut MacroAssembler,
    hops: u16,
    registers: OuterRegisters,
    found: Label,
) {
    let layout = Heap::machine_layout();
    let kinds = Heap::machine_frame_kinds();
    let object_space = macro_assembler.roles().object_space;
    let OuterRegisters {
        frame,
        address,
        spare,
        other,
        hops: remaining,
        ..
    } = registers;
    let field = |base: Register, displacement: i32| Memory { base, displacement };
    let assembler = macro_assembler.assembler();
    let hop = assembler.label();
    assembler.move_immediate(remaining, u64::from(hops));
    assembler.bind(hop);
    assembler.move_register(address, frame);
    assembler.add(address, object_space);
    assembler.load(
        Width::Word64,
        spare,
        field(address, layout.frame_kind_displacement),
    );
    assembler.compare_immediate(spare, small(kinds.closure));
    assembler.jump_if(Condition::NotEqual, found);
    assembler.load(
        Width::Word64,
        spare,
        field(address, layout.frame_kind_operand_displacement),
    );
    macro_assembler.jump_unless_object_of_format(spare, layout.closure_format, other, found);
    let assembler = macro_assembler.assembler();
    assembler.add(spare, object_space);
    assembler.load(
        Width::Word64,
        spare,
        field(spare, layout.closure_captured_frame_displacement),
    );
    macro_assembler.jump_unless_object_of_format(spare, layout.frame_format, other, found);
    let assembler = macro_assembler.assembler();
    assembler.move_register(frame, spare);
    assembler.add_immediate(remaining, -1);
    assembler.compare_immediate(remaining, 0);
    assembler.jump_if(Condition::NotEqual, hop);
}

/// Register `index` of the married frame whose married field's word
/// `registers.other` holds, on the activation stack: the header at the field's
/// offset from the stack's base, register 0 the argument count its flags
/// word holds below it (`Heap::register_slot`). A debug build refuses a
/// dead register's read by an exit before anything is written, so that
/// the interpreter refuses it as its own read is refused.
fn married_local(
    macro_assembler: &mut MacroAssembler,
    index: u16,
    access: OuterAccess,
    registers: OuterRegisters,
    program_counter: u32,
) {
    let layout = Heap::machine_layout();
    let (argument_count_shift, argument_count_mask) = Heap::machine_argument_count_field();
    let OuterRegisters {
        value,
        address,
        spare,
        other: married,
        ..
    } = registers;
    let stack_base = macro_assembler.block_word(BlockWord::StackBase);
    let assembler = macro_assembler.assembler();
    assembler.move_register(address, married);
    assembler.shift_right_logical(address, layout.tag_bits);
    assembler.shift_left(address, WORD_SHIFT);
    assembler.load(Width::Word64, spare, stack_base);
    assembler.add(address, spare);
    assembler.load(
        Width::Word64,
        spare,
        frame_slot(address, Heap::flags_register()),
    );
    assembler.shift_right_logical(spare, argument_count_shift);
    assembler.and_immediate(spare, argument_count_mask);
    assembler.shift_left(spare, WORD_SHIFT);
    assembler.subtract(address, spare);
    let slot = frame_slot(address, index);
    match access {
        OuterAccess::Load { destination } => {
            assembler.load(Width::Word64, value, slot);
            #[cfg(debug_assertions)]
            {
                let dead = assembler.label();
                assembler.move_immediate(spare, crate::heap::Word::DEAD_REGISTER_BITS);
                assembler.compare(value, spare);
                assembler.jump_if(Condition::Equal, dead);
                macro_assembler.exit_at(dead, ExitKind::Reexecute, program_counter);
            }
            macro_assembler.store_frame_register(destination, value);
        }
        OuterAccess::Store { .. } => assembler.store(Width::Word64, slot, value),
    }
    let _ = program_counter;
}

/// Register `index` of the divorced frame object `registers.frame`
/// names, its address in `registers.address`: the field
/// `Heap::frame_field_of_register` names from the argument count the
/// site records for the frame's code id, bounded by the frame's
/// registers -- or none, which reads `nil` and writes nothing. A site
/// that does not match calls its leaf out of line and starts over at
/// `start`; the access ends with a jump to `done`.
fn divorced_local(
    macro_assembler: &mut MacroAssembler,
    site: usize,
    index: u16,
    access: OuterAccess,
    registers: OuterRegisters,
    (start, done): (Label, Label),
) {
    let layout = Heap::machine_layout();
    let leaf_result = macro_assembler.roles().leaf_result;
    let OuterRegisters {
        frame,
        address,
        spare,
        other,
        hops: machine_words,
        ..
    } = registers;
    let assembler = macro_assembler.assembler();
    let (refill, argument, none, bounded) = (
        assembler.label(),
        assembler.label(),
        assembler.label(),
        assembler.label(),
    );
    // The site's check: the frame's code id, then the epoch.
    assembler.load(
        Width::Word64,
        spare,
        Memory {
            base: address,
            displacement: layout.frame_code_id_displacement,
        },
    );
    macro_assembler.load_body_word(other, site + OUTER_SITE_CODE);
    let assembler = macro_assembler.assembler();
    assembler.compare(spare, other);
    assembler.jump_if(Condition::NotEqual, refill);
    macro_assembler.load_body_word(other, site + OUTER_SITE_EPOCH);
    macro_assembler.load_machine_words(machine_words);
    macro_assembler.assembler().compare_memory(
        other,
        MacroAssembler::machine_word(machine_words, MachineWord::CodeTableEpoch),
    );
    macro_assembler
        .assembler()
        .jump_if(Condition::NotEqual, refill);
    // The argument count, bounded by the registers: `spare`, beside the
    // registers' count in `other`.
    macro_assembler.load_body_word(spare, site + OUTER_SITE_ARGUMENTS);
    macro_assembler.load_word_count(frame, other);
    let fixed_words = i32::try_from(1 + layout.frame_fixed_fields)
        .expect("internal: a frame object's fixed fields are few");
    let assembler = macro_assembler.assembler();
    assembler.add_immediate(other, -fixed_words);
    assembler.compare(spare, other);
    assembler.jump_if(Condition::BelowOrEqual, bounded);
    assembler.move_register(spare, other);
    assembler.bind(bounded);
    // An argument's field is its own number; a register above the header
    // is `HEADER_REGISTERS` fields lower; a header's number has none.
    assembler.compare_immediate(spare, i32::from(index));
    assembler.jump_if(Condition::Above, argument);
    match index.checked_sub(crate::bytecode::HEADER_REGISTERS) {
        Some(above) => {
            assembler.compare_immediate(spare, i32::from(above));
            assembler.jump_if(Condition::Above, none);
            assembler.compare_immediate(other, i32::from(above));
            assembler.jump_if(Condition::BelowOrEqual, none);
            divorced_field(macro_assembler, above, access, registers);
            macro_assembler.assembler().jump(done);
        }
        None => assembler.jump(none),
    }
    macro_assembler.assembler().bind(argument);
    divorced_field(macro_assembler, index, access, registers);
    let assembler = macro_assembler.assembler();
    assembler.jump(done);
    assembler.bind(none);
    if let OuterAccess::Load { destination } = access {
        let frame_register = macro_assembler.roles().frame;
        macro_assembler
            .assembler()
            .store_immediate32(frame_slot(frame_register, destination), layout.nil_word);
    }
    macro_assembler.assembler().jump(done);
    macro_assembler.call_leaf_out_of_line(
        refill,
        Leaf::OuterLocalSite.index(),
        [
            Operand::BodyWord(site),
            Operand::Register(frame),
            Operand::CodeAddress,
        ],
        leaf_result,
        start,
    );
}

/// The divorced frame's register field `field`, read into a load's
/// destination, or written from a store's value through the barrier.
fn divorced_field(
    macro_assembler: &mut MacroAssembler,
    field: u16,
    access: OuterAccess,
    registers: OuterRegisters,
) {
    let layout = Heap::machine_layout();
    let displacement = i32::try_from(layout.frame_fixed_fields + usize::from(field))
        .ok()
        .and_then(|fields| fields.checked_mul(8))
        .and_then(|bytes| bytes.checked_add(layout.fields_displacement))
        .expect("internal: a frame object's register fields lie within 2 GiB of its start");
    let OuterRegisters {
        frame,
        value,
        address,
        spare,
        ..
    } = registers;
    match access {
        OuterAccess::Load { destination } => {
            macro_assembler.assembler().load(
                Width::Word64,
                value,
                Memory {
                    base: address,
                    displacement,
                },
            );
            macro_assembler.store_frame_register(destination, value);
        }
        OuterAccess::Store { .. } => {
            macro_assembler.store_with_barrier(frame, displacement, value, spare);
        }
    }
}
