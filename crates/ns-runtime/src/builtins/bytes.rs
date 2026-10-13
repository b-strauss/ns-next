//! `ByteArray`'s leaves. A `ByteArray` is fixed-size once made, as an
//! `Array` is, and holds bytes, each an Integer from 0 to 255; its index
//! errors are `Array`'s, so a program meets one sequence protocol
//! whichever it holds.

use crate::builtins::collections::{
    checked_range, integer_argument, not_an_integer, out_of_range, source_start, IntegerArgument,
};
use crate::builtins::first_argument;
use crate::builtins::primitive::BytePrimitive;
use crate::builtins::text::search_start;
use crate::types::*;
use crate::{Heap, Mutator, Symbol};

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
        BytePrimitive::CopyRange => copy_range(mutator, receiver, selector, arguments),
        BytePrimitive::Replace => replace(mutator, receiver, selector, arguments),
        BytePrimitive::Concatenate => concatenate(mutator, receiver, selector, arguments),
        BytePrimitive::IndexOf => search(mutator, receiver, selector, arguments, false),
        BytePrimitive::LastIndexOf => search(mutator, receiver, selector, arguments, true),
        BytePrimitive::StartsWith => affix(mutator, receiver, selector, arguments, false),
        BytePrimitive::EndsWith => affix(mutator, receiver, selector, arguments, true),
        BytePrimitive::WriteCodePoint => write_code_point(mutator, receiver, selector, arguments),
    }
}

/// A `ByteArray` or a `String` argument read as bytes: a String counts as
/// its UTF-8 bytes, so that a program can write the bytes it means as a
/// string -- Newspeak has no byte literal -- and encoding never fails. The
/// bytes are read in place, a String's encoded as they are read, so a
/// comparison allocates nothing.
#[derive(Clone, Copy)]
pub(crate) struct ArgumentBytes<'gc> {
    object: ObjectReference<'gc>,
    /// Whether the argument is a String, whose units are code points,
    /// rather than a ByteArray, whose units are its bytes.
    text: bool,
    /// The number of units: bytes or code points.
    units: usize,
    /// The number of bytes.
    size: usize,
}

/// `value` as bytes when it is a `ByteArray` or a `String` -- a Symbol
/// among them -- and `None` for anything else.
pub(crate) fn argument_bytes<'gc>(
    heap: &Heap<'gc>,
    value: Value<'gc>,
) -> Option<ArgumentBytes<'gc>> {
    let object = value.as_object()?;
    if let Some(size) = heap.byte_array_size(object) {
        return Some(ArgumentBytes {
            object,
            text: false,
            units: size,
            size,
        });
    }
    let units = heap.string_size(object)?;
    let size = (0..units)
        .filter_map(|index| heap.string_code_point_at(object, index))
        .map(utf8_length)
        .sum();
    Some(ArgumentBytes {
        object,
        text: true,
        units,
        size,
    })
}

/// The number of UTF-8 bytes of `code_point`; every code point of a
/// String is a scalar value, so none counts 0.
fn utf8_length(code_point: u32) -> usize {
    char::from_u32(code_point).map_or(0, char::len_utf8)
}

impl<'gc> ArgumentBytes<'gc> {
    /// The number of bytes.
    pub(crate) fn size(self) -> usize {
        self.size
    }

    /// Whether the argument is the object `object`.
    fn is(self, object: ObjectReference<'gc>) -> bool {
        self.object.same(object)
    }

    /// The bytes, in order.
    pub(crate) fn bytes<'heap>(
        self,
        heap: &'heap Heap<'gc>,
    ) -> impl Iterator<Item = u8> + use<'heap, 'gc> {
        (0..self.units).flat_map(move |index| {
            let mut buffer = [0u8; 4];
            let length = if self.text {
                heap.string_code_point_at(self.object, index)
                    .and_then(char::from_u32)
                    .map_or(0, |character| character.encode_utf8(&mut buffer).len())
            } else {
                heap.byte_array_at(self.object, index).map_or(0, |byte| {
                    buffer[0] = byte;
                    1
                })
            };
            buffer.into_iter().take(length)
        })
    }

    /// The bytes in the 0-based, half-open `range`, copied out: what a
    /// store into a ByteArray reads first, since the receiver cannot be
    /// written while the argument is read.
    fn copy(self, heap: &Heap<'gc>, range: std::ops::Range<usize>) -> Vec<u8> {
        if self.text {
            self.bytes(heap)
                .skip(range.start)
                .take(range.len())
                .collect()
        } else {
            heap.with_byte_array(self.object, |data| data.bytes(range))
                .flatten()
                .unwrap_or_default()
        }
    }
}

/// What a search looks for: a byte, from an Integer argument -- `None`
/// for an Integer outside 0 to 255, which no ByteArray holds -- or the
/// bytes of a ByteArray or a String. One byte is found at the same places
/// either way.
#[derive(Clone, Copy)]
enum Needle<'gc> {
    Byte(Option<u8>),
    Bytes(ArgumentBytes<'gc>),
}

impl<'gc> Needle<'gc> {
    /// The argument as a needle; `None` for anything that is no Integer,
    /// ByteArray or String, which a search's failure block compares
    /// element by element with `=` and `startsWith:` and `endsWith:`
    /// refuse.
    fn of(heap: &Heap<'gc>, argument: Option<Value<'gc>>) -> Option<Needle<'gc>> {
        let argument = argument?;
        match integer_argument(heap, Some(argument)) {
            Some(IntegerArgument::Small(number)) => Some(Needle::Byte(u8::try_from(number).ok())),
            Some(IntegerArgument::Large { .. }) => Some(Needle::Byte(None)),
            None => argument_bytes(heap, argument).map(Needle::Bytes),
        }
    }

    /// The number of bytes it spans.
    fn size(self) -> usize {
        match self {
            Needle::Byte(_) => 1,
            Needle::Bytes(bytes) => bytes.size(),
        }
    }

    /// Whether it occurs in the ByteArray `object` at the 0-based
    /// `position`, every byte of it inside the ByteArray.
    fn occurs_at(self, heap: &Heap<'gc>, object: ObjectReference<'gc>, position: usize) -> bool {
        match self {
            Needle::Byte(byte) => byte.is_some() && heap.byte_array_at(object, position) == byte,
            Needle::Bytes(bytes) => heap
                .with_byte_array(object, |data| {
                    position
                        .checked_add(bytes.size())
                        .is_some_and(|end| end <= data.size())
                        && bytes
                            .bytes(heap)
                            .enumerate()
                            .all(|(offset, byte)| data.byte(position + offset) == Some(byte))
                })
                .unwrap_or(false),
        }
    }
}

/// `copyFrom:to:`: a new ByteArray of the bytes in the range, in
/// `Array`'s texts for a range the receiver does not hold.
fn copy_range<'gc>(
    mutator: &mut Mutator<'gc>,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: &[Value<'gc>],
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let (object, size) = byte_array(mutator, receiver)?;
    let range = match checked_range(
        &mutator.heap,
        receiver,
        selector,
        "copyFrom:",
        arguments.first().copied(),
        arguments.get(1).copied(),
        size,
    ) {
        Ok(range) => range,
        Err(error) => return Some(Err(error)),
    };
    let bytes = mutator
        .heap
        .with_byte_array(object, |data| data.bytes(range))
        .flatten()
        .unwrap_or_default();
    Some(Ok(Value::object(mutator.new_byte_array(&bytes))))
}

/// `replaceFrom:to:with:startingAt:` from a ByteArray or a String: stores
/// the source's bytes from `startingAt` on -- a String's UTF-8 bytes, the
/// start counted in bytes -- into the range, and answers the receiver.
/// `None` -- the sequence protocol's version runs, storing each element
/// through `at:put:` -- for any other source.
fn replace<'gc>(
    mutator: &mut Mutator<'gc>,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: &[Value<'gc>],
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let (object, size) = byte_array(mutator, receiver)?;
    let source = argument_bytes(&mutator.heap, arguments.get(2).copied()?)?;
    let range = match checked_range(
        &mutator.heap,
        receiver,
        selector,
        "replaceFrom:",
        arguments.first().copied(),
        arguments.get(1).copied(),
        size,
    ) {
        Ok(range) => range,
        Err(error) => return Some(Err(error)),
    };
    let first = match source_start(
        &mutator.heap,
        receiver,
        selector,
        arguments.get(3).copied(),
        range.len(),
        source.size(),
    ) {
        Ok(first) => first,
        Err(error) => return Some(Err(error)),
    };
    let source_range = first..first + range.len();
    // Every operand has been checked, so the store cannot stop halfway.
    // The receiver as its own source moves its bytes as `memmove` does;
    // any other source is read whole before the first byte is stored,
    // since the receiver cannot be written while it is read.
    let written = if source.is(object) {
        mutator.heap.with_byte_array_mut(object, |mut data| {
            data.copy_within(source_range, range.start)
        })
    } else {
        let bytes = source.copy(&mutator.heap, source_range);
        mutator
            .heap
            .with_byte_array_mut(object, |mut data| data.write(range.start, &bytes))
    };
    debug_assert_eq!(written, Some(true), "the ranges were checked");
    Some(Ok(receiver))
}

/// `,` with a ByteArray or a String: a new ByteArray of the receiver's
/// bytes followed by the argument's -- a String's UTF-8 bytes. `None` --
/// the sequence protocol's `,` runs -- for any other argument.
fn concatenate<'gc>(
    mutator: &mut Mutator<'gc>,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: &[Value<'gc>],
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let (object, size) = byte_array(mutator, receiver)?;
    let argument = argument_bytes(&mutator.heap, arguments.first().copied()?)?;
    // Two ByteArrays the heap holds can still join into one it cannot
    // commit: an error, found before anything is allocated, never an
    // abort.
    let Some(total) = size
        .checked_add(argument.size())
        .filter(|total| mutator.heap.byte_array_fits(*total))
    else {
        return Some(Err(NewspeakError::argument_error(
            selector,
            receiver,
            first_argument(arguments),
            format!(
                "{selector} cannot allocate a ByteArray of {} bytes",
                size as u128 + argument.size() as u128
            ),
        )));
    };
    let mut bytes = Vec::with_capacity(total);
    bytes.extend(
        mutator
            .heap
            .with_byte_array(object, |data| data.bytes(0..size))
            .flatten()
            .unwrap_or_default(),
    );
    bytes.extend(argument.bytes(&mutator.heap));
    Some(Ok(Value::object(mutator.new_byte_array(&bytes))))
}

/// `indexOf:startingAt:` and `lastIndexOf:startingAt:`: the 1-based index
/// of the first occurrence of the needle that starts at or after the
/// start, or of the last that starts at or before it, or 0. The start
/// must lie in 1..size + 1, and is checked whatever the needle, in a
/// String's texts; an empty needle is found at the start itself. `None`
/// -- the failure block compares element by element with `=` -- for a
/// needle that is no Integer, ByteArray or String.
fn search<'gc>(
    mutator: &Mutator<'gc>,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: &[Value<'gc>],
    backwards: bool,
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let heap = &mutator.heap;
    let (object, size) = byte_array(mutator, receiver)?;
    let position = match search_start(heap, receiver, selector, arguments.get(1).copied(), size) {
        Ok(position) => position,
        Err(error) => return Some(Err(error)),
    };
    let needle = Needle::of(heap, arguments.first().copied())?;
    let found = size.checked_sub(needle.size()).and_then(|last_position| {
        let occurs = |candidate: &usize| needle.occurs_at(heap, object, *candidate);
        if backwards {
            (0..=position.min(last_position)).rev().find(occurs)
        } else {
            (position..=last_position).find(occurs)
        }
    });
    Some(Ok(Value::small_integer(
        found.map_or(0, |position| position as i64 + 1),
    )))
}

/// `startsWith:` and `endsWith:`: whether the needle occurs at the start,
/// or at the end; an empty one always does. A needle that is no Integer,
/// ByteArray or String is an `ArgumentError` carrying it: unlike a search,
/// which `IndexedCollection`'s protocol defines for any element, these two
/// compare bytes only.
fn affix<'gc>(
    mutator: &Mutator<'gc>,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: &[Value<'gc>],
    at_end: bool,
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let heap = &mutator.heap;
    let (object, size) = byte_array(mutator, receiver)?;
    let Some(needle) = Needle::of(heap, arguments.first().copied()) else {
        return Some(Err(NewspeakError::argument_error(
            selector,
            receiver,
            first_argument(arguments),
            format!(
                "{selector} requires an Integer, a ByteArray or a String, got {}",
                heap.describe_argument(arguments.first().copied())
            ),
        )));
    };
    let occurs = size
        .checked_sub(needle.size())
        .is_some_and(|last_position| {
            needle.occurs_at(heap, object, if at_end { last_position } else { 0 })
        });
    Some(Ok(Value::boolean(occurs)))
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

/// `at:putCodePoint:`: stores the UTF-8 bytes of one code point from the
/// index on and answers how many it stored, 1 to 4. The index is checked
/// first, then the code point, then that its last byte lies inside the
/// ByteArray -- refused with the index of that byte, the first the
/// ByteArray lacks -- and nothing is written until all three pass, so a
/// refused store leaves the bytes as they were.
fn write_code_point<'gc>(
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
    let Some(character) = value
        .as_small_integer()
        .and_then(|number| u32::try_from(number).ok())
        .and_then(char::from_u32)
    else {
        return Some(Err(NewspeakError::argument_error(
            selector,
            receiver,
            value,
            format!(
                "{selector} requires a Unicode scalar value, got {}",
                mutator.heap.describe(value)
            ),
        )));
    };
    let mut buffer = [0u8; 4];
    let encoded = character.encode_utf8(&mut buffer).as_bytes();
    // `position` is below `size`, so the sum cannot overflow.
    let end = position + encoded.len();
    if end > size {
        return Some(Err(out_of_range(
            receiver,
            selector,
            end,
            first_argument(arguments),
            size,
        )));
    }
    let written = mutator
        .heap
        .with_byte_array_mut(object, |mut data| data.write(position, encoded));
    debug_assert_eq!(written, Some(true), "the range was checked");
    Some(Ok(Value::small_integer(encoded.len() as i64)))
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
