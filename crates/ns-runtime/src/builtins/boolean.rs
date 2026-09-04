//! `Boolean`/`True`/`False` native methods.

use crate::interpreter::{EvalResult, Signal};
use crate::types::*;
use crate::{Interpreter, Symbol};

pub fn dispatch(
    interp: &mut Interpreter,
    receiver: Value,
    selector: Symbol,
    args: &[Value],
    sender: Option<&FrameRef>,
) -> Option<EvalResult> {
    let Value::Bool(b) = receiver else {
        return None;
    };
    match selector.as_str() {
        "ifTrue:ifFalse:" => Some(super::call_block(
            interp,
            if b { &args[0] } else { &args[1] },
            vec![],
            sender,
        )),
        "ifTrue:" => Some(if b {
            super::call_block(interp, &args[0], vec![], sender)
        } else {
            Ok(Value::Nil)
        }),
        "ifFalse:" => Some(if !b {
            super::call_block(interp, &args[0], vec![], sender)
        } else {
            Ok(Value::Nil)
        }),
        "not" => Some(Ok(Value::Bool(!b))),
        // Fix (controller review, second pass): `b && truthy_arg(...)` /
        // `b || truthy_arg(...)` used to silently collapse *any*
        // non-`Ok(Value::Bool(true))` outcome of the block argument --
        // including a live `Signal::Return` (`x and: [^expr]`) and a
        // genuine `Signal::Error` -- into plain `false`, discarding both.
        // Written as explicit `if`/`else` branches instead so the block's
        // own `Result` (now `Result<bool, Signal>`, propagated via `?` in
        // `truthy_arg`) flows through `.map(Value::Bool)` into this arm's
        // own `EvalResult`, while still preserving the exact same
        // short-circuit behavior as before (`false and: [...]`/
        // `true or: [...]` never invoke the block at all).
        "&" | "and:" => Some(if b {
            truthy_arg(interp, &args[0], sender).map(Value::Bool)
        } else {
            Ok(Value::Bool(false))
        }),
        "|" | "or:" => Some(if b {
            Ok(Value::Bool(true))
        } else {
            truthy_arg(interp, &args[0], sender).map(Value::Bool)
        }),
        "printString" => Some(Ok(Value::Obj(
            interp.new_string(if b { "true" } else { "false" }.to_string()),
        ))),
        _ => None,
    }
}

/// `and:`/`or:`/`ifTrue:` etc. take a zero-arg block; invoking it goes
/// through the shared `call_block` primitive (Task 10 refactor -- this used
/// to be a local `invoke_block` copy of that same logic) rather than a full
/// `send`, since these are the language's fundamental control-flow
/// primitives and must not depend on `Block`'s own native methods being
/// registered yet in dispatch order.
///
/// Returns `Result<bool, Signal>` (not a bare `bool`) so a `Signal::Return`
/// or `Signal::Error` escaping the block propagates via `?` to `dispatch`'s
/// `"&"`/`"and:"`/`"|"`/`"or:"` arms above, instead of being silently
/// collapsed into `false` the way the pre-fix version did. A block result
/// that is `Ok` but not exactly `Value::Bool(true)` (a non-Boolean value, or
/// `Value::Bool(false)`) still maps to plain `Ok(false)`, matching the
/// pre-fix behavior for that case exactly -- only the `Err` case changed.
fn truthy_arg(
    interp: &mut Interpreter,
    value: &Value,
    sender: Option<&FrameRef>,
) -> Result<bool, Signal> {
    match super::call_block(interp, value, vec![], sender)? {
        Value::Bool(true) => Ok(true),
        _ => Ok(false),
    }
}
