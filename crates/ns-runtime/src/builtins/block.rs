//! `Block` (closure) native methods -- `value`..`value:value:value:`,
//! `numArgs`, and (Milestone 2) `whileTrue:`/`whileFalse:`.

use crate::interpreter::EvalResult;
use crate::types::*;
use crate::{Interpreter, Symbol};

pub fn dispatch(
    interp: &mut Interpreter,
    receiver: Value,
    selector: Symbol,
    args: &[Value],
) -> Option<EvalResult> {
    let Value::Obj(obj) = &receiver else {
        return None;
    };
    let closure = interp.heap.closure_data(obj)?;
    match selector.as_str() {
        "value" | "value:" | "value:value:" | "value:value:value:" => {
            Some(super::call_block(interp, &receiver, args.to_vec()))
        }
        "numArgs" => Some(Ok(Value::SmallInt(closure.params.len() as i64))),
        "whileTrue:" => Some(run_while(interp, &receiver, &args[0], true)),
        "whileFalse:" => Some(run_while(interp, &receiver, &args[0], false)),
        _ => None,
    }
}

/// `whileTrue:` (`expect == true`) / `whileFalse:` (`expect == false`):
/// evaluate `cond`; while its value is `Bool(expect)`, evaluate `body` and
/// repeat; a `Bool(!expect)` ends the loop with `nil`. A non-Boolean
/// condition value is sent `mustBeBoolean` (an ordinary virtual send --
/// `doesNotUnderstand:` by default) and that send's result is the loop
/// expression's value. The bytecode compiler's inlined loop reproduces
/// exactly this on its guard-failure path (M2 spec, "Control-flow
/// inlining"), so the two agree without re-running the condition.
fn run_while(interp: &mut Interpreter, cond: &Value, body: &Value, expect: bool) -> EvalResult {
    loop {
        match super::call_block(interp, cond, vec![])? {
            Value::Bool(b) if b == expect => {
                super::call_block(interp, body, vec![])?;
            }
            Value::Bool(_) => return Ok(Value::Nil),
            other => return interp.send(other, Symbol::MUST_BE_BOOLEAN, vec![]),
        }
    }
}
