//! `WeakArray`'s leaves, `garbageCollect`, `Ephemeron`'s fields, and the
//! question `Ephemeron` asks of a key, whether it is a value. A
//! `WeakArray` is fixed-size once made, as an `Array` is, and refuses an
//! index or a size in `Array`'s texts, so a program meets one sequence
//! protocol whichever it holds; what differs is that the collectors clear
//! a slot whose object nothing else reaches (`docs/design/core-library.md`,
//! "Weak references").

use crate::builtins::collections::{
    integer_argument, not_an_integer, out_of_range, IntegerArgument,
};
use crate::builtins::first_argument;
use crate::builtins::primitive::{Primitive, WeakPrimitive};
use crate::interpreter::instantiate;
use crate::interpreter::Arguments;
use crate::types::*;
use crate::{Mutator, Symbol};

/// `WeakArray`'s native methods, `garbageCollect`, `Ephemeron`'s accessors
/// and `objectIsValue:`. Each `WeakArray` leaf declines -- answers `None`
/// -- for a subject that is no `WeakArray`, a user subclass's instance
/// among them, which holds no weak slots, so that the failure block's
/// `super` send runs. An `Ephemeron` leaf declines for a subject that is
/// neither an `Ephemeron` nor an instance of a class that inherits
/// `Ephemeron`'s mixin. `garbageCollect` never declines, and
/// `objectIsValue:` only for a future it would answer `false` to.
pub(crate) fn execute<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: WeakPrimitive,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: Arguments<'_, 'gc>,
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    match primitive {
        WeakPrimitive::Allocate => allocate(mutator, receiver, selector, arguments),
        WeakPrimitive::Size => size(mutator, receiver),
        WeakPrimitive::ElementAt => element_at(mutator, receiver, selector, arguments),
        WeakPrimitive::ElementAtPut => element_at_put(mutator, receiver, selector, arguments),
        WeakPrimitive::Collect => Some(Ok(collect(mutator, receiver))),
        WeakPrimitive::EphemeronKey => ephemeron_field(mutator, receiver, KEY).map(Ok),
        WeakPrimitive::EphemeronValue => ephemeron_field(mutator, receiver, VALUE).map(Ok),
        WeakPrimitive::EphemeronFinalizer => ephemeron_field(mutator, receiver, FINALIZER).map(Ok),
        WeakPrimitive::EphemeronSetKey => {
            ephemeron_set_field(mutator, receiver, KEY, arguments).map(Ok)
        }
        WeakPrimitive::EphemeronSetValue => {
            ephemeron_set_field(mutator, receiver, VALUE, arguments).map(Ok)
        }
        WeakPrimitive::EphemeronSetFinalizer => {
            ephemeron_set_field(mutator, receiver, FINALIZER, arguments).map(Ok)
        }
        // Declined for a future it would answer `false` to, so the failure
        // path asks again of the future's value
        // (`interpreter::future::declines_for_a_future`).
        WeakPrimitive::IsValue => crate::interpreter::future::declines_for_a_future(
            mutator,
            Primitive::Weak(primitive),
            is_a_value(mutator, receiver),
            receiver,
            arguments,
        )
        .map(|answer| Ok(Value::boolean(answer))),
    }
}

/// The subject as a `WeakArray` and its size; `None` for anything else.
fn weak_array<'gc>(
    mutator: &Mutator<'gc>,
    receiver: Value<'gc>,
) -> Option<(ObjectReference<'gc>, usize)> {
    let object = receiver.as_object()?;
    Some((object, mutator.heap.weak_array_size(object)?))
}

fn size<'gc>(
    mutator: &Mutator<'gc>,
    receiver: Value<'gc>,
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let (_, size) = weak_array(mutator, receiver)?;
    Some(Ok(Value::small_integer(size as i64)))
}

/// The zero-based position of the index argument in a `WeakArray` of
/// `size` slots, or the `ArgumentError` `Array`'s leaves signal for it.
fn position<'gc>(
    mutator: &Mutator<'gc>,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: Arguments<'_, 'gc>,
    size: usize,
) -> Result<usize, NewspeakError<'gc>> {
    let argument = arguments.first(&mutator.heap);
    match integer_argument(&mutator.heap, argument) {
        Some(IntegerArgument::Small(index)) => match usize::try_from(index) {
            Ok(index) if index >= 1 && index <= size => Ok(index - 1),
            _ => Err(out_of_range(
                receiver,
                selector,
                index,
                first_argument(&mutator.heap, arguments),
                size,
            )),
        },
        Some(IntegerArgument::Large { .. }) => Err(out_of_range(
            receiver,
            selector,
            mutator.heap.describe_argument(argument),
            first_argument(&mutator.heap, arguments),
            size,
        )),
        None => Err(not_an_integer(&mutator.heap, receiver, selector, argument)),
    }
}

/// `at:`: the object in the slot, or nil once the collector reclaimed it.
fn element_at<'gc>(
    mutator: &Mutator<'gc>,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: Arguments<'_, 'gc>,
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let (object, size) = weak_array(mutator, receiver)?;
    Some(
        position(mutator, receiver, selector, arguments, size).map(|position| {
            mutator
                .heap
                .weak_array_element(object, position)
                .unwrap_or(Value::nil())
        }),
    )
}

/// `at:put:`: stores the value, any object, and answers it. A refused
/// index writes nothing.
fn element_at_put<'gc>(
    mutator: &mut Mutator<'gc>,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: Arguments<'_, 'gc>,
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let (object, size) = weak_array(mutator, receiver)?;
    let position = match position(mutator, receiver, selector, arguments, size) {
        Ok(position) => position,
        Err(error) => return Some(Err(error)),
    };
    let value = arguments.try_get(&mutator.heap, 1).unwrap_or(Value::nil());
    mutator.heap.weak_array_set_element(object, position, value);
    Some(Ok(value))
}

/// `WeakArray new: size`: a `WeakArray` of `size` nils. Its subject is the
/// class the forwarder runs in, which is only ever `WeakArray` itself --
/// §3.2's metaclasses are flat, so no subclass inherits `new:` -- and
/// anything else declines. A size that cannot be allocated
/// (`Heap::weak_array_fits`) is an error, found before anything is
/// allocated, never an abort: the size is the program's, and an abort is
/// not catchable.
fn allocate<'gc>(
    mutator: &mut Mutator<'gc>,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: Arguments<'_, 'gc>,
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let class = receiver.as_object()?;
    if mutator.heap.class_builtin(class) != Some(BuiltinClass::WeakArray) {
        return None;
    }
    let argument = arguments.first(&mutator.heap);
    let refusal = |message: String| {
        NewspeakError::argument_error(
            selector,
            receiver,
            first_argument(&mutator.heap, arguments),
            message,
        )
    };
    let size = match integer_argument(&mutator.heap, argument) {
        Some(IntegerArgument::Small(size)) => size,
        Some(IntegerArgument::Large { negative }) => {
            let described = mutator.heap.describe_argument(argument);
            return Some(Err(refusal(if negative {
                format!("{selector} requires a size of at least 0, got {described}")
            } else {
                format!("{selector} cannot allocate a WeakArray of {described} elements")
            })));
        }
        None => {
            return Some(Err(refusal(format!(
                "{selector} requires an Integer size, got {}",
                mutator.heap.describe_argument(argument)
            ))));
        }
    };
    if size < 0 {
        return Some(Err(refusal(format!(
            "{selector} requires a size of at least 0, got {size}"
        ))));
    }
    // A non-negative size that does not fit a `usize` -- above `u32::MAX` on
    // a 32-bit target -- is refused as an allocation that cannot be made,
    // not as a negative size.
    let Some(length) = usize::try_from(size)
        .ok()
        .filter(|length| mutator.heap.weak_array_fits(*length))
    else {
        return Some(Err(refusal(format!(
            "{selector} cannot allocate a WeakArray of {size} elements"
        ))));
    };
    Some(Ok(Value::object(
        mutator.heap.allocate_weak_array(class, length),
    )))
}

/// An ephemeron's key, the field the collectors read as the key.
const KEY: usize = 0;
/// An ephemeron's value, reached through a live key.
const VALUE: usize = 1;
/// An ephemeron's finalizer, reached through a live key.
const FINALIZER: usize = 2;

/// The field at `index` of `receiver`'s ephemeron state -- 0 the key, 1 the
/// value, 2 the finalizer; `None` when `receiver` has none.
///
/// An `Ephemeron` keeps the three in fields of its own format, which the
/// slot accessors do not reach, so that an ordinary object's slot pays
/// nothing for the ephemeron's format; the three slots `Kernel.ns`
/// declares give it its size and lay out a subclass's instance, which is an
/// ordinary object, held strongly (`docs/design/core-library.md`, "A user
/// subclass gets no payload"), whose three are its slots at the level
/// applying `Ephemeron`'s mixin.
fn ephemeron_field<'gc>(
    mutator: &Mutator<'gc>,
    receiver: Value<'gc>,
    index: usize,
) -> Option<Value<'gc>> {
    let object = receiver.as_object()?;
    if let Some(value) = mutator.heap.ephemeron_field(object, index) {
        return Some(value);
    }
    let base = ephemeron_slot_base(mutator, object)?;
    mutator
        .heap
        .instance_field(object, base.checked_add(index)?)
}

/// Stores the argument, any object, in the field at `index` of
/// `receiver`'s ephemeron state, as `ephemeron_field` finds it, through the
/// write barrier, and answers the receiver; `None` when `receiver` has
/// none. `key:`'s refusal of a value is `Kernel.ns`'s, before it sends
/// this: its text ends in the key's `printString`, which only Newspeak can
/// produce.
fn ephemeron_set_field<'gc>(
    mutator: &mut Mutator<'gc>,
    receiver: Value<'gc>,
    index: usize,
    arguments: Arguments<'_, 'gc>,
) -> Option<Value<'gc>> {
    let object = receiver.as_object()?;
    let value = arguments.first(&mutator.heap).unwrap_or(Value::nil());
    if mutator.heap.ephemeron_set_field(object, index, value) {
        return Some(receiver);
    }
    let field = ephemeron_slot_base(mutator, object)?.checked_add(index)?;
    mutator
        .heap
        .instance_set_field(object, field, value)
        .then_some(receiver)
}

/// Where the slots `Ephemeron`'s mixin declares begin in `object`'s
/// fields: the own slot base of the first level of its class's chain that
/// applies that mixin -- the kernel's `Ephemeron` above a subclass, or a
/// level a `<:` clause added (`Heap::with_level_applying_mixin_of`), so
/// that every class whose methods are `Ephemeron`'s finds the slots they
/// were declared with. `None` when no level applies it.
fn ephemeron_slot_base<'gc>(mutator: &Mutator<'gc>, object: ObjectReference<'gc>) -> Option<usize> {
    mutator.heap.with_level_applying_mixin_of(
        mutator.heap.object_class(object),
        mutator.builtin_class(BuiltinClass::Ephemeron),
        |application| application.own_slot_base,
    )
}

/// `garbageCollect`: asks for a full collection and answers the receiver.
/// It collects nothing itself: a collection is sound only at a safepoint
/// of the run loop, where nothing outside the heap holds a handle. The
/// return from the activated method that sent the kernel's
/// `garbageCollect` -- the kernel facet's, an ordinary method for this
/// reason -- is one, so the collection has run before that method's
/// sender runs on (`Interpreter::safepoint`).
fn collect<'gc>(mutator: &mut Mutator<'gc>, receiver: Value<'gc>) -> Value<'gc> {
    mutator.heap.request_collection();
    receiver
}

/// Whether `subject` is a value (§3.1.1): an immediate -- a SmallInteger,
/// a SmallFloat, nil, true, false -- always, and an object when
/// `instantiate::is_a_value` accepts it. A weak structure refuses such a
/// key, since an equal one can always be made again and an entry under it
/// would vanish or not as the collector happened to run. It answers only a
/// Boolean, so it hands the program nothing it did not already hold; and
/// it declines for a future it would answer `false` to, so that a future
/// standing for a value is refused as that value is.
fn is_a_value<'gc>(mutator: &Mutator<'gc>, subject: Value<'gc>) -> bool {
    match subject.as_object() {
        Some(object) => instantiate::is_a_value(mutator, object),
        None => true,
    }
}
