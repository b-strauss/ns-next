//! `String`, `Symbol`, and `Character` native methods.
//!
//! `Character` has no dedicated `ObjectData` payload variant --
//! `Mutator::new_character` represents a character as a one-`char`
//! `ObjectData::String`, tagged with the `BuiltinClass::Character` class so
//! dispatch still lands here (see that constructor's doc comment). This
//! module's `dispatch_character` therefore extracts its payload exactly like
//! `dispatch_string` does, just from a string that is always one `char`
//! long.

use crate::types::*;
use crate::{Heap, Mutator, Symbol};

/// Extracts an owned `String` from `argument` if it is a `String`-payload
/// object (covers both `String` and `Character` arguments, since they share a
/// payload representation). `None` for anything else, including `Symbol` (a
/// distinct payload) and non-`Object` values.
fn argument_string<'gc>(heap: &Heap<'gc>, argument: Option<&Value<'gc>>) -> Option<String> {
    match argument {
        Some(Value::Object(object)) => heap.str_value(*object),
        _ => None,
    }
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

pub(crate) fn dispatch_string<'gc>(
    mutator: &mut Mutator<'gc>,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: &[Value<'gc>],
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let Value::Object(object) = &receiver else {
        return None;
    };
    let string = mutator.heap.str_value(*object)?;
    match selector.as_str() {
        "size" => Some(Ok(Value::SmallInteger(string.chars().count() as i64))),
        "," => Some(match argument_string(&mutator.heap, arguments.first()) {
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
        }),
        "=" => Some(Ok(Value::Boolean(
            argument_string(&mutator.heap, arguments.first()).as_deref() == Some(string.as_str()),
        ))),
        "asSymbol" => Some(Ok(mutator.new_symbol(Symbol::intern(&string)))),
        "printString" => Some(Ok(Value::Object(mutator.new_string(quote_string(&string))))),
        _ => None,
    }
}

pub(crate) fn dispatch_symbol<'gc>(
    mutator: &mut Mutator<'gc>,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: &[Value<'gc>],
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let Value::Object(object) = &receiver else {
        return None;
    };
    let symbol = mutator.heap.symbol_value(*object)?;
    match selector.as_str() {
        "asString" => Some(Ok(Value::Object(
            mutator.new_string(symbol.as_str().to_string()),
        ))),
        "=" => Some(Ok(Value::Boolean(match arguments.first() {
            Some(Value::Object(argument_object)) => {
                mutator.heap.symbol_value(*argument_object) == Some(symbol)
            }
            _ => false,
        }))),
        "printString" => Some(Ok(Value::Object(
            mutator.new_string(format!("#{}", symbol.as_str())),
        ))),
        _ => None,
    }
}

pub(crate) fn dispatch_character<'gc>(
    mutator: &mut Mutator<'gc>,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: &[Value<'gc>],
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let Value::Object(object) = &receiver else {
        return None;
    };
    let string = mutator.heap.str_value(*object)?;
    // Invariant from `Mutator::new_character`: this payload is always
    // exactly one `char`. Fall back to `'\0'` rather than panicking if that
    // invariant is ever violated -- never panic on a Newspeak-input path.
    let character = string.chars().next().unwrap_or('\0');
    match selector.as_str() {
        "asInteger" => Some(Ok(Value::SmallInteger(character as i64))),
        "asString" => Some(Ok(Value::Object(mutator.new_string(character.to_string())))),
        "=" => Some(Ok(Value::Boolean(
            argument_string(&mutator.heap, arguments.first()).as_deref() == Some(string.as_str()),
        ))),
        _ => None,
    }
}
