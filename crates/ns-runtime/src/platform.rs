//! The transcript's output, the one part of the platform that reaches Rust.
//! `Platform`, `Transcript` and `Collections` are values in `Kernel.ns`
//! (`docs/design/core-library.md`, "The on-demand rule"): the platform holds
//! the other two in slots, and `Collections>>List` answers its class without
//! a leaf. What is left here is `show:` and `newLine`, the forwarders of
//! `Transcript`'s two kernel methods.

use std::io::Write;

use crate::builtins::primitive::PlatformPrimitive;
use crate::types::*;
use crate::Mutator;

/// Runs `primitive` for a `Transcript` receiver, and declines anything else,
/// which the kernel method's failure block then passes to `Object`.
pub fn execute<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: PlatformPrimitive,
    receiver: Value<'gc>,
    arguments: &[Value<'gc>],
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let Value::Object(object) = &receiver else {
        return None;
    };
    let class = mutator.heap().class_of_object(*object);
    if mutator.heap().class_builtin(class) != Some(BuiltinClass::Transcript) {
        return None;
    }
    Some(stream(mutator, primitive, receiver, arguments))
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
    }
}
