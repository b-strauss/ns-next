//! `Block` (closure) native methods -- `value`, `value:`, `value:value:`,
//! `value:value:value:`. Milestone 1 supports 0-3 arguments explicitly
//! (matches the reference library's common case; extend the match if a
//! fixture needs more).

use crate::types::*;
use crate::{Interpreter, Symbol};

pub fn dispatch(
    interp: &mut Interpreter,
    receiver: Value,
    selector: Symbol,
    args: &[Value],
) -> Option<Result<Value, NewspeakError>> {
    let Value::Obj(obj) = &receiver else {
        return None;
    };
    let closure = {
        let borrowed = obj.borrow();
        let ObjectData::Closure(c) = &borrowed.data else {
            return None;
        };
        c.clone()
    };
    match selector.as_str() {
        "value" | "value:" | "value:value:" | "value:value:value:" => {
            Some(super::call_block(interp, &receiver, args.to_vec()))
        }
        "numArgs" => Some(Ok(Value::SmallInt(closure.params.len() as i64))),
        _ => None,
    }
}
