//! The leaves of the futures, all `FutureImplementation`'s.
//! `FutureImplementation`'s initializer notes a new future through
//! `futureMade:ifFail:`. Its `resolve` asks whether a value is a future
//! through `objectIsFuture:ifFail:`, and resolves a chain of futures each
//! answering the next in its one activation: it reads each future's state
//! and computation (`futureState:ifFail:`, `futureComputation:ifFail:`),
//! takes over an unresolved one's computation (`futureTakeOver:ifFail:`),
//! and records the chain's value in every future it took over
//! (`future:resolveTo:ifFail:`), or sets them unresolved again when an
//! unwind abandons it (`futureAbandon:ifFail:`). Only `resolve` sends them,
//! and only to a future it holds: one future cannot send another its
//! private methods, and reading and writing the slots of the futures it
//! takes over is what keeps the depth of a chain's forcing constant
//! (`docs/design/object-model.md`, "Simultaneous slots").
//!
//! Every leaf but `IsFuture` declines for anything but a future, having
//! written nothing; for a future, none declines but `TakeOver` for a
//! future that is not unresolved, which `resolve` never hands it. Each
//! reads or writes only that future's two fields, through the write
//! barrier, allocates nothing and pushes nothing, so machine code runs
//! them through the leaf shim.

use crate::builtins::primitive::FuturePrimitive;
use crate::interpreter::future::{
    FutureState, COMPUTATION_FIELD, RESOLVED, RESOLVING, STATE_FIELD, UNRESOLVED,
};
use crate::interpreter::{Arguments, Signal, Step};
use crate::types::*;
use crate::Mutator;

/// What `futureState:ifFail:` answers for a future whose fields no
/// forcing wrote (`FutureState::Malformed`): beside the three states, so
/// that `resolve` compares small integers the leaf normalized and never a
/// value crafted code stored.
const MALFORMED: i64 = 3;

/// `Made` answers nil once it has set `Mutator::futures_made`, without
/// which the runtime would not see through the future the factory just
/// made.
pub(crate) fn execute<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: FuturePrimitive,
    receiver: Value<'gc>,
    arguments: Arguments<'_, 'gc>,
) -> Option<Result<Step<'gc>, Signal<'gc>>> {
    let future = mutator.as_future(receiver);
    let answer = match primitive {
        FuturePrimitive::IsFuture => Value::boolean(future.is_some()),
        FuturePrimitive::Made => {
            future?;
            mutator.note_futures_made();
            Value::nil()
        }
        FuturePrimitive::State => Value::small_integer(match mutator.future_state(future?) {
            FutureState::Unresolved(_) => UNRESOLVED,
            FutureState::Resolving => RESOLVING,
            FutureState::Resolved(_) => RESOLVED,
            FutureState::Malformed => MALFORMED,
        }),
        FuturePrimitive::Computation => mutator
            .heap
            .instance_field(future?, COMPUTATION_FIELD)
            .unwrap_or(Value::nil()),
        FuturePrimitive::TakeOver => {
            let future = future?;
            let FutureState::Unresolved(computation) = mutator.future_state(future) else {
                return None;
            };
            set_state(mutator, future, RESOLVING);
            computation
        }
        FuturePrimitive::ResolveTo => {
            let future = future?;
            let value = arguments.get(&mutator.heap, 0);
            mutator
                .heap
                .instance_set_field(future, COMPUTATION_FIELD, value);
            set_state(mutator, future, RESOLVED);
            Value::object(future)
        }
        FuturePrimitive::Abandon => {
            let future = future?;
            // Only a future still being resolved: one a forcing below this
            // one resolved meanwhile keeps its value.
            if let FutureState::Resolving = mutator.future_state(future) {
                set_state(mutator, future, UNRESOLVED);
            }
            Value::object(future)
        }
    };
    Some(Ok(Step::Value(answer)))
}

/// Writes `state` into `future`'s state field.
fn set_state<'gc>(mutator: &mut Mutator<'gc>, future: ObjectReference<'gc>, state: i64) {
    mutator
        .heap
        .instance_set_field(future, STATE_FIELD, Value::small_integer(state));
}
