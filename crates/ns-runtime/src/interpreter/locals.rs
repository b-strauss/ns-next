//! Register and outer-local access for a running activation, and the two
//! helpers `MakeClosure` and `InitializeSlot` write through.

use crate::heap::{ActivationReference, Heap};
use crate::types::*;

pub(crate) fn read_local(heap: &Heap, activation: ActivationReference, index: u16) -> Value {
    heap.activation_local(activation, index)
}

pub(crate) fn write_local(
    heap: &mut Heap,
    activation: ActivationReference,
    index: u16,
    value: Value,
) {
    heap.activation_set_local(activation, index, value);
}

/// Where a `Binding::OuterLocal` lives: in the running activation itself,
/// or in a frame some closure captured.
enum OuterFrame {
    Own(ActivationReference),
    Captured(FrameReference),
}

/// The activation `depth` hops out along the `captured_frame` chain. The
/// first hop leaves the running activation for its closure's captured frame;
/// the rest follow frames. It stops hopping at a non-closure activation, so a
/// depth past the lexical nesting lands on the outermost body rather than
/// running off the chain -- `image::decode::enclosing_body` is the static
/// counterpart and relies on exactly this, for an image whose `depth` it
/// verified against the lexical nesting rather than the chain.
fn frame_at_depth(heap: &Heap, activation: ActivationReference, depth: u16) -> OuterFrame {
    if depth == 0 {
        return OuterFrame::Own(activation);
    }
    let Some(mut frame) = heap.activation_captured(activation) else {
        return OuterFrame::Own(activation);
    };
    for _ in 1..depth {
        frame = heap.frame_captured(frame).unwrap_or(frame);
    }
    OuterFrame::Captured(frame)
}

/// The VM's `LoadOuterLocal`.
pub(crate) fn read_outer_local(
    heap: &Heap,
    activation: ActivationReference,
    depth: u16,
    index: u16,
) -> Value {
    match frame_at_depth(heap, activation, depth) {
        OuterFrame::Own(own) => heap.activation_local(own, index),
        OuterFrame::Captured(frame) => heap.frame_local(frame, index),
    }
}

/// The VM's `StoreOuterLocal`.
pub(crate) fn write_outer_local(
    heap: &mut Heap,
    activation: ActivationReference,
    depth: u16,
    index: u16,
    value: Value,
) {
    match frame_at_depth(heap, activation, depth) {
        OuterFrame::Own(own) => heap.activation_set_local(own, index, value),
        OuterFrame::Captured(frame) => heap.frame_set_local(frame, index, value),
    }
}

/// The `home` a closure created in `activation` gets: a closure activation
/// passes its own closure's `home` on; any other activation -- method,
/// initializer, superclass clause or message -- is its own return target, so it
/// is reified and is its own home. The VM's `MakeClosure` calls it.
pub(crate) fn home_of(heap: &mut Heap, activation: ActivationReference) -> FrameReference {
    match heap.activation_home(activation) {
        Some(home) => home,
        None => heap.reify(activation),
    }
}

/// Writes `value` into instance field `index` on `instance`'s
/// `ObjectData::Instance` fields -- silently skipped when `instance` is not an
/// object, or is an object with some other `ObjectData` shape, or `index` is
/// out of range. `vm::execute`'s `Instruction::InitializeSlot` arm writes a
/// slot initializer's value through it.
pub(crate) fn store_instance_field(heap: &mut Heap, instance: &Value, index: usize, value: Value) {
    if let Value::Object(object) = instance {
        heap.set_instance_field(*object, index, value);
    }
}
