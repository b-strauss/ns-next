//! The marker (`docs/design/memory.md`, "Marking").
//!
//! A child of `heap`, so it may name a handle's private index. It holds no
//! `Heap`, so it cannot read a header: the marking loop is
//! `Heap::drain_marking`, which tests and sets each object's mark bit as it
//! pops the object's index. The marker therefore pushes every index it is
//! handed, a root's included, without testing anything; a root reached
//! twice costs one extra pop, which finds the object already marked.

use crate::heap::{FrameReference, ObjectReference, Word};
use crate::trace::Tracer;

/// The work list of a marking: the slot indices of objects reached but not
/// yet visited. Its `Tracer` impl is how the roots enter it, through the
/// same edge enumeration forwarding walks (`trace.rs`), so a root a later
/// change adds is named in exactly one place and cannot be missed by one
/// of the two.
pub(crate) struct Marker {
    work: Vec<u32>,
}

impl Marker {
    pub(crate) fn new() -> Marker {
        Marker { work: Vec::new() }
    }

    /// Adds the object at slot `index` to the work list.
    pub(crate) fn push(&mut self, index: u32) {
        self.work.push(index);
    }

    /// The index of the next object to visit, or `None` when the traversal
    /// is done. The work list is explicit because the graph's depth is the
    /// program's to choose -- the reasoning that already made
    /// `MixinApplication`'s `Drop` iterative.
    pub(crate) fn next(&mut self) -> Option<u32> {
        self.work.pop()
    }
}

impl Tracer for Marker {
    fn object(&mut self, handle: &mut ObjectReference<'_>) {
        self.push(handle.index);
    }

    /// A frame is its frame object.
    fn frame(&mut self, handle: &mut FrameReference<'_>) {
        self.push(handle.index);
    }

    fn word(&mut self, word: &mut Word) {
        if let Some(index) = word.object_index() {
            self.push(index);
        }
    }
}
