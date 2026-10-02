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
        ObjectPrimitive::Class => Some(Ok(Value::object(mutator.class_of(&receiver)))),
        // Identity, and `Object`'s default `=`: declined for a future it
        // would answer `false` to (`interpreter::future::declines_for_a_future`).
        ObjectPrimitive::Identical | ObjectPrimitive::Equal => {
            crate::interpreter::future::declines_for_a_future(
                mutator,
                Primitive::Object(primitive),
                identical(&mutator.heap, &receiver, &arguments[0]),
                receiver,
                arguments,
            )
            .map(|answer| Ok(Value::boolean(answer)))
        }
        ObjectPrimitive::Hash => Some(Ok(Value::small_integer(identity_hash_of(
            &mut mutator.heap,
            &receiver,
        ) as i64))),
        ObjectPrimitive::PrintString => Some(Ok(Value::object(
            mutator.new_string("an Object".to_string()),
        ))),
    }
}

fn identical<'gc>(heap: &Heap<'gc>, first: &Value<'gc>, second: &Value<'gc>) -> bool {
    match (first.kind(), second.kind()) {
        (ValueKind::Nil, ValueKind::Nil) => true,
        (ValueKind::Boolean(left), ValueKind::Boolean(right)) => left == right,
        (ValueKind::SmallInteger(left), ValueKind::SmallInteger(right)) => left == right,
        (ValueKind::SmallFloat(left), ValueKind::SmallFloat(right)) => {
            left.to_bits() == right.to_bits()
        }
        // A float's identity is its bits: a boxed `Float` is a value
        // (§3.1.1), so two allocations of one float are the same float, as
        // two immediates of it are. Only two distinct handles ask the heap,
        // so `==` on one object, or on immediates, reads nothing; an
        // immediate and a boxed float never share bits.
        (ValueKind::Object(left), ValueKind::Object(right)) => {
            left.same(right)
                || heap.float_value(*first).is_some_and(|number| {
                    heap.float_value(*second)
                        .is_some_and(|other| other.to_bits() == number.to_bits())
                })
        }
        _ => false,
    }
}

fn identity_hash_of<'gc>(heap: &mut Heap<'gc>, value: &Value<'gc>) -> u32 {
    match value.kind() {
        // A boxed float hashes as an immediate one does, since `identical`
        // makes it the same float as every other allocation of its bits.
        ValueKind::Object(object) if heap.is_boxed_float(object) => 0,
        ValueKind::Object(object) => heap.ensure_identity_hash(object),
        // Immediates are values and get no distinct identity hash.
        _ => 0,
    }
}
