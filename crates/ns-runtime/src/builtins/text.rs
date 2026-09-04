//! `String`, `Symbol`, and `Character` native methods.
//!
//! `Character` has no dedicated `ObjectData` payload variant in Milestone 1
//! -- `Interpreter::new_character` represents a character as a one-`char`
//! `ObjectData::Str`, tagged with the `BuiltinClass::Character` class so
//! dispatch still lands here (see that constructor's doc comment). This
//! module's `dispatch_character` therefore extracts its payload exactly like
//! `dispatch_string` does, just from a string that is always one `char`
//! long.

use crate::types::*;
use crate::{Interpreter, Symbol};

/// Extracts an owned `String` from `arg` if it is a `Str`-payload object
/// (covers both `String` and `Character` arguments, since they share a
/// payload representation). `None` for anything else, including `Symbol`
/// (a distinct payload) and non-`Obj` values.
fn arg_str(arg: Option<&Value>) -> Option<String> {
    match arg {
        Some(Value::Obj(obj)) => match &obj.borrow().data {
            ObjectData::Str(s) => Some(s.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// Mirrors `ns_syntax::lexer`'s string-literal escaping (`lex_string`): a
/// string literal is delimited by `'`, and an embedded `'` is written as two
/// consecutive `'` characters (no other escapes exist for Newspeak strings).
fn quote_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push('\'');
            out.push('\'');
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

pub fn dispatch_string(
    interp: &mut Interpreter,
    receiver: Value,
    selector: Symbol,
    args: &[Value],
) -> Option<Result<Value, NewspeakError>> {
    let Value::Obj(obj) = &receiver else {
        return None;
    };
    let ObjectData::Str(s) = &obj.borrow().data else {
        return None;
    };
    match selector.as_str() {
        "size" => Some(Ok(Value::SmallInt(s.chars().count() as i64))),
        "," => Some(match arg_str(args.first()) {
            Some(other) => Ok(Value::Obj(interp.new_string(format!("{s}{other}")))),
            None => Err(NewspeakError {
                selector,
                receiver: receiver.clone(),
                message: format!(
                    "String concatenation (,) requires a String argument, got {:?}",
                    args.first()
                ),
            }),
        }),
        "=" => Some(Ok(Value::Bool(
            arg_str(args.first()).as_deref() == Some(s.as_str()),
        ))),
        "asSymbol" => Some(Ok(interp.new_symbol(Symbol::intern(s)))),
        "printString" => Some(Ok(Value::Obj(interp.new_string(quote_string(s))))),
        _ => None,
    }
}

pub fn dispatch_symbol(
    interp: &mut Interpreter,
    receiver: Value,
    selector: Symbol,
    args: &[Value],
) -> Option<Result<Value, NewspeakError>> {
    let Value::Obj(obj) = &receiver else {
        return None;
    };
    let ObjectData::Sym(sym) = &obj.borrow().data else {
        return None;
    };
    match selector.as_str() {
        "asString" => Some(Ok(Value::Obj(interp.new_string(sym.as_str().to_string())))),
        "=" => Some(Ok(Value::Bool(match args.first() {
            Some(Value::Obj(arg_obj)) => {
                matches!(&arg_obj.borrow().data, ObjectData::Sym(other) if other == sym)
            }
            _ => false,
        }))),
        "printString" => Some(Ok(Value::Obj(
            interp.new_string(format!("#{}", sym.as_str())),
        ))),
        _ => None,
    }
}

pub fn dispatch_character(
    interp: &mut Interpreter,
    receiver: Value,
    selector: Symbol,
    args: &[Value],
) -> Option<Result<Value, NewspeakError>> {
    let Value::Obj(obj) = &receiver else {
        return None;
    };
    let ObjectData::Str(s) = &obj.borrow().data else {
        return None;
    };
    // Invariant from `Interpreter::new_character`: this payload is always
    // exactly one `char`. Fall back to `'\0'` rather than panicking if that
    // invariant is ever violated -- never panic on a Newspeak-input path.
    let c = s.chars().next().unwrap_or('\0');
    match selector.as_str() {
        "asInteger" => Some(Ok(Value::SmallInt(c as i64))),
        "asString" => Some(Ok(Value::Obj(interp.new_string(c.to_string())))),
        "=" => Some(Ok(Value::Bool(
            arg_str(args.first()).as_deref() == Some(s.as_str()),
        ))),
        _ => None,
    }
}
