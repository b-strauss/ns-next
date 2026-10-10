//! The two leaves of the futures. `FutureImplementation`'s initializer
//! notes a new future through `futureMade:ifFail:`, and its
//! `doesNotUnderstand:` forces it through `futureValue:ifFail:` before
//! forwarding the message. Forcing an unresolved future is a step that
//! activates its block, or sends `value` to a computation that is none,
//! never a call back into Newspeak (`interpreter::future`).

use crate::builtins::primitive::FuturePrimitive;
use crate::interpreter::future::{force, AfterForcing};
use crate::interpreter::{Signal, Step};
use crate::types::*;
use crate::Mutator;

/// `Value` declines for anything but a future, `Made` likewise; `Made`
/// answers nil once it has set `Mutator::futures_made`, without which the
/// runtime would not see through the future the factory just made.
pub(crate) fn execute<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: FuturePrimitive,
    receiver: Value<'gc>,
) -> Option<Result<Step<'gc>, Signal<'gc>>> {
    let future = mutator.as_future(receiver)?;
    match primitive {
        FuturePrimitive::Value => Some(force(mutator, future, AfterForcing::Deliver)),
        FuturePrimitive::Made => {
            mutator.futures_made = true;
            Some(Ok(Step::Value(Value::nil())))
        }
    }
}
