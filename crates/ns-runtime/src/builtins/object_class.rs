//! `Object`'s native methods and the default `doesNotUnderstand:`
//! (`newspeak-spec.pdf` §5.5).

use crate::types::*;
use crate::{Interpreter, Symbol};

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
        "hash" => Some(Ok(Value::SmallInt(identity_hash_of(&receiver) as i64))),
        "printString" => Some(Ok(Value::Obj(interp.new_string("an Object".to_string())))),
        "doesNotUnderstand:" => {
            // Object's default doesNotUnderstand: (spec §5.5): must be
            // `protected`, always raises a MessageNotUnderstood-shaped
            // NewspeakError. Milestone 1 has no exception-class hierarchy
            // (Non-Goal); this produces the NewspeakError value directly
            // rather than a catchable Newspeak object.
            let msg_selector = extract_dnu_selector(&args[0]);
            Some(Err(NewspeakError {
                selector: msg_selector,
                receiver: receiver.clone(),
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
        (Value::Obj(x), Value::Obj(y)) => std::rc::Rc::ptr_eq(x, y),
        _ => false,
    }
}

fn identity_hash_of(value: &Value) -> u32 {
    match value {
        Value::Obj(obj) => obj.borrow().identity_hash,
        _ => 0, // primitives are values; Milestone 1 does not give them a
                // distinct identity hash (revisit if a fixture needs one)
    }
}

fn describe(_value: &Value, _interp: &Interpreter) -> String {
    "an object".to_string() // Task 9's printString work supersedes this for real classes
}

fn extract_dnu_selector(message_mirror_shaped_array: &Value) -> Symbol {
    // Matches `Interpreter::does_not_understand`'s `{selector. args}` array
    // shape from Task 6.
    if let Value::Obj(obj) = message_mirror_shaped_array {
        if let ObjectData::Array(elements) = &obj.borrow().data {
            if let Some(Value::Obj(sym_obj)) = elements.first() {
                if let ObjectData::Sym(s) = &sym_obj.borrow().data {
                    return *s;
                }
            }
        }
    }
    Symbol::intern("unknown")
}
