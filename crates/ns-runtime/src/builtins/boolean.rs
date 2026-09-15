//! `Boolean`/`True`/`False` native methods. The control-flow ones return the
//! block to activate as a step instead of calling it, so none of them waits
//! on a Rust frame for Newspeak code (`docs/design/execution.md`, "The
//! trampoline").

use crate::builtins::primitive::BooleanPrimitive;
use crate::interpreter::{Signal, Step, StepArguments};
use crate::types::*;
use crate::Mutator;

pub(crate) fn execute<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: BooleanPrimitive,
    receiver: Value<'gc>,
    arguments: &[Value<'gc>],
) -> Option<Result<Step<'gc>, Signal<'gc>>> {
    let Value::Boolean(boolean) = receiver else {
        return None;
    };
    Some(Ok(match primitive {
        BooleanPrimitive::IfTrueIfFalse => block_step(
            mutator,
            if boolean { arguments[0] } else { arguments[1] },
            Continuation::Deliver,
        ),
        BooleanPrimitive::IfFalseIfTrue => block_step(
            mutator,
            if boolean { arguments[1] } else { arguments[0] },
            Continuation::Deliver,
        ),
        BooleanPrimitive::IfTrue => {
            if boolean {
                block_step(mutator, arguments[0], Continuation::Deliver)
            } else {
                Step::Value(Value::Nil)
            }
        }
        BooleanPrimitive::IfFalse => {
            if boolean {
                Step::Value(Value::Nil)
            } else {
                block_step(mutator, arguments[0], Continuation::Deliver)
            }
        }
        BooleanPrimitive::Not => Step::Value(Value::Boolean(!boolean)),
        // The short circuit holds: `false and: [...]` and `true or: [...]`
        // never activate the block. Otherwise the block's value is coerced
        // when it returns (`Continuation::Truthy`), so a `^` out of it or an
        // error in it passes through untouched.
        BooleanPrimitive::And => {
            if boolean {
                block_step(mutator, arguments[0], Continuation::Truthy)
            } else {
                Step::Value(Value::Boolean(false))
            }
        }
        BooleanPrimitive::Or => {
            if boolean {
                Step::Value(Value::Boolean(true))
            } else {
                block_step(mutator, arguments[0], Continuation::Truthy)
            }
        }
        BooleanPrimitive::PrintString => Step::Value(Value::Object(
            mutator.new_string(if boolean { "true" } else { "false" }.to_string()),
        )),
    }))
}

/// `argument` activated as a block with no arguments, its value handed to
/// `then` -- or, when `argument` is not a block, `argument` itself taken as the
/// value, the reference library's `ifTrue: aValue` convention.
fn block_step<'gc>(
    mutator: &Mutator<'gc>,
    argument: Value<'gc>,
    then: Continuation<'gc>,
) -> Step<'gc> {
    if let Value::Object(object) = argument {
        if mutator.heap().is_closure(object) {
            return Step::Block {
                closure: object,
                arguments: StepArguments::ControlFlow,
                then,
            };
        }
    }
    match then {
        Continuation::Truthy => Step::Value(Value::Boolean(truthy(argument))),
        Continuation::Deliver => Step::Value(argument),
        Continuation::WhileCondition(_)
        | Continuation::WhileBody(_)
        | Continuation::DoElement(_)
        | Continuation::NestedClass(_)
        | Continuation::Instantiate(_) => {
            unreachable!("internal: a Boolean's control-flow native runs no loop")
        }
    }
}

/// `and:`/`or:`'s rule, which the compiler's inlined form reproduces in
/// bytecode (`compiler/lower.rs`, `emit_coerce_to_boolean`): only exactly `true`
/// is true, and a non-Boolean is `false`.
pub(crate) fn truthy<'gc>(value: Value<'gc>) -> bool {
    matches!(value, Value::Boolean(true))
}
