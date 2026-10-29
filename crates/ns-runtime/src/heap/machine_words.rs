//! The machine words: every word machine code reads or writes between two
//! leaf calls other than frame registers and object fields, which lie in
//! the activation stack's and the object space's reservations -- today
//! the safepoint request, which a back edge polls, the depth count and
//! limit, the end of the activation stack's ordinary room and the
//! immediates' classes and guards; eden's top and limit, which bump
//! allocation is to read and write in place, are to join them -- held in
//! an allocation of their own rather
//! than inline in the heap
//! (`docs/design/execution.md`, "The machine words").
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
/// application id table's address and bound 17 and 18, the rest free for
/// the later stages.
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
    /// holds it (`Mutator::set_depth_reserve`).
    DepthLimit,
    /// Where an ordinary push may end without a growth, in words from the
    /// activation stack's base (`FrameStack::ordinary_end`).
    OrdinaryStackEnd,
    /// The class object of immediate `index`, as the word a heap object
    /// stores for it: rewritten by the heap after every scavenge,
    /// compaction and move (`Mutator::rewrite_immediate_classes`).
    ImmediateClass(usize),
    /// The dispatch id an inline cache guards a send to immediate `index`
    /// on (`Heap::dispatch_id`).
    ImmediateGuard(usize),
    /// In a debug build, the generation the last frame pushed took: the
    /// next push, the heap's or a compiled callee's prologue, takes one
    /// past it (`Heap::peek_next_generation`). Unused in a release build.
    #[cfg_attr(
        not(debug_assertions),
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
}

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
        }
    }
}

/// The words the area holds: `MachineWord`'s, and room for the words the
/// later stages put here, so that adding one moves none.
const WORD_COUNT: usize = 32;

const _: () = assert!(MachineWord::ApplicationIdRows.index() < WORD_COUNT);

/// The area. One per heap, created with it and never replaced; a clone
/// is another handle to the same words, which the activation stack keeps
/// to write its end.
#[derive(Clone)]
pub(crate) struct MachineWords {
    words: Rc<[Cell<u64>; WORD_COUNT]>,
}

impl MachineWords {
    pub(crate) fn new() -> MachineWords {
        MachineWords {
            words: Rc::new(std::array::from_fn(|_| Cell::new(0))),
        }
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
        reason = "an index lies below `WORD_COUNT`, 32"
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
        offsets.sort_unstable();
        offsets.dedup();
        assert_eq!(offsets.len(), 9 + 2 * IMMEDIATE_COUNT);
    }

    #[test]
    fn a_clone_shares_the_words() {
        let words = MachineWords::new();
        let other = words.clone();
        other.set(MachineWord::DepthCount, 7);
        assert_eq!(words.get(MachineWord::DepthCount), 7);
    }
}
