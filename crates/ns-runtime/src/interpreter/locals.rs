//! Register and outer-local access for a running activation, and the helper
//! `InitializeSlot` writes through.

use crate::heap::{ActivationReference, Heap};
use crate::types::*;
use crate::Symbol;

use super::{EvaluationResult, Signal};

pub(crate) fn read_local<'gc>(
    heap: &Heap<'gc>,
    activation: ActivationReference,
    index: u16,
) -> Value<'gc> {
    heap.activation_local(activation, index)
}

pub(crate) fn write_local<'gc>(
    heap: &mut Heap<'gc>,
    activation: ActivationReference,
    index: u16,
    value: Value<'gc>,
) {
    heap.activation_set_local(activation, index, value);
}

/// Where a `Binding::OuterLocal` lives: in the running activation itself,
/// or in a frame some closure captured.
enum OuterFrame<'gc> {
    Own(ActivationReference),
    Captured(FrameReference<'gc>),
}

/// The activation `depth` hops out along the `captured_frame` chain. The
/// first hop leaves the running activation for its closure's captured frame;
/// the rest follow frames. It stops hopping at a non-closure activation, so a
/// depth past the lexical nesting lands on the outermost body rather than
/// running off the chain -- `image::decode::enclosing_body` is the static
/// counterpart and relies on exactly this, for an image whose `depth` it
/// verified against the lexical nesting rather than the chain.
fn frame_at_depth<'gc>(
    heap: &Heap<'gc>,
    activation: ActivationReference,
    depth: u16,
) -> OuterFrame<'gc> {
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
pub(crate) fn read_outer_local<'gc>(
    heap: &Heap<'gc>,
    activation: ActivationReference,
    depth: u16,
    index: u16,
) -> Value<'gc> {
    match frame_at_depth(heap, activation, depth) {
        OuterFrame::Own(own) => heap.activation_local(own, index),
        OuterFrame::Captured(frame) => heap.frame_local(frame, index),
    }
}

/// The VM's `StoreOuterLocal`.
pub(crate) fn write_outer_local<'gc>(
    heap: &mut Heap<'gc>,
    activation: ActivationReference,
    depth: u16,
    index: u16,
    value: Value<'gc>,
) {
    match frame_at_depth(heap, activation, depth) {
        OuterFrame::Own(own) => heap.activation_set_local(own, index, value),
        OuterFrame::Captured(frame) => heap.frame_set_local(frame, index, value),
    }
}

/// The VM's `LoadVector`. A value that is not the `Array` the compiler put
/// there, or an index past its end, can only come from a crafted image
/// (`CLAUDE.md`, "An image is untrusted input"), so this reports rather than
/// indexes.
pub(crate) fn load_vector<'gc>(
    heap: &Heap<'gc>,
    activation: ActivationReference,
    vector: u16,
    index: u16,
) -> EvaluationResult<'gc> {
    let value = heap.activation_local(activation, vector);
    match value {
        Value::Object(object) => match heap.array_at(object, index as usize) {
            Some(element) => Ok(element),
            None => Err(vector_error(value)),
        },
        _ => Err(vector_error(value)),
    }
}

/// The VM's `StoreVector`; the mirror of `load_vector` through
/// `Heap::array_set`.
pub(crate) fn store_vector<'gc>(
    heap: &mut Heap<'gc>,
    activation: ActivationReference,
    vector: u16,
    index: u16,
    source: u16,
) -> Result<(), Signal<'gc>> {
    let value = heap.activation_local(activation, vector);
    let written = match value {
        Value::Object(object) => {
            let source_value = heap.activation_local(activation, source);
            heap.array_set(object, index as usize, source_value)
        }
        _ => false,
    };
    if written {
        Ok(())
    } else {
        Err(vector_error(value))
    }
}

/// The error both `load_vector` and `store_vector` raise. Built here, never
/// in `vm.rs`: error text lives in `interpreter/`.
fn vector_error<'gc>(vector: Value<'gc>) -> Signal<'gc> {
    Signal::Error(NewspeakError {
        selector: Symbol::intern("indirectionVector"),
        receiver: vector,
        message: "internal: an indirection vector is not the Array its code expects".to_string(),
    })
}

/// The value of the running activation's receiver's own slot `own_index`
/// -- addressed as `Instruction::InitializeSlot` addresses it, from the
/// activation's slot base (`Heap::activation_slot_base`) -- when it holds
/// anything but `nil`. `None` for `nil`, a lazy slot not yet computed
/// (§6.3.3), and for a receiver or index with no such field, both of which
/// run the getter's initializer: `vm::execute`'s
/// `Instruction::ReturnSlotIfFilled` arm.
pub(crate) fn filled_own_slot<'gc>(
    heap: &Heap<'gc>,
    activation: ActivationReference,
    own_index: u32,
) -> Option<Value<'gc>> {
    let Value::Object(object) = heap.activation_receiver(activation) else {
        return None;
    };
    let base = heap.activation_slot_base(activation);
    heap.instance_field(object, base + own_index as usize)
        .filter(|value| !matches!(value, Value::Nil))
}

/// Writes `value` into instance field `index` on `instance`'s
/// `ObjectData::Instance` fields -- silently skipped when `instance` is not an
/// object, or is an object with some other `ObjectData` shape, or `index` is
/// out of range. `vm::execute`'s `Instruction::InitializeSlot` arm writes a
/// slot initializer's value through it.
pub(crate) fn store_instance_field<'gc>(
    heap: &mut Heap<'gc>,
    instance: &Value<'gc>,
    index: usize,
    value: Value<'gc>,
) {
    if let Value::Object(object) = instance {
        heap.set_instance_field(*object, index, value);
    }
}
