//! `Object`'s native methods and the default `doesNotUnderstand:`
//! (`newspeak-spec.pdf` §5.5).

use crate::types::*;
use crate::{Heap, Mutator, Symbol};

pub(crate) fn dispatch<'gc>(
    mutator: &mut Mutator<'gc>,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: &[Value<'gc>],
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    match selector.as_str() {
        "class" => Some(Ok(Value::Object(mutator.class_of(&receiver)))),
        "==" => Some(Ok(Value::Boolean(identical(&receiver, &arguments[0])))),
        "=" => Some(Ok(Value::Boolean(identical(&receiver, &arguments[0])))), // Object's default: identity
        "hash" => Some(Ok(Value::SmallInteger(
            identity_hash_of(&mut mutator.heap, &receiver) as i64,
        ))),
        "printString" => Some(Ok(Value::Object(
            mutator.new_string("an Object".to_string()),
        ))),
        "doesNotUnderstand:" => {
            // Object's default doesNotUnderstand: (spec §5.5): must be
            // `protected`, always raises a MessageNotUnderstood-shaped
            // NewspeakError. There is no exception-class hierarchy yet;
            // this produces the NewspeakError value directly
            // rather than a catchable Newspeak object.
            let message_selector =
                extract_does_not_understand_selector(&mutator.heap, &arguments[0]);
            Some(Err(NewspeakError {
                selector: message_selector,
                receiver,
                message: format!(
                    "{} does not understand #{}",
                    describe(&receiver, mutator),
                    message_selector
                ),
            }))
        }
        _ => None,
    }
}

fn identical<'gc>(first: &Value<'gc>, second: &Value<'gc>) -> bool {
    match (first, second) {
        (Value::Nil, Value::Nil) => true,
        (Value::Boolean(left), Value::Boolean(right)) => left == right,
        (Value::SmallInteger(left), Value::SmallInteger(right)) => left == right,
        (Value::Float(left), Value::Float(right)) => left.to_bits() == right.to_bits(),
        (Value::Object(left), Value::Object(right)) => left.same(*right),
        _ => false,
    }
}

fn identity_hash_of<'gc>(heap: &mut Heap<'gc>, value: &Value<'gc>) -> u32 {
    match value {
        Value::Object(object) => heap.ensure_identity_hash(*object),
        _ => 0, // primitives are values and get no distinct identity
                // hash (revisit if a fixture needs one)
    }
}

fn describe<'gc>(_value: &Value<'gc>, _m: &Mutator<'gc>) -> String {
    // The default `doesNotUnderstand:` message names the receiver by kind
    // only. `Heap::describe` is the value-aware description.
    "an object".to_string()
}

fn extract_does_not_understand_selector<'gc>(
    heap: &Heap<'gc>,
    message_mirror_shaped_array: &Value<'gc>,
) -> Symbol {
    // Matches `Mutator::decide_does_not_understand`'s `{selector. arguments}`
    // array shape.
    if let Value::Object(object) = message_mirror_shaped_array {
        if let Some(Value::Object(symbol_object)) = heap.array_at(*object, 0) {
            if let Some(symbol) = heap.symbol_value(symbol_object) {
                return symbol;
            }
        }
    }
    Symbol::UNKNOWN
}
