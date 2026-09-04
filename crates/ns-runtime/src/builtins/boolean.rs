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
        "ifTrue:ifFalse:" => Some(invoke_block(interp, if b { &args[0] } else { &args[1] })),
        "ifTrue:" => Some(if b {
            invoke_block(interp, &args[0])
        } else {
            Ok(Value::Nil)
        }),
        "ifFalse:" => Some(if !b {
            invoke_block(interp, &args[0])
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

fn invoke_block(interp: &mut Interpreter, value: &Value) -> Result<Value, NewspeakError> {
    // `and:`/`or:`/`ifTrue:` etc. take a zero-arg block; invoking it is the
    // same primitive `Block value` uses (Task 10) -- call through the
    // shared closure-invocation entry point directly rather than a full
    // `send`, since these are the language's fundamental control-flow
    // primitives and must not depend on `Block`'s own native methods being
    // registered yet in dispatch order.
    match value {
        Value::Obj(obj) => {
            if let ObjectData::Closure(closure) = &obj.borrow().data {
                let closure = std::rc::Rc::new(closure.clone());
                let bindings = closure.bindings.clone();
                return crate::interpreter::eval::invoke_closure(
                    interp,
                    &closure,
                    vec![],
                    &bindings,
                )
                .map_err(|signal| match signal {
                    crate::interpreter::Signal::Error(e) => e,
                    crate::interpreter::Signal::Return(..) => NewspeakError {
                        selector: Symbol::intern("value"),
                        receiver: value.clone(),
                        message: "non-local return escaped a Boolean control-flow block"
                            .to_string(),
                    },
                });
            }
            Ok(value.clone())
        }
        other => Ok(other.clone()),
    }
}

fn truthy_arg(interp: &mut Interpreter, value: &Value) -> bool {
    matches!(invoke_block(interp, value), Ok(Value::Bool(true)))
}
