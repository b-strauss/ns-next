//! Register and outer-local access for a running activation, the check
//! every own-slot instruction makes first, and the helper `InitializeSlot`
//! writes through.

use crate::Symbol;
use crate::heap::{ActivationReference, Heap, RegisterBase};
use crate::mixin::DispatchId;
use crate::types::*;

use super::{EvaluationResult, Mutator, Signal};

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
/// indexes. `register_base` is the running activation's window, which the
/// run loop holds: read through it, the vector's register costs no read of
/// the frame's argument count.
///
/// A nil holder answers nil: a vector not made yet, which only a block an
/// evaluation made over a frame stopped before its `MakeVector` reads --
/// the variable is in scope from its body's start and nil until written,
/// as the variables view shows it (`docs/design/debugger.md`,
/// "Evaluation"). Machine code exits for it, as for every holder that is
/// no `Array`, so the decision is made here alone.
pub(crate) fn load_vector<'gc>(
    heap: &Heap<'gc>,
    register_base: RegisterBase,
    vector: u16,
    index: u16,
) -> EvaluationResult<'gc> {
    let value = heap.register(register_base, vector);
    match value.as_object() {
        Some(object) => match heap.array_element(object, index as usize) {
            Some(element) => Ok(element),
            None => Err(vector_error(value)),
        },
        None => not_an_object_holder(value),
    }
}

/// `load_vector` of a holder that is no object: nil for a vector not made
/// yet, the error otherwise.
#[cold]
#[inline(never)]
fn not_an_object_holder(value: Value<'_>) -> EvaluationResult<'_> {
    if value.is_nil() {
        Ok(Value::nil())
    } else {
        Err(vector_error(value))
    }
}

/// The VM's `StoreVector`; the mirror of `load_vector` through
/// `Heap::array_set_element`.
pub(crate) fn store_vector<'gc>(
    heap: &mut Heap<'gc>,
    register_base: RegisterBase,
    vector: u16,
    index: u16,
    source: u16,
) -> Result<(), Signal<'gc>> {
    let value = heap.register(register_base, vector);
    let written = match value.as_object() {
        Some(object) => {
            let source_value = heap.register(register_base, source);
            heap.array_set_element(object, index as usize, source_value)
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
    Signal::Error(NewspeakError::error(
        Symbol::intern("indirectionVector"),
        vector,
        "internal: an indirection vector is not the Array its code expects",
    ))
}

/// Whether the running activation's receiver still has the slots of the
/// level the activation was entered at, which its own-slot instructions --
/// `InitializeSlot`, `InitializeSlotProvisionally`, `LoadOwnSlot` and
/// `ReturnSlotIfFilled` -- address by an index decided without looking at
/// the receiver. Each of them calls this before it reads or writes, so a
/// receiver the bulk forward replaced by an object of another shape is
/// never read or written in the layout the code was compiled for
/// (`docs/design/object-model.md`, "what a slot access checks").
///
/// The hot path is one compare of the receiver's header guard with the one
/// the activation recorded when it was pushed. Anything else takes the cold
/// check (`own_slots_cold_check`).
#[inline(always)]
pub(crate) fn check_own_slots<'gc>(
    mutator: &mut Mutator<'gc>,
    activation: ActivationReference,
) -> Result<(), Signal<'gc>> {
    let (receiver, recorded_guard) = mutator
        .heap
        .activation_receiver_and_recorded_guard(activation);
    if recorded_guard != DispatchId::NONE && mutator.heap.dispatch_id(receiver) == recorded_guard {
        return Ok(());
    }
    own_slots_cold_check(mutator, activation, receiver)
}

/// `check_own_slots` on a mismatch, or with no recorded guard: the
/// receiver still has the entered level's slots
/// (`receiver_keeps_entered_level`), or the access raises an ordinary
/// Newspeak error.
#[inline(never)]
fn own_slots_cold_check<'gc>(
    mutator: &mut Mutator<'gc>,
    activation: ActivationReference,
    receiver: Value<'gc>,
) -> Result<(), Signal<'gc>> {
    if receiver_keeps_entered_level(mutator, activation, receiver) {
        return Ok(());
    }
    let entered_level = mutator.heap.activation_entered_level(activation);
    Err(no_longer_has_the_slots(
        mutator,
        activation,
        receiver,
        entered_level,
    ))
}

/// The cold half of `check_own_slots`, without its error: whether
/// `receiver`'s application chain contains the application the activation
/// was entered at, or its successor, by its full-width index -- never the
/// class object's current application, which an install can rewrite while
/// this activation still runs the old code. What passes is a receiver
/// whose chain contains the entered application under another guard: the
/// forward replaced it by an instance of a subclass of the level's class.
/// (One replaced by another instance of the level's own application keeps
/// the guard and never gets here.) And one whose level an install
/// re-applied: the install builds a new application even for an edit that
/// changes only methods, and records it as the entered one's successor
/// when the level keeps its slot base and its own slots
/// (`docs/design/object-model.md`, "The entered level's successor"), so
/// the activation runs on; one whose slots changed is refused. When it
/// passes, the receiver's current guard is
/// recorded, so the accesses after it take the hot path again. An
/// immediate receiver has no slots to protect and passes; the instructions
/// skip it as before. `vm::initialize_slot_refused` asks this, rather than
/// `check_own_slots`, to tell a refusal of the receiver from one of the
/// value without building the error twice.
pub(crate) fn receiver_keeps_entered_level<'gc>(
    mutator: &mut Mutator<'gc>,
    activation: ActivationReference,
    receiver: Value<'gc>,
) -> bool {
    if receiver.as_object().is_none() {
        return true;
    }
    let entered_level = mutator.heap.activation_entered_level(activation);
    if !mutator.heap.receiver_has_level(receiver, entered_level) {
        return false;
    }
    let guard = mutator.heap.dispatch_id(receiver);
    mutator.heap.activation_record_guard(activation, guard);
    true
}

/// The error of an own-slot instruction whose receiver no longer has the
/// slots of the level its activation was entered at. It names the method
/// the program sent -- a lazy slot's getter, or the factory whose
/// initializer runs.
#[cold]
#[inline(never)]
fn no_longer_has_the_slots<'gc>(
    mutator: &Mutator<'gc>,
    activation: ActivationReference,
    receiver: Value<'gc>,
    entered_level: crate::heap::ApplicationIndex,
) -> Signal<'gc> {
    let level = mutator.heap.application(entered_level);
    let class = level.as_ref().map_or_else(
        || "its class".to_string(),
        |level| level.mixin.reflected_name(),
    );
    let selector = match mutator.heap.activation_kind(activation) {
        ActivationKind::Method(selector) => selector,
        _ => level.as_ref().map_or(Symbol::NEW, |level| {
            super::instantiate::factory_selector(level)
        }),
    };
    Signal::Error(NewspeakError::error(
        selector,
        receiver,
        format!(
            "{} no longer has the slots of {class}, which the running code was compiled for",
            mutator.heap.describe(receiver)
        ),
    ))
}

/// The value of the running activation's receiver's own slot `own_index`
/// -- addressed as `Instruction::InitializeSlot` addresses it, from the
/// activation's slot base (`Heap::activation_slot_base`) -- when it holds
/// anything but `nil`. `None` for `nil`, a lazy slot not yet computed
/// (§6.3.3), and for a receiver or index with no such field, both of which
/// run the getter's initializer: `vm::run_stretch`'s
/// `Instruction::ReturnSlotIfFilled` arm. The error of `check_own_slots`
/// when the receiver no longer has the slot.
pub(crate) fn filled_own_slot<'gc>(
    mutator: &mut Mutator<'gc>,
    activation: ActivationReference,
    own_index: u32,
) -> Result<Option<Value<'gc>>, Signal<'gc>> {
    check_own_slots(mutator, activation)?;
    let heap = &mutator.heap;
    let Some(object) = heap.activation_receiver(activation).as_object() else {
        return Ok(None);
    };
    let base = heap.activation_slot_base(activation);
    Ok(heap
        .instance_field(object, base + own_index as usize)
        .filter(|value| !value.is_nil()))
}

/// The value of the running activation's receiver's own slot `own_index`,
/// addressed as `InitializeSlot` addresses it -- for `LoadOwnSlot`, a
/// simultaneous clause's end reading each slot back without a getter send,
/// which a subclass could override. The same address `filled_own_slot`
/// computes, but without its "not yet computed" filter: `nil` is answered
/// for a receiver or index with no such field, exactly as `filled_own_slot`
/// treats those, but an actual `nil` field is simply what it holds here.
/// The error of `check_own_slots` when the receiver no longer has the slot.
pub(crate) fn load_own_slot<'gc>(
    mutator: &mut Mutator<'gc>,
    activation: ActivationReference,
    own_index: u32,
) -> EvaluationResult<'gc> {
    check_own_slots(mutator, activation)?;
    let heap = &mutator.heap;
    let Some(object) = heap.activation_receiver(activation).as_object() else {
        return Ok(Value::nil());
    };
    let base = heap.activation_slot_base(activation);
    Ok(heap
        .instance_field(object, base + own_index as usize)
        .unwrap_or(Value::nil()))
}

/// Writes `value` into instance field `index` of `instance`, an
/// `ObjectShape::Instance` -- silently skipped when `instance` is not an
/// object, or is an object of some other `ObjectShape`, or `index` is
/// out of range. `vm::run_stretch`'s `Instruction::InitializeSlot` arm
/// writes a slot initializer's value through it.
pub(crate) fn store_instance_field<'gc>(
    heap: &mut Heap<'gc>,
    instance: Value<'gc>,
    index: usize,
    value: Value<'gc>,
) {
    if let Some(object) = instance.as_object() {
        heap.instance_set_field(object, index, value);
    }
}
