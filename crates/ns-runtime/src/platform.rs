//! The transcript's output, the one part of the platform that reaches Rust.
//! The platform is an instance of the shipped `PlatformFactory.ns` module's
//! nested `Platform` (`docs/design/core-library.md`, "The platform"): its
//! members are lazy slots holding values of the module's nested classes,
//! and `Collections>>List`
//! answers its class without a leaf. What is left here is `show:` and
//! `newLine`, the forwarders of `Transcript`'s two methods.
//!
//! The leaf reads nothing off its receiver, only the string argument. The
//! capability is the VM mirror the forwarder sends to, which only shipped
//! source reaches (`docs/design/core-library.md`, "The VM mirror is the
//! whole Rust boundary"), not the receiver's class: `Transcript` is no
//! builtin, so the VM could not recognize its class anyway, and a user
//! subclass of it inherits a working `show:`.

use std::io::Write;

use crate::builtins::primitive::PlatformPrimitive;
use crate::types::*;
use crate::Mutator;

/// Runs `primitive` for whatever receiver the forwarder passed, which it
/// answers. It never declines.
pub fn execute<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: PlatformPrimitive,
    receiver: Value<'gc>,
    arguments: &[Value<'gc>],
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
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
