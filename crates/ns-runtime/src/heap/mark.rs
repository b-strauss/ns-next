//! The marker (`docs/design/memory.md`, "Marking").
//!
//! A child of `heap`, so it may name a handle's private offset. It holds no
//! `Heap`, so it cannot read a header: the marking loop is
//! `Heap::drain_marking`, which tests and sets each object's mark bit as it
//! pops the object's offset. The marker therefore pushes every offset it is
//! handed, a root's included, without testing anything; a root reached
//! twice costs one extra pop, which finds the object already marked.

use crate::heap::{FrameReference, ObjectReference, Word};
use crate::trace::Tracer;
use std::num::NonZeroU64;

/// The work list of a marking: the offsets of objects reached but not
/// yet visited. Its `Tracer` impl is how the roots enter it, through the
/// same edge enumeration forwarding walks (`trace.rs`), so a root a later
/// change adds is named in exactly one place and cannot be missed by one
/// of the two.
pub(crate) struct Marker {
    work: Vec<NonZeroU64>,
    /// Every `WeakPointers` object visited, whose words naming an object
    /// left unmarked are cleared once the marking is finished
    /// (`Heap::finish_marking`).
    pub(super) weak_objects: Vec<NonZeroU64>,
    /// Every `Ephemeron` visited before its key was marked, its guarded
    /// fields not yet traced: traced if the key is marked later, mourned
    /// if it never is (`Heap::finish_marking`).
    pub(super) ephemerons: Vec<NonZeroU64>,
}

impl Marker {
    pub(crate) fn new() -> Marker {
        Marker {
            work: Vec::new(),
            weak_objects: Vec::new(),
            ephemerons: Vec::new(),
        }
    }

    /// Adds the object at `offset` to the work list.
    pub(crate) fn push(&mut self, offset: NonZeroU64) {
        self.work.push(offset);
    }

    /// The offset of the next object to visit, or `None` when the traversal
    /// is done. The work list is explicit because the graph's depth is the
    /// program's to choose -- the reasoning that already made
    /// `MixinApplication`'s `Drop` iterative.
    pub(crate) fn next(&mut self) -> Option<NonZeroU64> {
        self.work.pop()
    }
}

impl Tracer for Marker {
    fn object(&mut self, handle: &mut ObjectReference<'_>) {
        self.push(handle.offset);
    }

    /// A frame is its frame object.
    fn frame(&mut self, handle: &mut FrameReference<'_>) {
        self.push(handle.offset);
    }

    fn word(&mut self, word: &mut Word) {
        if let Some(offset) = word.object_offset() {
            self.push(offset);
        }
    }
}
