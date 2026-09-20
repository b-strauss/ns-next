//! Mark bits and the marker (`docs/design/memory.md`, "Marking").
//!
//! A child of `heap`, so it may name a handle's private index. It holds no
//! `Heap`: the marking loop is `Heap::drain_marking`, which lends the
//! marker one arena record at a time, so the bits and the work lists are
//! borrowed beside the arena rather than through it.

use crate::heap::{FrameReference, ObjectReference};
use crate::trace::Tracer;

/// One bit per slot of one arena -- 1/448 of the object arena, which is why
/// the bit is here and not in the object header: `Object` is pinned at 56
/// bytes by `an_object_is_the_size_the_amplification_figure_assumes`, and
/// the image format's amplification figures are computed from that number.
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
    pub(crate) frames: MarkBits,
    object_work: Vec<u32>,
    frame_work: Vec<u32>,
}

impl Marker {
    pub(crate) fn new(object_count: usize, frame_count: usize) -> Marker {
        Marker {
            objects: MarkBits::new(object_count),
            frames: MarkBits::new(frame_count),
            object_work: Vec::new(),
            frame_work: Vec::new(),
        }
    }

    /// The next record to visit, or `None` when the traversal is done. The
    /// work lists are explicit because the graph's depth is the program's
    /// to choose -- the reasoning that already made `MixinApplication`'s
    /// `Drop` iterative.
    pub(crate) fn next(&mut self) -> Option<Work> {
        if let Some(index) = self.object_work.pop() {
            return Some(Work::Object(index));
        }
        self.frame_work.pop().map(Work::Frame)
    }
}

/// One arena record the marker still has to visit.
pub(crate) enum Work {
    Object(u32),
    Frame(u32),
}

impl Tracer for Marker {
    fn object(&mut self, handle: &mut ObjectReference<'_>) {
        if self.objects.set(handle.index) {
            self.object_work.push(handle.index);
        }
    }

    fn frame(&mut self, handle: &mut FrameReference<'_>) {
        if self.frames.set(handle.index) {
            self.frame_work.push(handle.index);
        }
    }
}
