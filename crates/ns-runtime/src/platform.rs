//! The minimal `platform` object: exposes the Core Library classes by name
//! plus a `Transcript`-style stdout stream. See the runtime spec's Core
//! Library section. Milestone 1 surface only: `transcript`, a handful of
//! class-name accessors, and the stream's `show:`/`newLine` -- nothing else
//! (collections, files, etc. all come from the classes themselves, not from
//! `platform`, per the spec's Core Library / platform split).

use std::io::Write;

use crate::types::*;
use crate::{Interpreter, Symbol};

/// The `platform` singleton, its `transcript` stream, and its
/// `collections` namespace all share `BuiltinClass::Platform` as their
/// `.class` tag (Milestone 1 had no second builtin-class slot to spare for
/// a stream-only class, and adding one now would renumber the image wire
/// format's builtin-class table for no behavioural gain) -- told apart
/// here by payload shape: the platform singleton is built with a
/// zero-field `ObjectData::Instance` (see `Interpreter::platform`), the
/// stream with a one-field one (see `Interpreter::new_stream_object`), the
/// namespace with a two-field one (see
/// `Interpreter::new_collections_namespace`). None ever holds real slot
/// data; the field count is purely a tag.
pub fn dispatch(
    interp: &mut Interpreter,
    receiver: Value,
    selector: Symbol,
    args: &[Value],
) -> Option<Result<Value, NewspeakError>> {
    let Value::Obj(obj) = &receiver else {
        return None;
    };
    let tag = match &obj.borrow().data {
        ObjectData::Instance(fields) => fields.len(),
        // Unreachable: every `BuiltinClass::Platform`-classed object is
        // built by one of the three constructors named above.
        _ => return None,
    };
    if tag == 2 {
        return dispatch_collections(interp, selector);
    }
    if tag != 0 {
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
        // The Core Library's `collections` namespace. Demand-driven: the
        // `run-corpus` whitelist's `Ducts.ns`, `NewspeakASTs.ns`, and
        // `V5BytecodeSimulation.ns` all open with
        // `private List = platform collections List.`, and none of them
        // reaches the platform any other way.
        "collections" => Some(Ok(Value::Obj(interp.new_collections_namespace()))),
        _ => None,
    }
}

/// The `collections` namespace's own protocol.
///
/// Exactly one member, because exactly one is asked for. `List` is the
/// Core Library's growable ordered sequence, which is the class this
/// runtime already has under its Smalltalk name `OrderedCollection` -- so
/// this is a name for an existing class, not a new collection
/// implementation. The corpus also asks this namespace for `Map`,
/// `IdentityMap`, `IdentitySet`, `WeakArray`, and `Collection`; those name
/// classes the runtime does not have, and inventing them (or aliasing them
/// onto `OrderedCollection`, which is not a dictionary and not a set)
/// would be a collections library this milestone deliberately does not
/// build. Modules needing them stay off the whitelist.
fn dispatch_collections(
    interp: &mut Interpreter,
    selector: Symbol,
) -> Option<Result<Value, NewspeakError>> {
    match selector.as_str() {
        "List" => Some(Ok(Value::Obj(
            interp.builtin_class(BuiltinClass::OrderedCollection),
        ))),
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
