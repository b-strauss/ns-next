//! `String` and `Symbol` native methods.

use crate::builtins::collections::{self, IntegerArgument};
use crate::builtins::first_argument;
use crate::builtins::primitive::{Primitive, StringPrimitive, SymbolPrimitive};
use crate::interpreter::Arguments;
use crate::types::*;
use crate::{Heap, Mutator, Symbol};

/// Whether `argument` is an object of `receiver`'s own class -- the guard
/// every `=` in this module opens with, and the reason each of them names
/// one class rather than one payload.
///
/// Each class hashes by its own content, a string by its text and a symbol
/// by its interned index, so a cross-class equality would break the one
/// rule a `hash` has, that `x = y` implies `x hash = y hash`. A string
/// equals only a string, and a symbol only a symbol.
fn of_the_same_class<'gc>(
    heap: &Heap<'gc>,
    receiver: ObjectReference<'gc>,
    argument: Option<Value<'gc>>,
) -> Option<ObjectReference<'gc>> {
    let other = argument.and_then(|value| value.as_object())?;
    heap.object_class(other)
        .same(heap.object_class(receiver))
        .then_some(other)
}

/// `String>>=`'s rule: an argument of the receiver's own class carrying
/// the same code points. Named rather than written twice, so that
/// `builtins::value`'s derived equality decides one exactly as the class's
/// own `=` does instead of growing a second copy of the rule.
pub(crate) fn text_equals<'gc>(
    heap: &Heap<'gc>,
    receiver: ObjectReference<'gc>,
    argument: Option<Value<'gc>>,
) -> bool {
    match of_the_same_class(heap, receiver, argument) {
        Some(other) => heap.strings_equal(receiver, other) == Some(true),
        None => false,
    }
}

/// `Symbol>>=`'s rule: a symbol is canonical, one object per text (§5.1.6),
/// so two symbols are equal exactly when they are the same object. A String
/// of the same text is never equal to it.
pub(crate) fn symbol_equals<'gc>(
    receiver: ObjectReference<'gc>,
    argument: Option<Value<'gc>>,
) -> bool {
    argument
        .and_then(|value| value.as_object())
        .is_some_and(|other| other.same(receiver))
}

/// A string's content hash: its text, not its address, since two separately
/// built strings of the same text are equal and so must hash alike.
pub(crate) fn string_hash(text: &str) -> i64 {
    crate::hash::content_hash(text)
}

/// A symbol's hash: its text's (`symbol::hash_of_symbol_text`), never its
/// number. §5.1.6 makes equal symbols one object, so it agrees with
/// `Symbol>>=` by construction. An interned symbol reads the hash the
/// interner stored -- one probe of the canonical table and one read, as
/// DeltaBlue's symbol-keyed lookups need it (`docs/design/execution.md`).
/// Only a program-made symbol whose text nothing has interned walks its
/// text: it is not cached, since such a symbol names no selector, slot or
/// class, and a word per entry of the weak table would be paid by every
/// one of them for the few a program hashes. `None` for anything but a
/// Symbol.
pub(crate) fn symbol_hash<'gc>(heap: &Heap<'gc>, object: ObjectReference<'gc>) -> Option<i64> {
    if let Some(symbol) = heap.symbol_value(object) {
        return Some(symbol.text_hash());
    }
    if !heap.is_symbol(object) {
        return None;
    }
    Some(crate::symbol::hash_of_symbol_text(
        heap.string_hash(object)?,
    ))
}

/// Mirrors `ns_syntax::lexer`'s string-literal escaping (`lex_string`): a
/// string literal is delimited by `'`, and an embedded `'` is written as two
/// consecutive `'` characters (no other escapes exist for Newspeak strings).
fn quote_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('\'');
    for character in text.chars() {
        if character == '\'' {
            out.push('\'');
            out.push('\'');
        } else {
            out.push(character);
        }
    }
    out.push('\'');
    out
}

/// `String with: code`: a one-element String. Primordial Soup's takes a
/// byte (`String_class_with`); this takes any code point, since an ns-next
/// String's elements are code points. Declines for a subject other than
/// `String` itself, as `Array new:` does.
fn from_code_point<'gc>(
    mutator: &mut Mutator<'gc>,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: Arguments<'_, 'gc>,
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let class = receiver.as_object()?;
    if mutator.heap.class_builtin(class) != Some(BuiltinClass::String) {
        return None;
    }
    let code_points = arguments
        .first(&mutator.heap)
        .and_then(|value| value.as_small_integer())
        .and_then(|code| u32::try_from(code).ok())
        .and_then(crate::CodePoints::from_code_point);
    Some(match code_points {
        Some(code_points) => Ok(Value::object(
            mutator.new_string_from_code_points(code_points),
        )),
        None => Err(NewspeakError::argument_error(
            selector,
            receiver,
            first_argument(&mutator.heap, arguments),
            format!(
                "{selector} requires a Unicode code point, an Integer from 0 to 1114111 \
                 outside 55296 to 57343, got {}",
                mutator
                    .heap
                    .describe_argument(arguments.first(&mutator.heap))
            ),
        )),
    })
}

/// The code point an element of `String withAll:`'s Array must be, as
/// `from_code_point` asks of its one argument: a `SmallInteger` that is a
/// Unicode scalar value.
fn code_point_of(element: Value<'_>) -> Option<u32> {
    element
        .as_small_integer()
        .and_then(|code| u32::try_from(code).ok())
        .filter(|code| char::from_u32(*code).is_some())
}

/// `String withAll: codePoints` for an Array of code points or a String:
/// a new String of them. `None` -- the class side gathers any other
/// collection into an Array and asks again -- for any other argument, or
/// a subject that is not `String` itself.
fn from_code_points<'gc>(
    mutator: &mut Mutator<'gc>,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: Arguments<'_, 'gc>,
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let class = receiver.as_object()?;
    if mutator.heap.class_builtin(class) != Some(BuiltinClass::String) {
        return None;
    }
    let argument = arguments.first(&mutator.heap)?.as_object()?;
    if let Some(code_points) = mutator.heap.string_code_points(argument) {
        return Some(Ok(Value::object(
            mutator.new_string_from_code_points(code_points),
        )));
    }
    let elements = mutator.heap.array_elements(argument)?;
    let mut code_points = Vec::with_capacity(elements.len());
    for element in elements {
        match code_point_of(element) {
            Some(code_point) => code_points.push(code_point),
            None => {
                return Some(Err(NewspeakError::argument_error(
                    selector,
                    receiver,
                    element,
                    format!(
                        "{selector} requires Unicode code points, Integers from 0 to 1114111 \
                         outside 55296 to 57343, got {}",
                        mutator.heap.describe(element)
                    ),
                )))
            }
        }
    }
    // Every element was checked to be a scalar value above, which is what
    // `from_wide` asks; it narrows a Latin-1 text, so the String is canonical.
    let code_points = crate::CodePoints::from_wide(code_points.into_boxed_slice());
    Some(Ok(Value::object(
        mutator.new_string_from_code_points(code_points),
    )))
}

/// The zero-based position a search's `startingAt:` argument names in a
/// sequence of `size` elements -- a String's code points, a ByteArray's
/// bytes -- or the `ArgumentError` carrying it: the start must lie in
/// 1..size + 1, one past the end being where only an empty argument is
/// found.
pub(crate) fn search_start<'gc>(
    heap: &Heap<'gc>,
    receiver: Value<'gc>,
    selector: Symbol,
    start_argument: Option<Value<'gc>>,
    size: usize,
) -> Result<usize, NewspeakError<'gc>> {
    // A `LargeInteger` start lies outside every sequence, so it takes the
    // out-of-range error, described as it is written.
    let start = match collections::integer_argument(heap, start_argument) {
        Some(IntegerArgument::Small(start)) => Some(start),
        Some(IntegerArgument::Large { .. }) => None,
        None => {
            return Err(collections::not_an_integer(
                heap,
                receiver,
                selector,
                start_argument,
            ))
        }
    };
    match start.map(usize::try_from) {
        Some(Ok(start)) if (1..=size + 1).contains(&start) => Ok(start - 1),
        _ => {
            let described = match start {
                Some(number) => number.to_string(),
                None => heap.describe_argument(start_argument),
            };
            Err(NewspeakError::argument_error(
                selector,
                receiver,
                start_argument.unwrap_or(Value::nil()),
                format!(
                    "{selector} {described} out of range: valid starts are 1..{} (size {size})",
                    size + 1
                ),
            ))
        }
    }
}

/// `indexOf:startingAt:` and `lastIndexOf:startingAt:` over code points:
/// the 1-based index of the first occurrence of the argument that starts
/// at or after the start, or of the last that starts at or before it, or
/// 0. The start must lie in 1..size + 1; an empty argument is found at
/// the start itself.
fn search<'gc>(
    mutator: &Mutator<'gc>,
    object: ObjectReference<'gc>,
    size: usize,
    backwards: bool,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: Arguments<'_, 'gc>,
) -> Result<Value<'gc>, NewspeakError<'gc>> {
    let heap = &mutator.heap;
    let needle = arguments
        .first(&mutator.heap)
        .and_then(|value| value.as_object())
        .and_then(|needle| heap.string_code_points(needle));
    let Some(needle) = needle else {
        return Err(NewspeakError::argument_error(
            selector,
            receiver,
            first_argument(&mutator.heap, arguments),
            format!(
                "{selector} requires a String argument, got {}",
                heap.describe_argument(arguments.first(&mutator.heap))
            ),
        ));
    };
    let position = search_start(
        heap,
        receiver,
        selector,
        arguments.try_get(&mutator.heap, 1),
        size,
    )?;
    let needle: Vec<u32> = needle.elements().collect();
    let Some(last_position) = size.checked_sub(needle.len()) else {
        return Ok(Value::small_integer(0));
    };
    let haystack: Vec<u32> = heap
        .string_code_points(object)
        .map(|code_points| code_points.elements().collect())
        .unwrap_or_default();
    let occurs_at = |position: &usize| {
        haystack
            .get(*position..*position + needle.len())
            .is_some_and(|candidate| candidate == needle.as_slice())
    };
    let found = if backwards {
        (0..=position.min(last_position)).rev().find(occurs_at)
    } else {
        (position..=last_position).find(occurs_at)
    };
    Ok(Value::small_integer(
        found.map_or(0, |position| position as i64 + 1),
    ))
}

/// `copyFrom: start to: stop`, 1-based and inclusive, over code points:
/// the range lies within the string, and the empty range is the one that
/// stops just before it starts, at either end. Its errors are `Array`'s
/// (`collections::checked_range`), so a program sees one text from either.
fn copy_range<'gc>(
    mutator: &mut Mutator<'gc>,
    object: ObjectReference<'gc>,
    size: usize,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: Arguments<'_, 'gc>,
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let range = match collections::checked_range(
        &mutator.heap,
        receiver,
        selector,
        "copyFrom:",
        arguments.first(&mutator.heap),
        arguments.try_get(&mutator.heap, 1),
        size,
    ) {
        Ok(range) => range,
        Err(error) => return Some(Err(error)),
    };
    let code_points = mutator.heap.string_slice(object, range.start, range.end)?;
    Some(Ok(Value::object(
        mutator.new_string_from_code_points(code_points),
    )))
}

/// `String fromUtf8: bytes from: start to: stop`: a new String of the
/// code points the bytes in the range encode, decoded strictly
/// (`CodePoints::from_utf8_strict`). A `bytes` that is no ByteArray is
/// refused before the range, which `copyFrom:to:`'s texts check, its
/// keyword `from:`; invalid UTF-8 is refused naming the first byte of the
/// bad sequence by its 1-based index in the ByteArray, not in the range,
/// so that `bytes at: position` is the bad byte. Declines for a subject
/// other than `String` itself, as `String with:` does.
fn from_utf8<'gc>(
    mutator: &mut Mutator<'gc>,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: Arguments<'_, 'gc>,
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let class = receiver.as_object()?;
    if mutator.heap.class_builtin(class) != Some(BuiltinClass::String) {
        return None;
    }
    let heap = &mutator.heap;
    let source = arguments.first(&mutator.heap);
    let Some((bytes, size)) = source
        .and_then(|value| value.as_object())
        .and_then(|object| Some((object, heap.byte_array_size(object)?)))
    else {
        return Some(Err(NewspeakError::argument_error(
            selector,
            receiver,
            first_argument(&mutator.heap, arguments),
            format!(
                "{selector} requires a ByteArray, got {}",
                heap.describe_argument(source)
            ),
        )));
    };
    let range = match collections::checked_range(
        heap,
        receiver,
        selector,
        "from:",
        arguments.try_get(&mutator.heap, 1),
        arguments.try_get(&mutator.heap, 2),
        size,
    ) {
        Ok(range) => range,
        Err(error) => return Some(Err(error)),
    };
    let start = range.start;
    let decoded = heap
        .with_byte_array(bytes, |data| {
            data.bytes(range)
                .map(|slice| crate::CodePoints::from_utf8_strict(&slice))
        })
        .flatten()?;
    Some(match decoded {
        Ok(code_points) => Ok(Value::object(
            mutator.new_string_from_code_points(code_points),
        )),
        Err(offset) => Err(NewspeakError::argument_error(
            selector,
            receiver,
            first_argument(&mutator.heap, arguments),
            format!("fromUtf8: invalid UTF-8 at byte {}", start + offset + 1),
        )),
    })
}

/// `asUtf8`: a new ByteArray of the receiver's UTF-8 bytes. Encoding a
/// String never fails; only a ByteArray the heap cannot commit is refused,
/// before anything is allocated, as `,` refuses one, since a failed
/// allocation would abort the process.
fn as_utf8<'gc>(
    mutator: &mut Mutator<'gc>,
    object: ObjectReference<'gc>,
    receiver: Value<'gc>,
    selector: Symbol,
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let code_points = mutator.heap.string_code_points(object)?;
    let size = code_points.utf8_size();
    if !mutator.heap.byte_array_fits(size) {
        return Some(Err(NewspeakError::argument_error(
            selector,
            receiver,
            receiver,
            format!("{selector} cannot allocate a ByteArray of {size} bytes"),
        )));
    }
    let bytes = code_points.to_utf8().into_bytes();
    Some(Ok(Value::object(mutator.new_byte_array(&bytes))))
}

pub(crate) fn execute_string<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: StringPrimitive,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: Arguments<'_, 'gc>,
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    // The class-side leaves: their subject is the class, not a String.
    match primitive {
        StringPrimitive::FromCodePoint => {
            return from_code_point(mutator, receiver, selector, arguments)
        }
        StringPrimitive::FromCodePoints => {
            return from_code_points(mutator, receiver, selector, arguments)
        }
        StringPrimitive::FromUtf8 => return from_utf8(mutator, receiver, selector, arguments),
        _ => {}
    }
    let object = receiver.as_object()?;
    // Declines for anything that is no String, before any leaf reads it.
    let size = mutator.heap.string_size(object)?;
    match primitive {
        // Unreachable: the early return above handles them; the arm keeps
        // the match exhaustive.
        StringPrimitive::FromCodePoint
        | StringPrimitive::FromCodePoints
        | StringPrimitive::FromUtf8 => None,
        StringPrimitive::AsUtf8 => as_utf8(mutator, object, receiver, selector),
        // Counted in place, as a String argument's bytes are, so that a
        // `StringBuilder`'s append copies nothing out to measure it.
        StringPrimitive::Utf8Size => {
            crate::builtins::bytes::argument_bytes(&mutator.heap, receiver)
                .map(|bytes| Ok(Value::small_integer(bytes.size() as i64)))
        }
        StringPrimitive::IndexOf | StringPrimitive::LastIndexOf => Some(search(
            mutator,
            object,
            size,
            primitive == StringPrimitive::LastIndexOf,
            receiver,
            selector,
            arguments,
        )),
        // Array's errors, so that a program sees one text from either.
        StringPrimitive::ElementAt => Some(
            match collections::integer_argument(&mutator.heap, arguments.first(&mutator.heap)) {
                Some(IntegerArgument::Small(index)) => {
                    match usize::try_from(index)
                        .ok()
                        .and_then(|index| index.checked_sub(1))
                        .and_then(|position| mutator.heap.string_code_point_at(object, position))
                    {
                        Some(code_point) => Ok(Value::small_integer(i64::from(code_point))),
                        None => Err(collections::out_of_range(
                            receiver,
                            selector,
                            index,
                            first_argument(&mutator.heap, arguments),
                            size,
                        )),
                    }
                }
                Some(IntegerArgument::Large { .. }) => Err(collections::out_of_range(
                    receiver,
                    selector,
                    mutator
                        .heap
                        .describe_argument(arguments.first(&mutator.heap)),
                    first_argument(&mutator.heap, arguments),
                    size,
                )),
                None => Err(collections::not_an_integer(
                    &mutator.heap,
                    receiver,
                    selector,
                    arguments.first(&mutator.heap),
                )),
            },
        ),
        StringPrimitive::CopyRange => {
            copy_range(mutator, object, size, receiver, selector, arguments)
        }
        StringPrimitive::Size => Some(Ok(Value::small_integer(size as i64))),
        StringPrimitive::Concatenate => {
            let joined = arguments
                .first(&mutator.heap)
                .and_then(|value| value.as_object())
                .and_then(|other| mutator.heap.string_concatenation(object, other));
            Some(match joined {
                Some(code_points) => Ok(Value::object(
                    mutator.new_string_from_code_points(code_points),
                )),
                None => Err(NewspeakError::argument_error(
                    selector,
                    receiver,
                    first_argument(&mutator.heap, arguments),
                    format!(
                        "String concatenation (,) requires a String argument, got {}",
                        mutator
                            .heap
                            .describe_argument(arguments.first(&mutator.heap))
                    ),
                )),
            })
        }
        StringPrimitive::Equal => {
            let equal = text_equals(&mutator.heap, object, arguments.first(&mutator.heap));
            crate::interpreter::future::declines_for_a_future(
                mutator,
                Primitive::String(primitive),
                equal,
                receiver,
                arguments,
            )
            .map(|answer| Ok(Value::boolean(answer)))
        }
        StringPrimitive::Hash => mutator
            .heap
            .string_hash(object)
            .map(|hash| Ok(Value::small_integer(hash))),
        StringPrimitive::AsSymbol => {
            let text = mutator.heap.str_value(object)?;
            Some(Ok(mutator.program_symbol(&text)))
        }
        StringPrimitive::PrintString => {
            let text = mutator.heap.str_value(object)?;
            Some(Ok(Value::object(mutator.new_string(quote_string(&text)))))
        }
    }
}

pub(crate) fn execute_symbol<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: SymbolPrimitive,
    receiver: Value<'gc>,
    arguments: Arguments<'_, 'gc>,
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let object = receiver.as_object()?;
    match primitive {
        SymbolPrimitive::AsString => {
            let text = mutator.heap.symbol_text(object)?;
            Some(Ok(Value::object(mutator.new_string(text))))
        }
        // Identity, a symbol being canonical (§5.1.6): it asks only whether
        // the receiver is one, never which.
        SymbolPrimitive::Equal => {
            if !mutator.heap.is_symbol(object) {
                return None;
            }
            let equal = symbol_equals(object, arguments.first(&mutator.heap));
            crate::interpreter::future::declines_for_a_future(
                mutator,
                Primitive::Symbol(primitive),
                equal,
                receiver,
                arguments,
            )
            .map(|answer| Ok(Value::boolean(answer)))
        }
        SymbolPrimitive::Hash => {
            let hash = symbol_hash(&mutator.heap, object)?;
            Some(Ok(Value::small_integer(hash)))
        }
        SymbolPrimitive::PrintString => {
            let text = mutator.heap.symbol_text(object)?;
            Some(Ok(Value::object(mutator.new_string(format!("#{text}")))))
        }
    }
}
