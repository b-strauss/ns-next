//! §3.1.1's conditions on a value *class* -- a value class may not define
//! `==`, and its enclosing object, and the enclosing object of every level
//! of its class chain, must be a value -- decided once, cold, where a
//! nested class object is created (`nested::after_clause`), as SOMns
//! decides them
//! (`InstantiationNode.signalExceptionsIfFaultFoundElseReturnClassObject`).
//! Unlike SOMns, the verdict is only *recorded* here, on the class object
//! (`ClassData::value_class_refusal`); `interpreter::instantiate`'s
//! `decide_instantiation` is what refuses, where an instance would come
//! into being. `docs/design/object-model.md`, "The two new conditions are
//! decided where a class object is created and refused where an instance
//! is made", explains why: the specification's own §2.3 example creates a
//! class whose enclosing object is mutable and never instantiates it.
//! `after_clause` calls in only for a class that inherits `Value`.

use crate::mixin::MixinApplication;
use crate::types::*;

use super::Mutator;

#[cold]
#[inline(never)]
pub(crate) fn check_value_class<'gc>(
    mutator: &Mutator<'gc>,
    application: &MixinApplication,
    enclosing: ObjectReference<'gc>,
    superclass: ClassReference<'gc>,
) -> Option<ValueClassRefusal> {
    if let Some(declaring) = application.identity_method {
        return Some(ValueClassRefusal::IdentityMethod { declaring });
    }
    if !super::instantiate::is_a_value(mutator, enclosing) {
        return Some(ValueClassRefusal::NonValueEnclosingObject {
            level: None,
            described: mutator.heap().describe(&Value::Object(enclosing)),
        });
    }
    // Every level's enclosing object, not only the class's own: an
    // inherited method reaches its own level's through `outer`. The walk
    // stops at a class that inherits `Value` and was created as a class of
    // its own -- by `after_clause`, as a builtin or as a module root, never
    // a level a `<:` clause added -- because that class was checked the
    // same way and everything above it holds by induction.
    let heap = mutator.heap();
    let mut level = Some(superclass);
    while let Some(class) = level {
        if heap.class_inherits_value(class) && !heap.class_is_applied_level(class) {
            return None;
        }
        if let Some(level_enclosing) = heap.class_enclosing(class) {
            if !super::instantiate::is_a_value(mutator, level_enclosing) {
                let level_name = heap
                    .class_application(class)
                    .map(|application| application.mixin.name);
                return Some(ValueClassRefusal::NonValueEnclosingObject {
                    level: level_name,
                    described: heap.describe(&Value::Object(level_enclosing)),
                });
            }
        }
        level = heap.class_superclass(class);
    }
    None
}
