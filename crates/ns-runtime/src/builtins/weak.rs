//! `WeakArray`'s leaves, `garbageCollect`, and the question `Ephemeron`
//! asks of a key, whether it is a value. A `WeakArray` is fixed-size
//! once made, as an `Array` is, and refuses an index or a size in
//! `Array`'s texts, so a program meets one sequence protocol whichever it
//! holds; what differs is that the collectors clear a slot whose object
//! nothing else reaches (`docs/design/core-library.md`, "Planned:
//! sub-project 6, weak references").

use crate::builtins::collections::{
    integer_argument, not_an_integer, out_of_range, IntegerArgument,
};
use crate::builtins::first_argument;
use crate::builtins::primitive::{Primitive, WeakPrimitive};
use crate::interpreter::instantiate;
use crate::types::*;
use crate::{Mutator, Symbol};

/// `WeakArray`'s native methods, `garbageCollect` and `objectIsValue:`.
/// Each `WeakArray` leaf declines -- answers `None` -- for a subject that
/// is no `WeakArray`, a user subclass's instance among them, which holds
/// no weak slots, so that the failure block's `super` send runs.
/// `garbageCollect` never declines, and `objectIsValue:` only for a future
/// it would answer `false` to.
pub(crate) fn execute<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: WeakPrimitive,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: &[Value<'gc>],
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    match primitive {
        WeakPrimitive::Allocate => allocate(mutator, receiver, selector, arguments),
        WeakPrimitive::Size => size(mutator, receiver),
        WeakPrimitive::ElementAt => element_at(mutator, receiver, selector, arguments),
        WeakPrimitive::ElementAtPut => element_at_put(mutator, receiver, selector, arguments),
        WeakPrimitive::Collect => Some(Ok(collect(mutator, receiver))),
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
    arguments: &[Value<'gc>],
    size: usize,
) -> Result<usize, NewspeakError<'gc>> {
    let argument = arguments.first().copied();
    match integer_argument(&mutator.heap, argument) {
        Some(IntegerArgument::Small(index)) => match usize::try_from(index) {
            Ok(index) if index >= 1 && index <= size => Ok(index - 1),
            _ => Err(out_of_range(
                receiver,
                selector,
                index,
                first_argument(arguments),
                size,
            )),
        },
        Some(IntegerArgument::Large { .. }) => Err(out_of_range(
            receiver,
            selector,
            mutator.heap.describe_argument(argument),
            first_argument(arguments),
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
    arguments: &[Value<'gc>],
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
    arguments: &[Value<'gc>],
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let (object, size) = weak_array(mutator, receiver)?;
    let position = match position(mutator, receiver, selector, arguments, size) {
        Ok(position) => position,
        Err(error) => return Some(Err(error)),
    };
    let value = arguments.get(1).copied().unwrap_or(Value::nil());
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
    arguments: &[Value<'gc>],
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let class = receiver.as_object()?;
    if mutator.heap.class_builtin(class) != Some(BuiltinClass::WeakArray) {
        return None;
    }
    let argument = arguments.first().copied();
    let refusal = |message: String| {
        NewspeakError::argument_error(selector, receiver, first_argument(arguments), message)
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
