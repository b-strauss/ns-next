//! `Closure` native methods -- `value`..`value:value:value:` and
//! `numArgs`. `value` returns the block to activate as a step.

use crate::Mutator;
use crate::builtins::primitive::BlockPrimitive;
use crate::interpreter::{Signal, Step, StepArguments};
use crate::types::*;

pub(crate) fn execute<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: BlockPrimitive,
    receiver: Value<'gc>,
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
    }))
}
