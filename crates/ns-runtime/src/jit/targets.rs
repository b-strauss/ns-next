//! The stubs a send's cell names for a site that holds no entry of its
//! own (`docs/design/execution.md`, "Sends in machine code"): the
//! polymorphic stub, which every send whose guard is not its cell's calls
//! and which scans a polymorphic site's block of four cells, and the
//! megamorphic stub, which probes the global cache's mirror for a
//! megamorphic site. Each continues as the send's hit would -- the
//! callee's current class `level` superclass steps up the chain's start,
//! then a jump to the entry the cell or row found records -- and a miss
//! in either goes to the miss leaf (`stubs::emit_miss`), which decides the
//! send as the interpreter's miss does.
//!
//! A stub is a lowering of `inline_cache::decided_miss`'s first two
//! steps, never a decision of its own: a block cell and a global row copy
//! what their entries record, and the stubs compare the key the
//! interpreter files the send under -- the polymorphic stub the guard the
//! site's cells compare, the megamorphic stub the key `GlobalCache::group`
//! hashes -- so that a hit is an entry the lookup recorded for exactly
//! that key. A stub stores into no frame and calls no leaf before its
//! miss, so that a miss, refused, leaves the sender's registers as they
//! were.
//!
//! Both are entered as the callee is -- the frame register at the
//! callee's register 0 on the send's slots, the send's return address on
//! top of the native stack -- and with what the send found in the roles
//! (`jit::sends`): `send_cells` the site's cell's address, `send_guard`
//! the word the send compared with it, `send_key` the selector's index
//! above the rule's tag (`inline_cache::rule_tag`), `callee_receiver` the
//! receiver and `callee_class`, at a `super` site S and at a lexical one
//! N -- `nil` where N does not exist.
//!
//! Beside them lie the stubs a cell names for a target that is no method
//! with code (`assemble_accessor_stubs`): a getter, a setter, a quick
//! method, a method that is a leaf-safe mirror row, and the leaf stub,
//! which runs `act`'s arm through the leaf shim.

use ns_assembler::{Assembled, Condition, Label, Memory, Register, Width};

use super::leaves::Leaf;
use super::macro_assembler::{Abi, MacroAssembler, Operand, Roles};
use super::stubs::{emit_miss, finish_stub, BlockWord, ExitKind};
use crate::builtins::primitive::{MachineCall, RowStub, MIRROR};
use crate::heap::machine_words::MachineWord;
use crate::interpreter::inline_cache::cells::{
    BLOCK_BYTES, CELL_BYTES, CELL_WORDS, CHAIN_START_BITS, MAXIMUM_GLOBAL_ROWS, ROW_BYTES,
    ROW_WORDS, VACANT_GUARD,
};
use crate::interpreter::inline_cache::POLYMORPHIC_WIDTH;
use crate::interpreter::inline_cache::{RULE_TAG_BITS, RULE_TAG_LEXICAL, RULE_TAG_MASK};
use crate::mixin::AccessorKind;

/// The bytes of a word.
const WORD_BYTES: i32 = 8;

/// A cell's words' displacements from its start (`Cell`'s order).
const CELL_GUARD: i32 = 0;
const CELL_ENTRY: i32 = WORD_BYTES;
const CELL_LEVEL: i32 = 2 * WORD_BYTES;
const CELL_CHAIN_START: i32 = 3 * WORD_BYTES;

/// A global row's words' displacements from its start (`GlobalRow`'s
/// order).
const ROW_GUARD: i32 = 0;
const ROW_SELECTOR: i32 = WORD_BYTES;
const ROW_RULE: i32 = 2 * WORD_BYTES;
const ROW_ENTRY: i32 = 3 * WORD_BYTES;
const ROW_LEVEL: i32 = 4 * WORD_BYTES;
const ROW_CHAIN_START: i32 = 5 * WORD_BYTES;

const _: () = assert!(CELL_WORDS == 4 && ROW_WORDS == 8);

/// How far a block's number is shifted to be its cells' offset.
const BLOCK_SHIFT: u8 = 7;

/// How far a group's index is shifted to be its first row's offset: a
/// group is two rows.
const GROUP_SHIFT: u8 = 7;

const _: () = assert!(1 << BLOCK_SHIFT == BLOCK_BYTES && 1 << GROUP_SHIFT == 2 * ROW_BYTES);

/// The bytes of a cell and of a row, as displacements.
#[expect(
    clippy::cast_possible_truncation,
    reason = "a cell is four words, asserted above"
)]
const CELL_STEP: i32 = CELL_BYTES as i32;
#[expect(
    clippy::cast_possible_truncation,
    reason = "a row is eight words, asserted above"
)]
const ROW_STEP: i32 = ROW_BYTES as i32;

/// The cell guard no compared word is, sign-extended from 32 bits.
const VACANT_GUARD_IMMEDIATE: i32 = -1;

const _: () = assert!(VACANT_GUARD == u64::MAX);

/// The bit of a rule's tag set exactly for a private lexical rule's and a
/// `super` rule's, whose chain starts at the class the send found -- N
/// or S, in `callee_class` -- rather than at the receiver's class.
const CHAIN_START_FOUND_BIT: i32 = 2;

/// How far the product is shifted right to bring the bits `GlobalCache`'s
/// hasher rotates down (`hash::FINISH_ROTATION`) to the bottom: the group
/// index takes no more of them than the rotation brings down, so the
/// shift, which fills the top with sign bits instead of the rotation's
/// low bits, answers the same index.
#[expect(
    clippy::cast_possible_truncation,
    reason = "a rotation of a 64-bit word is below 64"
)]
const GROUP_HASH_SHIFT: u8 = (u64::BITS - crate::hash::FINISH_ROTATION) as u8;

const _: () = assert!(MAXIMUM_GLOBAL_ROWS / 2 <= 1 << crate::hash::FINISH_ROTATION);

/// The polymorphic stub (`BlockWord::PolymorphicStub`), which a send whose
/// guard is not its cell's calls -- every send at a site that holds no
/// entry of its own, whose cell's guard no compared word is. A site with
/// an entry of its own missed it, and a vacant one has nothing to find,
/// so both go to the miss; a megamorphic site's cell names the
/// megamorphic stub, which it jumps to; a polymorphic one's names this
/// stub, which scans the four cells of the block the cell's `level`
/// numbers for the send's guard, as `InlineCaches::polymorphic_hit`
/// scans the block's entries, and continues at the first that holds it.
pub(crate) fn assemble_polymorphic_stub(abi: Abi) -> Assembled {
    let mut macro_assembler = MacroAssembler::new(abi);
    let roles = macro_assembler.roles();
    let (cell, guard, entry) = (roles.send_cells, roles.send_guard, roles.send_entry);
    let scratch = roles.idiom_scratch;
    let polymorphic_stub = macro_assembler.block_word(BlockWord::PolymorphicStub);
    let blocks = macro_assembler.block_word(BlockWord::PolymorphicBlocks);
    let word = |displacement: i32| Memory {
        base: cell,
        displacement,
    };
    let assembler = macro_assembler.assembler();
    let (miss, scan, hit) = (assembler.label(), assembler.label(), assembler.label());
    assembler.load(Width::Word64, entry, word(CELL_GUARD));
    assembler.compare_immediate(entry, VACANT_GUARD_IMMEDIATE);
    assembler.jump_if(Condition::NotEqual, miss);
    assembler.load(Width::Word64, entry, word(CELL_ENTRY));
    assembler.compare_immediate(entry, 0);
    assembler.jump_if(Condition::Equal, miss);
    assembler.compare_memory(entry, polymorphic_stub);
    assembler.jump_if(Condition::Equal, scan);
    assembler.jump_register(entry);
    assembler.bind(scan);
    assembler.load(Width::Word64, cell, word(CELL_LEVEL));
    assembler.shift_left(cell, BLOCK_SHIFT);
    assembler.load(Width::Word64, scratch, blocks);
    assembler.add(cell, scratch);
    let found: Vec<Label> = (0..POLYMORPHIC_WIDTH).map(|_| assembler.label()).collect();
    for (slot, label) in (0..).zip(&found) {
        assembler.compare_memory(guard, word(slot * CELL_STEP));
        assembler.jump_if(Condition::Equal, *label);
    }
    assembler.jump(miss);
    for (slot, label) in (0..).zip(&found).skip(1) {
        assembler.bind(*label);
        assembler.add_immediate(cell, slot * CELL_STEP);
        assembler.jump(hit);
    }
    assembler.bind(found[0]);
    assembler.bind(hit);
    split_rule_tag(&mut macro_assembler);
    emit_hit(
        &mut macro_assembler,
        CELL_LEVEL,
        CELL_ENTRY,
        CELL_CHAIN_START,
        miss,
    );
    macro_assembler.assembler().bind(miss);
    emit_miss(&mut macro_assembler);
    finish_stub(macro_assembler)
}

/// The megamorphic stub, which a megamorphic site's cell names
/// (`jit::cell_entry`): the key `inline_cache::decided_miss` probes the
/// global cache with -- the guard as a dispatch id, the selector and the
/// rule -- hashed as `GlobalCache::group` hashes it, and the two rows of
/// its group compared with it in the order the cache's `get` compares its
/// entries. A `super` or private lexical send's guard is S's or N's
/// application word, whose dispatch id the application id table answers
/// (`MacroAssembler::load_application_id`); a public lexical send's rule
/// carries N's dispatch id, `NONE` where N does not exist, as
/// `lexical_dispatch_id` finds it. A guard that names no application is
/// `DispatchId::NONE`, under which nothing is filed, and so a miss, as
/// is an application the table does not record.
///
/// Unlike `get`, a hit at the second row does not promote it: the order
/// within a group decides only which key a colliding insert evicts, never
/// what a key finds, and machine code writes no row.
pub(crate) fn assemble_megamorphic_stub(abi: Abi) -> Assembled {
    let mut macro_assembler = MacroAssembler::new(abi);
    let roles = macro_assembler.roles();
    let (cell, guard, key, rule) = (
        roles.send_cells,
        roles.send_guard,
        roles.send_key,
        roles.lookup_rule,
    );
    let (hash, class) = (roles.lookup_hash, roles.callee_class());
    let scratch = roles.idiom_scratch;
    let mask = macro_assembler.block_word(BlockWord::GlobalMask);
    let rows = macro_assembler.block_word(BlockWord::GlobalRows);
    let assembler = macro_assembler.assembler();
    let (miss, hit, guard_found, rule_found, no_lexical_class, second_way) = (
        assembler.label(),
        assembler.label(),
        assembler.label(),
        assembler.label(),
        assembler.label(),
        assembler.label(),
    );
    split_rule_tag(&mut macro_assembler);
    let assembler = macro_assembler.assembler();
    assembler.shift_right_arithmetic(key, RULE_TAG_BITS);
    assembler.test_immediate(rule, CHAIN_START_FOUND_BIT);
    assembler.jump_if(Condition::Equal, guard_found);
    macro_assembler.load_application_id(guard, guard, miss, miss);
    let assembler = macro_assembler.assembler();
    assembler.bind(guard_found);
    assembler.compare_immediate(rule, RULE_TAG_LEXICAL);
    assembler.jump_if(Condition::NotEqual, rule_found);
    macro_assembler.jump_unless_object(class, no_lexical_class);
    macro_assembler.load_application_word(rule, class);
    macro_assembler.load_application_id(rule, rule, no_lexical_class, miss);
    let assembler = macro_assembler.assembler();
    assembler.shift_left(rule, RULE_TAG_BITS);
    assembler.or_immediate(rule, RULE_TAG_LEXICAL);
    assembler.jump(rule_found);
    assembler.bind(no_lexical_class);
    assembler.move_immediate(rule, RULE_TAG_LEXICAL.unsigned_abs().into());
    assembler.bind(rule_found);
    emit_group(&mut macro_assembler, mask);
    let assembler = macro_assembler.assembler();
    assembler.shift_left(hash, GROUP_SHIFT);
    assembler.load(Width::Word64, scratch, rows);
    assembler.add(hash, scratch);
    assembler.move_register(cell, hash);
    let row = |displacement: i32| Memory {
        base: cell,
        displacement,
    };
    for (way, missed) in [(0, second_way), (1, miss)] {
        if way == 1 {
            assembler.bind(second_way);
            assembler.add_immediate(cell, ROW_STEP);
        }
        assembler.compare_memory(guard, row(ROW_GUARD));
        assembler.jump_if(Condition::NotEqual, missed);
        assembler.compare_memory(key, row(ROW_SELECTOR));
        assembler.jump_if(Condition::NotEqual, missed);
        assembler.compare_memory(rule, row(ROW_RULE));
        assembler.jump_if(Condition::Equal, hit);
        assembler.jump(missed);
    }
    assembler.bind(hit);
    emit_hit(
        &mut macro_assembler,
        ROW_LEVEL,
        ROW_ENTRY,
        ROW_CHAIN_START,
        miss,
    );
    macro_assembler.assembler().bind(miss);
    emit_miss(&mut macro_assembler);
    finish_stub(macro_assembler)
}

/// `lookup_rule :=` the rule's tag, the low bits of `send_key`, which
/// keeps them. Clobbers the flags.
fn split_rule_tag(macro_assembler: &mut MacroAssembler) {
    let roles = macro_assembler.roles();
    let assembler = macro_assembler.assembler();
    assembler.move_register(roles.lookup_rule, roles.send_key);
    assembler.and_immediate(roles.lookup_rule, RULE_TAG_MASK);
}

/// A stub's hit on the cell or row at `send_cells`, as the send's own hit
/// on its cell: the callee's current class `level` superclass steps up
/// from the chain's start -- N or S, already in `callee_class`, for a
/// private lexical or a `super` rule, the receiver's class otherwise
/// (`inline_cache::rule_chain_start`) -- then a jump to the recorded
/// entry, or to `miss` where the chain is shorter, which makes the send a
/// miss as it makes the interpreter's (`inline_cache::materialize`).
/// `lookup_rule`'s low bits are the rule's tag. The callee is entered
/// with `send_cells` moved so that the cell's or row's chain start word,
/// at `chain_start_displacement`, lies where a cell's does: a private or
/// `super` slot accessor's stub reads its recorded guard there.
fn emit_hit(
    macro_assembler: &mut MacroAssembler,
    level_displacement: i32,
    entry_displacement: i32,
    chain_start_displacement: i32,
    miss: Label,
) {
    let roles = macro_assembler.roles();
    let (cell, level, entry) = (roles.send_cells, roles.send_level, roles.send_entry);
    let (receiver, class) = (roles.callee_receiver(), roles.callee_class());
    let word = |displacement: i32| Memory {
        base: cell,
        displacement,
    };
    let assembler = macro_assembler.assembler();
    let start_found = assembler.label();
    assembler.test_immediate(roles.lookup_rule, CHAIN_START_FOUND_BIT);
    assembler.jump_if(Condition::NotEqual, start_found);
    macro_assembler.load_class(class, receiver);
    let assembler = macro_assembler.assembler();
    assembler.bind(start_found);
    assembler.load(Width::Word64, level, word(level_displacement));
    macro_assembler.walk_superclasses(class, level, miss);
    let assembler = macro_assembler.assembler();
    assembler.load(Width::Word64, entry, word(entry_displacement));
    if chain_start_displacement != CELL_CHAIN_START {
        assembler.add_immediate(cell, chain_start_displacement - CELL_CHAIN_START);
    }
    assembler.jump_register(entry);
}

/// `lookup_hash :=` the index of the group `GlobalCache::group` answers
/// the first slot of for the key -- `send_guard` the guard's dispatch id,
/// `send_key` the selector's index, `lookup_rule` the rule's word -- with
/// the group mask at `mask`: the hasher's sum and multiply per word of
/// the key, in the order the derived `Hash` writes them -- the guard, the
/// selector, the rule's discriminant, which is its tag, and a lexical
/// rule's N -- then the rotation's bits, masked. Clobbers the flags, the
/// idiom scratch register and `send_entry`.
pub(super) fn emit_group(macro_assembler: &mut MacroAssembler, mask: Memory) {
    let roles = macro_assembler.roles();
    let (hash, multiplier, scratch) = (roles.lookup_hash, roles.send_entry, roles.idiom_scratch);
    let assembler = macro_assembler.assembler();
    let done = assembler.label();
    assembler.move_immediate(multiplier, crate::hash::MULTIPLIER);
    assembler.move_register(hash, roles.send_guard);
    assembler.multiply_checked(hash, multiplier);
    assembler.add(hash, roles.send_key);
    assembler.multiply_checked(hash, multiplier);
    assembler.move_register(scratch, roles.lookup_rule);
    assembler.and_immediate(scratch, RULE_TAG_MASK);
    assembler.add(hash, scratch);
    assembler.multiply_checked(hash, multiplier);
    assembler.compare_immediate(scratch, RULE_TAG_LEXICAL);
    assembler.jump_if(Condition::NotEqual, done);
    assembler.move_register(scratch, roles.lookup_rule);
    assembler.shift_right_arithmetic(scratch, RULE_TAG_BITS);
    assembler.add(hash, scratch);
    assembler.multiply_checked(hash, multiplier);
    assembler.bind(done);
    assembler.shift_right_arithmetic(hash, GROUP_HASH_SHIFT);
    assembler.load(Width::Word64, scratch, mask);
    assembler.and(hash, scratch);
}

/// How many fields have a getter stub of their own: a getter of a field
/// past them runs through the leaf stub, as `act` runs it.
pub(crate) const GETTER_STUBS: u32 = 64;

/// How many fields have a setter stub of their own: a setter of a field
/// past them runs through the leaf stub, as `act` runs it.
pub(crate) const SETTER_STUBS: u32 = 64;

/// How many hidden slots have a nested class accessor's stub of their
/// own: an accessor whose slot lies past them is the interpreter's.
pub(crate) const NESTED_CLASS_STUBS: u32 = 64;

/// How many parameter positions have a stub of their own: a quick
/// method answering a parameter past them runs through the leaf stub.
pub(crate) const PARAMETER_STUBS: u8 = 16;

/// Where each entry of the accessor stubs lies in their code, as offsets
/// from its start (`assemble_accessor_stubs`).
pub(crate) struct AccessorEntries {
    /// The getter of field `index`, by index.
    pub(crate) getters: Vec<u32>,
    /// The setter of field `index`, by index.
    pub(crate) setters: Vec<u32>,
    /// The quick method answering parameter `position`, by position.
    pub(crate) parameters: Vec<u32>,
    /// The method whose fast path is mirror row `index`, by index.
    pub(crate) mirror_methods: Vec<u32>,
    /// The method whose fast path is a row with a stub of its own, by
    /// the stub's index (`RowStub::index`).
    pub(crate) row_stubs: Vec<u32>,
    /// The quick methods answering `nil`, `true` and `false`.
    pub(crate) nil: u32,
    pub(crate) true_constant: u32,
    pub(crate) false_constant: u32,
    /// The quick method answering its receiver.
    pub(crate) receiver: u32,
    /// The leaf stub (`Leaf::Act`).
    pub(crate) act: u32,
    /// The getter of a private or `super` chain of field `index`, by
    /// index.
    pub(crate) guarded_getters: Vec<u32>,
    /// The setter of a private or `super` chain of field `index`, by
    /// index.
    pub(crate) guarded_setters: Vec<u32>,
    /// The nested class accessor whose class is cached in hidden slot
    /// `index`, by index.
    pub(crate) nested_classes: Vec<u32>,
}

/// The stubs a cell names for a target that is no method with code
/// (`jit::cell_entry`), each what `Mutator::act_with_base` does for it and
/// nothing more, entered as a callee is -- the frame register at the
/// callee's register 0 on the send's slots, its arguments there, the
/// receiver in `callee_receiver` -- and answering the send's value by
/// `ret`, as a compiled callee's return does, having pushed nothing:
///
/// - a getter of a public or protected chain, whose cell's guard is the
///   receiver's own dispatch id, so that the receiver's layout is the
///   one the lookup found the field in: the receiver's field, once the
///   receiver is checked to be an instance holding it, as
///   `Heap::instance_field` checks it;
/// - a setter of a public or protected chain, whose cell's guard is the
///   receiver's dispatch id as a getter's is: once the receiver is
///   checked as the getter checks it, the send's one argument stored into
///   the field through the inline write barrier
///   (`MacroAssembler::store_with_barrier_at`), and the receiver;
/// - a quick method's constant, its receiver or its parameter
///   (`MethodFastPath::Constant`, `Receiver`, `Parameter`);
/// - a method whose fast path is a leaf-safe mirror row
///   (`MethodFastPath::Mirror`): the mirror method leaf
///   (`Leaf::MirrorMethod`) runs the row's leaf on the receiver, handed
///   over in the machine words, and the send's arguments in its slots, as
///   `Mutator::act_for_machine_code` runs it;
/// - a method whose fast path is a row with a stub of its own
///   (`MachineCall::Stub`): an Array's or a ByteArray's `at:` and
///   `at:put:`, which read or write the element an index in range names
///   as the row's leaf does -- an Array's store through the inline write
///   barrier, a byte's without one, since a byte names no object -- and
///   continue at the row's mirror method stub for every other case, so
///   that the row's failures stay its leaf's;
/// - a nested class's accessor on a public or protected chain, whose
///   cell's guard is the receiver's dispatch id, so that the hidden slot
///   the entry recorded is the receiver's (`nested::nested_class`'s hit):
///   the getter's checks and load of that slot, and the class it holds;
///   a slot that holds none yet -- nil, or a creation underway -- declines,
///   so that the interpreter makes the class;
/// - a getter or a setter of a private or `super` chain, whose field the
///   lookup decided from N's or S's chain, never the receiver's: the
///   receiver's guard compared with the one the entry recorded, which the
///   cell carries above its chain start, as `Mutator::slot_level_holds`
///   compares it first, then the public stubs' access; any other
///   receiver goes on to the leaf stub, which makes the rest of the check;
/// - the leaf stub, for every other target machine code runs this way --
///   a private or `super` slot accessor whose guard differs from the
///   recorded one or whose field is past the stubs', a setter past
///   `SETTER_STUBS`, a primitive that is a mirror send of a leaf-safe row,
///   a builtin class's accessor: the leaf runs `act`'s arm through the
///   shim (`Leaf::Act`) and answers its value, or the word no value is
///   when the arm would answer anything else, before it changed anything.
///
/// Every decline -- a receiver the getter's or the setter's check
/// refuses, the word no value is from the mirror method leaf or the leaf
/// stub's -- refuses the call before anything is written, so that the
/// caller re-executes the send and the interpreter runs it.
pub(crate) fn assemble_accessor_stubs(abi: Abi) -> (Assembled, AccessorEntries) {
    let mut macro_assembler = MacroAssembler::new(abi);
    let roles = macro_assembler.roles();
    let layout = crate::heap::Heap::machine_layout();
    let (field, answer, address) = (roles.accessor_field, roles.leaf_result, roles.idiom_scratch);
    let receiver = roles.callee_receiver();
    let assembler = macro_assembler.assembler();
    // Beside the getter's, the registers the setter and the row stubs
    // hold an index, an element's address and a scratch word in: none
    // the receiver, the field or the answer is in.
    let free: Vec<Register> = roles
        .scratch
        .iter()
        .copied()
        .filter(|register| ![receiver, field, answer, roles.send_cells].contains(register))
        .collect();
    let (index, element, spare) = (free[0], free[1], free[2]);
    let (decline, getter, setter, mirror_method, act) = (
        assembler.label(),
        assembler.label(),
        assembler.label(),
        assembler.label(),
        assembler.label(),
    );
    let (guarded_getter, guarded_setter) = (assembler.label(), assembler.label());
    let getters: Vec<Label> = (0..GETTER_STUBS)
        .map(|index| {
            let entry = assembler.label();
            assembler.bind(entry);
            assembler.move_immediate(field, u64::from(index));
            assembler.jump(getter);
            entry
        })
        .collect();
    let setters: Vec<Label> = (0..SETTER_STUBS)
        .map(|index| {
            let entry = assembler.label();
            assembler.bind(entry);
            assembler.move_immediate(field, u64::from(index));
            assembler.jump(setter);
            entry
        })
        .collect();
    let mut guarded = |count: u32, target: Label| -> Vec<Label> {
        (0..count)
            .map(|index| {
                let entry = assembler.label();
                assembler.bind(entry);
                assembler.move_immediate(field, u64::from(index));
                assembler.jump(target);
                entry
            })
            .collect()
    };
    let guarded_getters = guarded(GETTER_STUBS, guarded_getter);
    let guarded_setters = guarded(SETTER_STUBS, guarded_setter);
    let nested_class = assembler.label();
    let nested_classes: Vec<Label> = (0..NESTED_CLASS_STUBS)
        .map(|index| {
            let entry = assembler.label();
            assembler.bind(entry);
            assembler.move_immediate(field, u64::from(index));
            assembler.jump(nested_class);
            entry
        })
        .collect();
    let rows = u16::try_from(crate::builtins::primitive::MIRROR.len())
        .expect("internal: the mirror rows are numbered in 16 bits");
    let mirror_methods: Vec<Label> = (0..rows)
        .map(|row| {
            let entry = assembler.label();
            assembler.bind(entry);
            assembler.move_immediate(field, u64::from(row));
            assembler.jump(mirror_method);
            entry
        })
        .collect();
    let parameters: Vec<Label> = (0..PARAMETER_STUBS)
        .map(|position| {
            let entry = assembler.label();
            assembler.bind(entry);
            assembler.load(
                Width::Word64,
                answer,
                Memory {
                    base: roles.frame,
                    displacement: i32::from(position) * WORD_BYTES,
                },
            );
            assembler.return_();
            entry
        })
        .collect();
    let mut constant = |word: u64| {
        let entry = assembler.label();
        assembler.bind(entry);
        assembler.move_immediate(answer, word);
        assembler.return_();
        entry
    };
    let nil = constant(crate::heap::Immediate::NIL.bits());
    let true_constant = constant(crate::heap::Immediate::boolean(true).bits());
    let false_constant = constant(crate::heap::Immediate::boolean(false).bits());
    let receiver_entry = assembler.label();
    assembler.bind(receiver_entry);
    assembler.move_register(answer, receiver);
    assembler.return_();
    // The getter: `instance_field`'s checks, then the load; and a nested
    // class's accessor: the getter's, then whether the slot holds the
    // class, an object -- not nil, and not the `SmallInteger` of a
    // creation underway (`nested::is_creation_in_progress`).
    for (entry, holds_class) in [(getter, false), (nested_class, true)] {
        let assembler = macro_assembler.assembler();
        assembler.bind(entry);
        macro_assembler.jump_unless_object(receiver, decline);
        let assembler = macro_assembler.assembler();
        assembler.move_register(address, receiver);
        assembler.add(address, roles.object_space);
        assembler.load(
            Width::Word64,
            answer,
            Memory {
                base: address,
                displacement: layout.header_displacement,
            },
        );
        assembler.test_immediate(answer, layout.instance_format_bits);
        assembler.jump_if(Condition::NotEqual, decline);
        // The count of the words after the header, the class word first:
        // the fields are one fewer. The header's eight-bit count reads 255
        // for an object of 255 words or more, whose true count the heap
        // keeps in an overflow word; the stub takes 255 as it is, so such
        // an object has at least 254 fields here, more than any getter
        // stub's field (`GETTER_STUBS`, asserted below), and no field a
        // stub names is refused for it.
        assembler.shift_right_arithmetic(answer, layout.word_count_shift);
        assembler.and_immediate(answer, WORD_COUNT_FIELD_MASK);
        assembler.add_immediate(answer, -1);
        assembler.compare(answer, field);
        assembler.jump_if(Condition::BelowOrEqual, decline);
        assembler.shift_left(field, WORD_SHIFT);
        assembler.add(address, field);
        assembler.load(
            Width::Word64,
            answer,
            Memory {
                base: address,
                displacement: layout.fields_displacement,
            },
        );
        if holds_class {
            assembler.test_immediate(answer, layout.tag_mask);
            assembler.jump_if(Condition::NotEqual, decline);
        }
        assembler.return_();
    }
    let assembler = macro_assembler.assembler();
    // The setter: the getter's checks, then the send's argument, in its
    // slot 0, stored through the barrier, which keeps the receiver on
    // both its paths: the setter answers it.
    assembler.bind(setter);
    macro_assembler.jump_unless_object(receiver, decline);
    let assembler = macro_assembler.assembler();
    assembler.move_register(element, receiver);
    assembler.add(element, roles.object_space);
    assembler.load(
        Width::Word64,
        spare,
        Memory {
            base: element,
            displacement: layout.header_displacement,
        },
    );
    assembler.test_immediate(spare, layout.instance_format_bits);
    assembler.jump_if(Condition::NotEqual, decline);
    // The count as the getter takes it: a setter stub's field is below
    // every count the header's mark stands for (`SETTER_STUBS`).
    assembler.shift_right_arithmetic(spare, layout.word_count_shift);
    assembler.and_immediate(spare, WORD_COUNT_FIELD_MASK);
    assembler.add_immediate(spare, -1);
    assembler.compare(spare, field);
    assembler.jump_if(Condition::BelowOrEqual, decline);
    assembler.shift_left(field, WORD_SHIFT);
    assembler.add(element, field);
    assembler.load(Width::Word64, answer, argument_slot(roles, 0));
    macro_assembler.store_with_barrier_at(
        receiver,
        Memory {
            base: element,
            displacement: layout.fields_displacement,
        },
        answer,
        spare,
    );
    let assembler = macro_assembler.assembler();
    assembler.move_register(answer, receiver);
    assembler.return_();
    for (kind, label) in [
        (AccessorKind::Getter, guarded_getter),
        (AccessorKind::Setter, guarded_setter),
    ] {
        macro_assembler.assembler().bind(label);
        emit_guarded_accessor(
            &mut macro_assembler,
            kind,
            [index, element, spare],
            decline,
            act,
        );
    }
    let row_stubs: Vec<Label> = RowStub::ALL
        .into_iter()
        .map(|stub| {
            emit_row_stub(
                &mut macro_assembler,
                stub,
                [index, element, spare],
                mirror_method,
            )
        })
        .collect();
    let assembler = macro_assembler.assembler();
    // The mirror method: the receiver handed over in the machine words,
    // the row in the field's role, the send found by its return address.
    assembler.bind(mirror_method);
    macro_assembler.load_machine_words(address);
    let handed = MacroAssembler::machine_word(address, MachineWord::CalleeReceiver);
    let assembler = macro_assembler.assembler();
    assembler.store(Width::Word64, handed, receiver);
    assembler.load(
        Width::Word64,
        roles.return_address,
        Memory {
            base: roles.stack_pointer,
            displacement: 0,
        },
    );
    macro_assembler.call_leaf(
        Leaf::MirrorMethod.index(),
        [
            Operand::Register(roles.frame),
            Operand::Register(field),
            Operand::Register(roles.return_address),
        ],
    );
    let assembler = macro_assembler.assembler();
    assembler.compare_immediate(answer, NO_VALUE_IMMEDIATE);
    assembler.jump_if(Condition::Equal, decline);
    assembler.return_();
    // The leaf stub: the receiver handed over in the machine words, the
    // cell the send hit in `send_cells` -- its site's own on a direct hit,
    // a block's cell or a global row from a stub, 0 after the miss leaf.
    assembler.bind(act);
    macro_assembler.load_machine_words(address);
    let handed = MacroAssembler::machine_word(address, MachineWord::CalleeReceiver);
    let assembler = macro_assembler.assembler();
    assembler.store(Width::Word64, handed, receiver);
    assembler.load(
        Width::Word64,
        roles.return_address,
        Memory {
            base: roles.stack_pointer,
            displacement: 0,
        },
    );
    macro_assembler.call_leaf(
        Leaf::Act.index(),
        [
            Operand::Register(roles.frame),
            Operand::Register(roles.return_address),
            Operand::Register(roles.send_cells),
        ],
    );
    let assembler = macro_assembler.assembler();
    assembler.compare_immediate(answer, NO_VALUE_IMMEDIATE);
    assembler.jump_if(Condition::Equal, decline);
    assembler.return_();
    assembler.bind(decline);
    macro_assembler.exit(ExitKind::PushRefused, 0);
    let assembled = finish_stub(macro_assembler);
    let offset = |label: Label| match assembled.label_offsets[label.index()] {
        Some(offset) => offset,
        None => unreachable!("internal: every accessor entry is bound"),
    };
    let entries = AccessorEntries {
        getters: getters.into_iter().map(offset).collect(),
        setters: setters.into_iter().map(offset).collect(),
        parameters: parameters.into_iter().map(offset).collect(),
        mirror_methods: mirror_methods.into_iter().map(offset).collect(),
        row_stubs: row_stubs.into_iter().map(offset).collect(),
        nil: offset(nil),
        true_constant: offset(true_constant),
        false_constant: offset(false_constant),
        receiver: offset(receiver_entry),
        act: offset(act),
        guarded_getters: guarded_getters.into_iter().map(offset).collect(),
        guarded_setters: guarded_setters.into_iter().map(offset).collect(),
        nested_classes: nested_classes.into_iter().map(offset).collect(),
    };
    (assembled, entries)
}

/// The shared part of a getter's or a setter's stub of a private or
/// `super` chain, the field in the field's role: `act`'s guard check made
/// inline -- `Mutator::slot_level_holds`'s first compare, the receiver's
/// guard against the one its entry recorded, which the cell's chain start
/// word holds above the start (`cells::Cell`) -- then the read or the
/// write as the public stubs make it. A receiver that is no object, a
/// guard that differs, a cell that recorded none and a send that came
/// from the miss leaf, whose `send_cells` is 0, go on to the leaf stub
/// at `act`, which makes the check the interpreter makes -- the verified
/// pair, the walk -- and the access; a receiver with the guard but not
/// the field declines, as `act` would. `registers` are three registers
/// it may clobber: none is the receiver's, the field's, the answer's or
/// `send_cells`'.
fn emit_guarded_accessor(
    macro_assembler: &mut MacroAssembler,
    kind: AccessorKind,
    registers: [Register; 3],
    decline: Label,
    act: Label,
) {
    let roles = macro_assembler.roles();
    let layout = crate::heap::Heap::machine_layout();
    let [recorded, element, spare] = registers;
    let (receiver, answer, field, cell) = (
        roles.callee_receiver(),
        roles.leaf_result,
        roles.accessor_field,
        roles.send_cells,
    );
    let assembler = macro_assembler.assembler();
    assembler.compare_immediate(cell, 0);
    assembler.jump_if(Condition::Equal, act);
    macro_assembler.jump_unless_object(receiver, act);
    let assembler = macro_assembler.assembler();
    assembler.load(
        Width::Word64,
        recorded,
        Memory {
            base: cell,
            displacement: CELL_CHAIN_START,
        },
    );
    assembler.shift_right_logical(recorded, CHAIN_START_SHIFT);
    // `DispatchId::NONE`, recorded for a receiver without the level, is
    // never a match, whatever guard a header holds.
    assembler.compare_immediate(recorded, 0);
    assembler.jump_if(Condition::Equal, act);
    assembler.move_register(element, receiver);
    assembler.add(element, roles.object_space);
    assembler.load(
        Width::Word64,
        spare,
        Memory {
            base: element,
            displacement: layout.header_displacement,
        },
    );
    assembler.move_register(answer, spare);
    assembler.and_immediate(answer, layout.guard_mask);
    assembler.compare(answer, recorded);
    assembler.jump_if(Condition::NotEqual, act);
    // The guard holds: the public stubs' checks of the field, then the
    // access.
    assembler.test_immediate(spare, layout.instance_format_bits);
    assembler.jump_if(Condition::NotEqual, decline);
    assembler.shift_right_arithmetic(spare, layout.word_count_shift);
    assembler.and_immediate(spare, WORD_COUNT_FIELD_MASK);
    assembler.add_immediate(spare, -1);
    assembler.compare(spare, field);
    assembler.jump_if(Condition::BelowOrEqual, decline);
    assembler.shift_left(field, WORD_SHIFT);
    assembler.add(element, field);
    let slot = Memory {
        base: element,
        displacement: layout.fields_displacement,
    };
    match kind {
        AccessorKind::Getter => assembler.load(Width::Word64, answer, slot),
        AccessorKind::Setter => {
            assembler.load(Width::Word64, answer, argument_slot(roles, 0));
            macro_assembler.store_with_barrier_at(receiver, slot, answer, spare);
            macro_assembler.assembler().move_register(answer, receiver);
        }
    }
    macro_assembler.assembler().return_();
}

/// How far a cell's chain start word is shifted to bring its recorded
/// guard down (`cells::CHAIN_START_BITS`).
#[expect(
    clippy::cast_possible_truncation,
    reason = "the chain start takes two bits"
)]
const CHAIN_START_SHIFT: u8 = CHAIN_START_BITS as u8;

// The guarded stubs test the recorded guard against 0 for `NONE`, and
// `cells::chain_start_word` leaves 0 above the start for an entry that
// recorded none.
const _: () = assert!(crate::mixin::DispatchId::NONE.bits() == 0);

/// The stub of a method whose fast path is the row `stub` serves
/// (`MachineCall::Stub`), at the label it answers: the element its
/// index, in the send's slot 0, names -- read, or written with the value
/// in slot 1 and the value answered -- once the receiver and the index
/// are checked as the row's leaf checks them; every other case continues
/// at `mirror_method` with the row's number in the field's role, so that
/// the row's leaf decides it. `registers` are three registers it may
/// clobber: none is the receiver's, the field's or the answer's.
fn emit_row_stub(
    macro_assembler: &mut MacroAssembler,
    stub: RowStub,
    registers: [Register; 3],
    mirror_method: Label,
) -> Label {
    let roles = macro_assembler.roles();
    let layout = crate::heap::Heap::machine_layout();
    let [index, element, spare] = registers;
    let (receiver, answer) = (roles.callee_receiver(), roles.leaf_result);
    let Some(row) = MIRROR
        .iter()
        .position(|entry| entry.machine_call == MachineCall::Stub(stub))
    else {
        unreachable!("internal: every row stub serves a row");
    };
    let assembler = macro_assembler.assembler();
    let (entry, fallback) = (assembler.label(), assembler.label());
    assembler.bind(entry);
    assembler.load(Width::Word64, index, argument_slot(roles, 0));
    match stub {
        RowStub::ArrayAt => {
            macro_assembler.load_element_address(receiver, index, element, fallback);
            let assembler = macro_assembler.assembler();
            assembler.load(Width::Word64, answer, word_at(element));
        }
        RowStub::ArrayAtPut => {
            macro_assembler.load_element_address(receiver, index, element, fallback);
            macro_assembler
                .assembler()
                .load(Width::Word64, answer, argument_slot(roles, 1));
            macro_assembler.store_with_barrier_at(receiver, word_at(element), answer, spare);
            // The barrier's leaf keeps no scratch register: the value
            // answered is read again.
            macro_assembler
                .assembler()
                .load(Width::Word64, answer, argument_slot(roles, 1));
        }
        RowStub::ByteArrayAt => {
            macro_assembler.load_byte_address(receiver, index, element, spare, fallback);
            let assembler = macro_assembler.assembler();
            assembler.load(Width::Byte, answer, word_at(element));
            assembler.shift_left(answer, layout.tag_bits);
            assembler.or_immediate(answer, layout.small_integer_tag);
        }
        RowStub::ByteArrayAtPut => {
            macro_assembler.load_byte_address(receiver, index, element, spare, fallback);
            let assembler = macro_assembler.assembler();
            assembler.load(Width::Word64, answer, argument_slot(roles, 1));
            // A `SmallInteger` from 0 to 255: tagged, and its word below
            // 256's, unsigned, which a negative one's is not.
            assembler.test_immediate(answer, layout.small_integer_tag);
            assembler.jump_if(Condition::Equal, fallback);
            assembler.compare_immediate(answer, BYTE_LIMIT << layout.tag_bits);
            assembler.jump_if(Condition::AboveOrEqual, fallback);
            assembler.move_register(spare, answer);
            assembler.shift_right_logical(spare, layout.tag_bits);
            assembler.store(Width::Byte, word_at(element), spare);
        }
    }
    let assembler = macro_assembler.assembler();
    assembler.return_();
    assembler.bind(fallback);
    assembler.move_immediate(roles.accessor_field, row as u64);
    assembler.jump(mirror_method);
    entry
}

/// The send's argument slot `position`, where the callee's register 0
/// and on lie: the frame register points at the first.
fn argument_slot(roles: Roles, position: i32) -> Memory {
    Memory {
        base: roles.frame,
        displacement: position * WORD_BYTES,
    }
}

/// The word at `address`.
fn word_at(address: Register) -> Memory {
    Memory {
        base: address,
        displacement: 0,
    }
}

/// One past the largest byte, which `at:put:` on a ByteArray stores.
const BYTE_LIMIT: i32 = 256;

/// The eight bits of the header's word count, once shifted down.
const WORD_COUNT_FIELD_MASK: i32 = 0xFF;

const _: () = assert!((GETTER_STUBS as i32) < WORD_COUNT_FIELD_MASK - 1);
const _: () = assert!((NESTED_CLASS_STUBS as i32) < WORD_COUNT_FIELD_MASK - 1);
const _: () = assert!((SETTER_STUBS as i32) < WORD_COUNT_FIELD_MASK - 1);

/// How far a field's index is shifted to be its byte offset.
const WORD_SHIFT: u8 = 3;

/// The word no value is (`Word::NO_VALUE_BITS`), as an immediate operand.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the word is a tag over a payload of 0, asserted below"
)]
const NO_VALUE_IMMEDIATE: i32 = crate::heap::Word::NO_VALUE_BITS as i32;

const _: () = assert!(crate::heap::Word::NO_VALUE_BITS < 8);

/// These run machine code, so they are x64's whatever `cfg(ns_jit)` says.
#[cfg(all(test, target_arch = "x86_64"))]
mod tests {
    use ns_assembler::Memory;

    use super::emit_group;
    use crate::interpreter::inline_cache::{rule_word, CacheRule, GlobalCache};
    use crate::jit::stubs::testing::Machine;
    use crate::jit::stubs::{ExitKind, MachineExit};
    use crate::mixin::DispatchId;
    use crate::Symbol;

    /// The megamorphic stub's hash (`emit_group`) is the global cache's
    /// (`GlobalCache::group`), whose hasher's constants it shares
    /// (`hash::MULTIPLIER`, `hash::FINISH_ROTATION`): over every rule,
    /// guards and selectors from the smallest to the widest, and a lexical
    /// rule's N with them, at every group count from the one group of the
    /// contention gate to the most the mirror holds, the group machine
    /// code picks is the one the cache files the key in.
    #[test]
    fn the_megamorphic_hash_is_the_global_caches() {
        let mut machine = Machine::new();
        // The key in registers 0 to 2, the mask in 3; the group into 4.
        let body = machine.body(|macro_assembler| {
            let roles = macro_assembler.roles();
            macro_assembler.load_frame_register(roles.send_guard, 0);
            macro_assembler.load_frame_register(roles.send_key, 1);
            macro_assembler.load_frame_register(roles.lookup_rule, 2);
            emit_group(
                macro_assembler,
                Memory {
                    base: roles.frame,
                    displacement: 3 * 8,
                },
            );
            macro_assembler.store_frame_register(4, roles.lookup_hash);
            macro_assembler.exit(ExitKind::Reexecute, 0);
        });
        let span = DispatchId::LIMIT - DispatchId::FIRST_MINTED;
        let guards = (0..40u32)
            .map(|number| DispatchId::FIRST_MINTED + (number * 104_729) % span)
            .chain([DispatchId::FIRST_MINTED, DispatchId::LIMIT - 1]);
        let mut guards: Vec<DispatchId> = guards.map(DispatchId::from_counter).collect();
        guards.push(DispatchId::NONE);
        let selectors = [0u32, 1, 7, 255, 4096, 65_535, 1 << 20, u32::MAX >> 1];
        let mut rules = vec![
            CacheRule::Public,
            CacheRule::LexicalPrivate,
            CacheRule::Super,
        ];
        rules.extend(
            guards
                .iter()
                .take(8)
                .map(|guard| CacheRule::Lexical(*guard)),
        );
        rules.push(CacheRule::Lexical(DispatchId::NONE));
        let mut compared = 0;
        for groups in [1usize, 2, 512, 1 << 19] {
            let cache = GlobalCache::with_groups(groups);
            for guard in &guards {
                for selector in selectors {
                    let selector = Symbol::from_index(selector);
                    for rule in &rules {
                        let mut frame = [
                            u64::from(guard.bits()),
                            selector.index() as u64,
                            rule_word(*rule),
                            groups as u64 - 1,
                            u64::MAX,
                            0,
                            0,
                            0,
                        ];
                        let (exit, _) = machine.run(body, &mut frame, 0, false, &mut (), &[]);
                        assert_eq!(exit, MachineExit::Reexecute { program_counter: 0 });
                        let group = cache.group(*guard, selector, *rule);
                        assert_eq!(
                            usize::try_from(frame[4]).expect("a group index") * 2,
                            group,
                            "guard {}, selector {}, rule word {}, {groups} groups",
                            guard.bits(),
                            selector.index(),
                            rule_word(*rule)
                        );
                        compared += 1;
                    }
                }
            }
        }
        assert!(compared > 10_000);
    }
}
