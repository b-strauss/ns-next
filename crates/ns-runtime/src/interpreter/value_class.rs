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
    check_levels(mutator, superclass)
}

/// §3.1.1's enclosing-object condition on a body-less declaration's class
/// object `class` (`Mixin::has_body`), whose declaration was evaluated in
/// `declaration_enclosing`. The class object is two classes at once: the
/// declaration, a member class whose enclosing object §3.3 makes "the
/// object of which C is a member", `declaration_enclosing`, and the level
/// its last clause added, whose enclosing object §6.2 makes the named
/// class's. An instance's enclosing objects (§3.3, "the enclosing object
/// of o with respect to S") are those of both and of every level below,
/// and §3.1.1 asks that "Its enclosing objects (3.3) are all value
/// objects" -- the same objects a body version of the declaration is held
/// to, its own class's and its levels'.
#[cold]
#[inline(never)]
pub(crate) fn check_body_less_value_class<'gc>(
    mutator: &Mutator<'gc>,
    declaration_enclosing: ObjectReference<'gc>,
    class: ClassReference<'gc>,
) -> Option<ValueClassRefusal<'gc>> {
    if !super::instantiate::is_a_value(mutator, declaration_enclosing) {
        return Some(ValueClassRefusal::NonValueEnclosingObject {
            level: None,
            offender: declaration_enclosing,
        });
    }
    let heap = mutator.heap();
    if let Some(level_enclosing) = heap.class_enclosing(class)
        && !super::instantiate::is_a_value(mutator, level_enclosing)
    {
        // The level is named for the class its clause named, as a
        // level of a declaration with a body is: the class object's
        // own name is the declaration's.
        let level_name = heap
            .class_named(class)
            .and_then(|named| heap.class_name(named));
        return Some(ValueClassRefusal::NonValueEnclosingObject {
            level: level_name,
            offender: level_enclosing,
        });
    }
    heap.class_superclass(class)
        .and_then(|superclass| check_levels(mutator, superclass))
}

/// The enclosing object of every level of the chain from `superclass` up,
/// the walk `check_value_class` and `check_body_less_value_class` share
/// once the class's own enclosing object has passed.
fn check_levels<'gc>(
    mutator: &Mutator<'gc>,
    superclass: ClassReference<'gc>,
) -> Option<ValueClassRefusal<'gc>> {
    // Every level's enclosing object, not only the class's own: an
    // inherited method reaches its own level's through `outer`. The walk
    // stops at a class that inherits `Value` and was created as a class of
    // its own -- by `after_clause`, as a builtin or as a module root, never
    // a level a `<:` clause added but a body-less declaration's class
    // object, which is its declaration's class -- because that class was checked the
    // same way and everything above it holds by induction. Since the
    // redesign records a refusal rather than raising it, a class the walk
    // would stop at can itself carry one (a value class whose own
    // enclosing object was not a value is still created, not refused, per
    // `docs/design/object-model.md`), so a subclass built on it inherits
    // the fault instead of the walk trusting the class blindly.
    let heap = mutator.heap();
    let mut level = Some(superclass);
    while let Some(class) = level {
        let own_class = !heap.class_is_applied_level(class)
            || heap.class_body_less_declaration(class).is_some();
        if heap.class_inherits_value(class) && own_class {
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
                    let level_name = heap.class_name(class);
                    Some(ValueClassRefusal::NonValueEnclosingObject {
                        level: refused_level.or(level_name),
                        offender,
                    })
                }
            };
        }
        if let Some(level_enclosing) = heap.class_enclosing(class)
            && !super::instantiate::is_a_value(mutator, level_enclosing)
        {
            let level_name = heap.class_name(class);
            return Some(ValueClassRefusal::NonValueEnclosingObject {
                level: level_name,
                offender: level_enclosing,
            });
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
