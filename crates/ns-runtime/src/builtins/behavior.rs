//! The two leaves `Kernel.ns`'s `Class` and `Metaclass` forward to: a
//! class's name, and a metaclass's class. Nothing else about a class object
//! is answered at the base level; reflection is a mirror's
//! (`docs/design/object-model.md`, "Class sides and metaclasses").

use crate::builtins::primitive::BehaviorPrimitive;
use crate::types::*;
use crate::Mutator;

pub(crate) fn execute<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: BehaviorPrimitive,
    receiver: Value<'gc>,
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let Value::Object(object) = receiver else {
        return None;
    };
    if !mutator.heap().is_class(object) {
        return None;
    }
    match primitive {
        BehaviorPrimitive::Name => {
            // A metaclass has no mixin of its own to name -- its name is
            // `Metaclass>>name`'s, built in Newspeak from `thisClass name`.
            if mutator.heap().class_this_class(object).is_some() {
                return None;
            }
            let name = mutator
                .heap()
                .class_application(object)?
                .mixin
                .name
                .to_string();
            Some(Ok(Value::Object(mutator.new_string(name))))
        }
        BehaviorPrimitive::ThisClass => mutator
            .heap()
            .class_this_class(object)
            .map(|this_class| Ok(Value::Object(this_class))),
    }
}
