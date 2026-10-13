//! The element access family: one leaf reads or writes an element of a kind
//! -- an integer of 8 to 64 bits, signed or not, or a binary32 or binary64
//! float -- on a `ByteArray`, at a 1-based element index for the typed
//! arrays or a 1-based byte position for `DataView`, in either byte order.
//! What the row asks for is the `ElementAccess` it carries, so no access
//! branches on a view's class, and the views stay Newspeak over a
//! `ByteArray` slot (`docs/design/core-library.md`, "The typed arrays").

use crate::builtins::collections::{integer_argument, not_an_integer, IntegerArgument};
use crate::builtins::first_argument;
use crate::builtins::numbers::integer_operand;
use crate::builtins::primitive::{ElementAccess, ElementKind};
use crate::heap::Element;
use crate::types::*;
use crate::{ArbitraryInteger, Heap, Mutator, Symbol};

/// Runs `access` on the `ByteArray` `receiver`: the leaf's own arguments
/// are the index or position, and for a write the value. Declines --
/// answers `None` -- for a subject that is no `ByteArray`, so that the
/// view's failure block reports the slot it found instead; every other
/// refusal is an `ArgumentError`, raised before anything is written.
pub(crate) fn execute<'gc>(
    mutator: &mut Mutator<'gc>,
    access: ElementAccess,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: &[Value<'gc>],
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let object = receiver.as_object()?;
    let size = mutator.heap.byte_array_size(object)?;
    let kind = access.kind();
    let offset = match offset(&mutator.heap, access, receiver, selector, arguments, size) {
        Ok(offset) => offset,
        Err(error) => return Some(Err(error)),
    };
    if access.write() {
        let value = arguments.get(1).copied().unwrap_or(Value::nil());
        let element = match element_operand(&mutator.heap, kind, receiver, selector, value) {
            Ok(element) => element,
            Err(error) => return Some(Err(error)),
        };
        let written =
            mutator
                .heap
                .set_byte_array_element(object, offset, kind, access.big_endian(), element);
        debug_assert!(written, "the offset and the element were checked");
        return Some(Ok(value));
    }
    let element = mutator
        .heap
        .byte_array_element(object, offset, kind, access.big_endian());
    debug_assert!(element.is_some(), "the offset was checked");
    Some(Ok(match element {
        Some(Element::Float(value)) => mutator.new_float(value),
        Some(Element::Integer(value)) => new_integer(mutator, value),
        None => Value::nil(),
    }))
}

/// `value` as an Integer: a `SmallInteger` directly, so that reading the
/// elements of a narrow kind allocates nothing, and only a value past 61
/// bits -- an `Int64Array`'s or `Uint64Array`'s -- through
/// `ArbitraryInteger`, as a `LargeInteger`.
fn new_integer<'gc>(mutator: &mut Mutator<'gc>, value: i128) -> Value<'gc> {
    match i64::try_from(value) {
        Ok(small) if crate::heap::fits_small_integer(small) => Value::small_integer(small),
        _ => mutator.new_integer(ArbitraryInteger::from_i128(value)),
    }
}

/// The 0-based byte offset of the element the first argument names: an
/// element index from 1 to the number of whole elements, in `Array`'s
/// texts, or a byte position from 1 to the last at which the element's
/// bytes all lie inside, in `DataView`'s.
fn offset<'gc>(
    heap: &Heap<'gc>,
    access: ElementAccess,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: &[Value<'gc>],
    size: usize,
) -> Result<usize, NewspeakError<'gc>> {
    let width = access.kind().width();
    let argument = arguments.first().copied();
    let refusal = |index: &dyn std::fmt::Display| {
        if access.byte_position() {
            // The last position at which a whole element fits; 0 -- no
            // position at all -- when the bytes are fewer than its width.
            let last = (size + 1).saturating_sub(width);
            NewspeakError::argument_error(
                selector,
                receiver,
                first_argument(arguments),
                format!(
                    "{selector} position {index} out of range: valid positions are 1..{last} (size {size})"
                ),
            )
        } else {
            crate::builtins::collections::out_of_range(
                receiver,
                selector,
                index,
                first_argument(arguments),
                size / width,
            )
        }
    };
    match integer_argument(heap, argument) {
        Some(IntegerArgument::Small(index)) => {
            let first_byte = usize::try_from(index)
                .ok()
                .and_then(|index| index.checked_sub(1))
                .and_then(|index| {
                    if access.byte_position() {
                        Some(index)
                    } else {
                        index.checked_mul(width)
                    }
                });
            match first_byte {
                Some(first_byte)
                    if first_byte.checked_add(width).is_some_and(|end| end <= size) =>
                {
                    Ok(first_byte)
                }
                _ => Err(refusal(&index)),
            }
        }
        Some(IntegerArgument::Large { .. }) => Err(refusal(&heap.describe_argument(argument))),
        None => Err(not_an_integer(heap, receiver, selector, argument)),
    }
}

/// The value to store as an element of `kind`: for an integer kind an
/// Integer inside the kind's range, for a float kind a Float, which the
/// view has made of any Number with `asFloat`. Anything else is an
/// `ArgumentError` carrying the value.
fn element_operand<'gc>(
    heap: &Heap<'gc>,
    kind: ElementKind,
    receiver: Value<'gc>,
    selector: Symbol,
    value: Value<'gc>,
) -> Result<Element, NewspeakError<'gc>> {
    let Some((min, max)) = kind.integer_range() else {
        return heap.float_value(value).map(Element::Float).ok_or_else(|| {
            NewspeakError::argument_error(
                selector,
                receiver,
                value,
                format!("{selector} requires a Number, got {}", heap.describe(value)),
            )
        });
    };
    integer_operand(heap, Some(value))
        .and_then(|integer| narrow(&integer))
        .filter(|integer| (min..=max).contains(integer))
        .map(Element::Integer)
        .ok_or_else(|| {
            NewspeakError::argument_error(
                selector,
                receiver,
                value,
                format!(
                    "{selector} requires an Integer from {min} to {max}, got {}",
                    heap.describe(value)
                ),
            )
        })
}

/// `integer` as an `i128` when its magnitude fits one digit -- every
/// element kind's range does -- and `None` beyond, which no kind holds.
fn narrow(integer: &ArbitraryInteger) -> Option<i128> {
    match integer.magnitude() {
        [] => Some(0),
        [digit] if integer.is_negative() => Some(-i128::from(*digit)),
        [digit] => Some(i128::from(*digit)),
        _ => None,
    }
}
