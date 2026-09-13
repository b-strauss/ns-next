//! The minimal `platform` object: exposes the Core Library classes by name
//! plus a `Transcript`-style stdout stream. See `docs/design/core-library.md`.
//! The surface is widened only on demand ("The on-demand rule" there):
//! `transcript`, a handful of class-name accessors, the `collections`
//! namespace, and the stream's `show:`/`newLine` -- nothing else.

use std::io::Write;

use crate::types::*;
use crate::{Mutator, Symbol};

/// The `platform` singleton, its `transcript` stream, and its
/// `collections` namespace all share `BuiltinClass::Platform` as their
/// `.class` tag (a builtin class of their own would buy no behaviour), and
/// are told apart here by payload shape: the platform singleton is built
/// with a zero-field `ObjectData::Instance` (see `Interpreter::platform`), the
/// stream with a one-field one (see `Mutator::new_stream_object`), the
/// namespace with a two-field one (see
/// `Mutator::new_collections_namespace`). None ever holds real slot
/// data; the field count is purely a tag.
pub fn dispatch(
    m: &mut Mutator,
    receiver: Value,
    selector: Symbol,
    args: &[Value],
) -> Option<Result<Value, NewspeakError>> {
    let Value::Obj(obj) = &receiver else {
        return None;
    };
    // The `?` below is unreachable: every `BuiltinClass::Platform`-classed
    // object is built by one of the three constructors named above.
    let tag = m.heap().instance_field_count(*obj)?;
    if tag == 2 {
        return dispatch_collections(m, selector);
    }
    if tag != 0 {
        return dispatch_stream(m, receiver, selector, args);
    }
    match selector.as_str() {
        "transcript" => Some(Ok(Value::Obj(m.new_stream_object()))),
        // Class accessors: some real corpus modules reach the Core Library
        // classes directly off `platform` rather than a named sub-module --
        // only the Core Library names are supported.
        "Object" => Some(Ok(Value::Obj(m.builtin_class(BuiltinClass::Object)))),
        "Array" => Some(Ok(Value::Obj(m.builtin_class(BuiltinClass::Array)))),
        "OrderedCollection" => Some(Ok(Value::Obj(
            m.builtin_class(BuiltinClass::OrderedCollection),
        ))),
        "String" => Some(Ok(Value::Obj(m.builtin_class(BuiltinClass::Str)))),
        "Symbol" => Some(Ok(Value::Obj(m.builtin_class(BuiltinClass::Sym)))),
        // The Core Library's `collections` namespace. Demand-driven: the
        // `run-corpus` whitelist's `Ducts.ns`, `NewspeakASTs.ns`, and
        // `V5BytecodeSimulation.ns` all open with
        // `private List = platform collections List.`, and none of them
        // reaches the platform any other way.
        "collections" => Some(Ok(Value::Obj(m.new_collections_namespace()))),
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
/// would be a collections library of its own, which is deliberately not
/// built. Modules needing them stay off the whitelist.
fn dispatch_collections(m: &mut Mutator, selector: Symbol) -> Option<Result<Value, NewspeakError>> {
    match selector.as_str() {
        "List" => Some(Ok(Value::Obj(
            m.builtin_class(BuiltinClass::OrderedCollection),
        ))),
        _ => None,
    }
}

fn dispatch_stream(
    m: &mut Mutator,
    receiver: Value,
    selector: Symbol,
    args: &[Value],
) -> Option<Result<Value, NewspeakError>> {
    match selector.as_str() {
        "show:" => {
            if let Some(Value::Obj(s)) = args.first() {
                if let Some(text) = m.heap().str_value(*s) {
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
