//! Installing an edit of a class atomically: the commit after
//! `Mutator::prepare_edit` (`docs/design/liveness-and-reflection.md`, "The
//! install is one leaf, and its commit has no safepoint").

use std::rc::Rc;

use super::edit::{Edit, PreparedEdit};
use super::Mutator;
use crate::hash::IdentityMap;
use crate::heap::{ClassReference, MixinId, ObjectReference};
use crate::mixin::{Mixin, MixinApplication};

/// What the migration answers: the instances it replaced, their copies in
/// the new layout at the same positions, and the instances it kept.
struct Migration<'gc> {
    old: Vec<ObjectReference<'gc>>,
    new: Vec<ObjectReference<'gc>>,
    kept: Vec<ObjectReference<'gc>>,
}

impl<'gc> Mutator<'gc> {
    /// Answers the edited declaration's text, or a diagnostic with
    /// nothing changed.
    ///
    /// Prepares the edit and finds what it reaches, which is where
    /// everything that can fail fails; then commits without running
    /// Newspeak code, sending to a user object or polling a safepoint, so
    /// that no program observes a half-installed edit
    /// (`docs/design/liveness-and-reflection.md`, "The install is one
    /// leaf, and its commit has no safepoint"): every fresh mixin under
    /// its old id, every class object found re-applied superclasses first
    /// -- keeping its identity, as every class object does -- the
    /// instances whose layout changed copied into the new one by name and
    /// forwarded in one bulk forward, the others restamped.
    pub(crate) fn install_edit(&mut self, id: MixinId, edit: &Edit) -> Result<String, String> {
        let prepared = self.prepare_edit(id, edit)?;
        let mut fresh_by_id: IdentityMap<MixinId, Rc<Mixin>> = IdentityMap::default();
        for (replaced, fresh) in &prepared.replacements {
            fresh_by_id.insert(*replaced, Rc::clone(fresh));
        }
        for (_, class_side, fresh) in &prepared.synthesized {
            fresh_by_id.insert(*class_side, Rc::clone(fresh));
        }
        let (classes, instances) = self
            .heap
            .classes_and_instances_over(|candidate| fresh_by_id.contains_key(&candidate));
        let class_positions = self.class_positions(&classes);
        self.refuse_an_unmigratable_instance(&classes, &class_positions, &instances, &fresh_by_id)?;

        // The commit: nothing below fails.
        let old_applications: Vec<Rc<MixinApplication>> = classes
            .iter()
            .map(|class| {
                self.heap
                    .class_application(*class)
                    .expect("internal: the walk collects class objects")
            })
            .collect();
        self.replace_versions(&prepared);
        let new_applications = self.reapply(&classes, &old_applications);
        let migration = self.migrate(
            &classes,
            &class_positions,
            &old_applications,
            &new_applications,
            &instances,
        );
        // An unmoved instance, a class object whose metaclass was re-applied
        // among them, carries its class's old guard until restamped.
        self.heap.stamp_dispatch_ids(&migration.kept);
        if migration.old.is_empty() {
            // The applications changed under unchanged class objects, so
            // every entry decided for the old ones is stale.
            self.caches.invalidate();
        } else {
            debug_assert!(
                migration
                    .old
                    .iter()
                    .chain(&migration.new)
                    .all(|object| crate::builtins::reflection::may_be_forwarded(self, *object)),
                "internal: the install forwards only instances the forward accepts"
            );
            if crate::heap::forward_identity(self, &migration.old, &migration.new).is_err() {
                unreachable!("internal: each migrated instance is forwarded once, to a new one");
            }
        }
        Ok(prepared.declaration_text)
    }

    /// Each class object's position in `classes`, by its identity key.
    fn class_positions(&self, classes: &[ClassReference<'gc>]) -> IdentityMap<usize, usize> {
        classes
            .iter()
            .enumerate()
            .map(|(position, class)| (self.heap.object_identity_key(*class), position))
            .collect()
    }

    /// The migration diagnostic for the first instance whose layout may
    /// change and which the bulk forward would refuse. Decided before
    /// anything is replaced, from the old and the fresh mixins, since the
    /// layouts themselves are known only once re-applied: conservatively,
    /// a class's layout may change when a replaced level of its chain has
    /// another slot list or another nested list, so no instance the commit
    /// migrates escapes the check.
    fn refuse_an_unmigratable_instance(
        &self,
        classes: &[ClassReference<'gc>],
        class_positions: &IdentityMap<usize, usize>,
        instances: &[ObjectReference<'gc>],
        fresh_by_id: &IdentityMap<MixinId, Rc<Mixin>>,
    ) -> Result<(), String> {
        let may_change: Vec<bool> = classes
            .iter()
            .map(|class| self.layout_may_change(*class, fresh_by_id))
            .collect();
        for instance in instances {
            let class = self.heap.object_class(*instance);
            let changes = class_positions
                .get(&self.heap.object_identity_key(class))
                .is_some_and(|position| may_change[*position]);
            if changes && !crate::builtins::reflection::may_be_forwarded(self, *instance) {
                let name = self
                    .heap
                    .class_application(class)
                    .map_or_else(String::new, |application| {
                        application.mixin.reflected_name()
                    });
                return Err(format!(
                    "cannot migrate an instance of {name}, which is not an ordinary object"
                ));
            }
        }
        Ok(())
    }

    /// Whether a replaced level of `class`'s chain declares other slots,
    /// by name and in order, or other nested classes than the fresh
    /// version replacing it.
    fn layout_may_change(
        &self,
        class: ClassReference<'gc>,
        fresh_by_id: &IdentityMap<MixinId, Rc<Mixin>>,
    ) -> bool {
        let mut level = Some(class);
        while let Some(current) = level {
            if let Some(application) = self.heap.class_application(current) {
                let fresh = self
                    .heap
                    .known_mixin_id(&application.mixin)
                    .and_then(|id| fresh_by_id.get(&id));
                if let Some(fresh) = fresh {
                    let old = &application.mixin;
                    let same_slots = old
                        .slots
                        .iter()
                        .map(|slot| slot.name)
                        .eq(fresh.slots.iter().map(|slot| slot.name));
                    let same_nested = old
                        .nested
                        .iter()
                        .map(|nested| nested.name)
                        .eq(fresh.nested.iter().map(|nested| nested.name));
                    if !same_slots || !same_nested {
                        return true;
                    }
                }
            }
            level = self.heap.class_superclass(current);
        }
        false
    }

    /// Every fresh mixin under the id it replaces, a synthesized class
    /// side under the old one's, so that every metaclass keeps its mixin's
    /// id; and the edited text as its module's current one.
    fn replace_versions(&mut self, prepared: &PreparedEdit) {
        let text = &prepared.module_text;
        for (id, fresh) in &prepared.replacements {
            self.replace_mixin(*id, Rc::clone(fresh), Some(text.clone()));
        }
        // After the instance side's replacement, which forgets the class
        // side synthesized from its previous version; through the
        // mutator's replacement, which forgets the old class side's
        // applications, so a metaclass over the unchanged `Class` is not
        // answered the old one from the application cache.
        for (instance_side, class_side, fresh) in &prepared.synthesized {
            self.replace_mixin(*class_side, Rc::clone(fresh), Some(text.clone()));
            self.metaclass_mixins
                .insert(*instance_side, Rc::clone(fresh));
        }
        self.heap
            .set_current_module_text(prepared.module_root, text.clone());
    }

    /// Gives every class object of `classes` an application of its mixin's
    /// current version over its superclass's application now, superclasses
    /// first, so a superclass found is re-applied before its subclasses;
    /// its superclass clause is not evaluated again. Records an old level
    /// whose slot base and own slots the edit kept as succeeded by the new
    /// one, so an activation entered at it passes the slot check. Answers
    /// the new applications, by position.
    fn reapply(
        &mut self,
        classes: &[ClassReference<'gc>],
        old_applications: &[Rc<MixinApplication>],
    ) -> Vec<Rc<MixinApplication>> {
        let mut order: Vec<usize> = (0..classes.len()).collect();
        order.sort_by_cached_key(|position| {
            let mut length = 0_usize;
            let mut level = self.heap.class_superclass(classes[*position]);
            while let Some(current) = level {
                length += 1;
                level = self.heap.class_superclass(current);
            }
            length
        });
        let mut new_applications: Vec<Option<Rc<MixinApplication>>> = vec![None; classes.len()];
        for position in order {
            let class = classes[position];
            let old = &old_applications[position];
            let superclass = self
                .heap
                .class_superclass(class)
                .and_then(|superclass| self.heap.class_application(superclass));
            let id = self
                .heap
                .known_mixin_id(&old.mixin)
                .expect("internal: an applied mixin is registered");
            let current = self
                .heap
                .mixin(id)
                .expect("internal: a registered mixin has a row");
            let new = self.apply_mixin(&current, superclass.as_ref());
            if !self.heap.class_install_application(class, Rc::clone(&new)) {
                unreachable!("internal: the walk collects class objects");
            }
            let same_own_slots = old.mixin.slots.iter().map(|slot| slot.name).eq(new
                .mixin
                .slots
                .iter()
                .map(|slot| slot.name));
            if old.own_slot_base == new.own_slot_base && same_own_slots {
                self.heap.record_successor(old.index(), new.index());
            }
            new_applications[position] = Some(new);
        }
        new_applications
            .into_iter()
            .map(|new| new.expect("internal: every class object is re-applied"))
            .collect()
    }

    /// Allocates each instance whose class's fields changed in the new
    /// layout and copies its fields into it by name -- of two old fields
    /// of one name, the last -- leaving a field the edit added nil; runs
    /// no initializer and sends nothing.
    fn migrate(
        &mut self,
        classes: &[ClassReference<'gc>],
        class_positions: &IdentityMap<usize, usize>,
        old_applications: &[Rc<MixinApplication>],
        new_applications: &[Rc<MixinApplication>],
        instances: &[ObjectReference<'gc>],
    ) -> Migration<'gc> {
        // Per class, for each new field, the old field it takes; `None`
        // where the layout stayed.
        let copies: Vec<Option<Vec<Option<usize>>>> = old_applications
            .iter()
            .zip(new_applications)
            .map(|(old, new)| {
                let old_names = old.field_names();
                let new_names = new.field_names();
                (old_names != new_names).then(|| {
                    new_names
                        .iter()
                        .map(|name| old_names.iter().rposition(|old_name| old_name == name))
                        .collect()
                })
            })
            .collect();
        let mut migration = Migration {
            old: Vec::new(),
            new: Vec::new(),
            kept: Vec::new(),
        };
        for instance in instances {
            let class = self.heap.object_class(*instance);
            let position = class_positions
                .get(&self.heap.object_identity_key(class))
                .copied()
                .expect("internal: the walk collects the class of every instance it collects");
            let Some(copy) = &copies[position] else {
                migration.kept.push(*instance);
                continue;
            };
            let replacement = self.heap.allocate_instance(classes[position], copy.len());
            for (field, source) in copy.iter().enumerate() {
                let Some(source) = source else {
                    continue;
                };
                let value = self
                    .heap
                    .instance_field(*instance, *source)
                    .expect("internal: a migrated instance has its old layout's fields");
                if !self.heap.instance_set_field(replacement, field, value) {
                    unreachable!("internal: a replacement has its new layout's fields");
                }
            }
            migration.old.push(*instance);
            migration.new.push(replacement);
        }
        migration
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::heap::{ActivationHeader, ApplicationIndex};
    use crate::interpreter::edit::MemberEdit;
    use crate::interpreter::{HostResult, HostSignal, Interpreter};
    use crate::mixin::Mixin;
    use crate::types::{BuiltinClass, Continuation, FrameKind, SourceOrigin, Value};
    use crate::{Root, Symbol};
    use indoc::indoc;
    use std::path::PathBuf;
    use std::rc::Rc;

    fn load_into(interpreter: &mut Interpreter, source: &str) -> Root {
        let module = ns_syntax::parse(source).expect("parse");
        let resolved = crate::resolve(
            module,
            Rc::from(source),
            SourceOrigin::File(PathBuf::from("t.ns")),
        )
        .expect("resolve");
        interpreter.load_module(&resolved)
    }

    fn load(source: &str) -> (Interpreter, Root) {
        let mut interpreter = Interpreter::bootstrap();
        let class = load_into(&mut interpreter, source);
        (interpreter, class)
    }

    fn expect_ok(result: HostResult, what: &str) -> Root {
        match result {
            Ok(root) => root,
            Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
            Err(HostSignal::Error(error)) => {
                panic!("{what}: unexpected Newspeak error: {}", error.message)
            }
        }
    }

    fn send(interpreter: &mut Interpreter, receiver: &Root, selector: &str) -> Root {
        expect_ok(
            interpreter.send(receiver, Symbol::intern(selector), &[]),
            selector,
        )
    }

    fn send_with(
        interpreter: &mut Interpreter,
        receiver: &Root,
        selector: &str,
        arguments: &[Root],
    ) -> Root {
        expect_ok(
            interpreter.send(receiver, Symbol::intern(selector), arguments),
            selector,
        )
    }

    fn integer(interpreter: &mut Interpreter, root: &Root) -> Option<i64> {
        interpreter.enter(|mutator| mutator.fetch(root).as_small_integer())
    }

    fn integer_root(interpreter: &mut Interpreter, value: i64) -> Root {
        interpreter.stash(Value::small_integer(value))
    }

    fn string(interpreter: &mut Interpreter, root: &Root) -> Option<String> {
        interpreter.enter(|mutator| {
            let object = mutator.fetch(root).as_object()?;
            mutator.heap().str_value(object)
        })
    }

    /// The mixin the class object `class` applies.
    fn mixin_of(mutator: &Mutator<'_>, class: &Root) -> Rc<Mixin> {
        let class = mutator.fetch(class).as_object().expect("a class object");
        mutator
            .heap()
            .class_application(class)
            .expect("a class object")
            .mixin
            .clone()
    }

    /// The id of the declaration `name` nested in the mixin `class`
    /// applies.
    fn nested_id_of(interpreter: &mut Interpreter, class: &Root, name: &str) -> MixinId {
        interpreter.enter(|mutator| {
            let nested = mixin_of(mutator, class)
                .nested
                .iter()
                .find(|nested| nested.name.as_str() == name)
                .cloned()
                .expect("the declaration is nested there");
            mutator.heap.mixin_id(&nested)
        })
    }

    fn install(interpreter: &mut Interpreter, id: MixinId, edit: &Edit) -> String {
        match interpreter.enter(|mutator| mutator.install_edit(id, edit)) {
            Ok(text) => text,
            Err(diagnostic) => panic!("the install was refused: {diagnostic}"),
        }
    }

    fn instance_edit(operations: Vec<MemberEdit>) -> Edit {
        Edit {
            header: None,
            instance_side: operations,
            class_side: Vec::new(),
        }
    }

    fn add_method(text: &str) -> Edit {
        instance_edit(vec![MemberEdit::AddMethod(text.to_string())])
    }

    fn header_edit(text: &str) -> Edit {
        Edit {
            header: Some(text.to_string()),
            instance_side: Vec::new(),
            class_side: Vec::new(),
        }
    }

    const OUTER: &str = indoc! {"
        class Outer = Object new () (
          public class Inner = (| public x ::= 1. |) (
            public a = ( ^1 )
            public block = ( ^[ 1 ] )
          )
        )
    "};

    /// An `Outer` instance, its `Inner` class object, an `Inner` instance
    /// and `Inner`'s id.
    fn outer_and_inner(source: &str) -> (Interpreter, Root, Root, Root, MixinId) {
        let (mut interpreter, outer) = load(source);
        let instance = expect_ok(interpreter.instantiate(&outer, &[]), "an Outer");
        let inner_class = send(&mut interpreter, &instance, "Inner");
        let inner = send(&mut interpreter, &inner_class, "new");
        let inner_id = nested_id_of(&mut interpreter, &outer, "Inner");
        (interpreter, instance, inner_class, inner, inner_id)
    }

    /// A method added to a class answers on an instance made before.
    #[test]
    fn an_added_method_answers_on_an_existing_instance() {
        let (mut interpreter, _, _, inner, inner_id) = outer_and_inner(OUTER);
        let text = install(
            &mut interpreter,
            inner_id,
            &add_method("public b = ( ^x + 1 )"),
        );
        assert!(text.contains("public b = ( ^x + 1 )"));
        let answer = send(&mut interpreter, &inner, "b");
        assert_eq!(integer(&mut interpreter, &answer), Some(2));
    }

    /// A replaced method answers its new body on an existing instance.
    #[test]
    fn a_replaced_method_answers_the_new_value() {
        let (mut interpreter, _, _, inner, inner_id) = outer_and_inner(OUTER);
        install(&mut interpreter, inner_id, &add_method("public a = ( ^7 )"));
        let answer = send(&mut interpreter, &inner, "a");
        assert_eq!(integer(&mut interpreter, &answer), Some(7));
    }

    /// A removed method is no longer understood by an existing instance.
    #[test]
    fn a_removed_method_is_not_understood() {
        let (mut interpreter, _, _, inner, inner_id) = outer_and_inner(OUTER);
        install(
            &mut interpreter,
            inner_id,
            &instance_edit(vec![MemberEdit::RemoveMethod(Symbol::intern("a"))]),
        );
        match interpreter.send(&inner, Symbol::intern("a"), &[]) {
            Err(HostSignal::Error(error)) => assert!(
                error.message.contains("does not understand"),
                "{}",
                error.message
            ),
            Ok(_) => panic!("a removed method answered"),
            Err(HostSignal::Return) => panic!("a non-local return escaped"),
        }
    }

    /// A slot the header adds is nil on a migrated instance, whose other
    /// slots keep their values, and initialized on a new one: no
    /// initializer runs for an old instance.
    #[test]
    fn an_added_slot_is_nil_on_an_old_instance_and_initialized_on_a_new_one() {
        let (mut interpreter, _, inner_class, inner, inner_id) = outer_and_inner(OUTER);
        let five = integer_root(&mut interpreter, 5);
        send_with(&mut interpreter, &inner, "x:", &[five]);
        install(
            &mut interpreter,
            inner_id,
            &header_edit("public class Inner = (| public x ::= 1. public y = 2. |)"),
        );
        let slot_x = send(&mut interpreter, &inner, "x");
        assert_eq!(integer(&mut interpreter, &slot_x), Some(5));
        let slot_y = send(&mut interpreter, &inner, "y");
        assert!(interpreter.enter(|mutator| mutator.fetch(&slot_y).is_nil()));
        let fresh = send(&mut interpreter, &inner_class, "new");
        let slot_x = send(&mut interpreter, &fresh, "x");
        let slot_y = send(&mut interpreter, &fresh, "y");
        assert_eq!(integer(&mut interpreter, &slot_x), Some(1));
        assert_eq!(integer(&mut interpreter, &slot_y), Some(2));
    }

    /// A slot the header removes takes its accessors with it, and the
    /// slots around it keep their values in the smaller layout.
    #[test]
    fn a_removed_slot_s_neighbours_keep_their_values() {
        let source = indoc! {"
            class Outer = Object new () (
              public class Inner = (| public x ::= 1. public y ::= 2. public z ::= 3. |) ()
            )
        "};
        let (mut interpreter, _, _, inner, inner_id) = outer_and_inner(source);
        for (selector, value) in [("x:", 10), ("y:", 20), ("z:", 30)] {
            let value = integer_root(&mut interpreter, value);
            send_with(&mut interpreter, &inner, selector, &[value]);
        }
        install(
            &mut interpreter,
            inner_id,
            &header_edit("public class Inner = (| public x ::= 1. public z ::= 3. |)"),
        );
        let slot_x = send(&mut interpreter, &inner, "x");
        let slot_z = send(&mut interpreter, &inner, "z");
        assert_eq!(integer(&mut interpreter, &slot_x), Some(10));
        assert_eq!(integer(&mut interpreter, &slot_z), Some(30));
        assert!(interpreter.send(&inner, Symbol::intern("y"), &[]).is_err());
        let fields = interpreter.enter(|mutator| {
            let object = mutator.fetch(&inner).as_object().expect("an Inner");
            mutator.heap().instance_field_count(object)
        });
        assert_eq!(fields, Some(2));
    }

    /// A migrated instance keeps its identity hash.
    #[test]
    fn the_identity_hash_survives_a_migration() {
        let (mut interpreter, _, _, inner, inner_id) = outer_and_inner(OUTER);
        let hash = interpreter.enter(|mutator| {
            let object = mutator.fetch(&inner).as_object().expect("an Inner");
            mutator.heap_mut().object_ensure_identity_hash(object)
        });
        install(
            &mut interpreter,
            inner_id,
            &header_edit("public class Inner = (| public x ::= 1. public y = 2. |)"),
        );
        interpreter.enter(|mutator| {
            let object = mutator.fetch(&inner).as_object().expect("an Inner");
            assert_eq!(mutator.heap().instance_field_count(object), Some(2));
            assert_eq!(mutator.heap().object_identity_hash(object), hash);
        });
    }

    /// Whether the class objects `first` and `second` share their
    /// application, as `behavior:sharesApplicationWith:` answers it.
    fn share_application(interpreter: &mut Interpreter, first: &Root, second: &Root) -> bool {
        interpreter.enter(|mutator| {
            let first = mutator.fetch(first).as_object().expect("a class");
            let second = mutator.fetch(second).as_object().expect("a class");
            let heap = mutator.heap();
            heap.class_application_index(first) == heap.class_application_index(second)
        })
    }

    /// §6.2: an edit of a mixin reaches every application of it, the class
    /// object of each enclosing instance among them.
    #[test]
    fn two_class_objects_of_one_nested_mixin_both_see_an_edit() {
        let (mut interpreter, outer) = load(OUTER);
        let first_outer = expect_ok(interpreter.instantiate(&outer, &[]), "the first Outer");
        let second_outer = expect_ok(interpreter.instantiate(&outer, &[]), "the second Outer");
        let first_class = send(&mut interpreter, &first_outer, "Inner");
        let second_class = send(&mut interpreter, &second_outer, "Inner");
        let old_instance = send(&mut interpreter, &first_class, "new");
        let inner_id = nested_id_of(&mut interpreter, &outer, "Inner");
        install(&mut interpreter, inner_id, &add_method("public b = ( ^2 )"));
        for class in [&first_class, &second_class] {
            let instance = send(&mut interpreter, class, "new");
            let answer = send(&mut interpreter, &instance, "b");
            assert_eq!(integer(&mut interpreter, &answer), Some(2));
        }
        let answer = send(&mut interpreter, &old_instance, "b");
        assert_eq!(integer(&mut interpreter, &answer), Some(2));
        assert!(share_application(
            &mut interpreter,
            &first_class,
            &second_class
        ));
    }

    const BASE: &str = indoc! {"
        class Base = Object new () (
          public class Edited = (| public x ::= 1. |) (
            public a = ( ^1 )
          ) : (
            public make = ( ^self new )
          )
          public class Mixed = Object new <: Edited () ()
          public class Failure = Error new (| public detail ::= 1. |) ()
        )
    "};

    const SECOND: &str = indoc! {"
        class Second base: aBase = Object new (|
          public base = aBase.
        |) (
          public class Sub = Edited new (| public y ::= 2. |) (
            public both = ( ^x + y )
          )
          public Edited = ( ^base Edited )
        )
    "};

    /// A subclass in another module, a `<:` clause's level, the metaclass
    /// and a program's `Error` subclass are each re-applied over the edited
    /// mixin, their instances migrated by name.
    #[test]
    fn every_level_over_an_edited_mixin_is_reapplied() {
        let (mut interpreter, base_class) = load(BASE);
        let second_class = load_into(&mut interpreter, SECOND);
        let first_base = expect_ok(interpreter.instantiate(&base_class, &[]), "a Base");
        let second_base = expect_ok(interpreter.instantiate(&base_class, &[]), "a Base");
        let second = expect_ok(
            interpreter.instantiate(&second_class, std::slice::from_ref(&first_base)),
            "a Second",
        );
        let sub_class = send(&mut interpreter, &second, "Sub");
        let sub = send(&mut interpreter, &sub_class, "new");
        let ten = integer_root(&mut interpreter, 10);
        send_with(&mut interpreter, &sub, "x:", &[ten]);
        let mixed_class = send(&mut interpreter, &first_base, "Mixed");
        let mixed = send(&mut interpreter, &mixed_class, "new");
        let failure_class = send(&mut interpreter, &first_base, "Failure");
        let failure = send(&mut interpreter, &failure_class, "new");
        let boom = interpreter.enter(|mutator| {
            let text = mutator.new_string("boom".to_string());
            mutator.stash(Value::object(text))
        });
        send_with(&mut interpreter, &failure, "messageText:", &[boom]);
        let seven = integer_root(&mut interpreter, 7);
        send_with(&mut interpreter, &failure, "detail:", &[seven]);
        let first_edited = send(&mut interpreter, &first_base, "Edited");
        let second_edited = send(&mut interpreter, &second_base, "Edited");
        let edited_id = nested_id_of(&mut interpreter, &base_class, "Edited");
        let failure_id = nested_id_of(&mut interpreter, &base_class, "Failure");

        install(
            &mut interpreter,
            edited_id,
            &Edit {
                header: Some("public class Edited = (| public w = 5. public x ::= 1. |)".into()),
                instance_side: vec![MemberEdit::AddMethod("public a = ( ^2 )".into())],
                class_side: vec![MemberEdit::AddMethod(
                    "public makeAnother = ( ^self new )".into(),
                )],
            },
        );
        let both = send(&mut interpreter, &sub, "both");
        assert_eq!(integer(&mut interpreter, &both), Some(12));
        let slot_w = send(&mut interpreter, &sub, "w");
        assert!(interpreter.enter(|mutator| mutator.fetch(&slot_w).is_nil()));
        let answer = send(&mut interpreter, &sub, "a");
        assert_eq!(integer(&mut interpreter, &answer), Some(2));
        let answer = send(&mut interpreter, &mixed, "a");
        assert_eq!(integer(&mut interpreter, &answer), Some(2));
        let fresh_sub = send(&mut interpreter, &sub_class, "new");
        let slot_w = send(&mut interpreter, &fresh_sub, "w");
        assert_eq!(integer(&mut interpreter, &slot_w), Some(5));
        let made = send(&mut interpreter, &first_edited, "makeAnother");
        let answer = send(&mut interpreter, &made, "a");
        assert_eq!(integer(&mut interpreter, &answer), Some(2));
        assert!(share_application(
            &mut interpreter,
            &first_edited,
            &second_edited
        ));
        let metaclasses_share = interpreter.enter(|mutator| {
            let first = mutator.fetch(&first_edited).as_object().expect("a class");
            let second = mutator.fetch(&second_edited).as_object().expect("a class");
            let heap = mutator.heap();
            heap.class_application_index(heap.object_class(first))
                == heap.class_application_index(heap.object_class(second))
        });
        assert!(metaclasses_share);

        install(
            &mut interpreter,
            failure_id,
            &header_edit(
                "public class Failure = Error new (| public extra = 3. public detail ::= 1. |)",
            ),
        );
        let detail = send(&mut interpreter, &failure, "detail");
        assert_eq!(integer(&mut interpreter, &detail), Some(7));
        let text = send(&mut interpreter, &failure, "messageText");
        assert_eq!(string(&mut interpreter, &text).as_deref(), Some("boom"));
        let extra = send(&mut interpreter, &failure, "extra");
        assert!(interpreter.enter(|mutator| mutator.fetch(&extra).is_nil()));
        interpreter.enter(|mutator| {
            assert!(mutator.heap.mixin_is_edited(edited_id));
            assert!(mutator.heap.mixin_is_edited(failure_id));
        });
    }

    const HOLDERS: &str = indoc! {"
        class Holders = Object new () (
          public class Item = (| public x ::= 1. |) ()
          public class Holder = (| public held ::= nil. |) ()
          public capture: item = ( ^[ item ] )
        )
    "};

    /// A migrated instance is replaced in an instance's slot, an `Array`, a
    /// closure's copied value, a running activation's receiver and
    /// register, a `WeakArray` and an `Ephemeron`'s key.
    #[test]
    fn a_migrated_instance_is_replaced_in_every_holder() {
        let (mut interpreter, holders_class) = load(HOLDERS);
        let holders = expect_ok(interpreter.instantiate(&holders_class, &[]), "Holders");
        let item_class = send(&mut interpreter, &holders, "Item");
        let item = send(&mut interpreter, &item_class, "new");
        let holder_class = send(&mut interpreter, &holders, "Holder");
        let holder = send(&mut interpreter, &holder_class, "new");
        send_with(
            &mut interpreter,
            &holder,
            "held:",
            std::slice::from_ref(&item),
        );
        let closure = send_with(
            &mut interpreter,
            &holders,
            "capture:",
            std::slice::from_ref(&item),
        );
        let item_id = nested_id_of(&mut interpreter, &holders_class, "Item");
        let (array, weak, ephemeron) = interpreter.enter(|mutator| {
            let value = mutator.fetch(&item);
            let array = mutator.new_array(vec![value]);
            let weak_class = mutator.builtin_class(BuiltinClass::WeakArray);
            let weak = mutator.heap_mut().allocate_weak_array(weak_class, 1);
            assert!(mutator.heap_mut().weak_array_set_element(weak, 0, value));
            let ephemeron_class = mutator.builtin_class(BuiltinClass::Ephemeron);
            let ephemeron = mutator.heap_mut().allocate_ephemeron(ephemeron_class, 2);
            assert!(mutator.heap_mut().ephemeron_set_field(ephemeron, 0, value));
            (
                mutator.stash(Value::object(array)),
                mutator.stash(Value::object(weak)),
                mutator.stash(Value::object(ephemeron)),
            )
        });
        let edit = header_edit("public class Item = (| public x ::= 1. public y = 2. |)");
        interpreter.enter(|mutator| {
            let receiver = mutator.fetch(&item);
            let code = Rc::new(crate::bytecode::CompiledCode::new(
                crate::bytecode::CodeKind::Method,
                vec![crate::bytecode::Instruction::Return { register: 0 }],
                0,
                1,
                0,
                Vec::new(),
                ns_syntax::Span::new(0, 0),
            ));
            let activation = mutator.heap_mut().push_activation(
                ActivationHeader {
                    receiver,
                    current_class: None,
                    kind: FrameKind::TopLevel,
                    feedback_base: crate::interpreter::FeedbackBase::NOT_ASSIGNED,
                    code,
                    then: Continuation::Deliver,
                    counted: true,
                },
                &[receiver],
                0,
            );
            if let Err(diagnostic) = mutator.install_edit(item_id, &edit) {
                panic!("the install was refused: {diagnostic}");
            }
            let heap = mutator.heap();
            let item = mutator.fetch(&item).as_object().expect("an Item");
            assert_eq!(heap.instance_field_count(item), Some(2));
            let holder = mutator.fetch(&holder).as_object().expect("a Holder");
            let array = mutator.fetch(&array).as_object().expect("an Array");
            let weak = mutator.fetch(&weak).as_object().expect("a WeakArray");
            let ephemeron = mutator.fetch(&ephemeron).as_object().expect("an Ephemeron");
            let held = [
                ("a slot", heap.instance_field(holder, 0)),
                (
                    "an Array",
                    heap.array_elements(array)
                        .and_then(|elements| elements.first().copied()),
                ),
                ("a WeakArray", heap.weak_array_element(weak, 0)),
                ("an Ephemeron's key", heap.ephemeron_field(ephemeron, 0)),
                ("a receiver", Some(heap.activation_receiver(activation))),
                ("a register", Some(heap.activation_local(activation, 0))),
            ];
            for (holder, value) in held {
                assert!(
                    value
                        .and_then(|value| value.as_object())
                        .is_some_and(|object| object.same(item)),
                    "{holder} holds the new Item"
                );
            }
            mutator.heap_mut().pop_activation(activation);
        });
        let captured = send(&mut interpreter, &closure, "value");
        let same = interpreter.enter(|mutator| {
            let captured = mutator.fetch(&captured).as_object().expect("an Item");
            let item = mutator.fetch(&item).as_object().expect("an Item");
            captured.same(item)
        });
        assert!(same);
    }

    /// A closure made before the install runs the code it was made with;
    /// one made after runs the new method's.
    #[test]
    fn an_old_closure_keeps_its_code() {
        let (mut interpreter, _, _, inner, inner_id) = outer_and_inner(OUTER);
        let old_block = send(&mut interpreter, &inner, "block");
        install(
            &mut interpreter,
            inner_id,
            &add_method("public block = ( ^[ 2 ] )"),
        );
        let answer = send(&mut interpreter, &old_block, "value");
        assert_eq!(integer(&mut interpreter, &answer), Some(1));
        let new_block = send(&mut interpreter, &inner, "block");
        let answer = send(&mut interpreter, &new_block, "value");
        assert_eq!(integer(&mut interpreter, &answer), Some(2));
    }

    /// What a failed install must leave as it was: the current version of
    /// each id, each class object's application, each instance's guard.
    struct Snapshot {
        versions: Vec<Rc<Mixin>>,
        applications: Vec<Option<ApplicationIndex>>,
        guards: Vec<crate::mixin::DispatchId>,
    }

    fn snapshot(interpreter: &mut Interpreter, ids: &[MixinId], objects: &[&Root]) -> Snapshot {
        interpreter.enter(|mutator| {
            let heap = mutator.heap();
            Snapshot {
                versions: ids
                    .iter()
                    .map(|id| heap.mixin(*id).expect("a row"))
                    .collect(),
                applications: objects
                    .iter()
                    .map(|object| {
                        let object = mutator.fetch(object).as_object().expect("an object");
                        heap.is_class(object)
                            .then(|| heap.class_application_index(object))
                            .flatten()
                    })
                    .collect(),
                guards: objects
                    .iter()
                    .map(|object| heap.dispatch_id(mutator.fetch(object)))
                    .collect(),
            }
        })
    }

    /// Two edits that collide only together -- a method and a lazy slot of
    /// one name, a header slot and a method of one name -- are refused
    /// with the resolver's diagnostic, and the install leaves every
    /// version, application and guard as it was.
    #[test]
    fn a_failed_install_changes_nothing() {
        let (mut interpreter, outer_instance, inner_class, inner, inner_id) =
            outer_and_inner(OUTER);
        let class_side_id = interpreter.enter(|mutator| {
            let class_side = mutator
                .metaclass_mixins
                .get(&inner_id)
                .expect("Inner's metaclass synthesized its class side")
                .clone();
            mutator.heap.mixin_id(&class_side)
        });
        let ids = [inner_id, class_side_id];
        let objects = [&outer_instance, &inner_class, &inner];
        let before = snapshot(&mut interpreter, &ids, &objects);
        let colliding = instance_edit(vec![
            MemberEdit::AddMethod("public y = ( ^1 )".to_string()),
            MemberEdit::AddLazySlot("lazy y = 2.".to_string()),
        ]);
        // A send to a slot the header drops is late-bound, and resolves; a
        // header slot an added method collides with does not.
        let header_and_method = Edit {
            header: Some("public class Inner = (| public x ::= 1. public y = 2. |)".to_string()),
            instance_side: vec![MemberEdit::AddMethod("public y = ( ^3 )".to_string())],
            class_side: Vec::new(),
        };
        for edit in [colliding, header_and_method] {
            let diagnostic =
                match interpreter.enter(|mutator| mutator.install_edit(inner_id, &edit)) {
                    Ok(text) => panic!("the install succeeded: {text}"),
                    Err(diagnostic) => diagnostic,
                };
            assert!(
                diagnostic.starts_with("Inner's edited source does not resolve: "),
                "{diagnostic}"
            );
            let after = snapshot(&mut interpreter, &ids, &objects);
            for (old, new) in before.versions.iter().zip(&after.versions) {
                assert!(Rc::ptr_eq(old, new));
            }
            assert_eq!(before.applications, after.applications);
            assert!(before.guards == after.guards);
            interpreter.enter(|mutator| assert!(!mutator.heap.mixin_is_edited(inner_id)));
        }
        let answer = send(&mut interpreter, &inner, "a");
        assert_eq!(integer(&mut interpreter, &answer), Some(1));
    }

    /// An edit that leaves the class side undeclared installs a fresh
    /// synthesized one under the old one's id, which the metaclass's
    /// application names; the factory answers by its new selector.
    #[test]
    fn a_synthesized_class_side_keeps_its_id() {
        let (mut interpreter, _, inner_class, _, inner_id) = outer_and_inner(OUTER);
        let class_side_id = |interpreter: &mut Interpreter| {
            interpreter.enter(|mutator| {
                let class_side = mutator
                    .metaclass_mixins
                    .get(&inner_id)
                    .expect("Inner's metaclass synthesized its class side")
                    .clone();
                let metaclass_mixin = {
                    let class = mutator.fetch(&inner_class).as_object().expect("a class");
                    let metaclass = mutator.heap().object_class(class);
                    mutator
                        .heap()
                        .class_application(metaclass)
                        .expect("a metaclass")
                        .mixin
                        .clone()
                };
                assert!(Rc::ptr_eq(&class_side, &metaclass_mixin));
                mutator.heap().known_mixin_id(&class_side)
            })
        };
        let before = class_side_id(&mut interpreter);
        assert!(before.is_some());
        install(
            &mut interpreter,
            inner_id,
            &header_edit("public class Inner withX: aX = (| public x ::= aX. |)"),
        );
        assert_eq!(class_side_id(&mut interpreter), before);
        let three = integer_root(&mut interpreter, 3);
        let made = send_with(&mut interpreter, &inner_class, "withX:", &[three]);
        let slot_x = send(&mut interpreter, &made, "x");
        assert_eq!(integer(&mut interpreter, &slot_x), Some(3));
    }

    /// An initializer entered before an install that moved its level's
    /// slots reads its slot base from the level it was entered at, while
    /// its class object names the new application.
    #[test]
    fn a_class_object_s_current_application_may_differ_from_an_entered_level() {
        let source = indoc! {"
            class Outer = Object new () (
              public class Base = (| public a = 1. |) ()
              public class Sub = Base new (| public b = 2. |) ()
            )
        "};
        let (mut interpreter, outer) = load(source);
        let instance = expect_ok(interpreter.instantiate(&outer, &[]), "an Outer");
        let sub_class = send(&mut interpreter, &instance, "Sub");
        let sub = send(&mut interpreter, &sub_class, "new");
        let base_id = nested_id_of(&mut interpreter, &outer, "Base");
        let edit = header_edit("public class Base = (| public a = 1. public c = 3. |)");
        interpreter.enter(|mutator| {
            let class = mutator.fetch(&sub_class).as_object().expect("a class");
            let old = mutator.heap().class_application(class).expect("a class");
            let code = old
                .mixin
                .initializer_code
                .clone()
                .expect("Sub's slots need an initializer");
            assert!(code.addresses_own_slots);
            let receiver = mutator.fetch(&sub);
            let activation = mutator.heap_mut().push_activation(
                ActivationHeader {
                    receiver,
                    current_class: Some(class),
                    kind: FrameKind::Initializer(class),
                    feedback_base: crate::interpreter::FeedbackBase::NOT_ASSIGNED,
                    code,
                    then: Continuation::Deliver,
                    counted: true,
                },
                &[],
                0,
            );
            assert_eq!(mutator.heap().activation_slot_base(activation), 1);
            if let Err(diagnostic) = mutator.install_edit(base_id, &edit) {
                panic!("the install was refused: {diagnostic}");
            }
            let current = mutator.heap().class_application(class).expect("a class");
            assert_ne!(current.index(), old.index());
            assert_eq!(current.own_slot_base, 2);
            assert_eq!(mutator.heap().activation_slot_base(activation), 1);
            mutator.heap_mut().pop_activation(activation);
        });
    }
}
