//! `Closure` native methods -- `value`..`value:value:value:`,
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
    let closure = receiver.as_object()?;
    let parameters = mutator.heap().closure_parameter_count(closure)?;
    // A loop keeps its body and takes a non-block for a value of its own,
    // which a future is not: it stands for its value, which may well be a
    // block. Declining hands it to its caller's failure path, which sees through
    // it before the loop keeps anything. Asked only of a body that is no
    // block, as `do:` asks it, so a block body pays nothing for it.
    if let BlockPrimitive::WhileTrue | BlockPrimitive::WhileFalse = primitive {
        if let Some(body) = arguments.first().and_then(|value| value.as_object()) {
            if !mutator.heap().is_closure(body) && mutator.as_future(Value::object(body)).is_some()
            {
                return None;
            }
        }
    }
    // Whether the body is a block, decided once for the whole loop.
    let body = || {
        arguments[0]
            .as_object()
            .filter(|body| mutator.heap().is_closure(*body))
    };
    Some(Ok(match primitive {
        BlockPrimitive::Value => Step::Block {
            closure,
            arguments: StepArguments::Send,
            then: Continuation::Deliver,
        },
        BlockPrimitive::ArgumentCount => Step::Value(Value::small_integer(parameters as i64)),
        BlockPrimitive::WhileTrue => condition_step(Box::new(WhileLoop {
            condition: closure,
            body: body(),
            expect: true,
        })),
        BlockPrimitive::WhileFalse => condition_step(Box::new(WhileLoop {
            condition: closure,
            body: body(),
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
    match value.as_boolean() {
        Some(boolean) if boolean == state.expect => Ok(body_step(state)),
        Some(_) => Ok(Step::Value(Value::nil())),
        None => mutator.decide_send(value, Symbol::MUST_BE_BOOLEAN, &[]),
    }
}

/// The body returned: run the condition again.
pub(crate) fn after_body<'gc>(state: Box<WhileLoop<'gc>>) -> Step<'gc> {
    condition_step(state)
}

/// The body, activated -- or, when it is not a block, taken as its own value,
/// so the condition runs again straight away.
fn body_step<'gc>(state: Box<WhileLoop<'gc>>) -> Step<'gc> {
    match state.body {
        Some(body) => Step::Block {
            closure: body,
            arguments: StepArguments::ControlFlow,
            then: Continuation::WhileBody(state),
        },
        None => condition_step(state),
    }
}
