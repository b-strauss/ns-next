//! The transcript's output, the one part of the platform that reaches Rust.
//! The platform is an instance of the shipped `PlatformFactory.ns` module's
//! nested `Platform` (`docs/design/core-library.md`, "The platform"): its
//! members are lazy slots holding values of the module's nested classes,
//! and `Collections>>List` answers its class without a leaf. What is left
//! here is `show:` and `newLine`, the forwarders of `Transcript`'s two
//! methods.
//!
//! The leaf reads nothing off its receiver, only the string argument. The
//! capability is the VM mirror the forwarder sends to, which only shipped
//! source reaches (`docs/design/core-library.md`, "The VM mirror is the
//! whole Rust boundary"), not the receiver's class: `Transcript` is no
//! builtin, so the VM could not recognize its class anyway, and a user
//! subclass of it inherits a working `show:`.
//!
//! `show:` writes a string and nothing else: any other argument is an
//! ordinary Newspeak error rather than nothing written, and `print:`, a
//! Newspeak method beside it, is the message that writes any object's
//! `printString` (`docs/design/core-library.md`, "The platform").

use std::io::Write;

use crate::builtins::primitive::PlatformPrimitive;
use crate::types::*;
use crate::{Mutator, Symbol};

/// Runs `primitive` for whatever receiver the forwarder passed, which it
/// answers. It never declines: an argument `show:` cannot write is an
/// error of its own, so the forwarder's failure block never runs.
pub fn execute<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: PlatformPrimitive,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: &[Value<'gc>],
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    Some(stream(mutator, primitive, receiver, selector, arguments))
}

fn stream<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: PlatformPrimitive,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: &[Value<'gc>],
) -> Result<Value<'gc>, NewspeakError<'gc>> {
    match primitive {
        PlatformPrimitive::Show => {
            let text = match arguments.first() {
                Some(Value::Object(string)) => mutator.heap().str_value(*string),
                _ => None,
            };
            let Some(text) = text else {
                return Err(NewspeakError {
                    selector,
                    receiver,
                    message: format!(
                        "{selector} requires a String, got {}",
                        mutator.heap().describe_argument(arguments.first())
                    ),
                });
            };
            print!("{text}");
            let _ = std::io::stdout().flush();
            Ok(receiver)
        }
        PlatformPrimitive::NewLine => {
            println!();
            Ok(receiver)
        }
    }
}
