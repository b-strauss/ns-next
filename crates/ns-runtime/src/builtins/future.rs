//! The three leaves of the futures. `FutureImplementation`'s initializer
//! notes a new future through `futureMade:ifFail:`, and its `resolve`
//! asks whether a value is a future through `objectIsFuture:ifFail:`
//! and forces a future its computation answered through
//! `futureValue:ifFail:`, whose step is that future's own `resolve`
//! (`interpreter::future`).

use crate::builtins::primitive::FuturePrimitive;
use crate::interpreter::{Signal, Step};
use crate::types::*;
use crate::Mutator;

/// `IsFuture` answers whether `receiver` is a future and never declines; `Value`
/// and `Made` decline for anything but a future. `Made` answers nil once
/// it has set `Mutator::futures_made`, without which the runtime would not
/// see through the future the factory just made.
pub(crate) fn execute<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: FuturePrimitive,
    receiver: Value<'gc>,
) -> Option<Result<Step<'gc>, Signal<'gc>>> {
    let future = mutator.as_future(receiver);
    match primitive {
        FuturePrimitive::IsFuture => Some(Ok(Step::Value(Value::boolean(future.is_some())))),
        FuturePrimitive::Value => Some(Ok(mutator.forcing(future?))),
        FuturePrimitive::Made => {
            future?;
            mutator.note_futures_made();
            Some(Ok(Step::Value(Value::nil())))
        }
    }
}
