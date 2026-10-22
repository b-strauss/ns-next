//! §3.1.1's enclosing-object condition on a value *class* -- the enclosing
//! object of every level of its class chain must be a value -- decided once,
//! cold, where a nested class object is created (`nested::after_clause`), as
//! SOMns decides a value class's conditions
//! (`InstantiationNode.signalExceptionsIfFaultFoundElseReturnClassObject`).
//! Unlike SOMns, the verdict is only *recorded* here, on the class object
//! (`Mutator::record_value_class_refusal`); `interpreter::instantiate`'s
//! `allocate_for_factory` is what refuses, where an instance would come into
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
) -> Option<ValueClassRefusal<'gc>> {
    if !super::instantiate::is_a_value(mutator, enclosing) {
        return Some(ValueClassRefusal::NonValueEnclosingObject {
            level: None,
            offender: enclosing,
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
                    offender,
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
                        offender,
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
                    offender: level_enclosing,
                });
            }
        }
        level = heap.class_superclass(class);
    }
    None
}

impl<'gc> Mutator<'gc> {
    /// Records `refusal` on `class` as heap data: an `Array` of the
    /// offending object and the level's name as a Symbol, or nil for the
    /// class's own enclosing object (`Heap::class_value_refusal` reads it
    /// back). Heap data rather than a rendered text, so the collector keeps
    /// the offender and the message describes it when `new` refuses.
    /// Answers whether the write happened: `false` when `class` is no class
    /// object.
    pub fn record_value_class_refusal(
        &mut self,
        class: ClassReference<'gc>,
        refusal: ValueClassRefusal<'gc>,
    ) -> bool {
        let ValueClassRefusal::NonValueEnclosingObject { level, offender } = refusal;
        let level = level.map_or(Value::nil(), |name| self.new_symbol(name));
        let record = self.new_array(vec![Value::object(offender), level]);
        self.heap.class_set_value_refusal(class, record)
    }
}
