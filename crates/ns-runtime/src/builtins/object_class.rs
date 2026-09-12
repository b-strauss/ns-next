//! `Object`'s native methods and the default `doesNotUnderstand:`
//! (`newspeak-spec.pdf` §5.5).

use crate::types::*;
use crate::{Heap, Interpreter, Symbol};

pub fn dispatch(
    interp: &mut Interpreter,
    receiver: Value,
    selector: Symbol,
    args: &[Value],
) -> Option<Result<Value, NewspeakError>> {
    match selector.as_str() {
        "class" => Some(Ok(Value::Obj(interp.class_of(&receiver)))),
        "==" => Some(Ok(Value::Bool(identical(&receiver, &args[0])))),
        "=" => Some(Ok(Value::Bool(identical(&receiver, &args[0])))), // Object's default: identity
        "hash" => Some(Ok(Value::SmallInt(
            identity_hash_of(&interp.heap, &receiver) as i64,
        ))),
        "printString" => Some(Ok(Value::Obj(interp.new_string("an Object".to_string())))),
        "doesNotUnderstand:" => {
            // Object's default doesNotUnderstand: (spec §5.5): must be
            // `protected`, always raises a MessageNotUnderstood-shaped
            // NewspeakError. There is no exception-class hierarchy yet;
            // this produces the NewspeakError value directly
            // rather than a catchable Newspeak object.
            let msg_selector = extract_dnu_selector(&interp.heap, &args[0]);
            Some(Err(NewspeakError {
                selector: msg_selector,
                receiver,
                message: format!(
                    "{} does not understand #{}",
                    describe(&receiver, interp),
                    msg_selector
                ),
            }))
        }
        _ => None,
    }
}

fn identical(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Nil, Value::Nil) => true,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::SmallInt(x), Value::SmallInt(y)) => x == y,
        (Value::Float(x), Value::Float(y)) => x.to_bits() == y.to_bits(),
        (Value::Obj(x), Value::Obj(y)) => x.same(*y),
        _ => false,
    }
}

fn identity_hash_of(heap: &Heap, value: &Value) -> u32 {
    match value {
        Value::Obj(obj) => heap.identity_hash(*obj),
        _ => 0, // primitives are values and get no distinct identity
                // hash (revisit if a fixture needs one)
    }
}

fn describe(_value: &Value, _interp: &Interpreter) -> String {
    // The default `doesNotUnderstand:` message names the receiver by kind
    // only. `Heap::describe` is the value-aware description.
    "an object".to_string()
}

fn extract_dnu_selector(heap: &Heap, message_mirror_shaped_array: &Value) -> Symbol {
    // Matches `Interpreter::does_not_understand`'s `{selector. args}` array
    // shape.
    if let Value::Obj(obj) = message_mirror_shaped_array {
        if let Some(Value::Obj(sym_obj)) = heap.array_at(*obj, 0) {
            if let Some(s) = heap.sym_value(sym_obj) {
                return s;
            }
        }
    }
    Symbol::UNKNOWN
}
