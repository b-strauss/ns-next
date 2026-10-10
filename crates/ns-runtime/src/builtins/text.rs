//! `String` and `Symbol` native methods.

use crate::builtins::collections::{self, IntegerArgument};
use crate::builtins::first_argument;
use crate::builtins::primitive::{Primitive, StringPrimitive, SymbolPrimitive};
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

/// A symbol's hash: its interned number. §5.1.6 makes equal symbols one
/// object, so the number is as stable as the symbol is and agrees with
/// `Symbol>>=` by construction. It is a `u32`, so it is already the
/// non-negative sub-2^32 answer a `hash` gives.
pub(crate) fn symbol_hash(symbol: Symbol) -> i64 {
    symbol.index() as i64
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
    arguments: &[Value<'gc>],
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let class = receiver.as_object()?;
    if mutator.heap.class_builtin(class) != Some(BuiltinClass::String) {
        return None;
    }
    let code_points = arguments
        .first()
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
            first_argument(arguments),
            format!(
                "{selector} requires a Unicode code point, an Integer from 0 to 1114111 \
                 outside 55296 to 57343, got {}",
                mutator.heap.describe_argument(arguments.first().copied())
            ),
        )),
    })
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
    arguments: &[Value<'gc>],
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let range = match collections::checked_range(
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
    let code_points = mutator.heap.string_slice(object, range.start, range.end)?;
    Some(Ok(Value::object(
        mutator.new_string_from_code_points(code_points),
    )))
}

pub(crate) fn execute_string<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: StringPrimitive,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: &[Value<'gc>],
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    if primitive == StringPrimitive::FromCodePoint {
        return from_code_point(mutator, receiver, selector, arguments);
    }
    let object = receiver.as_object()?;
    // Declines for anything that is no String, before any leaf reads it.
    let size = mutator.heap.string_size(object)?;
    match primitive {
        // Unreachable: the early return above handles it; the arm keeps the match exhaustive.
        StringPrimitive::FromCodePoint => None,
        // Array's errors, so that a program sees one text from either.
        StringPrimitive::ElementAt => Some(
            match collections::integer_argument(&mutator.heap, arguments.first().copied()) {
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
                            first_argument(arguments),
                            size,
                        )),
                    }
                }
                Some(IntegerArgument::Large { .. }) => Err(collections::out_of_range(
                    receiver,
                    selector,
                    mutator.heap.describe_argument(arguments.first().copied()),
                    first_argument(arguments),
                    size,
                )),
                None => Err(collections::not_an_integer(
                    &mutator.heap,
                    receiver,
                    selector,
                    arguments.first().copied(),
                )),
            },
        ),
        StringPrimitive::CopyRange => {
            copy_range(mutator, object, size, receiver, selector, arguments)
        }
        StringPrimitive::Size => Some(Ok(Value::small_integer(size as i64))),
        StringPrimitive::Concatenate => {
            let joined = arguments
                .first()
                .and_then(|value| value.as_object())
                .and_then(|other| mutator.heap.string_concatenation(object, other));
            Some(match joined {
                Some(code_points) => Ok(Value::object(
                    mutator.new_string_from_code_points(code_points),
                )),
                None => Err(NewspeakError::argument_error(
                    selector,
                    receiver,
                    first_argument(arguments),
                    format!(
                        "String concatenation (,) requires a String argument, got {}",
                        mutator.heap.describe_argument(arguments.first().copied())
                    ),
                )),
            })
        }
        StringPrimitive::Equal => {
            let equal = text_equals(&mutator.heap, object, arguments.first().copied());
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
            Some(Ok(mutator.new_symbol(Symbol::intern(&text))))
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
    arguments: &[Value<'gc>],
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let object = receiver.as_object()?;
    match primitive {
        SymbolPrimitive::AsString => {
            let symbol = mutator.heap.symbol_value(object)?;
            Some(Ok(Value::object(
                mutator.new_string(symbol.as_str().to_string()),
            )))
        }
        // Identity, a symbol being canonical (§5.1.6): it asks only whether
        // the receiver is one, never which.
        SymbolPrimitive::Equal => {
            if !mutator.heap.is_symbol(object) {
                return None;
            }
            let equal = symbol_equals(object, arguments.first().copied());
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
            let symbol = mutator.heap.symbol_value(object)?;
            Some(Ok(Value::small_integer(symbol_hash(symbol))))
        }
        SymbolPrimitive::PrintString => {
            let symbol = mutator.heap.symbol_value(object)?;
            Some(Ok(Value::object(
                mutator.new_string(format!("#{}", symbol.as_str())),
            )))
        }
    }
}
