//! `String`, `Symbol`, and `Character` native methods.
//!
//! `Character` has no dedicated `ObjectData` payload variant --
//! `Mutator::new_character` represents a character as a one-`char`
//! `ObjectData::String`, tagged with the `BuiltinClass::Character` class so
//! dispatch still lands here (see that constructor's doc comment). This
//! module's `execute_character` therefore extracts its payload exactly like
//! `execute_string` does, just from a string that is always one `char`
//! long.

use crate::builtins::primitive::{CharacterPrimitive, StringPrimitive, SymbolPrimitive};
use crate::types::*;
use crate::{Heap, Mutator, Symbol};

/// Extracts an owned `String` from `argument` if it is a `String`-payload
/// object (covers both `String` and `Character` arguments, since they share a
/// payload representation). `None` for anything else, including `Symbol` (a
/// distinct payload) and non-`Object` values.
///
/// Concatenation's rule, and only concatenation's: `'ab' , "c"` is meant to
/// work. Equality reads the class instead (`of_the_same_class`), which is
/// what keeps `"a" = 'a'` false.
fn argument_string<'gc>(heap: &Heap<'gc>, argument: Option<&Value<'gc>>) -> Option<String> {
    match argument {
        Some(Value::Object(object)) => heap.str_value(*object),
        _ => None,
    }
}

/// Whether `argument` is an object of `receiver`'s own class -- the guard
/// every `=` in this module opens with, and the reason each of them names
/// one class rather than one payload.
///
/// `String` and `Character` share an `ObjectData::String` payload, so a
/// rule that compared payloads alone made `"a" = 'a'` true. That was
/// tolerable only while both hashed by identity: once each class hashes by
/// its own content -- a character by its code point, a string by its text
/// -- a cross-class equality breaks the one rule a `hash` has, that `x = y`
/// implies `x hash = y hash`. A character equals only a character, a
/// string only a string, and a symbol only a symbol.
fn of_the_same_class<'gc>(
    heap: &Heap<'gc>,
    receiver: ObjectReference<'gc>,
    argument: Option<&Value<'gc>>,
) -> Option<ObjectReference<'gc>> {
    let Some(Value::Object(other)) = argument else {
        return None;
    };
    heap.class_of_object(*other)
        .same(heap.class_of_object(receiver))
        .then_some(*other)
}

/// `String>>=` and `Character>>=`'s rule: an argument of the receiver's own
/// class carrying the same text. Named rather than written twice, so that
/// `builtins::value`'s derived equality decides one exactly as the class's
/// own `=` does instead of growing a second copy of the rule.
pub(crate) fn text_equals<'gc>(
    heap: &Heap<'gc>,
    receiver: ObjectReference<'gc>,
    text: &str,
    argument: Option<&Value<'gc>>,
) -> bool {
    match of_the_same_class(heap, receiver, argument) {
        Some(other) => heap.str_value(other).as_deref() == Some(text),
        None => false,
    }
}

/// `Symbol>>=`'s rule: equal symbols are one interned symbol. A `Symbol`
/// payload only ever belongs to the one `Symbol` class, so the class guard
/// decides nothing here on its own; it is written out so that the three
/// rules read alike and a later payload that is shared the way `String`'s
/// is cannot quietly reopen the cross-class hole.
pub(crate) fn symbol_equals<'gc>(
    heap: &Heap<'gc>,
    receiver: ObjectReference<'gc>,
    symbol: Symbol,
    argument: Option<&Value<'gc>>,
) -> bool {
    match of_the_same_class(heap, receiver, argument) {
        Some(other) => heap.symbol_value(other) == Some(symbol),
        None => false,
    }
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

/// The one `char` a `Character`'s payload holds. The invariant comes from
/// `Mutator::new_character`; a payload that ever violated it falls back to
/// `'\0'` rather than panicking, because this is on a Newspeak-input path.
pub(crate) fn character_of(text: &str) -> char {
    text.chars().next().unwrap_or('\0')
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

pub(crate) fn execute_string<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: StringPrimitive,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: &[Value<'gc>],
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let Value::Object(object) = &receiver else {
        return None;
    };
    let string = mutator.heap.str_value(*object)?;
    match primitive {
        StringPrimitive::Size => Some(Ok(Value::SmallInteger(string.chars().count() as i64))),
        StringPrimitive::Concatenate => {
            Some(match argument_string(&mutator.heap, arguments.first()) {
                Some(other) => Ok(Value::Object(
                    mutator.new_string(format!("{string}{other}")),
                )),
                None => Err(NewspeakError {
                    selector,
                    receiver,
                    message: format!(
                        "String concatenation (,) requires a String argument, got {}",
                        mutator.heap.describe_argument(arguments.first())
                    ),
                }),
            })
        }
        StringPrimitive::Equal => Some(Ok(Value::Boolean(text_equals(
            &mutator.heap,
            *object,
            &string,
            arguments.first(),
        )))),
        StringPrimitive::Hash => Some(Ok(Value::SmallInteger(string_hash(&string)))),
        StringPrimitive::AsSymbol => Some(Ok(mutator.new_symbol(Symbol::intern(&string)))),
        StringPrimitive::PrintString => {
            Some(Ok(Value::Object(mutator.new_string(quote_string(&string)))))
        }
    }
}

pub(crate) fn execute_symbol<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: SymbolPrimitive,
    receiver: Value<'gc>,
    arguments: &[Value<'gc>],
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let Value::Object(object) = &receiver else {
        return None;
    };
    let symbol = mutator.heap.symbol_value(*object)?;
    match primitive {
        SymbolPrimitive::AsString => Some(Ok(Value::Object(
            mutator.new_string(symbol.as_str().to_string()),
        ))),
        SymbolPrimitive::Equal => Some(Ok(Value::Boolean(symbol_equals(
            &mutator.heap,
            *object,
            symbol,
            arguments.first(),
        )))),
        SymbolPrimitive::Hash => Some(Ok(Value::SmallInteger(symbol_hash(symbol)))),
        SymbolPrimitive::PrintString => Some(Ok(Value::Object(
            mutator.new_string(format!("#{}", symbol.as_str())),
        ))),
    }
}

pub(crate) fn execute_character<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: CharacterPrimitive,
    receiver: Value<'gc>,
    arguments: &[Value<'gc>],
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let Value::Object(object) = &receiver else {
        return None;
    };
    let string = mutator.heap.str_value(*object)?;
    let character = character_of(&string);
    match primitive {
        CharacterPrimitive::AsInteger => Some(Ok(Value::SmallInteger(character as i64))),
        CharacterPrimitive::AsString => {
            Some(Ok(Value::Object(mutator.new_string(character.to_string()))))
        }
        CharacterPrimitive::Equal => Some(Ok(Value::Boolean(text_equals(
            &mutator.heap,
            *object,
            &string,
            arguments.first(),
        )))),
    }
}
