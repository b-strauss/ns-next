//! The minimal `platform` object: exposes the Core Library classes by name
//! plus a `Transcript`-style stdout stream. See `docs/design/core-library.md`.
//! The surface is widened only on demand ("The on-demand rule" there):
//! `transcript`, the `collections` namespace, and the stream's
//! `show:`/`newLine` -- nothing else. The predefined classes are not among
//! them: `platform Array` reaches `Object`'s accessor, which the platform
//! inherits like every object (§3.2).
//!
//! `Platform`'s methods are kernel primitive methods (`Kernel.ns`): each is
//! a forwarder to a row of `MIRROR` with a super-send fallback,
//! so a program reaches every leaf here through the mirror send, never a
//! direct `Primitive::Platform` lookup on `BuiltinClass::Platform`. §8.3
//! still separates the platform, which is the environment, from the VM
//! mirror, which is the virtual machine -- only how a leaf is reached
//! changed, not what `platform`, its `transcript` stream and its
//! `collections` namespace are.

use std::io::Write;

use crate::builtins::primitive::PlatformPrimitive;
use crate::types::*;
use crate::Mutator;

/// The `platform` singleton, its `transcript` stream, and its
/// `collections` namespace all share `BuiltinClass::Platform` as their
/// `.class` tag (a builtin class of their own would buy no behaviour), and
/// are told apart here by payload shape: the platform singleton is built
/// with a zero-field `ObjectData::Instance` (see `Interpreter::platform`), the
/// stream with a one-field one (see `Mutator::new_stream_object`), the
/// namespace with a two-field one (see
/// `Mutator::new_collections_namespace`). None ever holds real slot
/// data; the field count is purely a tag.
pub fn execute<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: PlatformPrimitive,
    receiver: Value<'gc>,
    arguments: &[Value<'gc>],
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let Value::Object(object) = &receiver else {
        return None;
    };
    // The `?` below is unreachable: every `BuiltinClass::Platform`-classed
    // object is built by one of the three constructors named above.
    let tag = mutator.heap().instance_field_count(*object)?;
    match primitive {
        PlatformPrimitive::List => (tag == 2).then(|| collections_list(mutator)),
        PlatformPrimitive::Show | PlatformPrimitive::NewLine => {
            (tag != 0 && tag != 2).then(|| stream(mutator, primitive, receiver, arguments))
        }
        _ if tag != 0 => None,
        PlatformPrimitive::Transcript => Some(Ok(Value::Object(mutator.new_stream_object()))),
        // The Core Library's `collections` namespace. Demand-driven: the
        // `run-corpus` whitelist's `Ducts.ns`, `NewspeakASTs.ns`, and
        // `V5BytecodeSimulation.ns` all open with
        // `private List = platform collections List.`, and none of them
        // reaches the platform any other way.
        PlatformPrimitive::Collections => {
            Some(Ok(Value::Object(mutator.new_collections_namespace())))
        }
    }
}

/// The `collections` namespace's `List` -- its only member, because exactly
/// one is asked for. `List` is the Core Library's growable ordered sequence,
/// which is the class this runtime already has under its Smalltalk name
/// `OrderedCollection` -- so this is a name for an existing class, not a new
/// collection implementation. The corpus also asks this namespace for `Map`,
/// `IdentityMap`, `IdentitySet`, `WeakArray`, and `Collection`; those name
/// classes the runtime does not have, and inventing them (or aliasing them
/// onto `OrderedCollection`, which is not a dictionary and not a set) would
/// be a collections library of its own, which is deliberately not built.
/// Modules needing them stay off the whitelist.
fn collections_list<'gc>(mutator: &mut Mutator<'gc>) -> Result<Value<'gc>, NewspeakError<'gc>> {
    Ok(Value::Object(
        mutator.builtin_class(BuiltinClass::OrderedCollection),
    ))
}

fn stream<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: PlatformPrimitive,
    receiver: Value<'gc>,
    arguments: &[Value<'gc>],
) -> Result<Value<'gc>, NewspeakError<'gc>> {
    match primitive {
        PlatformPrimitive::Show => {
            if let Some(Value::Object(string)) = arguments.first() {
                if let Some(text) = mutator.heap().str_value(*string) {
                    print!("{text}");
                    let _ = std::io::stdout().flush();
                }
            }
            Ok(receiver)
        }
        PlatformPrimitive::NewLine => {
            println!();
            Ok(receiver)
        }
        _ => unreachable!("internal: `stream` is only called for `Show`/`NewLine`"),
    }
}
