//! Installing an edit of a class atomically: the commit after
//! `Mutator::prepare_edit` (`docs/design/liveness-and-reflection.md`, "The
//! install is one leaf, and its commit has no safepoint").

use std::rc::Rc;

use super::Mutator;
use super::edit::{Edit, EditedTexts, PreparedEdit};
use crate::hash::IdentityMap;
use crate::heap::{ClassReference, MixinId, ObjectReference};
use crate::mixin::{DispatchId, Mixin, MixinApplication};

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
    pub(crate) fn install_edit(&mut self, id: MixinId, edit: &Edit) -> Result<EditedTexts, String> {
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
        // Every module text an agent holds may be this edit's.
        self.breakpoints.raise_text_revision();
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
            super::inline_cache::vacate_every_cache(self, crate::trace_events::VacateCause::Edit);
        } else {
            debug_assert!(
                migration
                    .old
                    .iter()
                    .chain(&migration.new)
                    .all(|object| crate::builtins::reflection::may_be_forwarded(self, *object)),
                "internal: the install forwards only instances the forward accepts"
            );
            if crate::heap::forward_identity(
                self,
                &migration.old,
                &migration.new,
                crate::trace_events::VacateCause::Edit,
            )
            .is_err()
            {
                unreachable!("internal: each migrated instance is forwarded once, to a new one");
            }
        }
        // Once the commit is done: the copies keyed on an old application
        // were compiled against its layout and lookup, and no other copy
        // was -- the migration forwards only instances of the classes it
        // rebuilt, so that it frees what the bulk forward's leaf frees
        // only for them.
        let rebuilt: Vec<DispatchId> = old_applications
            .iter()
            .map(|application| application.dispatch_ids.instances)
            .filter(|id| *id != DispatchId::NONE)
            .collect();
        crate::jit::invalidate_applications(self, &rebuilt);
        // Every span counts in the module's text, which the edit changed.
        self.debug_metadata.clear();
        self.breakpoints.forget_filter_decisions();
        // The edited class's breakpoints stand on bodies the install
        // replaced: each is set again by its offset on the new ones.
        if !self.has_no_breakpoints() {
            let (module_root, _) = self.path_from_module_root(id);
            if let Some(root) = self.heap.mixin(module_root) {
                let module = crate::debugger::ModuleName::of_root(&root);
                crate::debugger::breakpoints::module_built(self, &module);
            }
        }
        Ok(prepared.texts)
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
    ///
    /// A defensive lock behind `may_be_forwarded`: no program instance is
    /// known to reach it, since a user subclass of `Array`, `String` or
    /// `Ephemeron` has `FixedPointers` instances, which the forward
    /// accepts. It is kept for any format a program brings that the
    /// forward refuses, one loaded from an image as much as one read from
    /// source, so that an install refuses before it replaces anything
    /// rather than meeting a refused forward inside its commit.
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
                    let same_nested = self
                        .heap
                        .known_mixin_id(old)
                        .map_or(&[][..], |id| self.heap.nested_of(id))
                        .iter()
                        .map(|nested_id| self.heap.mixin(*nested_id).map(|nested| nested.name))
                        .eq(fresh
                            .declared_nested()
                            .iter()
                            .map(|nested| Some(nested.name)));
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
    /// id. Each holds its own text, so the module's text, assembled from
    /// its declarations' current versions, holds the edit from here on.
    fn replace_versions(&mut self, prepared: &PreparedEdit) {
        let mut adopted = Vec::with_capacity(prepared.replacements.len());
        for (id, fresh) in &prepared.replacements {
            let children = self.replace_mixin(*id, Rc::clone(fresh));
            adopted.push((*id, children));
        }
        // After every replacement, so that each fresh child is known by
        // the id it was paired with and keeps it.
        for (id, children) in adopted {
            self.heap.adopt_nested(id, children);
        }
        // After the instance side's replacement, which forgets the class
        // side synthesized from its previous version; through the
        // mutator's replacement, which forgets the old class side's
        // applications, so a metaclass over the unchanged `Class` is not
        // answered the old one from the application cache.
        for (instance_side, class_side, fresh) in &prepared.synthesized {
            self.replace_mixin(*class_side, Rc::clone(fresh));
            self.metaclass_mixins
                .insert(*instance_side, Rc::clone(fresh));
        }
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
    /// layout and copies its fields into it by their `FieldName` -- a slot
    /// by its declaring level and its name -- leaving nil a field the edit
    /// added, or moved to another level; runs no initializer and sends
    /// nothing.
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
                let old_names = old.field_names(&self.heap);
                let new_names = new.field_names(&self.heap);
                (old_names != new_names).then(|| {
                    new_names
                        .iter()
                        .map(|name| old_names.iter().position(|old_name| old_name == name))
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
    use crate::interpreter::Arguments;
    use crate::interpreter::edit::MemberEdit;
    use crate::interpreter::{HostResult, HostSignal, Interpreter};
    use crate::mixin::Mixin;
    use crate::types::{ActivationKind, BuiltinClass, Continuation, SourceOrigin, Value};
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
            let mixin = mixin_of(mutator, class);
            let nested = mutator
                .heap
                .nested_mixins(&mixin)
                .into_iter()
                .find(|nested| nested.name.as_str() == name)
                .expect("the declaration is nested there");
            mutator.heap.mixin_id(&nested)
        })
    }

    fn install(interpreter: &mut Interpreter, id: MixinId, edit: &Edit) -> String {
        match interpreter.enter(|mutator| mutator.install_edit(id, edit)) {
            Ok(texts) => texts.declaration,
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

    // Images of an edited program

    /// `class`'s module written with its source, `roots` the graph's roots.
    fn image_of(interpreter: &mut Interpreter, class: &Root, roots: &[Root]) -> Vec<u8> {
        image_with(interpreter, class, roots, true)
    }

    fn image_with(
        interpreter: &mut Interpreter,
        class: &Root,
        roots: &[Root],
        with_source: bool,
    ) -> Vec<u8> {
        interpreter.enter(|mutator| {
            let class = mutator.fetch(class).as_object().expect("a class object");
            let name = mutator.heap.class_declaration(class).expect("a root").name;
            let identity = crate::modules::ModuleIdentity::undeclared("t.ns", name.as_str());
            let modules = [crate::image::ModuleSource {
                class,
                identity: &identity,
                with_source,
            }];
            let roots: Vec<Value<'_>> = roots.iter().map(|root| mutator.fetch(root)).collect();
            crate::image::snapshot(mutator, &modules, &roots).expect("snapshot")
        })
    }

    fn loaded(bytes: &[u8]) -> crate::image::LoadedImage {
        match crate::image::load(bytes) {
            Ok(image) => image,
            Err(message) => panic!("load: {message}"),
        }
    }

    /// `source` loaded and instantiated: the module class, its `Inner`
    /// class object, an `Inner` made before any edit, and `Inner`'s id.
    fn module_and_inner(source: &str) -> (Interpreter, Root, Root, Root, MixinId) {
        let (mut interpreter, outer) = load(source);
        let instance = expect_ok(interpreter.instantiate(&outer, &[]), "an Outer");
        let inner_class = send(&mut interpreter, &instance, "Inner");
        let inner = send(&mut interpreter, &inner_class, "new");
        let inner_id = nested_id_of(&mut interpreter, &outer, "Inner");
        (interpreter, outer, inner_class, inner, inner_id)
    }

    fn is_nil(interpreter: &mut Interpreter, root: &Root) -> bool {
        interpreter.enter(|mutator| mutator.fetch(root).is_nil())
    }

    fn contains(bytes: &[u8], needle: &str) -> bool {
        bytes
            .windows(needle.len())
            .any(|window| window == needle.as_bytes())
    }

    /// A module whose root class applies `Value` through its one `<:`
    /// clause.
    const ROOT_WITH_A_VALUE_CLAUSE: &str = indoc! {"
        class Root = Object new <: Value () (
          public a = ( ^1 )
        )
    "};

    /// An edit replaces the module's root mixin with a version compiled
    /// from the edited text, which is the same module: its identity, which
    /// the host set when it loaded the module, goes over to the new
    /// version, since an edit changes members and never the metadata above
    /// the class.
    #[test]
    fn an_installed_edit_keeps_the_identity() {
        let mut interpreter = Interpreter::bootstrap();
        let source = indoc! {"
            (* :module: com.acme.Outer@1.2.0 *)
            class Outer = Object new () (
              public a = ( ^1 )
            )
        "};
        let module = ns_syntax::parse(source).expect("parse");
        let mut resolved = crate::resolve(
            module,
            Rc::from(source),
            SourceOrigin::File(PathBuf::from("t.ns")),
        )
        .expect("resolve");
        let identity = Rc::new(crate::modules::ModuleIdentity {
            path: "libs/Outer.ns".to_string(),
            id: crate::modules::ModuleId::parse("com.acme.Outer", "Outer").expect("an id"),
            version: Some(crate::modules::Version::parse("1.2.0").expect("a version")),
        });
        resolved.identity = Some(Rc::clone(&identity));
        let outer = interpreter.load_module(&resolved);
        let (root_id, before) = interpreter.enter(|mutator| {
            let mixin = mixin_of(mutator, &outer);
            (mutator.heap.mixin_id(&mixin), mixin)
        });
        let loaded_identity = before.module_identity.clone().expect("the root has one");
        assert!(Rc::ptr_eq(&loaded_identity, &identity));
        install(&mut interpreter, root_id, &add_method("public b = ( ^2 )"));
        let after = interpreter.enter(|mutator| mixin_of(mutator, &outer));
        assert!(
            !Rc::ptr_eq(&before, &after),
            "the edit replaced the root mixin"
        );
        let kept = after
            .module_identity
            .clone()
            .expect("the new version has one");
        assert!(Rc::ptr_eq(&kept, &identity));
    }

    /// An edited nested class is written and loads edited: its added
    /// method answers on an instance made before the edit.
    // spec: image.write.edited
    #[test]
    fn an_edited_program_loads_edited() {
        let (mut interpreter, outer, _, inner, inner_id) = module_and_inner(OUTER);
        install(
            &mut interpreter,
            inner_id,
            &add_method("public b = ( ^x + 41 )"),
        );
        let bytes = image_of(&mut interpreter, &outer, &[inner]);
        let mut image = loaded(&bytes);
        let inner = image.roots[0].clone();
        let answer = send(&mut image.interpreter, &inner, "b");
        assert_eq!(integer(&mut image.interpreter, &answer), Some(42));
    }

    /// A slot added through the header is in the image's layout: an old
    /// instance migrated to `nil`, a new one initialized.
    // spec: image.write.edited
    #[test]
    fn a_header_edit_loads_with_its_layout() {
        let (mut interpreter, outer, inner_class, old, inner_id) = module_and_inner(OUTER);
        install(
            &mut interpreter,
            inner_id,
            &header_edit("public class Inner = (| public x ::= 1. public y ::= 2. |)"),
        );
        let new = send(&mut interpreter, &inner_class, "new");
        let bytes = image_of(&mut interpreter, &outer, &[old, new]);
        let mut image = loaded(&bytes);
        let (old, new) = (image.roots[0].clone(), image.roots[1].clone());
        let old_y = send(&mut image.interpreter, &old, "y");
        assert!(is_nil(&mut image.interpreter, &old_y));
        let new_y = send(&mut image.interpreter, &new, "y");
        assert_eq!(integer(&mut image.interpreter, &new_y), Some(2));
        let old_x = send(&mut image.interpreter, &old, "x");
        assert_eq!(integer(&mut image.interpreter, &old_x), Some(1));
    }

    /// After a growing edit of `Inner`, the loaded `Outer` still slices
    /// its method from its own text, and the loaded module's text holds
    /// the edit.
    // spec: image.write.source
    #[test]
    fn an_enclosing_method_keeps_its_source_through_an_image() {
        let (mut interpreter, outer, inner_class, _, inner_id) = module_and_inner(NESTED_AND_AFTER);
        install(
            &mut interpreter,
            inner_id,
            &add_method("public added = ( ^'a method that moves the text after it' )"),
        );
        let bytes = image_of(&mut interpreter, &outer, &[inner_class]);
        let mut image = loaded(&bytes);
        let outer = image.modules[0].class.clone();
        let inner_id = nested_id_of(&mut image.interpreter, &outer, "Inner");
        image.interpreter.enter(|mutator| {
            let outer_id = mutator
                .heap
                .known_mixin_id(&mixin_of(mutator, &outer))
                .expect("Outer is applied");
            assert_eq!(
                method_source_of(mutator, outer_id, "after").as_deref(),
                Some("public after = ( ^2 )")
            );
            assert_eq!(
                method_source_of(mutator, inner_id, "added").as_deref(),
                Some("public added = ( ^'a method that moves the text after it' )")
            );
        });
        let module = image.modules[0]
            .source
            .clone()
            .expect("the image carries source");
        assert!(module.contains("public added") && module.contains("public after = ( ^2 )"));
    }

    /// An image of an edited program holds each declaration's text once:
    /// the removed method's text is nowhere in it.
    // spec: image.write.source
    // spec: image.write.edited
    #[test]
    fn an_image_of_an_edited_program_holds_no_old_text() {
        let (mut interpreter, outer, inner_class, _, inner_id) = module_and_inner(NESTED_AND_AFTER);
        install(
            &mut interpreter,
            inner_id,
            &instance_edit(vec![MemberEdit::RemoveMethod("a".to_string())]),
        );
        let bytes = image_of(&mut interpreter, &outer, &[inner_class]);
        assert!(!contains(&bytes, "public a = ( ^1 )"));
        assert!(contains(&bytes, "public after = ( ^2 )"));
    }

    /// An edit of a module's root class whose `<:` clause applies `Value`
    /// is written: the clause's level names the root's old version, and
    /// the root rule holds by id.
    #[test]
    fn an_edited_root_with_a_clause_is_written() {
        let (mut interpreter, root) = load(ROOT_WITH_A_VALUE_CLAUSE);
        let instance = expect_ok(interpreter.instantiate(&root, &[]), "a Root");
        let root_id = interpreter.enter(|mutator| {
            let mixin = mixin_of(mutator, &root);
            mutator
                .heap
                .known_mixin_id(&mixin)
                .expect("the root is applied")
        });
        install(&mut interpreter, root_id, &add_method("public b = ( ^2 )"));
        let bytes = image_of(&mut interpreter, &root, &[instance]);
        let mut image = loaded(&bytes);
        let instance = image.roots[0].clone();
        let answer = send(&mut image.interpreter, &instance, "b");
        assert_eq!(integer(&mut image.interpreter, &answer), Some(2));
    }

    /// A reified mixin made before an edit names the edited declaration's
    /// row once written, and loads as the reified mixin of the edited
    /// version: the one the loaded module's class applies, with the method
    /// the edit added.
    // spec: image.write.edited
    #[test]
    fn a_reified_mixin_of_an_edited_declaration_loads_as_its_current_version() {
        let (mut interpreter, outer, _, _, inner_id) = module_and_inner(OUTER);
        let reified = interpreter.enter(|mutator| {
            let inner = mutator.heap.mixin(inner_id).expect("Inner is registered");
            let object = mutator.reified_mixin(&inner);
            mutator.stash(Value::object(object))
        });
        install(
            &mut interpreter,
            inner_id,
            &add_method("public b = ( ^x + 41 )"),
        );
        let bytes = image_of(&mut interpreter, &outer, &[reified]);
        let report = crate::image::inspect::inspect(&bytes).expect("inspect");
        let inner_row = report
            .mixins
            .iter()
            .position(|line| line.starts_with("Inner "))
            .expect("a row for Inner");
        assert!(
            report
                .graph_objects
                .iter()
                .any(|object| object.ends_with(&format!("mixin row {inner_row}"))),
            "the record names Inner's row"
        );
        let mut image = loaded(&bytes);
        let outer = image.modules[0].class.clone();
        let reified = image.roots[0].clone();
        let inner_id = nested_id_of(&mut image.interpreter, &outer, "Inner");
        image.interpreter.enter(|mutator| {
            let object = mutator
                .fetch(&reified)
                .as_object()
                .expect("a reified mixin");
            let mixin = mutator
                .mixin_of_reified(object)
                .expect("the root is a reified mixin");
            let current = mutator.heap.mixin(inner_id).expect("Inner is registered");
            assert!(Rc::ptr_eq(&mixin, &current), "the current version");
            assert!(mixin.compiled_method(Symbol::intern("b")).is_some());
        });
    }

    /// A closure made by a method an edit removed keeps its block across
    /// an image: the block and its home frame's body, the removed method's,
    /// enter the code table.
    // spec: image.write.edited
    #[test]
    fn a_closure_of_a_removed_method_runs_after_a_load() {
        let (mut interpreter, outer, _, inner, inner_id) = module_and_inner(OUTER);
        let block = send(&mut interpreter, &inner, "block");
        install(
            &mut interpreter,
            inner_id,
            &instance_edit(vec![MemberEdit::RemoveMethod("block".to_string())]),
        );
        let bytes = image_of(&mut interpreter, &outer, &[block]);
        let mut image = loaded(&bytes);
        let block = image.roots[0].clone();
        let answer = send(&mut image.interpreter, &block, "value");
        assert_eq!(integer(&mut image.interpreter, &answer), Some(1));
    }

    /// The same across a full collection before the snapshot: the
    /// superseded version whose code the closure runs is what admits that
    /// code into the image, so the collection keeps it while a closure
    /// runs its code, though nothing else holds it.
    #[cfg(debug_assertions)]
    #[test]
    fn a_closure_of_a_removed_method_reaches_an_image_after_a_collection() {
        let (mut interpreter, outer, _, inner, inner_id) = module_and_inner(OUTER);
        let block = send(&mut interpreter, &inner, "block");
        install(
            &mut interpreter,
            inner_id,
            &instance_edit(vec![MemberEdit::RemoveMethod("block".to_string())]),
        );
        interpreter.collect_now();
        let bytes = image_of(&mut interpreter, &outer, &[block]);
        let mut image = loaded(&bytes);
        let block = image.roots[0].clone();
        let answer = send(&mut image.interpreter, &block, "value");
        assert_eq!(integer(&mut image.interpreter, &answer), Some(1));
    }

    /// A replaced method's closure keeps the old block's answer after a
    /// load, while the method answers the new one.
    // spec: image.write.edited
    #[test]
    fn a_closure_of_a_replaced_method_keeps_its_old_code_after_a_load() {
        let (mut interpreter, outer, _, inner, inner_id) = module_and_inner(OUTER);
        let old_block = send(&mut interpreter, &inner, "block");
        install(
            &mut interpreter,
            inner_id,
            &add_method("public block = ( ^[ 2 ] )"),
        );
        let bytes = image_of(&mut interpreter, &outer, &[old_block, inner]);
        let mut image = loaded(&bytes);
        let (old_block, inner) = (image.roots[0].clone(), image.roots[1].clone());
        let old_answer = send(&mut image.interpreter, &old_block, "value");
        assert_eq!(integer(&mut image.interpreter, &old_answer), Some(1));
        let new_block = send(&mut image.interpreter, &inner, "block");
        let new_answer = send(&mut image.interpreter, &new_block, "value");
        assert_eq!(integer(&mut image.interpreter, &new_answer), Some(2));
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
            &instance_edit(vec![MemberEdit::RemoveMethod("a".to_string())]),
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

    // Nested declarations through ids

    /// After `Inner` was edited, `Outer`'s current version names `Inner` by
    /// id, so every reader reaches the edited version; the version `Outer`
    /// was built with holds no `Inner` any more, since registering it took
    /// its nested declarations.
    #[test]
    fn an_enclosing_mixin_reaches_its_nested_edit_through_the_id() {
        let (mut interpreter, outer) = load(OUTER);
        let inner = nested_id_of(&mut interpreter, &outer, "Inner");
        install(
            &mut interpreter,
            inner,
            &add_method("public added = ( ^42 )"),
        );
        interpreter.enter(|mutator| {
            let outer_mixin = mixin_of(mutator, &outer);
            let outer_id = mutator
                .heap
                .known_mixin_id(&outer_mixin)
                .expect("registered");
            assert!(outer_mixin.is_adopted());
            assert_eq!(mutator.heap.nested_of(outer_id), &[inner]);
            let current = mutator.heap.mixin(inner).expect("a row");
            assert!(current.compiled_method(Symbol::intern("added")).is_some());
        });
    }

    /// Registering a module's root registers every declaration nested in
    /// it, so no tree keeps an unregistered child.
    #[test]
    fn registering_a_root_registers_its_whole_tree() {
        let (mut interpreter, outer) = load(indoc! {"
            Newspeak3
            'Test'
            class Outer = (
            ) (
              class Middle = (
              ) (
                class Inner = (
                ) (
                )
              )
            )
        "});
        interpreter.enter(|mutator| {
            let outer_id = mutator
                .heap
                .known_mixin_id(&mixin_of(mutator, &outer))
                .expect("registered");
            let mut parent = outer_id;
            for name in ["Middle", "Inner"] {
                let children = mutator.heap.nested_of(parent).to_vec();
                assert_eq!(children.len(), 1, "{name} is the only declaration there");
                let child = children[0];
                let mixin = mutator.heap.mixin(child).expect("a row");
                assert_eq!(mixin.name.as_str(), name);
                assert!(mixin.is_adopted(), "{name} is registered");
                assert_eq!(mutator.heap.enclosing_mixin_id(child), Some(parent));
                parent = child;
            }
            assert!(mutator.heap.nested_of(parent).is_empty());
        });
    }

    /// The nested-class accessor of an enclosing class edited before its
    /// nested class reaches the nested class's later edit: the lookup reads
    /// the nested declaration through its id (`dispatch.rs`), not the
    /// version the enclosing class's current version was built with.
    #[test]
    fn the_nested_class_accessor_reaches_the_edited_version() {
        let (mut interpreter, outer) = load(OUTER);
        let outer_id = interpreter.enter(|mutator| {
            let outer_mixin = mixin_of(mutator, &outer);
            mutator.heap.mixin_id(&outer_mixin)
        });
        install(
            &mut interpreter,
            outer_id,
            &add_method("public innerAdded = ( ^Inner new added )"),
        );
        let inner = nested_id_of(&mut interpreter, &outer, "Inner");
        install(
            &mut interpreter,
            inner,
            &add_method("public added = ( ^42 )"),
        );
        let instance = expect_ok(interpreter.instantiate(&outer, &[]), "an Outer");
        let answer = send(&mut interpreter, &instance, "innerAdded");
        assert_eq!(integer(&mut interpreter, &answer), Some(42));
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

    /// A slot a subclass gains, shadowing its superclass's slot of the
    /// same name, starts nil, and the shadowed slot keeps its value: a
    /// field migrates by its declaring level and its name, so the new
    /// slot never takes the superclass's field.
    #[test]
    fn a_shadowed_slot_keeps_its_value() {
        let source = indoc! {"
            class Outer = Object new () (
              public class Base = (| public b ::= 2. public c ::= 3. |) ()
              public class Inner = Base (| public c ::= 4. |) (
                public superB = ( ^super b )
                public superC = ( ^super c )
              )
            )
        "};
        let (mut interpreter, _, _, inner, inner_id) = outer_and_inner(source);
        install(
            &mut interpreter,
            inner_id,
            &header_edit("public class Inner = Base (| public b ::= 7. public c ::= 4. |)"),
        );
        let own_b = send(&mut interpreter, &inner, "b");
        let super_b = send(&mut interpreter, &inner, "superB");
        let own_c = send(&mut interpreter, &inner, "c");
        let super_c = send(&mut interpreter, &inner, "superC");
        let is_nil = interpreter.enter(|mutator| mutator.fetch(&own_b).is_nil());
        assert!(is_nil, "the new shadowing slot starts nil");
        assert_eq!(integer(&mut interpreter, &super_b), Some(2));
        assert_eq!(integer(&mut interpreter, &own_c), Some(4));
        assert_eq!(integer(&mut interpreter, &super_c), Some(3));
    }

    /// A chain that applies one mixin twice has two levels whose slots
    /// share the mixin and the name; a layout-changing edit keeps each
    /// level's field with its own value, since a slot is named by its
    /// mixin's occurrence in the chain as well.
    #[test]
    fn a_mixin_applied_twice_keeps_both_levels_slots() {
        let source = indoc! {"
            class Outer = Object new () (
              class Marker tag: value = Object new (| public markerTag = value. |) ()
              class Middle = Object new () ()
              public class Inner = Object new <: Marker tag: 1 <: Middle <: Marker tag: 2 (| public own = 5. |) ()
            )
        "};
        let (mut interpreter, _, _, inner, inner_id) = outer_and_inner(source);
        install(
            &mut interpreter,
            inner_id,
            &header_edit(
                "public class Inner = Object new <: Marker tag: 1 <: Middle <: Marker tag: 2 (| public own = 5. public added = 6. |)",
            ),
        );
        let fields: Vec<Option<i64>> = interpreter.enter(|mutator| {
            let object = mutator.fetch(&inner).as_object().expect("an Inner");
            (0..4)
                .map(|index| {
                    mutator
                        .heap()
                        .instance_field(object, index)
                        .and_then(|value| value.as_small_integer())
                })
                .collect()
        });
        assert_eq!(fields, vec![Some(1), Some(2), Some(5), None]);
        let tag = send(&mut interpreter, &inner, "markerTag");
        assert_eq!(integer(&mut interpreter, &tag), Some(2));
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
        let before = snapshot(&mut interpreter, &[edited_id, failure_id], &[]);

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
        // Both ids name a fresh version, each of its own install: `Edited`
        // the first edit's, `Failure` the second's.
        let after = snapshot(&mut interpreter, &[edited_id, failure_id], &[]);
        for (old, new) in before.versions.iter().zip(&after.versions) {
            assert!(!Rc::ptr_eq(old, new));
        }
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
                1,
                1,
                0,
                Vec::new(),
                ns_syntax::Span::new(0, 0),
            ));
            let activation = mutator
                .heap_mut()
                .push_activation(
                    ActivationHeader {
                        receiver,
                        current_class: None,
                        kind: ActivationKind::TopLevel,
                        method: None,
                        feedback_base: crate::interpreter::FeedbackBase::NOT_ASSIGNED,
                        code,
                        then: Continuation::Deliver,
                        counted: true,
                    },
                    Arguments::Values(&[receiver].as_slice()),
                    0,
                )
                .expect("the stack holds the frame");
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
                    Ok(texts) => panic!("the install succeeded: {}", texts.declaration),
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
                .own_initializer_code
                .clone()
                .expect("Sub's slots need an initializer");
            assert!(code.addresses_own_slots);
            let receiver = mutator.fetch(&sub);
            let activation = mutator
                .heap_mut()
                .push_activation(
                    ActivationHeader {
                        receiver,
                        current_class: Some(class),
                        kind: ActivationKind::Initializer(
                            class,
                            crate::interpreter::instantiate::InitializerBody::OwnPart,
                        ),
                        method: None,
                        feedback_base: crate::interpreter::FeedbackBase::NOT_ASSIGNED,
                        code,
                        then: Continuation::Deliver,
                        counted: true,
                    },
                    Arguments::NONE,
                    0,
                )
                .expect("the stack holds the frame");
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

    /// `Outer`'s and `Inner`'s method source, sliced from each version's
    /// own text.
    fn method_source_of(mutator: &Mutator<'_>, id: MixinId, selector: &str) -> Option<String> {
        let mixin = mutator.heap.mixin(id)?;
        let method = mixin.compiled_method(Symbol::intern(selector))?;
        mixin
            .text
            .as_ref()?
            .slice(method.code.source)
            .map(str::to_string)
    }

    const NESTED_AND_AFTER: &str = indoc! {"
        Newspeak3
        'Test'
        class Outer = Object new () (
          (* Inner's comment *)
          public class Inner = (| public x ::= 1. |) (
            public a = ( ^1 )
          )
          public after = ( ^2 )
        )
    "};

    /// An edit that grows `Inner` leaves `Outer`'s text as it was read, so
    /// `Outer`'s method still slices; the module's text holds the edit.
    #[test]
    fn an_enclosing_declaration_keeps_its_text_across_a_nested_edit() {
        let (mut interpreter, outer) = load(NESTED_AND_AFTER);
        let _instance = expect_ok(interpreter.instantiate(&outer, &[]), "an Outer");
        let inner_id = nested_id_of(&mut interpreter, &outer, "Inner");
        install(
            &mut interpreter,
            inner_id,
            &add_method("public added = ( ^'a method that moves the text after it' )"),
        );
        interpreter.enter(|mutator| {
            let outer_id = mutator
                .heap
                .known_mixin_id(&mixin_of(mutator, &outer))
                .expect("Outer is applied");
            assert_eq!(
                method_source_of(mutator, outer_id, "after").as_deref(),
                Some("public after = ( ^2 )")
            );
            assert_eq!(
                method_source_of(mutator, inner_id, "added").as_deref(),
                Some("public added = ( ^'a method that moves the text after it' )")
            );
            let module = mutator
                .heap
                .module_source(outer_id)
                .expect("the module has text");
            assert!(module.starts_with("Newspeak3\n'Test'\n"));
            assert!(module.contains("public added") && module.contains("(* Inner's comment *)"));
            assert_eq!(
                mutator.heap.declaration_source(outer_id),
                module
                    .find("class Outer")
                    .map(|start| module[start..].trim_end().to_string())
            );
        });
    }

    /// After the edit, no text the module's mixins hold contains `Inner`'s
    /// old members: the old version is held by the table of superseded
    /// versions alone, never by its enclosing declaration's text.
    #[test]
    fn an_edit_leaves_no_old_text_in_the_enclosing_declaration() {
        let (mut interpreter, outer) = load(NESTED_AND_AFTER);
        let _instance = expect_ok(interpreter.instantiate(&outer, &[]), "an Outer");
        let inner_id = nested_id_of(&mut interpreter, &outer, "Inner");
        install(
            &mut interpreter,
            inner_id,
            &instance_edit(vec![MemberEdit::RemoveMethod("a".to_string())]),
        );
        interpreter.enter(|mutator| {
            let outer_mixin = mixin_of(mutator, &outer);
            let outer_text = outer_mixin.text.as_ref().expect("Outer has text");
            assert!(!outer_text.cut_text().contains("public a = ( ^1 )"));
            let inner = mutator.heap.mixin(inner_id).expect("Inner is registered");
            let inner_text = inner.text.as_ref().expect("Inner has text");
            assert!(!inner_text.cut_text().contains("public a = ( ^1 )"));
        });
    }

    /// An unedited module assembles to the text it was read in, byte for
    /// byte: prologue, comments around and between members, a class side,
    /// non-ASCII text and declarations three deep.
    #[test]
    fn an_unedited_module_assembles_to_its_text() {
        let source = indoc! {"
            Newspeak3
            'Test'
            (* the module's comment *)
            class Outer = Object new () (
              (* \u{e4}\u{1F600} *)
              public class Middle = () (
                public class Innermost = () ( public c = ( ^'\u{e9}' ) )
                public b = ( ^2 )
              ) : ( public made = ( ^self new ) )
              public a = ( ^1 )
            ) : (
              public marker = ( ^7 )
            )
            (* trailing *)
        "};
        let (mut interpreter, outer) = load(source);
        let _instance = expect_ok(interpreter.instantiate(&outer, &[]), "an Outer");
        interpreter.enter(|mutator| {
            let root = mixin_of(mutator, &outer);
            assert_eq!(mutator.heap.assembled_text(&root).as_deref(), Some(source));
        });
    }

    /// A program loaded from an image with source is edited like one
    /// compiled from source, and its instances migrate.
    #[test]
    fn a_loaded_program_is_edited() {
        let (mut interpreter, outer, _, inner, _) = module_and_inner(OUTER);
        let bytes = image_of(&mut interpreter, &outer, &[inner]);
        let mut image = loaded(&bytes);
        let outer = image.modules[0].class.clone();
        let inner = image.roots[0].clone();
        let inner_id = nested_id_of(&mut image.interpreter, &outer, "Inner");
        install(
            &mut image.interpreter,
            inner_id,
            &header_edit("public class Inner = (| public x ::= 1. public y ::= 5. |)"),
        );
        let x_value = send(&mut image.interpreter, &inner, "x");
        assert_eq!(integer(&mut image.interpreter, &x_value), Some(1));
        let y_value = send(&mut image.interpreter, &inner, "y");
        assert!(is_nil(&mut image.interpreter, &y_value));
    }

    /// A loaded module's root declaration is edited like a nested one: the
    /// edit answers on an instance made before the image, and the module's
    /// text keeps its prologue and its nested declaration, so a second
    /// edit parses it again.
    #[test]
    fn a_loaded_root_is_edited() {
        let (mut interpreter, outer) = load(NESTED_AND_AFTER);
        let instance = expect_ok(interpreter.instantiate(&outer, &[]), "an Outer");
        let bytes = image_of(&mut interpreter, &outer, &[instance]);
        let mut image = loaded(&bytes);
        let outer = image.modules[0].class.clone();
        let instance = image.roots[0].clone();
        let root_id = image.interpreter.enter(|mutator| {
            mutator
                .heap
                .known_mixin_id(&mixin_of(mutator, &outer))
                .expect("the root is applied")
        });
        install(
            &mut image.interpreter,
            root_id,
            &add_method("public b = ( ^41 )"),
        );
        let answer = send(&mut image.interpreter, &instance, "b");
        assert_eq!(integer(&mut image.interpreter, &answer), Some(41));
        install(
            &mut image.interpreter,
            root_id,
            &add_method("public c = ( ^b + 1 )"),
        );
        let answer = send(&mut image.interpreter, &instance, "c");
        assert_eq!(integer(&mut image.interpreter, &answer), Some(42));
        image.interpreter.enter(|mutator| {
            let module = mutator
                .heap
                .module_source(root_id)
                .expect("the module has text");
            assert!(module.starts_with("Newspeak3\n'Test'\n"), "{module}");
            assert!(module.contains("public b = ( ^41 )"));
            assert!(module.contains("public c = ( ^b + 1 )"));
            assert!(module.contains("(* Inner's comment *)"));
            assert_eq!(
                method_source_of(mutator, root_id, "after").as_deref(),
                Some("public after = ( ^2 )")
            );
        });
    }

    /// Edited before an image and again after its load, written again and
    /// loaded again: the second image holds both edits.
    // spec: image.write.edited
    #[test]
    fn a_loaded_program_edited_and_written_again_loads_with_both_edits() {
        let (mut interpreter, outer, _, inner, inner_id) = module_and_inner(OUTER);
        install(&mut interpreter, inner_id, &add_method("public b = ( ^2 )"));
        let first = image_of(&mut interpreter, &outer, &[inner]);
        let mut first = loaded(&first);
        let outer = first.modules[0].class.clone();
        let inner = first.roots[0].clone();
        let inner_id = nested_id_of(&mut first.interpreter, &outer, "Inner");
        install(
            &mut first.interpreter,
            inner_id,
            &add_method("public c = ( ^3 )"),
        );
        let second = image_of(&mut first.interpreter, &outer, &[inner]);
        let mut second = loaded(&second);
        let inner = second.roots[0].clone();
        let b_value = send(&mut second.interpreter, &inner, "b");
        assert_eq!(integer(&mut second.interpreter, &b_value), Some(2));
        let c_value = send(&mut second.interpreter, &inner, "c");
        assert_eq!(integer(&mut second.interpreter, &c_value), Some(3));
        let text = second.modules[0]
            .source
            .clone()
            .expect("the image carries source");
        assert!(text.contains("public b = ( ^2 )") && text.contains("public c = ( ^3 )"));
    }

    /// An image written without source: the edit answers 5b2a's
    /// diagnostic and changes nothing.
    #[test]
    fn a_loaded_program_without_source_is_refused() {
        let (mut interpreter, outer, _, inner, _) = module_and_inner(OUTER);
        let bytes = image_with(&mut interpreter, &outer, &[inner], false);
        let mut image = loaded(&bytes);
        let outer = image.modules[0].class.clone();
        let inner_id = nested_id_of(&mut image.interpreter, &outer, "Inner");
        let refused = image
            .interpreter
            .enter(|mutator| mutator.install_edit(inner_id, &add_method("public b = ( ^2 )")));
        match refused {
            Err(message) => assert_eq!(message, "Inner has no source text to edit"),
            Ok(_) => panic!("a program without source was edited"),
        }
    }

    /// A crafted image whose text holds other classes than its code: the
    /// edit is refused with the existing diagnostic, never a panic.
    #[test]
    fn a_loaded_program_whose_text_lacks_the_declaration_is_refused() {
        let (mut interpreter, outer, _, inner, _) = module_and_inner(OUTER);
        let mut bytes = image_of(&mut interpreter, &outer, &[inner]);
        // `Inner`'s own text, swapped for text of the same length that
        // declares a class of another name: the text table is
        // length-prefixed, so the swap keeps every other byte, and the
        // module assembles into a valid module without `Inner`.
        let inner_text = "class Inner = (| public x ::= 1. |)";
        let swapped = "class Other = (| public x ::= 1. |)";
        let start = bytes
            .windows(inner_text.len())
            .position(|window| window == inner_text.as_bytes())
            .expect("the image carries Inner's text");
        bytes[start..start + inner_text.len()].copy_from_slice(swapped.as_bytes());
        let mut image = loaded(&bytes);
        let outer = image.modules[0].class.clone();
        let inner_id = nested_id_of(&mut image.interpreter, &outer, "Inner");
        let refused = image
            .interpreter
            .enter(|mutator| mutator.install_edit(inner_id, &add_method("public b = ( ^2 )")));
        match refused {
            Err(message) => {
                assert_eq!(message, "Inner's source text does not hold its declaration");
            }
            Ok(_) => panic!("an edit was installed from a text without its declaration"),
        }
    }
}
