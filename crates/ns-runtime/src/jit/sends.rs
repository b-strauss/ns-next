//! A send in machine code, and what a compiled call enters and leaves
//! through (`docs/design/execution.md`, "Sends in machine code"): the
//! call through a site's cell, the callee's prologue -- its machine entry,
//! the push's checks and the words of its frame the push writes -- and the
//! return by `ret`. Only a body's lean code has a prologue: its twin is
//! entered by a transfer and by a resumption alone
//! (`baseline::BodyCopy`).
//!
//! Every value stays in its frame register across a send, so a compiled
//! frame's registers are the interpreter's at every send, and the callee
//! lies on the send's argument slots as the interpreter's push lays it:
//! its arguments are those slots, its header follows them, and the call
//! writes the header's first two words, the receiver and the current
//! class, which nothing can derive later, and the callee's prologue the
//! guard and the entered level of code that addresses its own slots,
//! which its own-slot instructions update. Every other header word is
//! written when an exit materializes the frame (`jit::materialize`).

use ns_assembler::{Condition, Label, Memory, Register, Width};

use crate::bytecode::{CallSite, CompiledCode, HEADER_REGISTERS};
use crate::heap::machine_words::MachineWord;
use crate::heap::{Heap, Immediate};
use crate::interpreter::inline_cache::cells::{ChainStart, CELL_BYTES, CELL_WORDS};
use crate::interpreter::inline_cache::{rule_tag, site_index, CacheRule, RULE_TAG_BITS};
use crate::mixin::DispatchId;
use crate::Symbol;

use super::baseline::{BodyAssembler, BodyRole, Lowering};
use super::leaves::Leaf;
use super::macro_assembler::{MacroAssembler, Operand};
use super::optimizing::snapshot::{BodyFacts, SnapshotInstruction};
use super::stubs::{BlockWord, ExitKind};

/// The bytes of a frame register.
pub(super) const REGISTER_BYTES: i32 = 8;

/// A cell's words' displacements from its start (`Cell`'s order).
const CELL_GUARD: i32 = 0;
const CELL_ENTRY: i32 = REGISTER_BYTES;
const CELL_LEVEL: i32 = 2 * REGISTER_BYTES;
const CELL_CHAIN_START: i32 = 3 * REGISTER_BYTES;

const _: () = assert!(CELL_WORDS == 4 && CELL_BYTES == 32);

// A lexical site tests the one bit of its cell's chain start that N's
// sets and no other start's does.
const _: () = assert!(
    ChainStart::LexicalClass as u64 == 1
        && ChainStart::ReceiverClass as u64 & 1 == 0
        && ChainStart::Superclass as u64 & 1 == 0
);

/// The callee's prologue, at its machine entry -- the body's first byte --
/// where a compiled call enters it, the frame register at the callee's
/// register 0 -- on the send's slots, or for a block a closure stub calls
/// past them, where the stub copied a block row's arguments
/// (`targets::ClosureStub`) -- and the call's return address on top of
/// the native stack: the checks the interpreter's push makes, each of
/// which, failing, refuses the call before anything is written, so that
/// the caller re-executes the send and the interpreter decides what the
/// push does -- the depth limit's error, a growth of the activation
/// stack, the safepoint (`Mutator::push_on_slots`); then the count, and
/// the words of the frame the push writes but the header: the named
/// locals nil, the guard and the entered level of code that addresses its
/// own slots (`own_slot_words`) -- but no reified word, program counter
/// or flags word of a method's or a factory's frame, which a lean frame
/// never needs and its transfer into the twin writes
/// (`closures::transfer_to_twin`) -- and in a debug build the dead register's
/// word in the scratch registers and the flags word with the push's
/// generation, the rest of the header the exit's walk's
/// (`jit::materialize`) -- but for an initializer's kind, which its
/// code does not say: the prologue adds the tag the initializer leaf
/// handed over to the flags word (`MacroAssembler::record_frame_kind`);
/// and for a block's what `Step::Block`'s push writes besides
/// (`block_frame_words`): its closure, its copied values and whether it
/// counts, the named locals past the copied values nil; last, the spend
/// of the body's length from its optimization budget
/// (`BodyAssembler::spend_budget`), which the run loop's push, entering
/// past the prologue, does not make.
/// A factory's current class is the class object it runs on, its
/// receiver. The run
/// loop's push writes all of it itself and enters the body past the
/// prologue, at its first instruction; an interpreter's send that calls
/// the body enters at the prologue, through the call stub, as a compiled
/// call does (`jit::called`).
///
/// The depth limit word is a bound, not the limit: at or past it the
/// interpreter decides, as it does while a `doesNotUnderstand:` handler
/// holds the reserve the word does not count (`MachineWord::DepthLimit`).
/// A real safepoint request lowers the word to 0, so that the one compare
/// refuses every push while one is pending and the interpreter takes the
/// safepoint at the push (`Heap::write_safepoint_request`); a stress
/// mode's hold leaves it, and the frames it lets nest take the hold at
/// their polls.
pub(super) fn prologue(body: &mut BodyAssembler, facts: &BodyFacts) {
    let role = body.role;
    let macro_assembler = &mut body.macro_assembler;
    let roles = macro_assembler.roles();
    let machine_words = roles.idiom_scratch;
    let (depth, scratch, end) = (roles.depth_count, roles.frame_extent, roles.stack_end);
    let word = |name| MacroAssembler::machine_word(machine_words, name);
    // Optimized code's frame reaches as far as its deepest inlined scope's.
    let frame_end_bytes = (i32::from(facts.argument_count)
        + i32::from(HEADER_REGISTERS)
        + register_number(facts.registers_above_header()))
    .max(register_number(body.deepest_extent as usize))
        * REGISTER_BYTES;
    let deepest_chain = i32::from(body.deepest_chain);
    let stack_base = macro_assembler.block_word(BlockWord::StackBase);
    // The machine entry is what a compiled call, a stub's tail jump and
    // the call stubs reach.
    macro_assembler.assembler().enter_called();
    macro_assembler.load_machine_words(machine_words);
    let assembler = macro_assembler.assembler();
    assembler.load(Width::Word64, depth, word(MachineWord::DepthCount));
    assembler.compare_memory(depth, word(MachineWord::DepthLimit));
    macro_assembler.exit_if(
        Condition::AboveOrEqual,
        ExitKind::PushRefused,
        super::stubs::REFUSED_AT_THE_BOUND,
    );
    // Every inlined scope of optimized code is an activation the
    // interpreter would push, which an exit inside it materializes: a
    // frame too deep for its deepest chain is refused, for its body's
    // baseline code to push them one by one and overflow exactly where the
    // interpreter does (`jit::entered`).
    if deepest_chain > 0 {
        let assembler = macro_assembler.assembler();
        assembler.move_register(scratch, depth);
        assembler.add_immediate(scratch, deepest_chain);
        assembler.compare_memory(scratch, word(MachineWord::DepthLimit));
        macro_assembler.exit_if(
            Condition::AboveOrEqual,
            ExitKind::PushRefused,
            super::stubs::REFUSED_WITHIN_THE_INLINED_DEPTH,
        );
    }
    let assembler = macro_assembler.assembler();
    assembler.move_register(scratch, roles.frame);
    assembler.load(Width::Word64, end, stack_base);
    assembler.subtract(scratch, end);
    assembler.add_immediate(scratch, frame_end_bytes);
    assembler.load(Width::Word64, end, word(MachineWord::OrdinaryStackEnd));
    assembler.shift_left(end, WORD_SHIFT);
    assembler.compare(scratch, end);
    // Past the ordinary end the growth leaf makes the room the
    // interpreter's push would, and only a frame the ordinary room cannot
    // hold refuses the call.
    macro_assembler.grow_stack_if(
        Condition::Above,
        Leaf::GrowStack.index(),
        scratch,
        ExitKind::PushRefused,
    );
    let assembler = macro_assembler.assembler();
    assembler.add_immediate(depth, 1);
    assembler.store(Width::Word64, word(MachineWord::DepthCount), depth);
    // Only now, every check passed: the receiver and the current class the
    // call handed over.
    let receiver_place = Heap::receiver_register(facts.argument_count);
    assembler.store(
        Width::Word64,
        frame_slot(roles.frame, receiver_place),
        roles.callee_receiver(),
    );
    // A factory runs on the class object it instantiates, which is its
    // receiver: a cell's hit hands over the receiver's class, its
    // metaclass, as the current class.
    let current_class = match role {
        BodyRole::Factory => roles.callee_receiver(),
        BodyRole::Method | BodyRole::Initializer | BodyRole::Block | BodyRole::EnteredOnly => {
            roles.callee_class()
        }
    };
    assembler.store(
        Width::Word64,
        frame_slot(
            roles.frame,
            Heap::current_class_register(facts.argument_count),
        ),
        current_class,
    );
    if facts.addresses_own_slots {
        own_slot_words(macro_assembler, facts.argument_count, scratch, end);
    }
    let assembler = macro_assembler.assembler();
    let header_end = facts.argument_count + HEADER_REGISTERS;
    // A block's copied values follow its header, which its call fills
    // below rather than nils.
    let copied = if role == BodyRole::Block {
        facts.copied_count
    } else {
        0
    };
    for register in header_end.saturating_add(copied)..facts.local_count.max(header_end) {
        let nil = i32::try_from(Immediate::NIL.bits()).expect("internal: nil is a small word");
        assembler.store_immediate32(frame_slot(roles.frame, register), nil);
    }
    #[cfg(debug_assertions)]
    debug_frame_words(body, facts);
    // Last, over the flags word a debug build's generation was just
    // written into, and over the dead register's word in a scratch register
    // a copied value goes into.
    match role {
        BodyRole::Initializer => body
            .macro_assembler
            .record_frame_kind(facts.argument_count, scratch),
        BodyRole::Block => block_frame_words(&mut body.macro_assembler, facts),
        BodyRole::Method | BodyRole::Factory | BodyRole::EnteredOnly => {}
    }
    // After every word of the frame is written, where the call holds
    // nothing in a scratch register that the budget leaf's call could
    // clobber.
    body.spend_budget(facts.instruction_count, 0);
}

/// What `Step::Block`'s push writes into a block's frame besides what
/// every push writes (`Mutator::push_step`), from what the call handed
/// over: the closure as the frame's operand (`Roles::callee_closure`),
/// read before anything else uses its register; the argument the call
/// places itself, `cull:`'s receiver, into a block of one parameter
/// (`Roles::callee_argument`); the closure's copied values into the
/// registers after the header, in order, as `Heap::fill_copied_values`
/// copies them -- after a debug build's dead register's word, which a
/// copied value past the named locals overwrites as the push's does; and
/// what the call adds to the flags word (`Roles::callee_counted`): the
/// `counted` bit as the call said, which a frame the exit materializes
/// keeps (`Heap::write_machine_frame`), and the slots it shifted the frame
/// by, which the block's return reads (`unshift_block_frame`). Clobbers
/// the flags, `frame_extent`, `stack_end` and the idiom scratch register.
fn block_frame_words(macro_assembler: &mut MacroAssembler, facts: &BodyFacts) {
    let roles = macro_assembler.roles();
    let layout = Heap::machine_layout();
    let (closure, address, value) = (roles.callee_closure(), roles.frame_extent, roles.stack_end);
    let argument_count = facts.argument_count;
    let assembler = macro_assembler.assembler();
    assembler.store(
        Width::Word64,
        frame_slot(roles.frame, Heap::operand_register(argument_count)),
        closure,
    );
    if argument_count == 1 {
        let placed = assembler.label();
        let no_value = i32::try_from(crate::heap::Word::NO_VALUE_BITS)
            .expect("internal: the word no value is is a small constant");
        assembler.compare_immediate(roles.callee_argument(), no_value);
        assembler.jump_if(Condition::Equal, placed);
        assembler.store(
            Width::Word64,
            frame_slot(roles.frame, 0),
            roles.callee_argument(),
        );
        assembler.bind(placed);
    }
    if facts.copied_count > 0 {
        assembler.move_register(address, closure);
        assembler.add(address, roles.object_space);
        let first = i32::try_from(layout.closure_fixed_fields)
            .ok()
            .and_then(|fields| fields.checked_mul(REGISTER_BYTES))
            .and_then(|bytes| bytes.checked_add(layout.fields_displacement))
            .expect("internal: a closure's fixed fields are few");
        let header_end = argument_count + HEADER_REGISTERS;
        for position in 0..facts.copied_count {
            let displacement = i32::from(position)
                .checked_mul(REGISTER_BYTES)
                .and_then(|bytes| bytes.checked_add(first))
                .expect("internal: a closure's copied values lie within 2 GiB of its start");
            assembler.load(
                Width::Word64,
                value,
                Memory {
                    base: address,
                    displacement,
                },
            );
            assembler.store(
                Width::Word64,
                frame_slot(roles.frame, header_end + position),
                value,
            );
        }
    }
    record_counted(macro_assembler, argument_count);
}

/// The flags word of a block's frame taking `argument_count` arguments
/// with what `callee_counted` holds added: the `counted` bit, set or not
/// as the call said, and the slots the call shifted the frame by
/// (`frames::CALL_SHIFT_BITS`) -- over the word a debug build's
/// generation was written into, or a release build's constant flags.
/// Clobbers the idiom scratch register.
fn record_counted(macro_assembler: &mut MacroAssembler, argument_count: u16) {
    let roles = macro_assembler.roles();
    let (flags, addend) = (roles.idiom_scratch, roles.callee_counted());
    let place = frame_slot(roles.frame, argument_count + Heap::flags_register());
    let counted_bit = i32::try_from(Heap::machine_counted_word())
        .expect("internal: the counted bit lies in the flags word's low half");
    let assembler = macro_assembler.assembler();
    #[cfg(debug_assertions)]
    assembler.load(Width::Word64, flags, place);
    #[cfg(not(debug_assertions))]
    assembler.move_immediate(flags, Heap::machine_flags_word(argument_count));
    // Both words say counted: the bit taken off, and the call's put back.
    assembler.add_immediate(flags, -counted_bit);
    assembler.add(flags, addend);
    assembler.store(Width::Word64, place, flags);
}

/// The guard and the entered level of a frame whose code addresses its own
/// slots, as the push records them (`Heap::push_activation`): the entered
/// level is the current class's application, the class object's
/// application word -- already the `SmallInteger` the header holds -- or
/// `UNREGISTERED` for a class that names none; the guard is the
/// receiver's when the receiver's own class is that level, the first
/// compare of `Heap::receiver_has_level`, and none otherwise, so that the
/// first own-slot access takes the cold check, which walks the receiver's
/// chain for the level and records its guard (`jit::baseline`'s own-slot
/// templates). Written here, once, rather than by the exit's walk, which
/// would throw a recorded guard away (`Heap::write_machine_frame`).
/// Clobbers the flags, `scratch` and `spare`.
fn own_slot_words(
    macro_assembler: &mut MacroAssembler,
    argument_count: u16,
    scratch: Register,
    spare: Register,
) {
    let roles = macro_assembler.roles();
    let layout = Heap::machine_layout();
    let (guard, entered_level) = Heap::own_slot_registers(argument_count);
    let (guard, entered_level) = (
        frame_slot(roles.frame, guard),
        frame_slot(roles.frame, entered_level),
    );
    let (none_word, unregistered_word) = Heap::machine_own_slot_words();
    let none_word = i32::try_from(none_word).expect("internal: the guard none is a small word");
    let receiver = roles.callee_receiver();
    macro_assembler.load_application_word(scratch, roles.callee_class());
    let assembler = macro_assembler.assembler();
    let (registered, recorded) = (assembler.label(), assembler.label());
    assembler.test_immediate(scratch, layout.small_integer_tag);
    assembler.jump_if(Condition::NotEqual, registered);
    assembler.move_immediate(scratch, unregistered_word);
    assembler.bind(registered);
    assembler.store(Width::Word64, entered_level, scratch);
    assembler.store_immediate32(guard, none_word);
    macro_assembler.jump_unless_object(receiver, recorded);
    let assembler = macro_assembler.assembler();
    assembler.move_register(spare, receiver);
    assembler.add(spare, roles.object_space);
    assembler.load(
        Width::Word64,
        spare,
        Memory {
            base: spare,
            displacement: layout.class_word_displacement,
        },
    );
    macro_assembler.load_application_word(spare, spare);
    let assembler = macro_assembler.assembler();
    assembler.compare(spare, scratch);
    assembler.jump_if(Condition::NotEqual, recorded);
    assembler.move_register(spare, receiver);
    assembler.add(spare, roles.object_space);
    assembler.load(
        Width::Word64,
        spare,
        Memory {
            base: spare,
            displacement: layout.header_displacement,
        },
    );
    // The frame holds the guard as a `SmallInteger`.
    assembler.and_immediate(spare, layout.guard_mask);
    assembler.shift_left(spare, layout.tag_bits);
    assembler.or_immediate(spare, layout.small_integer_tag);
    assembler.store(Width::Word64, guard, spare);
    assembler.bind(recorded);
}

/// A debug build's words of a pushed frame the interpreter's push writes
/// and the walk does not: the dead register's word in every scratch
/// register (`heap.rs`'s `fill_temporaries`), and the flags word with the
/// generation the push takes from the machine words, so that a handle to
/// a frame since returned and pushed again is refused; and the reified
/// word nil, which a release build's lean frame leaves as the stack held
/// it, so that a transfer can hold the frame to being unreified
/// (`closures::transfer_to_twin`).
#[cfg(debug_assertions)]
fn debug_frame_words(body: &mut BodyAssembler, facts: &BodyFacts) {
    let macro_assembler = &mut body.macro_assembler;
    let roles = macro_assembler.roles();
    let header_end = facts.argument_count + HEADER_REGISTERS;
    let first_scratch = facts.local_count.max(header_end);
    let window_end = header_end + u16::try_from(facts.registers_above_header()).unwrap_or(u16::MAX);
    let poison = roles.poison;
    let assembler = macro_assembler.assembler();
    assembler.move_immediate(poison, crate::heap::Word::DEAD_REGISTER_BITS);
    for register in first_scratch..window_end {
        assembler.store(Width::Word64, frame_slot(roles.frame, register), poison);
    }
    let reified = Heap::machine_divorce(facts.argument_count).reified_register;
    let nil = i32::try_from(Immediate::NIL.bits()).expect("internal: nil is a small word");
    assembler.store_immediate32(frame_slot(roles.frame, reified), nil);
    macro_assembler.store_frame_generation(facts.argument_count);
}

/// Where the call machine code makes at an instruction lays its callee,
/// and the selector the leaves a callee calls read of it
/// (`JitState::call_site`).
#[derive(Clone, Copy)]
pub(crate) struct CallSlots {
    /// The first slot: the callee's register 0.
    pub(crate) argument_base: u16,
    /// How many arguments the instruction places there.
    pub(crate) argument_count: u16,
    pub(crate) selector: Symbol,
}

/// Where the instruction of `code` at `program_counter` lays the callee
/// its call enters: a cached send's and an initializer invocation's on
/// its argument slots, `InitializeInstance`'s above the factory's window,
/// where `Mutator::push_above` pushes it, with the factory's own
/// arguments copied there, and `ResolveFuture`'s, a future's `resolve`,
/// above the window with no argument, where the interpreter's push of it
/// lays it. `None` for an instruction that calls nothing.
pub(crate) fn call_slots(code: &CompiledCode, program_counter: u32) -> Option<CallSlots> {
    // A cached send is read off its slot without decoding the
    // instruction, which would build its argument list.
    if let Some(send) = code.cached_send(program_counter as usize) {
        return Some(CallSlots {
            argument_base: send.argument_base,
            argument_count: crate::heap::register_number(send.argument_count),
            selector: send.selector,
        });
    }
    // A closure's creation calls nothing, so the form that needs no heap
    // answers for every instruction that might.
    let instruction =
        SnapshotInstruction::of_unclosed(&code.instruction(program_counter as usize))?;
    call_slots_of(&BodyFacts::of(code), &instruction)
}

/// `call_slots` from the snapshot form of the instruction.
pub(crate) fn call_slots_of(
    facts: &BodyFacts,
    instruction: &SnapshotInstruction,
) -> Option<CallSlots> {
    let selector = match instruction {
        SnapshotInstruction::SendLexical { selector, .. }
        | SnapshotInstruction::SendPublic { selector, .. }
        | SnapshotInstruction::SendSuper { selector, .. } => {
            let (argument_base, sources) = instruction.argument_slots()?;
            return Some(CallSlots {
                argument_base,
                argument_count: crate::heap::register_number(sources.len()),
                selector: *selector,
            });
        }
        SnapshotInstruction::InitializeSuperclass { selector, .. } => *selector,
        SnapshotInstruction::InitializeAppliedLevel { .. } => Symbol::NEW,
        SnapshotInstruction::InitializeInstance { .. } => {
            return Some(CallSlots {
                argument_base: facts.window_end(),
                argument_count: facts.argument_count,
                selector: Symbol::NEW,
            });
        }
        SnapshotInstruction::ResolveFuture { .. } => {
            return Some(CallSlots {
                argument_base: facts.window_end(),
                argument_count: 0,
                selector: Symbol::RESOLVE,
            });
        }
        _ => return None,
    };
    let (argument_base, sources) = instruction.argument_slots()?;
    Some(CallSlots {
        argument_base,
        argument_count: crate::heap::register_number(sources.len()),
        selector,
    })
}

/// One past the last register of a frame running `code`: its window's
/// end, above which a callee no send lays on the slots is pushed
/// (`InitializeInstance`'s, as `Mutator::push_above` pushes it).
pub(crate) fn window_end(code: &CompiledCode) -> u16 {
    BodyFacts::of(code).window_end()
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
pub(super) fn frame_slot(frame: Register, register: u16) -> Memory {
    Memory {
        base: frame,
        displacement: i32::from(register) * REGISTER_BYTES,
    }
}

/// `Return` and `ImplicitReturn`: the frame's `register` into the value
/// register, the count taken back, and `ret` to the caller's send, which
/// stores the value -- or to the entry stub, for the frame the run loop
/// entered, whose pop the run loop makes (`stubs::assemble_entry_stub`),
/// or to the call stub, for a callee an interpreter's send called, which
/// the send's destination takes the value of (`stubs::assemble_call_stub`).
/// A frame pushed by compiled code since the last exit delivers its value,
/// so the return tests no continuation. A frame running a body's lean
/// code is never reified -- its `MakeClosure` transfers it into the twin
/// first, and a frame the interpreter reified goes on in machine code only
/// in the twin (`jit::resumed`) -- so only the twin tests the reified
/// word, and divorces a reified frame first, as the pop does
/// (`divorce_if_reified`).
pub(super) fn return_from(body: &mut BodyAssembler, register: u16) -> Lowering {
    if body.reifies {
        divorce_if_reified(body);
    }
    let leaf_result = body.macro_assembler.roles().leaf_result;
    body.macro_assembler
        .load_frame_register(leaf_result, register);
    return_loaded(body);
    Lowering::Transfer
}

/// The rest of `return_from` once the value is in the value register
/// (`Roles::leaf_result`): a block's frame moved back down, the count
/// taken back and `ret`. Optimized code, which never reifies its frame,
/// returns a value it holds in a machine register through it.
pub(super) fn return_loaded(body: &mut BodyAssembler) {
    let argument_count = body.argument_count;
    let block = body.role == BodyRole::Block;
    let macro_assembler = &mut body.macro_assembler;
    if block {
        unshift_block_frame(macro_assembler, argument_count);
    }
    return_value(macro_assembler);
}

/// The frame register of a block's frame taking `argument_count`
/// arguments moved back down by the slots its call shifted it up by
/// (`frames::CALL_SHIFT_BITS`, 0 for a call that shifted nothing, and for
/// a frame the run loop pushed), so that the send it returns to finds its
/// own frame where it left it. Clobbers the flags and `stack_end`.
fn unshift_block_frame(macro_assembler: &mut MacroAssembler, argument_count: u16) {
    let roles = macro_assembler.roles();
    let shift = roles.stack_end;
    let place = frame_slot(roles.frame, argument_count + Heap::flags_register());
    let assembler = macro_assembler.assembler();
    assembler.load(Width::Word64, shift, place);
    assembler.shift_right_logical(shift, Heap::machine_call_shift_shift());
    assembler.shift_left(shift, WORD_SHIFT);
    assembler.subtract(roles.frame, shift);
}

/// The divorce of a reified frame at its return, as the pop divorces it
/// (`Heap::pop_activation`), for code that reifies its frame: nothing for
/// a frame whose reified word is nil; for one that retains its registers,
/// the divorce leaf, which copies them and remembers the frame object as
/// `Heap::divorce` does (`Leaf::Divorce`); otherwise, inline, the frame
/// object's program counter the one its header holds, its sender nil,
/// `uncontinuable` true and married nil. Then the reified word is nil, so
/// that the pop of a frame the run loop entered machine code at divorces
/// nothing again. Every word stored into the frame object is an
/// immediate, which the write barrier passes over at its first early
/// out, so none takes it. Clobbers the flags and every scratch register.
fn divorce_if_reified(body: &mut BodyAssembler) {
    let program_counter = body.program_counter;
    let divorce = Heap::machine_divorce(body.argument_count);
    let macro_assembler = &mut body.macro_assembler;
    let roles = macro_assembler.roles();
    let nil = i32::try_from(Immediate::NIL.bits()).expect("internal: nil is a small word");
    // Two of the scratch registers, which the return is free to use.
    let (frame_object, word) = (roles.scratch[0], roles.scratch[1]);
    let field = |displacement: i32| Memory {
        base: frame_object,
        displacement,
    };
    let assembler = macro_assembler.assembler();
    let (unreified, retained) = (assembler.label(), assembler.label());
    assembler.load(
        Width::Word64,
        frame_object,
        frame_slot(roles.frame, divorce.reified_register),
    );
    assembler.compare_immediate(frame_object, nil);
    assembler.jump_if(Condition::Equal, unreified);
    assembler.load(
        Width::Word64,
        word,
        frame_slot(roles.frame, divorce.flags_register),
    );
    assembler.test_immediate(word, divorce.retains_registers_bit);
    assembler.jump_if(Condition::NotEqual, retained);
    assembler.add(frame_object, roles.object_space);
    assembler.load(
        Width::Word64,
        word,
        frame_slot(roles.frame, divorce.program_counter_register),
    );
    assembler.store(
        Width::Word64,
        field(divorce.program_counter_displacement),
        word,
    );
    assembler.store_immediate32(field(divorce.sender_displacement), nil);
    assembler.store_immediate32(field(divorce.uncontinuable_displacement), divorce.true_word);
    assembler.store_immediate32(field(divorce.married_displacement), nil);
    assembler.store_immediate32(frame_slot(roles.frame, divorce.reified_register), nil);
    assembler.bind(unreified);
    // The leaf divorces and clears the reified word itself
    // (`Heap::divorce_at_machine_return`).
    macro_assembler.call_leaf_out_of_line(
        retained,
        Leaf::Divorce.index(),
        [
            Operand::Register(roles.frame),
            Operand::Immediate(u64::from(program_counter)),
            Operand::CodeAddress,
        ],
        roles.leaf_result,
        unreified,
    );
}

/// For a body's twin, the program counter of the
/// instruction being lowered stored into the frame's header, as the
/// interpreter stores it at the send that instruction is (`vm::send`,
/// `vm::send_mirror`) and at a `MakeClosure` (`vm::make_closure_into`),
/// so that the frame's divorce records the one the interpreter's would:
/// after every exit of the instruction, as every store a template makes
/// into its frame -- right before the call of a send through its cell, on
/// the paths that go on past an initializer invocation's leaf or a
/// mirror send's, neither of which reads it, and first in a
/// `MakeClosure`, which has no exit. Lean code stores nothing: no divorce
/// reads a lean frame's.
pub(super) fn store_program_counter(body: &mut BodyAssembler) {
    if !body.reifies {
        return;
    }
    let register = Heap::machine_divorce(body.argument_count).program_counter_register;
    let word = Heap::machine_program_counter_word(body.program_counter);
    let frame = body.macro_assembler.roles().frame;
    body.macro_assembler
        .assembler()
        .store_immediate32(frame_slot(frame, register), word);
}

/// The return of the value in the value register (`Roles::leaf_result`):
/// the count taken back and `ret`, as `return_from` returns -- also
/// `ReturnSlotIfFilled`'s return of a filled lazy slot (`jit::baseline`).
pub(super) fn return_value(macro_assembler: &mut MacroAssembler) {
    let roles = macro_assembler.roles();
    let machine_words = roles.idiom_scratch;
    macro_assembler.load_machine_words(machine_words);
    let depth = MacroAssembler::machine_word(machine_words, MachineWord::DepthCount);
    let assembler = macro_assembler.assembler();
    let count = roles.depth_count;
    assembler.load(Width::Word64, count, depth);
    assembler.add_immediate(count, -1);
    assembler.store(Width::Word64, depth, count);
    assembler.return_();
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

/// An initializer invocation -- `InitializeInstance`, `InitializeSuperclass`
/// or `InitializeAppliedLevel` -- as a compiled call of the body it
/// decides (`docs/design/execution.md`, "Sends in machine code"): its
/// arguments placed where its callee lies -- `slots`, the argument slots
/// and their sources, or, for `InitializeInstance`, `None`, the factory's
/// own arguments copied above its window, where `Mutator::push_above`
/// pushes the initializer, once the stack is found to have room for them
/// -- then the initializer leaf (`Leaf::Initializer`), which decides the
/// invocation as `vm.rs`'s arm does and answers the callee's machine entry,
/// compiling it at its second entry, with its receiver, its level and its
/// kind handed over in the machine words; or, for a decision that runs no
/// code, the stub answering its receiver, the instance; or 0, at which the
/// invocation exits, nothing written but the argument slots, so that the
/// interpreter decides it again and raises its error or pushes the body.
/// The callee is called as a send's is, and its value stored into
/// `destination`. A placement that needs a register to spare exits.
pub(super) fn initialize(
    body: &mut BodyAssembler,
    slots: Option<(u16, &[u16])>,
    destination: u16,
) -> Lowering {
    let program_counter = body.program_counter;
    let argument_count = body.argument_count;
    let (argument_base, count) = match slots {
        Some((argument_base, sources)) => {
            let Ok(count) = u16::try_from(sources.len()) else {
                return Lowering::Exit;
            };
            let placement = Placement::of(argument_base, sources);
            let placement_registers = body.macro_assembler.roles().placement.len();
            if placement.moves.len() > placement_registers && placement.overlaps() {
                return Lowering::Exit;
            }
            place(&mut body.macro_assembler, &placement);
            (argument_base, count)
        }
        None => {
            let argument_base = body.window_end;
            if argument_base.checked_add(argument_count).is_none() {
                return Lowering::Exit;
            }
            copy_above_window(body, argument_base);
            (argument_base, argument_count)
        }
    };
    let macro_assembler = &mut body.macro_assembler;
    let roles = macro_assembler.roles();
    macro_assembler.call_leaf(
        Leaf::Initializer.index(),
        [
            Operand::Register(roles.frame),
            Operand::Immediate(u64::from(program_counter)),
            Operand::CodeAddress,
        ],
    );
    call_decided(body, argument_base, count, destination);
    Lowering::Transfer
}

/// The call of the callee a leaf decided and answered in `leaf_result`
/// -- its machine entry, or a stub's -- with its receiver and current
/// class in the machine words, its register 0 at `argument_base`, where
/// `count` arguments lie, and its value stored into `destination`; or, for
/// 0, an exit before anything is written but what the instruction placed
/// for its callee, so that the interpreter decides the instruction again.
/// The tail of an initializer invocation's call and of a future's forcing
/// (`closures::resolve_future`).
pub(super) fn call_decided(
    body: &mut BodyAssembler,
    argument_base: u16,
    count: u16,
    destination: u16,
) {
    let program_counter = body.program_counter;
    let macro_assembler = &mut body.macro_assembler;
    let roles = macro_assembler.roles();
    let entry = roles.send_entry;
    let assembler = macro_assembler.assembler();
    assembler.compare_immediate(roles.leaf_result, 0);
    macro_assembler.exit_if(Condition::Equal, ExitKind::Reexecute, program_counter);
    // After the last exit, as every template's store into its frame: the
    // leaf decided without reading it.
    store_program_counter(body);
    let macro_assembler = &mut body.macro_assembler;
    let machine_words = roles.idiom_scratch;
    macro_assembler.load_machine_words(machine_words);
    let assembler = macro_assembler.assembler();
    assembler.move_register(entry, roles.leaf_result);
    assembler.load(
        Width::Word64,
        roles.callee_receiver(),
        MacroAssembler::machine_word(machine_words, MachineWord::CalleeReceiver),
    );
    assembler.load(
        Width::Word64,
        roles.callee_class(),
        MacroAssembler::machine_word(machine_words, MachineWord::CalleeClass),
    );
    let slots = i32::from(argument_base) * REGISTER_BYTES;
    assembler.add_immediate(roles.frame, slots);
    body.call_send(entry);
    let macro_assembler = &mut body.macro_assembler;
    macro_assembler
        .assembler()
        .add_immediate(roles.frame, -slots);
    #[cfg(debug_assertions)]
    give_back_dead_registers(body, argument_base.saturating_add(count));
    #[cfg(not(debug_assertions))]
    let _ = count;
    body.macro_assembler
        .store_frame_register(destination, roles.leaf_result);
}

/// `InitializeInstance`'s placement: the factory's own arguments, registers
/// `0 .. argument_count`, copied into the registers from `argument_base`,
/// its window's end, once the activation stack's ordinary room is found to
/// reach past them -- the room the callee's prologue checks for its whole
/// frame only after they are written -- and an exit that re-executes the
/// instruction otherwise, nothing written, so that the interpreter's push
/// grows the stack or raises.
fn copy_above_window(body: &mut BodyAssembler, argument_base: u16) {
    let program_counter = body.program_counter;
    let argument_count = body.argument_count;
    let macro_assembler = &mut body.macro_assembler;
    let roles = macro_assembler.roles();
    let (extent, end) = (roles.frame_extent, roles.stack_end);
    let stack_base = macro_assembler.block_word(BlockWord::StackBase);
    let machine_words = roles.idiom_scratch;
    macro_assembler.load_machine_words(machine_words);
    let assembler = macro_assembler.assembler();
    assembler.move_register(extent, roles.frame);
    assembler.load(Width::Word64, end, stack_base);
    assembler.subtract(extent, end);
    assembler.add_immediate(
        extent,
        (i32::from(argument_base) + i32::from(argument_count)) * REGISTER_BYTES,
    );
    assembler.load(
        Width::Word64,
        end,
        MacroAssembler::machine_word(machine_words, MachineWord::OrdinaryStackEnd),
    );
    assembler.shift_left(end, WORD_SHIFT);
    assembler.compare(extent, end);
    // Past the ordinary end the growth leaf makes the room; only where the
    // ordinary room cannot hold the arguments does the instruction go to
    // the interpreter, nothing written. No register lives across the call
    // but the pinned ones: the copy below reads the frame afresh.
    let grow = macro_assembler.assembler().label();
    let rejoin = macro_assembler.assembler().label();
    macro_assembler.assembler().jump_if(Condition::Above, grow);
    macro_assembler.assembler().bind(rejoin);
    macro_assembler.call_leaf_out_of_line_or_exit(
        grow,
        Leaf::GrowStack.index(),
        [
            Operand::Register(extent),
            Operand::Immediate(0),
            Operand::Immediate(0),
        ],
        program_counter,
        rejoin,
    );
    let value = roles.placement[0];
    for argument in 0..argument_count {
        macro_assembler.load_frame_register(value, argument);
        macro_assembler.store_frame_register(argument_base + argument, value);
    }
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
            // The chain start's bit N's sets: the word holds a private or
            // `super` slot accessor's recorded guard above it.
            assembler.load(Width::Word64, level, cell_word(CELL_CHAIN_START));
            assembler.test_immediate(level, ChainStart::LexicalClass as i32);
            assembler.jump_if(Condition::NotEqual, private);
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
    store_program_counter(body);
    body.macro_assembler
        .assembler()
        .add_immediate(roles.frame, slots);
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
