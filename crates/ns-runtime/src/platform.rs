//! The minimal `platform` object: exposes the Core Library classes by name
//! plus a `Transcript`-style stdout stream. See the runtime spec's Core
//! Library section. Milestone 1 surface only: `transcript`, a handful of
//! class-name accessors, and the stream's `show:`/`newLine` -- nothing else
//! (collections, files, etc. all come from the classes themselves, not from
//! `platform`, per the spec's Core Library / platform split).

use std::io::Write;

use crate::types::*;
use crate::{Interpreter, Symbol};

/// Both the `platform` singleton and its `transcript` stream share
/// `BuiltinClass::Platform` as their `.class` tag (Milestone 1 has no
/// second builtin-class slot to spare for a stream-only class) -- told
/// apart here by payload shape: the platform singleton is built with a
/// zero-field `ObjectData::Instance` (see `Interpreter::platform`), the
/// stream with a one-field one (see `Interpreter::new_stream_object`).
/// Neither ever holds real slot data; the field count is purely a tag.
pub fn dispatch(
    interp: &mut Interpreter,
    receiver: Value,
    selector: Symbol,
    args: &[Value],
) -> Option<Result<Value, NewspeakError>> {
    let Value::Obj(obj) = &receiver else {
        return None;
    };
    let is_platform =
        matches!(&obj.borrow().data, ObjectData::Instance(fields) if fields.is_empty());
    if !is_platform {
        return dispatch_stream(interp, receiver, selector, args);
    }
    match selector.as_str() {
        "transcript" => Some(Ok(Value::Obj(interp.new_stream_object()))),
        // Class accessors: some real corpus modules reach the Core Library
        // classes directly off `platform` rather than a named sub-module --
        // Milestone 1 supports the Core Library names only.
        "Object" => Some(Ok(Value::Obj(interp.builtin_class(BuiltinClass::Object)))),
        "Array" => Some(Ok(Value::Obj(interp.builtin_class(BuiltinClass::Array)))),
        "OrderedCollection" => Some(Ok(Value::Obj(
            interp.builtin_class(BuiltinClass::OrderedCollection),
        ))),
        "String" => Some(Ok(Value::Obj(interp.builtin_class(BuiltinClass::Str)))),
        "Symbol" => Some(Ok(Value::Obj(interp.builtin_class(BuiltinClass::Sym)))),
        _ => None,
    }
}

fn dispatch_stream(
    _interp: &mut Interpreter,
    receiver: Value,
    selector: Symbol,
    args: &[Value],
) -> Option<Result<Value, NewspeakError>> {
    match selector.as_str() {
        "show:" => {
            if let Some(Value::Obj(s)) = args.first() {
                if let ObjectData::Str(text) = &s.borrow().data {
                    print!("{text}");
                    let _ = std::io::stdout().flush();
                }
            }
            Some(Ok(receiver))
        }
        "newLine" => {
            println!();
            Some(Ok(receiver))
        }
        _ => None,
    }
}
