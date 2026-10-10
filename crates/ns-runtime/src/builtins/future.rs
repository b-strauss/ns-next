//! The two leaves of the futures. `Future computing:` makes one through
//! `futureComputing:ifFail:`, and `FutureImplementation`'s
//! `doesNotUnderstand:` forces it through `futureValue:ifFail:` before
//! forwarding the message. Forcing an unresolved future is a step that
//! activates its block, never a call back into Newspeak
//! (`interpreter::future`).

use crate::builtins::primitive::FuturePrimitive;
use crate::interpreter::future::{force, AfterForcing};
use crate::interpreter::{Signal, Step};
use crate::types::*;
use crate::Mutator;

/// `Value` declines for anything but a future, `Computing` for anything but
/// a closure: its subject is the computation, which `Future computing:`
/// wraps in a block of its own when it is none.
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
        FuturePrimitive::Computing => {
            let closure = receiver
                .as_object()
                .filter(|object| mutator.heap.is_closure(*object))?;
            Some(Ok(Step::Value(Value::object(mutator.new_future(closure)))))
        }
    }
}
