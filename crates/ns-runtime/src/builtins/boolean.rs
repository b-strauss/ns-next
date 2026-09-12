//! `Boolean`/`True`/`False` native methods. The control-flow ones return the
//! block to activate as a step instead of calling it, so none of them waits
//! on a Rust frame for Newspeak code (`docs/design/execution.md`, "The
//! trampoline (1b)").

use crate::interpreter::{Signal, Step, StepArgs};
use crate::types::*;
use crate::Interpreter;
use crate::Symbol;

pub(crate) fn dispatch(
    interp: &mut Interpreter,
    receiver: Value,
    selector: Symbol,
    args: &[Value],
) -> Option<Result<Step, Signal>> {
    let Value::Bool(b) = receiver else {
        return None;
    };
    Some(Ok(match selector.as_str() {
        "ifTrue:ifFalse:" => block_step(
            interp,
            if b { args[0] } else { args[1] },
            Continuation::Deliver,
        ),
        "ifFalse:ifTrue:" => block_step(
            interp,
            if b { args[1] } else { args[0] },
            Continuation::Deliver,
        ),
        "ifTrue:" => {
            if b {
                block_step(interp, args[0], Continuation::Deliver)
            } else {
                Step::Value(Value::Nil)
            }
        }
        "ifFalse:" => {
            if b {
                Step::Value(Value::Nil)
            } else {
                block_step(interp, args[0], Continuation::Deliver)
            }
        }
        "not" => Step::Value(Value::Bool(!b)),
        // The short circuit holds: `false and: [...]` and `true or: [...]`
        // never activate the block. Otherwise the block's value is coerced
        // when it returns (`Continuation::Truthy`), so a `^` out of it or an
        // error in it passes through untouched.
        "&" | "and:" => {
            if b {
                block_step(interp, args[0], Continuation::Truthy)
            } else {
                Step::Value(Value::Bool(false))
            }
        }
        "|" | "or:" => {
            if b {
                Step::Value(Value::Bool(true))
            } else {
                block_step(interp, args[0], Continuation::Truthy)
            }
        }
        "printString" => Step::Value(Value::Obj(
            interp.new_string(if b { "true" } else { "false" }.to_string()),
        )),
        _ => return None,
    }))
}

/// `arg` activated as a block with no arguments, its value handed to `then`
/// -- or, when `arg` is not a block, `arg` itself taken as the value, the
/// reference library's `ifTrue: aValue` convention.
fn block_step(interp: &Interpreter, arg: Value, then: Continuation) -> Step {
    if let Value::Obj(obj) = arg {
        if let Some(closure) = interp.heap_ref().closure_data(obj) {
            return Step::Block {
                closure,
                args: StepArgs::ControlFlow,
                then,
            };
        }
    }
    match then {
        Continuation::Truthy => Step::Value(Value::Bool(truthy(arg))),
        _ => Step::Value(arg),
    }
}

/// `and:`/`or:`'s rule, which the compiler's inlined form reproduces in
/// bytecode (`compiler/lower.rs`, `emit_coerce_to_bool`): only exactly `true`
/// is true, and a non-Boolean is `false`.
pub(crate) fn truthy(value: Value) -> bool {
    matches!(value, Value::Bool(true))
}
