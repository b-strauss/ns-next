//! The machine words: every word of the heap that machine code reads or
//! writes between two leaf calls -- today the safepoint request, which a
//! back edge polls; eden's top and limit, which bump allocation is to
//! read and write in place, and the depth count, are to join it -- held
//! in an allocation of their own rather than inline in the heap
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

/// The words of the area, by index.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum MachineWord {
    /// Not zero while the run loop's next safepoint must take its slow
    /// path (`Heap::safepoint_requested`); machine code's poll compares
    /// it with zero.
    SafepointRequested = 0,
}

/// The words the area holds: `MachineWord`'s, and room for the words the
/// later stages put here, so that adding one moves none.
const WORD_COUNT: usize = 8;

const _: () = assert!((MachineWord::SafepointRequested as usize) < WORD_COUNT);

/// The area. One per heap, created with it and never replaced.
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
        self.words[word as usize].get()
    }

    #[inline(always)]
    pub(crate) fn set(&self, word: MachineWord, value: u64) {
        self.words[word as usize].set(value);
    }

    /// The area's address, exposed, which machine code reads its words
    /// at (`jit::stubs::BlockWord::MachineWords`).
    #[cfg(ns_jit)]
    pub(crate) fn address(&self) -> usize {
        Rc::as_ptr(&self.words).expose_provenance()
    }

    /// `word`'s byte offset from the area's address.
    #[cfg(ns_jit)]
    pub(crate) fn displacement(word: MachineWord) -> i32 {
        word as i32 * 8
    }
}
