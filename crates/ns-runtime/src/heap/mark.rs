//! Mark bits and the marker (`docs/design/memory.md`, "Marking").
//!
//! A child of `heap`, so it may name a handle's private index. It holds no
//! `Heap`: the marking loop is `Heap::drain_marking`, which lends the
//! marker one arena record at a time, so the bits and the work list are
//! borrowed beside the arena rather than through it.

use crate::heap::{FrameReference, ObjectReference, Word};
use crate::trace::Tracer;

/// One bit per slot of the object arena -- 1/128 of its 16-byte slots
/// (`an_object_slot_is_one_boxed_block_of_words`), before the blocks
/// themselves. Not the header's mark bit, which belongs to the collector of
/// a later road step (`header.rs`).
pub(crate) struct MarkBits {
    words: Vec<u64>,
    length: usize,
}

impl MarkBits {
    pub(crate) fn new(length: usize) -> MarkBits {
        MarkBits {
            words: vec![0; length.div_ceil(64)],
            length,
        }
    }

    /// Sets the bit for `index` and answers whether it had been clear.
    /// That answer is what keeps the traversal from visiting an object
    /// twice, and so from running forever on a cycle.
    fn set(&mut self, index: u32) -> bool {
        let index = index as usize;
        debug_assert!(index < self.length, "internal: a handle past its arena");
        let word = index / 64;
        let mask = 1u64 << (index % 64);
        let was_clear = self.words[word] & mask == 0;
        self.words[word] |= mask;
        was_clear
    }

    pub(crate) fn is_set(&self, index: u32) -> bool {
        let index = index as usize;
        debug_assert!(index < self.length, "internal: a handle past its arena");
        self.words[index / 64] & (1u64 << (index % 64)) != 0
    }
}

/// The tracer that marks. Marking and forwarding share one edge
/// enumeration (`trace.rs`), so a field a later change adds is named in
/// exactly one place and cannot be missed by one of the two.
pub(crate) struct Marker {
    pub(crate) objects: MarkBits,
    work: Vec<u32>,
}

impl Marker {
    pub(crate) fn new(object_count: usize) -> Marker {
        Marker {
            objects: MarkBits::new(object_count),
            work: Vec::new(),
        }
    }

    /// The index of the next object to visit, or `None` when the traversal
    /// is done. The work list is explicit because the graph's depth is the
    /// program's to choose -- the reasoning that already made
    /// `MixinApplication`'s `Drop` iterative.
    pub(crate) fn next(&mut self) -> Option<u32> {
        self.work.pop()
    }

    fn reach(&mut self, index: u32) {
        if self.objects.set(index) {
            self.work.push(index);
        }
    }
}

impl Tracer for Marker {
    fn object(&mut self, handle: &mut ObjectReference<'_>) {
        self.reach(handle.index);
    }

    /// A frame is its frame object.
    fn frame(&mut self, handle: &mut FrameReference<'_>) {
        self.reach(handle.index);
    }

    fn word(&mut self, word: &mut Word) {
        if let Some(index) = word.object_index() {
            self.reach(index);
        }
    }
}
