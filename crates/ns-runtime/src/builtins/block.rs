//! `Closure` native methods -- `value`..`value:value:value:`,
//! `numArgs`, and `whileTrue:`/`whileFalse:`. Each returns the block to
//! activate -- or a loop's send of `value` to a body that is no block -- as
//! a step; the loops continue through `Continuation`s.

use crate::builtins::primitive::BlockPrimitive;
use crate::interpreter::{Signal, Step, StepArguments};
use crate::types::*;
use crate::Mutator;

pub(crate) fn execute<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: BlockPrimitive,
    receiver: Value<'gc>,
    arguments: &[Value<'gc>],
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
        BlockPrimitive::WhileTrue => condition_step(Box::new(WhileLoop {
            condition: closure,
            body: loop_body(mutator, arguments)?,
            expect: true,
        })),
        BlockPrimitive::WhileFalse => condition_step(Box::new(WhileLoop {
            condition: closure,
            body: loop_body(mutator, arguments)?,
            expect: false,
        })),
    }))
}

/// What a loop runs as its body, decided once for the whole loop: an
/// object never stops or starts being a closure. `None` for a missing body
/// (the `?` keeps the leaf total rather than indexed) and for a future, which
/// stands for its value, which may well be a block: declining hands it to
/// the caller's failure path, which sees through it. Whether it is a future
/// is asked only of a body that is no block, as `do:` asks it, so a block
/// body pays nothing for it.
fn loop_body<'gc>(mutator: &Mutator<'gc>, arguments: &[Value<'gc>]) -> Option<LoopAction<'gc>> {
    let body = LoopAction::of(mutator.heap(), *arguments.first()?);
    match body {
        LoopAction::Sent(sent) if mutator.as_future(sent).is_some() => None,
        _ => Some(body),
    }
}

// `whileTrue:` (`expect == true`) / `whileFalse:` (`expect == false`):
// evaluate the condition; while its value is `Boolean(expect)`, evaluate the
// body and repeat; a `Boolean(!expect)` ends the loop with `nil`; a non-Boolean
// condition value signals `NonBooleanReceiver`. The inlined loop signals the
// same error, built by the same function, on its guard-failure path
// (`docs/design/compiler.md`, "Control-flow inlining"). Condition and
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
        Some(boolean) if boolean == state.expect => Ok(body_step(mutator, state)),
        Some(_) => Ok(Step::Value(Value::nil())),
        None => Err(Signal::Error(
            crate::interpreter::unwind::non_boolean_condition(&mutator.heap, value),
        )),
    }
}

/// The body returned: run the condition again.
pub(crate) fn after_body<'gc>(state: Box<WhileLoop<'gc>>) -> Step<'gc> {
    condition_step(state)
}

/// The body: a closure activated as a control-flow step, or anything else
/// sent `value`, whose answer is dropped before the condition runs again.
fn body_step<'gc>(mutator: &mut Mutator<'gc>, state: Box<WhileLoop<'gc>>) -> Step<'gc> {
    match state.body {
        LoopAction::Closure(body) => Step::Block {
            closure: body,
            arguments: StepArguments::ControlFlow,
            then: Continuation::WhileBody(state),
        },
        LoopAction::Sent(body) => {
            let receiver = Value::object(state.condition);
            mutator.send_value_then(receiver, body, None, Continuation::WhileBody(state))
        }
    }
}
