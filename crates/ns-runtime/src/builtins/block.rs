//! `Block` (closure) native methods -- `value`..`value:value:value:`,
//! `numArgs`, and `whileTrue:`/`whileFalse:`. Each returns the block to
//! activate as a step; the loops continue through `Continuation`s.

use crate::builtins::primitive::BlockPrimitive;
use crate::interpreter::{Signal, Step, StepArguments};
use crate::types::*;
use crate::{Mutator, Symbol};

pub(crate) fn execute<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: BlockPrimitive,
    receiver: Value<'gc>,
    arguments: &[Value<'gc>],
) -> Option<Result<Step<'gc>, Signal<'gc>>> {
    let Value::Object(closure) = receiver else {
        return None;
    };
    let parameters = mutator.heap().closure_parameter_count(closure)?;
    Some(Ok(match primitive {
        BlockPrimitive::Value => Step::Block {
            closure,
            arguments: StepArguments::Send,
            then: Continuation::Deliver,
        },
        BlockPrimitive::ArgumentCount => Step::Value(Value::SmallInteger(parameters as i64)),
        BlockPrimitive::WhileTrue => condition_step(Box::new(WhileLoop {
            condition: closure,
            body: arguments[0],
            expect: true,
        })),
        BlockPrimitive::WhileFalse => condition_step(Box::new(WhileLoop {
            condition: closure,
            body: arguments[0],
            expect: false,
        })),
    }))
}

// `whileTrue:` (`expect == true`) / `whileFalse:` (`expect == false`):
// evaluate the condition; while its value is `Boolean(expect)`, evaluate the
// body and repeat; a `Boolean(!expect)` ends the loop with `nil`; a non-Boolean
// condition value is sent `mustBeBoolean`, and that send's result is the
// loop's value. The inlined loop reproduces exactly this on its guard-failure
// path (`docs/design/compiler.md`, "Control-flow inlining"). Condition and
// body replace each other on the activation stack: each one's continuation
// says what runs next.

fn condition_step<'gc>(state: Box<WhileLoop<'gc>>) -> Step<'gc> {
    Step::Block {
        closure: state.condition,
        arguments: StepArguments::ControlFlow,
        then: Continuation::WhileCondition(state),
    }
}

/// The condition returned `value`.
pub(crate) fn after_condition<'gc>(
    mutator: &mut Mutator<'gc>,
    state: Box<WhileLoop<'gc>>,
    value: Value<'gc>,
) -> Result<Step<'gc>, Signal<'gc>> {
    match value {
        Value::Boolean(boolean) if boolean == state.expect => Ok(body_step(mutator, state)),
        Value::Boolean(_) => Ok(Step::Value(Value::Nil)),
        other => mutator.decide_send(other, Symbol::MUST_BE_BOOLEAN, &[]),
    }
}

/// The body returned: run the condition again.
pub(crate) fn after_body<'gc>(state: Box<WhileLoop<'gc>>) -> Step<'gc> {
    condition_step(state)
}

/// The body, activated -- or, when it is not a block, taken as its own value,
/// so the condition runs again straight away.
fn body_step<'gc>(mutator: &Mutator<'gc>, state: Box<WhileLoop<'gc>>) -> Step<'gc> {
    if let Value::Object(body) = state.body {
        if mutator.heap().is_closure(body) {
            return Step::Block {
                closure: body,
                arguments: StepArguments::ControlFlow,
                then: Continuation::WhileBody(state),
            };
        }
    }
    condition_step(state)
}
