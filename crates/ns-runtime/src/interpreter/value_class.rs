//! §3.1.1's enclosing-object condition on a value *class* -- the enclosing
//! object of every level of its class chain must be a value -- decided once,
//! cold, where a nested class object is created (`nested::after_clause`), as
//! SOMns decides a value class's conditions
//! (`InstantiationNode.signalExceptionsIfFaultFoundElseReturnClassObject`).
//! Unlike SOMns, the verdict is only *recorded* here, on the class object
//! (`ClassData::value_class_refusal`); `interpreter::instantiate`'s
//! `decide_instantiation` is what refuses, where an instance would come into
//! being. `docs/design/object-model.md`, "A value object's four conditions",
//! explains why: the specification's own §2.3 example creates a class whose
//! enclosing object is mutable and never instantiates it. `after_clause`
//! calls in only for a class that inherits `Value`.
//!
//! §3.1.1's other class-level condition, no identity method, is not decided
//! here: it is a property of the *application*, computed once by
//! `Mutator::apply_mixin` and folded straight into
//! `MixinApplication::instantiation_refusal`, so it needs no class object
//! and no call from here to cover a module's root.

use crate::types::*;

use super::Mutator;

#[cold]
#[inline(never)]
pub(crate) fn check_value_class<'gc>(
    mutator: &Mutator<'gc>,
    enclosing: ObjectReference<'gc>,
    superclass: ClassReference<'gc>,
) -> Option<ValueClassRefusal> {
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
    // same way and everything above it holds by induction. Since the
    // redesign records a refusal rather than raising it, a class the walk
    // would stop at can itself carry one (a value class whose own
    // enclosing object was not a value is still created, not refused, per
    // `docs/design/object-model.md`), so a subclass built on it inherits
    // the fault instead of the walk trusting the class blindly.
    let heap = mutator.heap();
    let mut level = Some(superclass);
    while let Some(class) = level {
        if heap.class_inherits_value(class) && !heap.class_is_applied_level(class) {
            return match heap.class_value_refusal(class) {
                None => None,
                Some(ValueClassRefusal::NonValueEnclosingObject {
                    level: refused_level,
                    described,
                }) => {
                    // A refusal naming no level is about the stop class's
                    // own enclosing object, so it names the stop class; one
                    // that already names a level inherited its fault from
                    // further up, and that level is still the one at fault.
                    let level_name = heap
                        .class_application(class)
                        .map(|application| application.mixin.name);
                    Some(ValueClassRefusal::NonValueEnclosingObject {
                        level: refused_level.or(level_name),
                        described,
                    })
                }
            };
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
