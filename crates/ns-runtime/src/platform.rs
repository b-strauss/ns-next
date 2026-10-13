//! The transcript's output and the monotonic clock, the two parts of the
//! platform that reach Rust. The platform is an instance of the shipped
//! `PlatformFactory.ns` module's nested `Platform`
//! (`docs/design/core-library.md`, "The platform"): its members are lazy
//! slots holding values of the module's nested classes and the facet of
//! the shipped `CollectionsFactory.ns` module, whose `List` is Newspeak and
//! needs no leaf; that module is reached through the mirror's
//! `moduleInstanceNamed:` (`builtins::module`). What is left here is
//! `show:` and `newLine`, the forwarders of `Transcript`'s two methods, and
//! the clock `Stopwatch` reads, nanoseconds since one origin per process,
//! so that two interpreters in one process agree.
//!
//! The leaves read nothing off their receiver; `show:` reads only its string
//! argument. The capability is the VM mirror the forwarder sends to, which only shipped
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

use crate::builtins::first_argument;
use crate::builtins::primitive::PlatformPrimitive;
use crate::types::*;
use crate::{ArbitraryInteger, Mutator, Symbol};

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

/// The instant every monotonic reading is measured from, taken on the
/// first reading. One per process, so two interpreters' stopwatches agree.
static CLOCK_ORIGIN: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

fn stream<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: PlatformPrimitive,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: &[Value<'gc>],
) -> Result<Value<'gc>, NewspeakError<'gc>> {
    match primitive {
        PlatformPrimitive::Show => {
            let text = arguments
                .first()
                .and_then(|value| value.as_object())
                .and_then(|string| mutator.heap().str_value(string));
            let Some(text) = text else {
                return Err(NewspeakError::argument_error(
                    selector,
                    receiver,
                    first_argument(arguments),
                    format!(
                        "{selector} requires a String, got {}",
                        mutator.heap().describe_argument(arguments.first().copied())
                    ),
                ));
            };
            print!("{text}");
            let _ = std::io::stdout().flush();
            Ok(receiver)
        }
        PlatformPrimitive::NewLine => {
            println!();
            Ok(receiver)
        }
        // Nanoseconds since the process's origin, exactly: a `SmallInteger`
        // holds 36 years of them, and an integer is unbounded beyond, so
        // the `u128` is converted whole, as its two `u64` digits, rather than
        // saturated.
        PlatformPrimitive::MonotonicNanoseconds => {
            let elapsed = CLOCK_ORIGIN.get_or_init(std::time::Instant::now).elapsed();
            let nanoseconds = elapsed.as_nanos();
            Ok(mutator.new_integer(ArbitraryInteger::from_parts(
                false,
                vec![nanoseconds as u64, (nanoseconds >> u64::BITS) as u64],
            )))
        }
    }
}
