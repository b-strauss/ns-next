//! `Object`'s native methods. Its default `doesNotUnderstand:`
//! (`newspeak-spec.pdf` §5.5) is Newspeak in `Kernel.ns`, which signals a
//! `MessageNotUnderstood`.

use crate::builtins::primitive::{ObjectPrimitive, Primitive};
use crate::types::*;
use crate::{Heap, Mutator};

pub(crate) fn execute<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: ObjectPrimitive,
    receiver: Value<'gc>,
    arguments: &[Value<'gc>],
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    match primitive {
        ObjectPrimitive::Class => Some(Ok(Value::Object(mutator.class_of(&receiver)))),
        // Identity, and `Object`'s default `=`: declined for a future it
        // would answer `false` to (`interpreter::future::declines_for_a_future`).
        ObjectPrimitive::Identical | ObjectPrimitive::Equal => {
            crate::interpreter::future::declines_for_a_future(
                mutator,
                Primitive::Object(primitive),
                identical(&receiver, &arguments[0]),
                receiver,
                arguments,
            )
            .map(|answer| Ok(Value::Boolean(answer)))
        }
        ObjectPrimitive::Hash => Some(Ok(Value::SmallInteger(identity_hash_of(
            &mut mutator.heap,
            &receiver,
        ) as i64))),
        ObjectPrimitive::PrintString => Some(Ok(Value::Object(
            mutator.new_string("an Object".to_string()),
        ))),
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
