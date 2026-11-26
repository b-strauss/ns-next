//! `Closure` native methods -- `value`..`value:value:value:`,
//! `valueWithArguments:` and `numArgs`. `value` and `valueWithArguments:`
//! return the block to activate as a step, whose push checks the number
//! of arguments against the block's parameters.

use crate::Mutator;
use crate::builtins::primitive::BlockPrimitive;
use crate::interpreter::{Arguments, Signal, Step, StepArguments};
use crate::types::*;

/// `None` -- the kernel method's failure block -- for a receiver that is
/// no closure, and for `valueWithArguments:` given anything but an Array.
pub(crate) fn execute<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: BlockPrimitive,
    receiver: Value<'gc>,
    arguments: Arguments<'_, 'gc>,
) -> Option<Result<Step<'gc>, Signal<'gc>>> {
    let closure = receiver.as_object()?;
    let parameters = mutator.heap().closure_parameter_count(closure)?;
    Some(Ok(match primitive {
        BlockPrimitive::Value => Step::Block {
            closure,
            arguments: StepArguments::Send,
            then: Continuation::Deliver,
        },
        BlockPrimitive::ArgumentCount => Step::Value(Value::small_integer(parameters as i64)),
        BlockPrimitive::ValueWithArguments => {
            let array = arguments.first(&mutator.heap)?.as_object()?;
            // A copy of the elements as they are now: the block may
            // change the Array, and its parameters keep what it was given.
            let elements = mutator.heap.array_elements(array)?;
            Step::Block {
                closure,
                arguments: StepArguments::Owned(elements),
                then: Continuation::Deliver,
            }
        }
    }))
}
