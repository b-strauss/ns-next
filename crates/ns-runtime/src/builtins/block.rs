//! `Block` (closure) native methods -- `value`..`value:value:value:`,
//! `numArgs`, and `whileTrue:`/`whileFalse:`. Each returns the block to
//! activate as a step; the loops continue through `Continuation`s.

use crate::interpreter::{Signal, Step, StepArgs};
use crate::types::*;
use crate::{Interpreter, Symbol};

pub(crate) fn dispatch(
    interp: &mut Interpreter,
    receiver: Value,
    selector: Symbol,
    args: &[Value],
) -> Option<Result<Step, Signal>> {
    let Value::Obj(closure) = receiver else {
        return None;
    };
    let params = interp.heap_ref().closure_param_count(closure)?;
    Some(Ok(match selector.as_str() {
        "value" | "value:" | "value:value:" | "value:value:value:" => Step::Block {
            closure,
            args: StepArgs::Send,
            then: Continuation::Deliver,
        },
        "numArgs" => Step::Value(Value::SmallInt(params as i64)),
        "whileTrue:" => condition_step(Box::new(WhileLoop {
            condition: closure,
            body: args[0],
            expect: true,
        })),
        "whileFalse:" => condition_step(Box::new(WhileLoop {
            condition: closure,
            body: args[0],
            expect: false,
        })),
        _ => return None,
    }))
}

// `whileTrue:` (`expect == true`) / `whileFalse:` (`expect == false`):
// evaluate the condition; while its value is `Bool(expect)`, evaluate the
// body and repeat; a `Bool(!expect)` ends the loop with `nil`; a non-Boolean
// condition value is sent `mustBeBoolean`, and that send's result is the
// loop's value. The inlined loop reproduces exactly this on its guard-failure
// path (`docs/design/compiler.md`, "Control-flow inlining"). Condition and
// body replace each other on the activation stack: each one's continuation
// says what runs next.

fn condition_step(state: Box<WhileLoop>) -> Step {
    Step::Block {
        closure: state.condition,
        args: StepArgs::ControlFlow,
        then: Continuation::WhileCondition(state),
    }
}

/// The condition returned `value`.
pub(crate) fn after_condition(
    interp: &mut Interpreter,
    state: Box<WhileLoop>,
    value: Value,
) -> Result<Step, Signal> {
    match value {
        Value::Bool(b) if b == state.expect => Ok(body_step(interp, state)),
        Value::Bool(_) => Ok(Step::Value(Value::Nil)),
        other => interp.decide_send(other, Symbol::MUST_BE_BOOLEAN, &[]),
    }
}

/// The body returned: run the condition again.
pub(crate) fn after_body(state: Box<WhileLoop>) -> Step {
    condition_step(state)
}

/// The body, activated -- or, when it is not a block, taken as its own value,
/// so the condition runs again straight away.
fn body_step(interp: &Interpreter, state: Box<WhileLoop>) -> Step {
    if let Value::Obj(body) = state.body {
        if interp.heap_ref().is_closure(body) {
            return Step::Block {
                closure: body,
                args: StepArgs::ControlFlow,
                then: Continuation::WhileBody(state),
            };
        }
    }
    condition_step(state)
}
