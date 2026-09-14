//! `Block` (closure) native methods -- `value`..`value:value:value:`,
//! `numArgs`, and `whileTrue:`/`whileFalse:`. Each returns the block to
//! activate as a step; the loops continue through `Continuation`s.

use crate::interpreter::{Signal, Step, StepArguments};
use crate::types::*;
use crate::{Mutator, Symbol};

pub(crate) fn dispatch(
    mutator: &mut Mutator,
    receiver: Value,
    selector: Symbol,
    arguments: &[Value],
) -> Option<Result<Step, Signal>> {
    let Value::Object(closure) = receiver else {
        return None;
    };
    let parameters = mutator.heap().closure_parameter_count(closure)?;
    Some(Ok(match selector.as_str() {
        "value" | "value:" | "value:value:" | "value:value:value:" => Step::Block {
            closure,
            arguments: StepArguments::Send,
            then: Continuation::Deliver,
        },
        "numArgs" => Step::Value(Value::SmallInteger(parameters as i64)),
        "whileTrue:" => condition_step(Box::new(WhileLoop {
            condition: closure,
            body: arguments[0],
            expect: true,
        })),
        "whileFalse:" => condition_step(Box::new(WhileLoop {
            condition: closure,
            body: arguments[0],
            expect: false,
        })),
        _ => return None,
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

fn condition_step(state: Box<WhileLoop>) -> Step {
    Step::Block {
        closure: state.condition,
        arguments: StepArguments::ControlFlow,
        then: Continuation::WhileCondition(state),
    }
}

/// The condition returned `value`.
pub(crate) fn after_condition(
    mutator: &mut Mutator,
    state: Box<WhileLoop>,
    value: Value,
) -> Result<Step, Signal> {
    match value {
        Value::Boolean(boolean) if boolean == state.expect => Ok(body_step(mutator, state)),
        Value::Boolean(_) => Ok(Step::Value(Value::Nil)),
        other => mutator.decide_send(other, Symbol::MUST_BE_BOOLEAN, &[]),
    }
}

/// The body returned: run the condition again.
pub(crate) fn after_body(state: Box<WhileLoop>) -> Step {
    condition_step(state)
}

/// The body, activated -- or, when it is not a block, taken as its own value,
/// so the condition runs again straight away.
fn body_step(mutator: &Mutator, state: Box<WhileLoop>) -> Step {
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
