//! The one leaf of `Kernel.ns`'s `Future`: its `doesNotUnderstand:` forces
//! it through `futureValue:ifFail:` before forwarding the message. Forcing
//! an unresolved future is a step that activates its block, never a call
//! back into Newspeak (`interpreter::future`).

use crate::builtins::primitive::FuturePrimitive;
use crate::interpreter::future::{force, AfterForcing};
use crate::interpreter::{Signal, Step};
use crate::types::*;
use crate::Mutator;

/// Declines for anything but a future.
pub(crate) fn execute<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: FuturePrimitive,
    receiver: Value<'gc>,
) -> Option<Result<Step<'gc>, Signal<'gc>>> {
    match primitive {
        FuturePrimitive::Value => {
            let future = mutator.as_future(receiver)?;
            Some(force(mutator, future, AfterForcing::Deliver))
        }
    }
}
