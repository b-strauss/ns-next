//! `ByteArray`'s leaves. A `ByteArray` is fixed-size once made, as an
//! `Array` is, and holds bytes, each an Integer from 0 to 255; its index
//! errors are `Array`'s, so a program meets one sequence protocol
//! whichever it holds.

use crate::builtins::collections::{
    integer_argument, not_an_integer, out_of_range, IntegerArgument,
};
use crate::builtins::first_argument;
use crate::builtins::primitive::BytePrimitive;
use crate::types::*;
use crate::{Mutator, Symbol};

/// `ByteArray`'s native methods. Each declines -- answers `None` -- for a
/// subject that is no `ByteArray`, a user subclass's instance among them,
/// so that the failure block's `super` send runs.
pub(crate) fn execute<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: BytePrimitive,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: &[Value<'gc>],
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    match primitive {
        BytePrimitive::Allocate => allocate(mutator, receiver, selector, arguments),
        BytePrimitive::Size => size(mutator, receiver),
        BytePrimitive::ElementAt => element_at(mutator, receiver, selector, arguments),
        BytePrimitive::ElementAtPut => element_at_put(mutator, receiver, selector, arguments),
    }
}

/// The subject as a `ByteArray` and its size; `None` for anything else.
fn byte_array<'gc>(
    mutator: &Mutator<'gc>,
    receiver: Value<'gc>,
) -> Option<(ObjectReference<'gc>, usize)> {
    let object = receiver.as_object()?;
    Some((object, mutator.heap.byte_array_size(object)?))
}

fn size<'gc>(
    mutator: &Mutator<'gc>,
    receiver: Value<'gc>,
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let (_, size) = byte_array(mutator, receiver)?;
    Some(Ok(Value::small_integer(size as i64)))
}

/// The zero-based position of the index argument in a `ByteArray` of
/// `size` bytes, or the `ArgumentError` `Array`'s leaves signal for it.
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

/// `at:`: the byte at the index, an Integer from 0 to 255.
fn element_at<'gc>(
    mutator: &Mutator<'gc>,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: &[Value<'gc>],
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let (object, size) = byte_array(mutator, receiver)?;
    Some(
        position(mutator, receiver, selector, arguments, size).map(|position| {
            let byte = mutator.heap.byte_array_at(object, position).unwrap_or(0);
            Value::small_integer(i64::from(byte))
        }),
    )
}

/// `at:put:`: stores the byte and answers the value. The index is checked
/// first, then the value, and nothing is written until both pass, so a
/// refused store leaves the byte as it was.
fn element_at_put<'gc>(
    mutator: &mut Mutator<'gc>,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: &[Value<'gc>],
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let (object, size) = byte_array(mutator, receiver)?;
    let position = match position(mutator, receiver, selector, arguments, size) {
        Ok(position) => position,
        Err(error) => return Some(Err(error)),
    };
    let value = arguments.get(1).copied().unwrap_or(Value::nil());
    let Some(byte) = value
        .as_small_integer()
        .and_then(|number| u8::try_from(number).ok())
    else {
        return Some(Err(NewspeakError::argument_error(
            selector,
            receiver,
            value,
            format!(
                "{selector} requires an Integer from 0 to 255, got {}",
                mutator.heap.describe(value)
            ),
        )));
    };
    mutator.heap.byte_array_set(object, position, byte);
    Some(Ok(value))
}

/// `ByteArray new: size`: a `ByteArray` of `size` zero bytes. Its subject
/// is the class the forwarder runs in, which is only ever `ByteArray`
/// itself -- §3.2's metaclasses are flat, so no subclass inherits `new:`
/// -- and anything else declines. A size that cannot be allocated -- past
/// the reservation, or more than the operating system grants
/// (`Heap::byte_array_fits`) -- is an error, found before anything is
/// allocated, never an abort: the size is the program's, and an abort is
/// not catchable.
fn allocate<'gc>(
    mutator: &mut Mutator<'gc>,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: &[Value<'gc>],
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let class = receiver.as_object()?;
    if mutator.heap.class_builtin(class) != Some(BuiltinClass::ByteArray) {
        return None;
    }
    let argument = arguments.first().copied();
    // Described only once a refusal needs it: a size that passes costs no
    // text.
    let refusal = |heap: &crate::Heap<'gc>, message: &dyn Fn(String) -> String| {
        NewspeakError::argument_error(
            selector,
            receiver,
            first_argument(arguments),
            message(heap.describe_argument(argument)),
        )
    };
    let cannot_allocate = |heap: &crate::Heap<'gc>| {
        refusal(heap, &|described| {
            format!("{selector} cannot allocate a ByteArray of {described} bytes")
        })
    };
    let below_zero = |heap: &crate::Heap<'gc>| {
        refusal(heap, &|described| {
            format!("{selector} requires a size of at least 0, got {described}")
        })
    };
    let size = match integer_argument(&mutator.heap, argument) {
        Some(IntegerArgument::Small(size)) => size,
        Some(IntegerArgument::Large { negative: false }) => {
            return Some(Err(cannot_allocate(&mutator.heap)));
        }
        Some(IntegerArgument::Large { negative: true }) => {
            return Some(Err(below_zero(&mutator.heap)));
        }
        None => {
            return Some(Err(refusal(&mutator.heap, &|described| {
                format!("{selector} requires an Integer size, got {described}")
            })));
        }
    };
    if size < 0 {
        return Some(Err(below_zero(&mutator.heap)));
    }
    // A non-negative size that does not fit a `usize` -- above `u32::MAX` on
    // a 32-bit target -- is refused as an allocation that cannot be made,
    // not as a negative size.
    let Some(size) = usize::try_from(size)
        .ok()
        .filter(|size| mutator.heap.byte_array_fits(*size))
    else {
        return Some(Err(cannot_allocate(&mutator.heap)));
    };
    Some(Ok(Value::object(
        mutator.heap.allocate_zeroed_byte_array(class, size),
    )))
}
