//! `Boolean`/`True`/`False` native methods.

use crate::types::*;
use crate::{Interpreter, Symbol};

pub fn dispatch(
    interp: &mut Interpreter,
    receiver: Value,
    selector: Symbol,
    args: &[Value],
) -> Option<Result<Value, NewspeakError>> {
    let Value::Bool(b) = receiver else {
        return None;
    };
    match selector.as_str() {
        "ifTrue:ifFalse:" => Some(super::call_block(
            interp,
            if b { &args[0] } else { &args[1] },
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
        "&" | "and:" => Some(Ok(Value::Bool(b && truthy_arg(interp, &args[0])))),
        "|" | "or:" => Some(Ok(Value::Bool(b || truthy_arg(interp, &args[0])))),
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
fn truthy_arg(interp: &mut Interpreter, value: &Value) -> bool {
    matches!(
        super::call_block(interp, value, vec![]),
        Ok(Value::Bool(true))
    )
}
