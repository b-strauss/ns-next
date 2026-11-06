//! The machine words: every word machine code reads or writes between two
//! leaf calls other than frame registers and object fields, which lie in
//! the activation stack's and the object space's reservations -- today
//! the safepoint request, which a back edge polls, the depth count and
//! limit, the end of the activation stack's ordinary room, the
//! immediates' classes and guards, and what an allocation in machine code
//! writes and counts -- held in an allocation of their own rather than
//! inline in the heap (`docs/design/execution.md`, "The machine words").
//! Eden's top and limit, which machine code bumps in place, lie in an
//! allocation of `ns-memory`'s, the reservation's own cursor
//! (`ns_memory::SharedCursor`), for the same reason.
//!
//! # Why apart from the heap
//!
//! Machine code runs while `CodeZone::enter` holds the mutator as its
//! context, a `&mut` protected for the whole entry, and a leaf it calls
//! gets a `&mut Mutator` derived from that one. A word inline in the
//! mutator that machine code read through an address taken before the
//! entry would be a foreign access to the protected reference -- undefined
//! behaviour as soon as a leaf wrote it, though no processor would notice
//! and Miri cannot run machine code to say so. So no such word lies in
//! the mutator's own bytes: the area is an allocation the heap points to,
//! which no retag of the mutator, the heap or any reference to them
//! covers.
//!
//! The words are `Cell`s behind an `Rc` that this module never lends
//! mutably (`Rc::get_mut` is never called): Rust reads and writes them
//! only through shared references to interior-mutable cells, which assert
//! no uniqueness, and machine code through the address `address` exposes,
//! derived from the `Rc`'s own pointer without a reference in between. So
//! no access from either side invalidates the other's, at any time -- the
//! address does not even depend on being asked afresh. An `Rc` rather
//! than a `Box`: a `Box` is retagged as unique wherever it moves, as a
//! `&mut` is, an `Rc`'s pointer never.

use std::cell::Cell;
use std::rc::Rc;

/// How many immediates there are: `nil`, `false`, `true`, a `SmallInteger`
/// and a `Float`, the indexes `Value::immediate_index` answers.
pub(crate) const IMMEDIATE_COUNT: usize = 5;

/// Where the first `ImmediateClass` word lies.
const IMMEDIATE_CLASSES: usize = 4;

/// Where the first `ImmediateGuard` word lies.
const IMMEDIATE_GUARDS: usize = IMMEDIATE_CLASSES + IMMEDIATE_COUNT;

/// The words of the area. Every word's index is fixed: the fieldless words
/// 0 to 3, the immediates' classes 4 to 8 and their guards 9 to 13, the
/// next generation 14, the callee's receiver and class 15 and 16, the
/// application id table's address and bound 17 and 18, the objects
/// machine code allocated 19, an `Array`'s class word and header 20 and
/// 21, the callee's kind 22, a closure's class word and header 23 and 24,
/// a frame object's 25 and 26, the code table's epoch 27, the machine
/// entry table's bound 28, the callee closure and whether it counts 29
/// and 30, a future's class word and header 31 and 32, the unpointed
/// budget 33, the deopt stress mode's countdown 34, the rest free for the
/// later stages.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum MachineWord {
    /// Not zero while the run loop's next safepoint must take its slow
    /// path (`Heap::safepoint_requested`); machine code's poll compares
    /// it with zero.
    SafepointRequested,
    /// How many activations count against the depth limit
    /// (`Heap::counted_depth`), suspended ones included: the heap's push
    /// adds one for a counted activation, its pop takes one away.
    DepthCount,
    /// The count at which a counted push is refused:
    /// `MAX_ACTIVATION_DEPTH` while the depth limit's reserve is free,
    /// `MAX_ACTIVATION_DEPTH + DEPTH_RESERVE` while a VM error's `signal`
    /// holds it (`Mutator::set_depth_reserve`) -- and 0 while a real
    /// safepoint request is pending, so that a compiled callee's one
    /// compare refuses its push for the request too
    /// (`Heap::write_safepoint_request`).
    DepthLimit,
    /// Where an ordinary push may end without a growth, in words from the
    /// activation stack's base (`FrameStack::ordinary_end`).
    OrdinaryStackEnd,
    /// The class object of immediate `index`, as the word a heap object
    /// stores for it: rewritten by the heap after every scavenge,
    /// compaction and move (`Mutator::rewrite_machine_classes`).
    ImmediateClass(usize),
    /// The dispatch id an inline cache guards a send to immediate `index`
    /// on (`Heap::dispatch_id`).
    ImmediateGuard(usize),
    /// In a debug build, the generation the last frame pushed took: the
    /// next push, the heap's or a compiled callee's prologue, takes one
    /// past it (`Heap::peek_next_generation`). Unused in a release build.
    #[cfg_attr(
        all(not(debug_assertions), not(test)),
        expect(dead_code, reason = "only a debug build's generation reads it")
    )]
    NextGeneration,
    /// A compiled call's receiver, as a stored word, handed between
    /// machine code and a leaf: the miss leaf writes the receiver it
    /// decided, which the miss stub hands the callee's machine entry
    /// (`Roles::callee_receiver`), and the leaf stub and the mirror method
    /// stub write the send's, which their leaves read
    /// (`Heap::machine_callee_receiver`). Read only right after it was
    /// written.
    CalleeReceiver,
    /// The current class the miss leaf decided, as `CalleeReceiver` is.
    CalleeClass,
    /// The address of the application id table's row 0
    /// (`heap/application_ids.rs`), fixed for the heap's life.
    ApplicationIds,
    /// How many rows of the application id table are recorded: an
    /// application index at or past it is read nowhere.
    ApplicationIdRows,
    /// How many objects machine code's inline bump placed in eden since
    /// the heap last counted them into its own counts
    /// (`Heap::settle_machine_allocations`): one add per allocation
    /// rather than a call, so that the heap's counts stay the
    /// interpreter's.
    MachineAllocations,
    /// The kernel's `Array` class object, as the class word of an object
    /// stores it: rewritten after every scavenge, compaction and move, as
    /// `ImmediateClass` is.
    ArrayClass,
    /// The header word of an `Array` of no words, guarding on its
    /// application's dispatch id: an allocation in machine code adds its
    /// word count (`Heap::machine_word_count_bits`).
    ArrayHeader,
    /// The kind tag of the frame the initializer leaf decided, shifted to
    /// where a flags word holds it (`Heap::hand_machine_initializer`):
    /// what the callee's prologue adds to its flags word, since an
    /// initializer's code does not say which of its mixin's bodies it runs
    /// as (`jit::sends::prologue`). Read only right after it was written,
    /// as `CalleeReceiver` is.
    CalleeKind,
    /// The kernel's `Closure` class object, as `ArrayClass` holds the
    /// `Array` class: the class word of a closure `MakeClosure` makes in
    /// machine code (`jit::closures`).
    ClosureClass,
    /// The header word of a closure of no words, as `ArrayHeader` is an
    /// `Array`'s.
    ClosureHeader,
    /// The kernel-private `Activation` class object, the class word of a
    /// frame object `MakeClosure` reifies in machine code.
    FrameClass,
    /// The header word of a frame object of no words.
    FrameHeader,
    /// The epoch of the heap's code table (`Heap::code_table_epoch`),
    /// renewed whenever a full collection frees a code id: a closure
    /// site's code ids, which machine code stores into the frame objects
    /// and closures it makes, name their bodies only while the site
    /// records this epoch (`jit::closures`).
    CodeTableEpoch,
    /// How many rows of the machine entry table are committed
    /// (`heap/machine_entries.rs`): a code id at or past it is read
    /// nowhere, and its body is called by the interpreter alone.
    MachineEntryRows,
    /// The closure an interpreter's send calls a block's body for, as
    /// `CalleeReceiver` is the receiver: the closure call stub hands it to
    /// the block's prologue (`Roles::callee_closure`), which stores it as
    /// its frame's operand. Read only right after it was written.
    CalleeClosure,
    /// 1 when the closure's activation the interpreter's send calls counts
    /// against the depth limit, 0 when the control flow exempts it
    /// (`Roles::callee_counted`), handed over as `CalleeClosure` is.
    CalleeCounted,
    /// The kernel-private `FutureImplementation` class object, as
    /// `ArrayClass` holds the `Array` class: what `ResolveFuture` in
    /// machine code compares a value's class word with, and the class word
    /// of a future `MakeFuture` makes there (`jit::closures`).
    FutureClass,
    /// The header word of a future of no words, as `ArrayHeader` is an
    /// `Array`'s -- or 0 until the heap has made a future
    /// (`Mutator::futures_made`), which only Rust sets, so that machine
    /// code's `MakeFuture` leaves the first one to the generic leaf, which
    /// sets it, and never makes a future the runtime would not look for.
    FutureHeader,
    /// The budget a body's baseline code spends from until the JIT points
    /// its budget word at the body's row (`heap/optimization_budgets.rs`):
    /// always negative (`UNPOINTED_BUDGET`), so that the first spend from
    /// it calls the budget leaf, which points the word and writes this one
    /// back. What every budget word holds at install and wherever its row
    /// is not known, so that the spend needs no test for a word naming
    /// none.
    UnpointedBudget,
    /// Under the deopt stress mode, how many interrupt points optimized
    /// code passes before the next leaves it: each point subtracts one,
    /// and the one that brings it to zero exits there
    /// (`MacroAssembler::count_down_to_deoptimize`); the run loop's
    /// safepoint sets it afresh (`JitState::rotate_deoptimization_countdown`).
    /// Under the invalidate stress mode alone it starts at `u64::MAX` after
    /// each of the mode's invalidations, which no run brings to zero, so
    /// that it counts the points passed since
    /// (`JitState::invalidation_due_for_stress`).
    /// Unread outside the two modes.
    DeoptCountdown,
}

/// What `MachineWord::UnpointedBudget` holds: negative, and far enough
/// from `i64::MIN` that no run of spends between two of the budget leaf's
/// calls, which writes it back, wraps it.
pub(crate) const UNPOINTED_BUDGET: i64 = -(1 << 62);

impl MachineWord {
    /// The word's index in the area.
    #[inline(always)]
    pub(crate) const fn index(self) -> usize {
        match self {
            MachineWord::SafepointRequested => 0,
            MachineWord::DepthCount => 1,
            MachineWord::DepthLimit => 2,
            MachineWord::OrdinaryStackEnd => 3,
            MachineWord::ImmediateClass(index) => {
                assert!(index < IMMEDIATE_COUNT);
                IMMEDIATE_CLASSES + index
            }
            MachineWord::ImmediateGuard(index) => {
                assert!(index < IMMEDIATE_COUNT);
                IMMEDIATE_GUARDS + index
            }
            MachineWord::NextGeneration => IMMEDIATE_GUARDS + IMMEDIATE_COUNT,
            MachineWord::CalleeReceiver => IMMEDIATE_GUARDS + IMMEDIATE_COUNT + 1,
            MachineWord::CalleeClass => IMMEDIATE_GUARDS + IMMEDIATE_COUNT + 2,
            MachineWord::ApplicationIds => IMMEDIATE_GUARDS + IMMEDIATE_COUNT + 3,
            MachineWord::ApplicationIdRows => IMMEDIATE_GUARDS + IMMEDIATE_COUNT + 4,
            MachineWord::MachineAllocations => IMMEDIATE_GUARDS + IMMEDIATE_COUNT + 5,
            MachineWord::ArrayClass => IMMEDIATE_GUARDS + IMMEDIATE_COUNT + 6,
            MachineWord::ArrayHeader => IMMEDIATE_GUARDS + IMMEDIATE_COUNT + 7,
            MachineWord::CalleeKind => IMMEDIATE_GUARDS + IMMEDIATE_COUNT + 8,
            MachineWord::ClosureClass => IMMEDIATE_GUARDS + IMMEDIATE_COUNT + 9,
            MachineWord::ClosureHeader => IMMEDIATE_GUARDS + IMMEDIATE_COUNT + 10,
            MachineWord::FrameClass => IMMEDIATE_GUARDS + IMMEDIATE_COUNT + 11,
            MachineWord::FrameHeader => IMMEDIATE_GUARDS + IMMEDIATE_COUNT + 12,
            MachineWord::CodeTableEpoch => IMMEDIATE_GUARDS + IMMEDIATE_COUNT + 13,
            MachineWord::MachineEntryRows => IMMEDIATE_GUARDS + IMMEDIATE_COUNT + 14,
            MachineWord::CalleeClosure => IMMEDIATE_GUARDS + IMMEDIATE_COUNT + 15,
            MachineWord::CalleeCounted => IMMEDIATE_GUARDS + IMMEDIATE_COUNT + 16,
            MachineWord::FutureClass => IMMEDIATE_GUARDS + IMMEDIATE_COUNT + 17,
            MachineWord::FutureHeader => IMMEDIATE_GUARDS + IMMEDIATE_COUNT + 18,
            MachineWord::UnpointedBudget => IMMEDIATE_GUARDS + IMMEDIATE_COUNT + 19,
            MachineWord::DeoptCountdown => IMMEDIATE_GUARDS + IMMEDIATE_COUNT + 20,
        }
    }
}

/// The words the area holds: `MachineWord`'s, and room for the words the
/// later stages put here, so that adding one moves none.
const WORD_COUNT: usize = 40;

const _: () = assert!(MachineWord::DeoptCountdown.index() < WORD_COUNT);

/// The area. One per heap, created with it and never replaced; a clone
/// is another handle to the same words, which the activation stack keeps
/// to write its end.
#[derive(Clone)]
pub(crate) struct MachineWords {
    words: Rc<[Cell<u64>; WORD_COUNT]>,
}

impl MachineWords {
    pub(crate) fn new() -> MachineWords {
        let words = MachineWords {
            words: Rc::new(std::array::from_fn(|_| Cell::new(0))),
        };
        words.set(
            MachineWord::UnpointedBudget,
            UNPOINTED_BUDGET.cast_unsigned(),
        );
        words
    }

    #[inline(always)]
    pub(crate) fn get(&self, word: MachineWord) -> u64 {
        self.words[word.index()].get()
    }

    #[inline(always)]
    pub(crate) fn set(&self, word: MachineWord, value: u64) {
        self.words[word.index()].set(value);
    }

    /// The area's address, exposed, which machine code reads its words
    /// at (`jit::stubs::BlockWord::MachineWords`).
    #[cfg(ns_jit)]
    pub(crate) fn address(&self) -> usize {
        Rc::as_ptr(&self.words).expose_provenance()
    }

    /// `word`'s byte offset from the area's address.
    #[cfg(any(ns_jit, test))]
    #[expect(
        clippy::cast_possible_truncation,
        reason = "an index lies below `WORD_COUNT`, 40"
    )]
    pub(crate) fn offset(word: MachineWord) -> i32 {
        const BYTES_PER_WORD: i32 = 8;
        word.index() as i32 * BYTES_PER_WORD
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_word_has_its_own_fixed_offset() {
        let mut offsets = vec![
            MachineWords::offset(MachineWord::SafepointRequested),
            MachineWords::offset(MachineWord::DepthCount),
            MachineWords::offset(MachineWord::DepthLimit),
            MachineWords::offset(MachineWord::OrdinaryStackEnd),
        ];
        for index in 0..IMMEDIATE_COUNT {
            offsets.push(MachineWords::offset(MachineWord::ImmediateClass(index)));
            offsets.push(MachineWords::offset(MachineWord::ImmediateGuard(index)));
        }
        assert_eq!(MachineWords::offset(MachineWord::SafepointRequested), 0);
        assert_eq!(MachineWords::offset(MachineWord::DepthCount), 8);
        assert_eq!(MachineWords::offset(MachineWord::ImmediateClass(0)), 32);
        assert_eq!(MachineWords::offset(MachineWord::ImmediateGuard(0)), 72);
        offsets.push(MachineWords::offset(MachineWord::NextGeneration));
        offsets.push(MachineWords::offset(MachineWord::CalleeReceiver));
        offsets.push(MachineWords::offset(MachineWord::CalleeClass));
        offsets.push(MachineWords::offset(MachineWord::ApplicationIds));
        offsets.push(MachineWords::offset(MachineWord::ApplicationIdRows));
        offsets.push(MachineWords::offset(MachineWord::MachineAllocations));
        offsets.push(MachineWords::offset(MachineWord::ArrayClass));
        offsets.push(MachineWords::offset(MachineWord::ArrayHeader));
        offsets.push(MachineWords::offset(MachineWord::CalleeKind));
        offsets.push(MachineWords::offset(MachineWord::ClosureClass));
        offsets.push(MachineWords::offset(MachineWord::ClosureHeader));
        offsets.push(MachineWords::offset(MachineWord::FrameClass));
        offsets.push(MachineWords::offset(MachineWord::FrameHeader));
        offsets.push(MachineWords::offset(MachineWord::CodeTableEpoch));
        offsets.push(MachineWords::offset(MachineWord::MachineEntryRows));
        offsets.push(MachineWords::offset(MachineWord::CalleeClosure));
        offsets.push(MachineWords::offset(MachineWord::CalleeCounted));
        offsets.push(MachineWords::offset(MachineWord::FutureClass));
        offsets.push(MachineWords::offset(MachineWord::FutureHeader));
        offsets.push(MachineWords::offset(MachineWord::UnpointedBudget));
        offsets.push(MachineWords::offset(MachineWord::DeoptCountdown));
        offsets.sort_unstable();
        offsets.dedup();
        assert_eq!(offsets.len(), 25 + 2 * IMMEDIATE_COUNT);
    }

    #[test]
    fn a_clone_shares_the_words() {
        let words = MachineWords::new();
        let other = words.clone();
        other.set(MachineWord::DepthCount, 7);
        assert_eq!(words.get(MachineWord::DepthCount), 7);
    }
}
