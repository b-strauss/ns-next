//! The leaves `Kernel.ns`'s `Behavior`, `Class` and `Metaclass` forward to:
//! a class's name, a metaclass's class, and the three parts of a class
//! object's state the Newspeak side of a value class object's `=` compares
//! when `builtins::value`'s walk declines -- its application, its enclosing
//! object and its superclass. Nothing else about a class object is answered
//! at the base level; reflection is a mirror's
//! (`docs/design/object-model.md`, "Class sides and metaclasses").

use crate::builtins::primitive::BehaviorPrimitive;
use crate::types::*;
use crate::Mutator;

pub(crate) fn execute<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: BehaviorPrimitive,
    receiver: Value<'gc>,
    arguments: &[Value<'gc>],
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    let object = receiver.as_object()?;
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
            Some(Ok(Value::object(mutator.new_string(name))))
        }
        BehaviorPrimitive::ThisClass => mutator
            .heap()
            .class_this_class(object)
            .map(|this_class| Ok(Value::object(this_class))),
        BehaviorPrimitive::SharesApplication => {
            let other = arguments.first().and_then(|value| value.as_object())?;
            if !mutator.heap().is_class(other) {
                return None;
            }
            let own = mutator.heap().class_application_index(object)?;
            let others = mutator.heap().class_application_index(other)?;
            Some(Ok(Value::boolean(own == others)))
        }
        BehaviorPrimitive::Enclosing => Some(Ok(mutator
            .heap()
            .class_enclosing(object)
            .map_or(Value::nil(), Value::object))),
        BehaviorPrimitive::Superclass => Some(Ok(mutator
            .heap()
            .class_superclass(object)
            .map_or(Value::nil(), Value::object))),
    }
}
