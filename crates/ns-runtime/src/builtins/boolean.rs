//! `Boolean`/`True`/`False` native methods.

use crate::interpreter::{EvalResult, Signal};
use crate::types::*;
use crate::{Interpreter, Symbol};

pub fn dispatch(
    interp: &mut Interpreter,
    receiver: Value,
    selector: Symbol,
    args: &[Value],
) -> Option<EvalResult> {
    let Value::Bool(b) = receiver else {
        return None;
    };
    match selector.as_str() {
        "ifTrue:ifFalse:" => Some(super::call_block(
            interp,
            if b { &args[0] } else { &args[1] },
            vec![],
        )),
        "ifFalse:ifTrue:" => Some(super::call_block(
            interp,
            if b { &args[1] } else { &args[0] },
            vec![],
        )),
        "ifTrue:" => Some(if b {
            super::call_block(interp, &args[0], vec![])
        } else {
            Ok(Value::Nil)
        }),
        "ifFalse:" => Some(if !b {
            super::call_block(interp, &args[0], vec![])
        } else {
            Ok(Value::Nil)
        }),
        "not" => Some(Ok(Value::Bool(!b))),
        // Explicit `if`/`else` branches rather than `b && truthy_arg(...)`:
        // a boolean operator would collapse *any* non-`Ok(Value::Bool(true))`
        // outcome of the block argument -- including a live `Signal::Return`
        // (`x and: [^expr]`) and a genuine `Signal::Error` -- into plain
        // `false`, discarding both. The block's own `Result<bool, Signal>`
        // flows through `.map(Value::Bool)` into this arm's `EvalResult`
        // instead, and the short circuit still holds: `false and: [...]` /
        // `true or: [...]` never invoke the block at all.
        "&" | "and:" => Some(if b {
            truthy_arg(interp, &args[0]).map(Value::Bool)
        } else {
            Ok(Value::Bool(false))
        }),
        "|" | "or:" => Some(if b {
            Ok(Value::Bool(true))
        } else {
            truthy_arg(interp, &args[0]).map(Value::Bool)
        }),
        "printString" => Some(Ok(Value::Obj(
            interp.new_string(if b { "true" } else { "false" }.to_string()),
        ))),
        _ => None,
    }
}

/// `and:`/`or:`/`ifTrue:` etc. take a zero-arg block; invoking it goes
/// through the shared `call_block` primitive rather than a full
/// `send`, since these are the language's fundamental control-flow
/// primitives and must not depend on `Block`'s own native methods being
/// registered yet in dispatch order.
///
/// Returns `Result<bool, Signal>` (not a bare `bool`) so a `Signal::Return`
/// or `Signal::Error` escaping the block propagates via `?` to `dispatch`'s
/// `"&"`/`"and:"`/`"|"`/`"or:"` arms above, instead of being silently
/// collapsed into `false`. A block result that is `Ok` but not exactly
/// `Value::Bool(true)` (a non-Boolean value, or `Value::Bool(false)`) maps
/// to plain `Ok(false)`.
fn truthy_arg(interp: &mut Interpreter, value: &Value) -> Result<bool, Signal> {
    match super::call_block(interp, value, vec![])? {
        Value::Bool(true) => Ok(true),
        _ => Ok(false),
    }
}
