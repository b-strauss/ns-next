//! Installing the edits of several builders atomically: the commit after
//! `Mutator::prepare_install` (`docs/design/liveness-and-reflection.md`,
//! "The install is one leaf, and its commit has no safepoint", "Planned:
//! the complete install").

pub(super) mod prepared;
pub(super) mod superclasses;

use std::rc::Rc;

use super::Mutator;
use super::edit::{EditedTexts, InstallEntry};
use crate::hash::IdentityMap;
use crate::heap::reclaim::StaleIds;
use crate::heap::{ClassReference, MixinId, ObjectReference};
use crate::mixin::{DispatchId, Mixin, MixinApplication};
use crate::types::Value;
use prepared::PreparedInstall;
use superclasses::Reparenting;

/// What the migration answers: the instances it replaced, their copies in
/// the new layout at the same positions, and the instances it kept.
struct Migration<'gc> {
    old: Vec<ObjectReference<'gc>>,
    new: Vec<ObjectReference<'gc>>,
    kept: Vec<ObjectReference<'gc>>,
}

impl<'gc> Mutator<'gc> {
    /// Answers each entry's edited declaration's texts, in order, or a
    /// diagnostic with nothing changed.
    ///
    /// Refuses an `epoch` an install or a module load has moved since the
    /// preparation that answered it; takes the prepared install the
    /// leaves cached for the same entries and epoch, or prepares them
    /// again; and finds what the install reaches, which is where
    /// everything that can fail fails. Then commits, once for every
    /// module's replacements together, without running Newspeak code,
    /// sending to a user object or polling a safepoint, so that no program
    /// observes a half-installed edit (`docs/design/liveness-and-reflection.md`,
    /// "The install is one leaf, and its commit has no safepoint"): every
    /// fresh mixin under its old id, every class object found re-applied
    /// superclasses first -- keeping its identity, as every class object
    /// does, a class object whose superclass clause changed over what the
    /// new clause answered -- the instances whose layout changed copied
    /// into the new one by name and forwarded in one bulk forward, the
    /// others restamped; and raises the epoch.
    ///
    /// `superclasses` pairs each class object whose new superclass clause
    /// the preparation asked to evaluate (`Mutator::clause_class_objects`)
    /// with what the clause answered. A class object of such a declaration
    /// the pairs lack was made while the clauses ran, and refuses the
    /// install, as do an answer that is no class and one that would make a
    /// class inherit from itself (`superclasses`).
    ///
    /// An install of no entries answers at once, installing nothing and
    /// leaving the epoch, so that it fails no install it runs within.
    pub(crate) fn commit_install(
        &mut self,
        entries: &[InstallEntry],
        epoch: u64,
        superclasses: &[(ClassReference<'gc>, Value<'gc>)],
    ) -> Result<Vec<EditedTexts>, String> {
        if entries.is_empty() {
            return Ok(Vec::new());
        }
        let prepared = self.prepared_for_commit(entries, epoch)?;
        let mut reparentings = self.clause_reparentings(&prepared, superclasses)?;
        // Before any walk of a superclass chain, each of which assumes it
        // ends.
        self.refuse_a_cycle(&reparentings)?;
        let mut fresh_by_id: IdentityMap<MixinId, Rc<Mixin>> = IdentityMap::default();
        for (replaced, fresh) in &prepared.replacements {
            fresh_by_id.insert(*replaced, Rc::clone(fresh));
        }
        for (_, class_side, fresh) in &prepared.synthesized {
            fresh_by_id.insert(*class_side, Rc::clone(fresh));
        }
        reparentings.extend(self.root_reparentings(&prepared));
        let reparented: IdentityMap<usize, usize> = reparentings
            .iter()
            .enumerate()
            .map(|(index, reparenting)| (self.heap.object_identity_key(reparenting.class), index))
            .collect();
        let (classes, instances) = self.heap.classes_and_instances_over(
            |candidate| fresh_by_id.contains_key(&candidate),
            |class| reparented.contains_key(&self.heap.object_identity_key(class)),
        );
        let class_positions = self.class_positions(&classes);
        self.refuse_an_unmigratable_instance(
            &classes,
            &class_positions,
            &instances,
            &fresh_by_id,
            &reparented,
        )?;

        // The commit: nothing below fails.
        let old_applications: Vec<Rc<MixinApplication>> = classes
            .iter()
            .map(|class| {
                self.heap
                    .class_application(*class)
                    .expect("internal: the walk collects class objects")
            })
            .collect();
        let stale = self.stale_after(&prepared, &old_applications);
        self.replace_versions(&prepared);
        // Every module text an agent holds may be this edit's.
        self.breakpoints.raise_text_revision();
        let new_applications =
            self.reapply(&classes, &old_applications, &reparentings, &reparented);
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
            // every entry decided for the old ones is stale, and only
            // those: no other lookup changed.
            super::inline_cache::vacate_stale(self, &stale);
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
                crate::heap::Invalidation::Stale(&stale),
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
        let rebuilt: Vec<DispatchId> = stale.guards.iter().copied().collect();
        crate::jit::invalidate_applications(self, &rebuilt);
        // Every span counts in the module's text, which the edit changed.
        self.debug_metadata.clear();
        self.breakpoints.forget_filter_decisions();
        // The edited classes' breakpoints stand on bodies the install
        // replaced: each is set again by its offset on the new ones.
        if !self.has_no_breakpoints() {
            let mut module_roots: Vec<MixinId> = Vec::new();
            for entry in entries {
                if let InstallEntry::Edit(id, _) = entry {
                    let (module_root, _) = self.path_from_module_root(*id);
                    if !module_roots.contains(&module_root) {
                        module_roots.push(module_root);
                    }
                }
            }
            for module_root in module_roots {
                if let Some(root) = self.heap.mixin(module_root) {
                    let module = crate::debugger::ModuleName::of_root(&root);
                    crate::debugger::breakpoints::module_built(self, &module);
                }
            }
        }
        // A preparation of the world before this commit is stale: the
        // epoch tells a commit so, and the one cached is dropped.
        self.install_epoch += 1;
        self.prepared_install = None;
        // The install stress mode: every entry the scoped vacate kept is
        // what a lookup decides now, and every cell still copies its entry.
        #[cfg(debug_assertions)]
        if self.stress_install {
            let checked = self
                .verify_caches_against_lookups()
                .and_then(|()| self.caches.verify_cells(&self.heap, &self.jit));
            if let Err(problem) = checked {
                panic!("internal: after an install, {problem}");
            }
        }
        Ok(prepared.texts.clone())
    }

    /// Prepares and commits `entries`, as the install leaves do between
    /// them when no clause is evaluated.
    #[cfg(test)]
    pub(crate) fn install_entries(
        &mut self,
        entries: &[InstallEntry],
    ) -> Result<Vec<EditedTexts>, String> {
        if entries.is_empty() {
            return self.commit_install(entries, self.install_epoch, &[]);
        }
        let epoch = self.prepare_install(entries)?.epoch;
        self.commit_install(entries, epoch, &[])
    }

    /// Installs `edit` of the declaration `id` alone, as a builder's
    /// `install` does.
    #[cfg(test)]
    pub(crate) fn install_one(
        &mut self,
        id: MixinId,
        edit: &super::edit::Edit,
    ) -> Result<EditedTexts, String> {
        let mut texts = self.install_entries(&[InstallEntry::Edit(id, edit.clone())])?;
        Ok(texts.remove(0))
    }

    /// Refuses `replacements` when a module root's new version carries an
    /// identity the loader would refuse beside the world's other modules
    /// (`docs/design/modules.md`, decision 5): two of one id and one
    /// version, or two of one id where either has none, with the loader's
    /// message naming both paths, earlier row first. Only the ids an
    /// install changes are compared, so an install never answers for a
    /// pair of modules it did not touch.
    pub(super) fn refuse_a_colliding_identity(
        &self,
        replacements: &[(MixinId, Rc<Mixin>)],
    ) -> Result<(), String> {
        use crate::modules::ModuleIdentity;
        let changed: Vec<(Rc<ModuleIdentity>, Rc<ModuleIdentity>)> = replacements
            .iter()
            .filter(|(id, _)| self.heap.enclosing_mixin_id(*id).is_none())
            .filter_map(|(id, fresh)| {
                let old = self.heap.mixin(*id)?.module_identity.clone()?;
                let new = fresh.module_identity.clone()?;
                (!Rc::ptr_eq(&old, &new)).then_some((old, new))
            })
            .collect();
        if changed.is_empty() {
            return Ok(());
        }
        let identities: Vec<Rc<ModuleIdentity>> = self
            .heap
            .module_identities()
            .into_iter()
            .map(|identity| {
                changed
                    .iter()
                    .find(|(old, _)| Rc::ptr_eq(old, &identity))
                    .map_or(identity, |(_, new)| Rc::clone(new))
            })
            .filter(|identity| changed.iter().any(|(_, new)| new.id == identity.id))
            .collect();
        crate::modules::check_distinct(identities.iter().map(Rc::as_ref))
    }

    /// What the commit of `prepared` makes stale, read before it replaces
    /// anything: the guard and the index of every application it
    /// re-applies, `old_applications`, and every declaration's version it
    /// replaces. Whatever a lookup an install changes found was decided
    /// for one of these, so that every other cache entry and optimized
    /// copy stays warm (`docs/design/dispatch.md`, "Invalidation").
    fn stale_after(
        &self,
        prepared: &PreparedInstall,
        old_applications: &[Rc<MixinApplication>],
    ) -> StaleIds {
        let mut stale = StaleIds::default();
        for application in old_applications {
            let guard = application.dispatch_ids.instances;
            if guard != DispatchId::NONE {
                stale.guards.insert(guard);
            }
            stale.applications.insert(application.index());
        }
        stale.declarations = prepared
            .replacements
            .iter()
            .map(|(id, _)| *id)
            .chain(
                prepared
                    .synthesized
                    .iter()
                    .map(|(_, class_side, _)| *class_side),
            )
            .filter_map(|id| self.heap.mixin(id))
            .collect();
        stale
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
    /// another slot list or another nested list, or a level of its chain is
    /// re-parented (`reparented`), so no instance the commit migrates
    /// escapes the check.
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
        reparented: &IdentityMap<usize, usize>,
    ) -> Result<(), String> {
        let may_change: Vec<bool> = classes
            .iter()
            .map(|class| self.layout_may_change(*class, fresh_by_id, reparented))
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
    /// version replacing it, or a level of the chain is re-parented.
    fn layout_may_change(
        &self,
        class: ClassReference<'gc>,
        fresh_by_id: &IdentityMap<MixinId, Rc<Mixin>>,
        reparented: &IdentityMap<usize, usize>,
    ) -> bool {
        let mut level = Some(class);
        while let Some(current) = level {
            if reparented.contains_key(&self.heap.object_identity_key(current)) {
                return true;
            }
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
    fn replace_versions(&mut self, prepared: &PreparedInstall) {
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
        debug_assert!(
            prepared.added.iter().all(|(enclosing, fresh)| {
                self.heap.known_mixin_id(fresh).is_some_and(|id| {
                    self.heap.enclosing_mixin_id(id) == Some(*enclosing)
                        && self.heap.nested_of(*enclosing).contains(&id)
                })
            }),
            "internal: an added declaration is registered under the one it is added to"
        );
        // Its parent's nested row no longer names it, so the parent's new
        // applications reserve it no hidden slot; its class objects and
        // instances keep the application they have.
        for id in &prepared.removed {
            self.heap.make_obsolete(*id);
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
    /// its superclass clause is not evaluated again. A class object of
    /// `reparentings` (`reparented`, by identity key) gets its new
    /// superclass first and its `<:` levels built anew over it
    /// (`nested::build_applied_levels`); once any chain may have changed,
    /// every nested class object has §3.1.1's verdict decided again.
    /// Records an old level whose slot base and own slots the edit kept as
    /// succeeded by the new one, so an activation entered at it passes the
    /// slot check. Answers the new applications, by position.
    fn reapply(
        &mut self,
        classes: &[ClassReference<'gc>],
        old_applications: &[Rc<MixinApplication>],
        reparentings: &[Reparenting<'gc>],
        reparented: &IdentityMap<usize, usize>,
    ) -> Vec<Rc<MixinApplication>> {
        // Before the sort, so that a re-parented class object sorts after
        // its new superclass and every class object over it after it.
        for reparenting in reparentings {
            if !self
                .heap
                .class_set_superclass(reparenting.class, reparenting.superclass)
            {
                unreachable!("internal: a re-parented class is a class object");
            }
        }
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
            let mut superclass = self.heap.class_superclass(class).and_then(|superclass| {
                self.heap
                    .class_application(superclass)
                    .map(|application| (superclass, application))
            });
            let id = self
                .heap
                .known_mixin_id(&old.mixin)
                .expect("internal: an applied mixin is registered");
            let current = self
                .heap
                .mixin(id)
                .expect("internal: a registered mixin has a row");
            let reparenting = reparented
                .get(&self.heap.object_identity_key(class))
                .map(|index| &reparentings[*index]);
            if let Some(reparenting) = reparenting
                && let Some(declaration) = reparenting.body_less
            {
                new_applications[position] =
                    Some(self.reparent_body_less(class, declaration, reparenting, superclass));
                continue;
            }
            if let Some(reparenting) = reparenting
                && !reparenting.applied.is_empty()
            {
                // Every applied class was found a class object before the
                // commit began (`Mutator::clause_reparentings`).
                let Ok(top) = super::nested::build_applied_levels(
                    self,
                    &current,
                    superclass,
                    &reparenting.applied,
                ) else {
                    unreachable!("internal: every applied class is a class object");
                };
                if let Some((level, _)) = &top
                    && !self.heap.class_set_superclass(class, *level)
                {
                    unreachable!("internal: a re-parented class is a class object");
                }
                superclass = top;
            }
            let new = self.apply_mixin(
                &current,
                superclass.as_ref().map(|(_, application)| application),
            );
            if !self.heap.class_install_application(class, Rc::clone(&new)) {
                unreachable!("internal: the walk collects class objects");
            }
            if !reparentings.is_empty() && self.heap.class_this_class(class).is_none() {
                self.redecide_value_class(class, new.inherits_value);
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
    use crate::interpreter::edit::{Edit, MemberEdit};
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
        match interpreter.enter(|mutator| mutator.install_one(id, edit)) {
            Ok(texts) => texts.declaration,
            Err(diagnostic) => panic!("the install was refused: {diagnostic}"),
        }
    }

    fn instance_edit(operations: Vec<MemberEdit>) -> Edit {
        Edit {
            header: None,
            instance_side: operations,
            class_side: Vec::new(),
            new_name: None,
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
            new_name: None,
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
    /// version, the same `Rc`, since the edit leaves the id and version its
    /// text declares as they were.
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

    /// `source` loaded into `interpreter` as the module at `path` with
    /// the id and version its `:module:` declares.
    fn load_identified(interpreter: &mut Interpreter, source: &str, path: &str) -> Root {
        let module = ns_syntax::parse(source).expect("parse");
        let (id, version) =
            crate::modules::declared_identity(&module, source).expect("a declared identity");
        let mut resolved = crate::resolve(
            module,
            Rc::from(source),
            SourceOrigin::File(PathBuf::from(path)),
        )
        .expect("resolve");
        resolved.identity = Some(Rc::new(crate::modules::ModuleIdentity {
            path: path.to_string(),
            id,
            version,
        }));
        interpreter.load_module(&resolved)
    }

    fn identity_of(
        interpreter: &mut Interpreter,
        class: &Root,
    ) -> Rc<crate::modules::ModuleIdentity> {
        interpreter.enter(|mutator| {
            mixin_of(mutator, class)
                .module_identity
                .clone()
                .expect("the root has an identity")
        })
    }

    /// A rename of an annotated root is the same module under another id:
    /// its mixin id, its path and its class object are kept, and its
    /// `:module:` id ends in the new name.
    // spec: runtime.install
    #[test]
    fn a_root_rename_changes_the_identity_in_place() {
        let mut interpreter = Interpreter::bootstrap();
        let source = indoc! {"
            (* :module: com.acme.Json@1.2.0 *)
            class Json = Object new () (
              public a = ( ^1 )
            )
        "};
        let json = load_identified(&mut interpreter, source, "libs/Json.ns");
        let instance = expect_ok(interpreter.instantiate(&json, &[]), "a Json");
        let root_id = interpreter.enter(|mutator| {
            let mixin = mixin_of(mutator, &json);
            mutator.heap.mixin_id(&mixin)
        });
        let edit = Edit {
            header: None,
            instance_side: Vec::new(),
            class_side: Vec::new(),
            new_name: Some(Symbol::intern("JsonCodec")),
        };
        let texts = match interpreter.enter(|mutator| mutator.install_one(root_id, &edit)) {
            Ok(texts) => texts,
            Err(diagnostic) => panic!("the install was refused: {diagnostic}"),
        };
        assert_eq!(
            texts.header,
            "(* :module: com.acme.JsonCodec@1.2.0 *)\nclass JsonCodec = Object new () "
        );
        let identity = identity_of(&mut interpreter, &json);
        assert_eq!(identity.path, "libs/Json.ns");
        assert_eq!(identity.display_name(), "com.acme.JsonCodec@1.2.0");
        interpreter.enter(|mutator| {
            let mixin = mixin_of(mutator, &json);
            assert_eq!(mutator.heap.known_mixin_id(&mixin), Some(root_id));
            assert_eq!(mixin.name.as_str(), "JsonCodec");
            let object = mutator.fetch(&instance).as_object().expect("an instance");
            let class = mutator.heap.object_class(object);
            let json = mutator.fetch(&json).as_object().expect("a class object");
            assert_eq!(
                mutator.heap.object_identity_key(class),
                mutator.heap.object_identity_key(json)
            );
        });
        let answer = send(&mut interpreter, &instance, "a");
        assert_eq!(integer(&mut interpreter, &answer), Some(1));
    }

    /// An identity another module of the world has is refused with the
    /// loader's message, and the module keeps its version and identity.
    // spec: runtime.install
    #[test]
    fn a_colliding_identity_changes_nothing() {
        let mut interpreter = Interpreter::bootstrap();
        let first = load_identified(
            &mut interpreter,
            indoc! {"
                (* :module: com.acme.Util@1.0.0 *)
                class Util = Object new () ()
            "},
            "v1/Util.ns",
        );
        load_identified(
            &mut interpreter,
            indoc! {"
                (* :module: com.acme.Util@2.0.0 *)
                class Util = Object new () ()
            "},
            "v2/Util.ns",
        );
        let (root_id, before) = interpreter.enter(|mutator| {
            let mixin = mixin_of(mutator, &first);
            (mutator.heap.mixin_id(&mixin), mixin)
        });
        let identity = identity_of(&mut interpreter, &first);
        let edit = header_edit("(* :module: com.acme.Util@2.0.0 *)\nclass Util = Object new () ");
        let refused = interpreter.enter(|mutator| mutator.install_one(root_id, &edit));
        match refused {
            Ok(_) => panic!("the colliding identity was installed"),
            Err(diagnostic) => assert_eq!(
                diagnostic,
                "two modules are `com.acme.Util@2.0.0`: v1/Util.ns and v2/Util.ns"
            ),
        }
        interpreter.enter(|mutator| {
            assert!(Rc::ptr_eq(&mixin_of(mutator, &first), &before));
        });
        assert!(Rc::ptr_eq(
            &identity_of(&mut interpreter, &first),
            &identity
        ));
        let malformed =
            header_edit("(* :module: com.acme.Util@1.0 *)\nclass Util = Object new () ");
        let refused = interpreter.enter(|mutator| mutator.install_one(root_id, &malformed));
        match refused {
            Ok(_) => panic!("the malformed version was installed"),
            Err(diagnostic) => assert!(
                diagnostic.starts_with("`1.0` is no version: "),
                "{diagnostic}"
            ),
        }
        let reserved = header_edit("(* :module: newspeak.Util *)\nclass Util = Object new () ");
        let refused = interpreter.enter(|mutator| mutator.install_one(root_id, &reserved));
        match refused {
            Ok(_) => panic!("the reserved identity was installed"),
            Err(diagnostic) => assert!(diagnostic.contains("lies in the namespace `newspeak.`")),
        }
    }

    /// A rename to the id of a module that declares no version is
    /// refused, whatever version the renamed one declares, with the
    /// loader's message naming both paths.
    // spec: runtime.install
    #[test]
    fn a_rename_to_a_versionless_module_s_id_changes_nothing() {
        let mut interpreter = Interpreter::bootstrap();
        let util = load_identified(
            &mut interpreter,
            indoc! {"
                (* :module: com.acme.Util@1.0.0 *)
                class Util = Object new () ()
            "},
            "a/Util.ns",
        );
        load_identified(
            &mut interpreter,
            indoc! {"
                (* :module: com.acme.Tool *)
                class Tool = Object new () ()
            "},
            "b/Tool.ns",
        );
        let (root_id, before) = interpreter.enter(|mutator| {
            let mixin = mixin_of(mutator, &util);
            (mutator.heap.mixin_id(&mixin), mixin)
        });
        let identity = identity_of(&mut interpreter, &util);
        let edit = Edit {
            header: None,
            instance_side: Vec::new(),
            class_side: Vec::new(),
            new_name: Some(Symbol::intern("Tool")),
        };
        match interpreter.enter(|mutator| mutator.install_one(root_id, &edit)) {
            Ok(_) => panic!("the colliding rename was installed"),
            Err(diagnostic) => assert_eq!(
                diagnostic,
                "two modules are `com.acme.Tool` and one of them has no version: a/Util.ns and \
                 b/Tool.ns"
            ),
        }
        interpreter.enter(|mutator| {
            assert!(Rc::ptr_eq(&mixin_of(mutator, &util), &before));
        });
        assert!(Rc::ptr_eq(&identity_of(&mut interpreter, &util), &identity));
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
    // spec: vm.heap.id-reclaimed-only-when-unnamed
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
                new_name: None,
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
            if let Err(diagnostic) = mutator.install_one(item_id, &edit) {
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

    /// Whether `snapshot` still holds of `ids` and `objects`: the same
    /// version of each id, application of each class object and guard of
    /// each object.
    fn assert_unchanged(
        interpreter: &mut Interpreter,
        before: &Snapshot,
        ids: &[MixinId],
        objects: &[&Root],
    ) {
        let after = snapshot(interpreter, ids, objects);
        for (old, new) in before.versions.iter().zip(&after.versions) {
            assert!(Rc::ptr_eq(old, new), "a version changed");
        }
        assert_eq!(before.applications, after.applications);
        assert!(before.guards == after.guards, "a guard changed");
    }

    /// The id of the module root `class` applies.
    fn root_id_of(interpreter: &mut Interpreter, class: &Root) -> MixinId {
        interpreter.enter(|mutator| {
            let mixin = mixin_of(mutator, class);
            mutator.heap.mixin_id(&mixin)
        })
    }

    /// A refused install's diagnostic.
    fn install_refusal(interpreter: &mut Interpreter, entries: &[InstallEntry]) -> String {
        match interpreter.enter(|mutator| mutator.install_entries(entries)) {
            Ok(_) => panic!("the install succeeded"),
            Err(diagnostic) => diagnostic,
        }
    }

    /// Two entries of one module, of `Outer` and of its nested `Inner`,
    /// compose on one text in their order: both methods answer, the
    /// module's text holds both, and each entry answers its own
    /// declaration's text.
    // spec: semantics.reflection.atomic-install
    // spec: runtime.install
    #[test]
    fn two_edits_of_one_module_compose_on_one_text() {
        let (mut interpreter, outer_instance, _, inner, inner_id) = outer_and_inner(OUTER);
        let outer_id = interpreter.enter(|mutator| {
            mutator
                .heap
                .enclosing_mixin_id(inner_id)
                .expect("Inner is nested in Outer")
        });
        let entries = [
            InstallEntry::Edit(outer_id, add_method("public fromOuter = ( ^10 )")),
            InstallEntry::Edit(inner_id, add_method("public fromInner = ( ^x + 19 )")),
        ];
        let texts = match interpreter.enter(|mutator| mutator.install_entries(&entries)) {
            Ok(texts) => texts,
            Err(diagnostic) => panic!("the install was refused: {diagnostic}"),
        };
        assert_eq!(texts.len(), 2);
        assert!(texts[0].declaration.starts_with("class Outer"));
        assert!(texts[0].declaration.contains("public fromOuter = ( ^10 )"));
        assert!(
            texts[0]
                .declaration
                .contains("public fromInner = ( ^x + 19 )")
        );
        assert!(texts[1].declaration.starts_with("public class Inner"));
        assert!(
            texts[1]
                .declaration
                .contains("public fromInner = ( ^x + 19 )")
        );
        assert!(!texts[1].declaration.contains("fromOuter"));
        let from_outer = send(&mut interpreter, &outer_instance, "fromOuter");
        assert_eq!(integer(&mut interpreter, &from_outer), Some(10));
        let from_inner = send(&mut interpreter, &inner, "fromInner");
        assert_eq!(integer(&mut interpreter, &from_inner), Some(20));
        interpreter.enter(|mutator| {
            let module = mutator
                .heap
                .module_source(outer_id)
                .expect("the module has text");
            assert!(module.contains("public fromOuter = ( ^10 )"), "{module}");
            assert!(
                module.contains("public fromInner = ( ^x + 19 )"),
                "{module}"
            );
        });
    }

    /// Two entries of one declaration are refused, naming it, before any
    /// text is read, and nothing changes.
    // spec: semantics.reflection.atomic-install
    #[test]
    fn two_edits_of_one_declaration_are_refused() {
        let (mut interpreter, outer_instance, inner_class, inner, inner_id) =
            outer_and_inner(OUTER);
        let objects = [&outer_instance, &inner_class, &inner];
        let before = snapshot(&mut interpreter, &[inner_id], &objects);
        let entries = [
            InstallEntry::Edit(inner_id, add_method("public b = ( ^2 )")),
            InstallEntry::Edit(inner_id, add_method("public c = ( ^3 )")),
        ];
        assert_eq!(
            install_refusal(&mut interpreter, &entries),
            "two builders edit Inner"
        );
        assert_unchanged(&mut interpreter, &before, &[inner_id], &objects);
        assert!(interpreter.send(&inner, Symbol::intern("b"), &[]).is_err());
    }

    /// An entry that fails fails the whole install: the second module's
    /// syntax error leaves the first module's method uninstalled and every
    /// version, application and guard as it was.
    // spec: semantics.reflection.atomic-install
    // spec: runtime.install
    #[test]
    fn a_failed_entry_fails_the_whole_install() {
        let (mut interpreter, outer_instance, inner_class, inner, inner_id) =
            outer_and_inner(OUTER);
        let other = load_into(
            &mut interpreter,
            indoc! {"
                class Other = Object new () (
                  public a = ( ^1 )
                )
            "},
        );
        let other_instance = expect_ok(interpreter.instantiate(&other, &[]), "an Other");
        let other_id = root_id_of(&mut interpreter, &other);
        let ids = [inner_id, other_id];
        let objects = [&outer_instance, &inner_class, &inner, &other_instance];
        let before = snapshot(&mut interpreter, &ids, &objects);
        let entries = [
            InstallEntry::Edit(inner_id, add_method("public b = ( ^2 )")),
            InstallEntry::Edit(other_id, add_method("public broken = ( ^ )")),
        ];
        let diagnostic = install_refusal(&mut interpreter, &entries);
        assert!(
            diagnostic.starts_with("Other's edited source does not parse: "),
            "{diagnostic}"
        );
        assert_unchanged(&mut interpreter, &before, &ids, &objects);
        assert!(interpreter.send(&inner, Symbol::intern("b"), &[]).is_err());
        interpreter.enter(|mutator| {
            let module = mutator
                .heap
                .module_source(other_id)
                .expect("the module has text");
            assert!(!module.contains("broken"));
        });
    }

    /// A commit whose epoch an install moved since its preparation is
    /// refused, naming the first entry's declaration, and installs
    /// nothing: the world stays as the other install left it.
    // spec: semantics.reflection.atomic-install
    // spec: runtime.install
    #[test]
    fn a_commit_after_another_install_is_refused() {
        let (mut interpreter, outer_instance, inner_class, inner, inner_id) =
            outer_and_inner(OUTER);
        let outer_id = interpreter.enter(|mutator| {
            mutator
                .heap
                .enclosing_mixin_id(inner_id)
                .expect("Inner is nested in Outer")
        });
        let entries = [InstallEntry::Edit(
            inner_id,
            add_method("public b = ( ^2 )"),
        )];
        let epoch = match interpreter.enter(|mutator| mutator.prepare_install(&entries)) {
            Ok(prepared) => prepared.epoch,
            Err(diagnostic) => panic!("the preparation was refused: {diagnostic}"),
        };
        install(
            &mut interpreter,
            outer_id,
            &add_method("public between = ( ^7 )"),
        );
        let ids = [inner_id, outer_id];
        let objects = [&outer_instance, &inner_class, &inner];
        let before = snapshot(&mut interpreter, &ids, &objects);
        let refused = interpreter.enter(|mutator| mutator.commit_install(&entries, epoch, &[]));
        match refused {
            Ok(_) => panic!("a stale commit installed"),
            Err(diagnostic) => assert_eq!(
                diagnostic,
                "an install ran between the preparation and the commit of Inner's install; \
                 nothing was installed"
            ),
        }
        assert_unchanged(&mut interpreter, &before, &ids, &objects);
        assert!(interpreter.send(&inner, Symbol::intern("b"), &[]).is_err());
        let between = send(&mut interpreter, &outer_instance, "between");
        assert_eq!(integer(&mut interpreter, &between), Some(7));
    }

    /// The epoch rises at every commit and every module load, and neither
    /// at a preparation nor at a refused install.
    // spec: runtime.install
    #[test]
    fn the_epoch_rises_at_a_commit_and_a_module_load() {
        let (mut interpreter, _, _, _, inner_id) = outer_and_inner(OUTER);
        let epoch =
            |interpreter: &mut Interpreter| interpreter.enter(|mutator| mutator.install_epoch);
        let loaded = epoch(&mut interpreter);
        let entries = [InstallEntry::Edit(
            inner_id,
            add_method("public b = ( ^2 )"),
        )];
        let prepared = interpreter.enter(|mutator| {
            mutator
                .prepare_install(&entries)
                .map(|prepared| prepared.epoch)
        });
        assert_eq!(prepared.ok(), Some(loaded));
        assert_eq!(epoch(&mut interpreter), loaded);
        install_refusal(
            &mut interpreter,
            &[InstallEntry::Edit(inner_id, add_method("public c = ( ^ )"))],
        );
        assert_eq!(epoch(&mut interpreter), loaded);
        install(&mut interpreter, inner_id, &add_method("public b = ( ^2 )"));
        assert_eq!(epoch(&mut interpreter), loaded + 1);
        load_into(
            &mut interpreter,
            indoc! {"
                class Other = Object new () ()
            "},
        );
        assert_eq!(epoch(&mut interpreter), loaded + 2);
    }

    /// A preparation of the entries and the epoch the cache holds answers
    /// the cached one, which a commit drops.
    // spec: runtime.install
    #[test]
    fn a_preparation_is_cached_until_a_commit() {
        let (mut interpreter, _, _, _, inner_id) = outer_and_inner(OUTER);
        let entries = [InstallEntry::Edit(
            inner_id,
            add_method("public b = ( ^2 )"),
        )];
        let prepare = |interpreter: &mut Interpreter| match interpreter
            .enter(|mutator| mutator.prepare_install(&entries))
        {
            Ok(prepared) => prepared,
            Err(diagnostic) => panic!("the preparation was refused: {diagnostic}"),
        };
        let first = prepare(&mut interpreter);
        let second = prepare(&mut interpreter);
        assert!(Rc::ptr_eq(&first, &second));
        let other = [InstallEntry::Edit(
            inner_id,
            add_method("public c = ( ^3 )"),
        )];
        if let Err(diagnostic) = interpreter.enter(|mutator| mutator.install_entries(&other)) {
            panic!("the install was refused: {diagnostic}");
        }
        assert!(interpreter.enter(|mutator| mutator.prepared_install.is_none()));
        let third = prepare(&mut interpreter);
        assert!(!Rc::ptr_eq(&first, &third));
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
            new_name: None,
        };
        for edit in [colliding, header_and_method] {
            let diagnostic = match interpreter.enter(|mutator| mutator.install_one(inner_id, &edit))
            {
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
            if let Err(diagnostic) = mutator.install_one(base_id, &edit) {
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
            .enter(|mutator| mutator.install_one(inner_id, &add_method("public b = ( ^2 )")));
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
            .enter(|mutator| mutator.install_one(inner_id, &add_method("public b = ( ^2 )")));
        match refused {
            Err(message) => {
                assert_eq!(message, "Inner's source text does not hold its declaration");
            }
            Ok(_) => panic!("an edit was installed from a text without its declaration"),
        }
    }

    // Nested classes added, replaced and removed

    fn add_class(text: &str) -> Edit {
        instance_edit(vec![MemberEdit::AddClass(text.to_string())])
    }

    fn remove_class(name: &str) -> Edit {
        instance_edit(vec![MemberEdit::RemoveClass(name.to_string())])
    }

    /// The id of the declaration `id` is nested in.
    fn enclosing_of(interpreter: &mut Interpreter, id: MixinId) -> MixinId {
        interpreter.enter(|mutator| {
            mutator
                .heap
                .enclosing_mixin_id(id)
                .expect("the declaration is nested")
        })
    }

    /// The message of the error a send of `selector` to `receiver` raises.
    fn send_error(interpreter: &mut Interpreter, receiver: &Root, selector: &str) -> String {
        match interpreter.send(receiver, Symbol::intern(selector), &[]) {
            Ok(_) => panic!("{selector} was understood"),
            Err(HostSignal::Return) => panic!("{selector}: a non-local return escaped"),
            Err(HostSignal::Error(error)) => error.message,
        }
    }

    /// A removed class lives on obsolete: an instance and the class object
    /// made before the install keep running its last version, the enclosing
    /// class no longer understands its name, its row stays and is obsolete,
    /// and the module's text no longer holds it.
    // spec: runtime.install
    // spec: vm.heap.obsolete-mixin-out-of-tree
    #[test]
    fn a_removed_class_runs_on_obsolete() {
        let (mut interpreter, outer_instance, inner_class, inner, inner_id) =
            outer_and_inner(OUTER);
        let outer_id = enclosing_of(&mut interpreter, inner_id);
        install(&mut interpreter, outer_id, &remove_class("Inner"));
        let answer = send(&mut interpreter, &inner, "a");
        assert_eq!(integer(&mut interpreter, &answer), Some(1));
        let made = send(&mut interpreter, &inner_class, "new");
        let slot = send(&mut interpreter, &made, "x");
        assert_eq!(integer(&mut interpreter, &slot), Some(1));
        let message = send_error(&mut interpreter, &outer_instance, "Inner");
        assert!(message.contains("does not understand #Inner"), "{message}");
        interpreter.enter(|mutator| {
            assert!(mutator.heap.is_obsolete(inner_id));
            assert!(!mutator.heap.is_obsolete(outer_id));
            assert!(!mutator.heap.nested_of(outer_id).contains(&inner_id));
            assert_eq!(mutator.heap.enclosing_mixin_id(inner_id), Some(outer_id));
            let module = mutator.heap.module_source(outer_id).expect("Outer's text");
            assert!(!module.contains("class Inner"), "{module}");
            let tree = mutator
                .heap
                .module_trees()
                .into_iter()
                .find(|tree| tree.contains(&inner_id))
                .expect("Inner has a tree");
            assert_eq!(tree[0], inner_id, "Inner roots a tree of its own");
            assert!(!tree.contains(&outer_id));
        });
    }

    /// The id of the mixin the class object `class` applies, which the
    /// heap registered when it applied it.
    fn applied_id_of(interpreter: &mut Interpreter, class: &Root) -> MixinId {
        interpreter.enter(|mutator| {
            let mixin = mixin_of(mutator, class);
            mutator
                .heap
                .known_mixin_id(&mixin)
                .expect("an applied mixin is registered")
        })
    }

    /// Whether any mixin `interpreter`'s heap names is called `name`.
    fn names_a_mixin(interpreter: &mut Interpreter, name: &str) -> bool {
        interpreter.enter(|mutator| {
            mutator.heap.mixin_ids().into_iter().any(|id| {
                mutator
                    .heap
                    .mixin(id)
                    .is_some_and(|mixin| mixin.name.as_str() == name)
            })
        })
    }

    /// A removed class an image reaches is written as a tree of its own
    /// and loads obsolete: its instance answers its last version, its
    /// class object makes more and a block of its method runs, while it
    /// stands in no module's tree and its former module's text does not
    /// hold it. Written again from the loaded world, it loads obsolete
    /// again.
    // spec: image.load.obsolete
    // spec: image.mixins
    // spec: image.write
    #[test]
    fn an_obsolete_class_loads_obsolete() {
        let (mut interpreter, outer, inner_class, inner, inner_id) = module_and_inner(OUTER);
        let outer_id = enclosing_of(&mut interpreter, inner_id);
        install(&mut interpreter, outer_id, &remove_class("Inner"));
        let block = send(&mut interpreter, &inner, "block");
        let outer_instance = expect_ok(interpreter.instantiate(&outer, &[]), "an Outer");
        let mut bytes = image_of(
            &mut interpreter,
            &outer,
            &[inner, inner_class, block, outer_instance],
        );
        for round in ["first", "second"] {
            let mut image = loaded(&bytes);
            let [inner, inner_class, block, outer_instance] = image.roots.as_slice() else {
                panic!("{round}: four roots");
            };
            let (inner, inner_class, block, outer_instance) = (
                inner.clone(),
                inner_class.clone(),
                block.clone(),
                outer_instance.clone(),
            );
            let answer = send(&mut image.interpreter, &inner, "a");
            assert_eq!(integer(&mut image.interpreter, &answer), Some(1), "{round}");
            let made = send(&mut image.interpreter, &inner_class, "new");
            let slot = send(&mut image.interpreter, &made, "x");
            assert_eq!(integer(&mut image.interpreter, &slot), Some(1), "{round}");
            let value = send(&mut image.interpreter, &block, "value");
            assert_eq!(integer(&mut image.interpreter, &value), Some(1), "{round}");
            let message = send_error(&mut image.interpreter, &outer_instance, "Inner");
            assert!(message.contains("does not understand #Inner"), "{message}");
            let loaded_inner = applied_id_of(&mut image.interpreter, &inner_class);
            let loaded_outer = root_id_of(&mut image.interpreter, &image.modules[0].class);
            image.interpreter.enter(|mutator| {
                assert!(mutator.heap.is_obsolete(loaded_inner), "{round}");
                assert!(!mutator.heap.is_obsolete(loaded_outer), "{round}");
                assert_eq!(mutator.heap.enclosing_mixin_id(loaded_inner), None);
                assert!(!mutator.heap.nested_of(loaded_outer).contains(&loaded_inner));
                let module = mutator
                    .heap
                    .module_source(loaded_outer)
                    .expect("Outer's text");
                assert!(!module.contains("class Inner"), "{round}: {module}");
                let inner_mixin = mutator.heap.mixin(loaded_inner).expect("Inner's row");
                let own = mutator
                    .heap
                    .assembled_text(&inner_mixin)
                    .expect("Inner keeps its text");
                assert!(own.contains("public a = ( ^1 )"), "{round}: {own}");
            });
            let module = image.modules[0]
                .source
                .clone()
                .expect("the image has source");
            assert!(!module.contains("class Inner"), "{round}: {module}");
            bytes = image_of(
                &mut image.interpreter,
                &image.modules[0].class,
                &[inner, inner_class, block, outer_instance],
            );
        }
    }

    /// A subclass in another module of a removed class, and a class nested
    /// in the removed one, load with it: the removed class's tree is
    /// written whole, after both modules' trees, and the subclass's
    /// instance inherits its last version.
    // spec: image.load.obsolete
    #[test]
    fn a_subclass_in_another_module_of_an_obsolete_class_loads() {
        let source = indoc! {"
            class Outer = Object new () (
              public class Inner = (| public x ::= 1. |) (
                public class Deep = () (
                  public d = ( ^4 )
                )
                public a = ( ^1 )
              )
            )
        "};
        let (mut interpreter, outer, inner_class, inner, inner_id) = module_and_inner(source);
        let deep_class = send(&mut interpreter, &inner, "Deep");
        let deep = send(&mut interpreter, &deep_class, "new");
        let other_source = indoc! {"
            class Other usingInner: innerClass = Object new (
              | public inner = innerClass. |
            ) (
              public class Sub = inner () (
                public answer = ( ^a + 1 )
              )
            )
        "};
        let other = load_into(&mut interpreter, other_source);
        let other_instance = expect_ok(
            interpreter.instantiate(&other, std::slice::from_ref(&inner_class)),
            "an Other",
        );
        let sub_class = send(&mut interpreter, &other_instance, "Sub");
        let sub = send(&mut interpreter, &sub_class, "new");
        let outer_id = enclosing_of(&mut interpreter, inner_id);
        install(&mut interpreter, outer_id, &remove_class("Inner"));
        let bytes = interpreter.enter(|mutator| {
            let outer_identity = crate::modules::ModuleIdentity::undeclared("t.ns", "Outer");
            let other_identity = crate::modules::ModuleIdentity::undeclared("other.ns", "Other");
            let class = |root: &Root| mutator.fetch(root).as_object().expect("a class object");
            let modules = [
                crate::image::ModuleSource {
                    class: class(&outer),
                    identity: &outer_identity,
                    with_source: true,
                },
                crate::image::ModuleSource {
                    class: class(&other),
                    identity: &other_identity,
                    with_source: true,
                },
            ];
            let roots = [
                mutator.fetch(&sub),
                mutator.fetch(&deep),
                mutator.fetch(&deep_class),
            ];
            crate::image::snapshot(mutator, &modules, &roots).expect("snapshot")
        });
        let mut image = loaded(&bytes);
        let [sub, deep, deep_class] = image.roots.as_slice() else {
            panic!("three roots");
        };
        let (sub, deep, deep_class) = (sub.clone(), deep.clone(), deep_class.clone());
        let answer = send(&mut image.interpreter, &sub, "answer");
        assert_eq!(integer(&mut image.interpreter, &answer), Some(2));
        let answer = send(&mut image.interpreter, &deep, "d");
        assert_eq!(integer(&mut image.interpreter, &answer), Some(4));
        let deep_id = applied_id_of(&mut image.interpreter, &deep_class);
        image.interpreter.enter(|mutator| {
            assert!(
                mutator.heap.is_obsolete(deep_id),
                "Deep is obsolete through Inner"
            );
            let root = mutator
                .heap
                .obsolete_root(deep_id)
                .expect("Deep stands in an obsolete tree");
            assert_eq!(
                mutator.heap.mixin(root).expect("Inner's row").name.as_str(),
                "Inner"
            );
            assert_eq!(mutator.heap.enclosing_mixin_id(root), None);
        });
        assert_eq!(image.modules.len(), 2);
    }

    /// A removed class nothing of the graph reaches is not written: the
    /// image holds no row of it and none of its text, though the world
    /// that wrote it still holds it.
    // spec: image.write
    #[test]
    fn an_unreachable_obsolete_class_is_not_written() {
        let (mut interpreter, outer, _inner_class, _inner, inner_id) = module_and_inner(OUTER);
        let outer_id = enclosing_of(&mut interpreter, inner_id);
        install(&mut interpreter, outer_id, &remove_class("Inner"));
        let outer_instance = expect_ok(interpreter.instantiate(&outer, &[]), "an Outer");
        assert!(names_a_mixin(&mut interpreter, "Inner"));
        let bytes = image_of(&mut interpreter, &outer, &[outer_instance]);
        assert!(!contains(&bytes, "public a = ( ^1 )"));
        let mut image = loaded(&bytes);
        assert!(!names_a_mixin(&mut image.interpreter, "Inner"));
    }

    /// A class added to an enclosing declaration gets a fresh id declared
    /// in it, and an existing instance of the enclosing class answers it.
    // spec: runtime.install
    #[test]
    fn an_added_class_gets_an_id_under_its_enclosing_one() {
        let (mut interpreter, outer_instance, _, _, inner_id) = outer_and_inner(OUTER);
        let outer_id = enclosing_of(&mut interpreter, inner_id);
        let edit = add_class("public class Added = () (\n  public b = ( ^2 )\n)");
        let prepared = interpreter.enter(|mutator| {
            mutator.prepare_entries(&[InstallEntry::Edit(outer_id, edit.clone())])
        });
        let prepared = match prepared {
            Ok(prepared) => prepared,
            Err(diagnostic) => panic!("the install was refused: {diagnostic}"),
        };
        let [(enclosing, fresh)] = prepared.added.as_slice() else {
            panic!("one class is added");
        };
        assert_eq!(*enclosing, outer_id);
        assert_eq!(fresh.name.as_str(), "Added");
        assert!(prepared.removed.is_empty());
        install(&mut interpreter, outer_id, &edit);
        let added_class = send(&mut interpreter, &outer_instance, "Added");
        let added = send(&mut interpreter, &added_class, "new");
        let answer = send(&mut interpreter, &added, "b");
        assert_eq!(integer(&mut interpreter, &answer), Some(2));
        interpreter.enter(|mutator| {
            let nested = mutator.heap.nested_of(outer_id).to_vec();
            assert_eq!(nested.len(), 2);
            assert_eq!(nested[0], inner_id);
            let added_id = nested[1];
            assert_ne!(added_id, inner_id);
            assert_eq!(mutator.heap.enclosing_mixin_id(added_id), Some(outer_id));
            let added = mutator.heap.mixin(added_id).expect("Added's row");
            assert_eq!(added.name.as_str(), "Added");
        });
    }

    /// An `addFromSource:` of an existing name replaces the class, keeping
    /// its id, its class objects and its instances, which migrate: a kept
    /// slot keeps its value, an added one starts nil.
    // spec: runtime.install
    #[test]
    fn a_replaced_nested_class_keeps_its_id_and_class_objects() {
        let (mut interpreter, outer_instance, inner_class, inner, inner_id) =
            outer_and_inner(OUTER);
        let outer_id = enclosing_of(&mut interpreter, inner_id);
        let five = integer_root(&mut interpreter, 5);
        drop(send_with(&mut interpreter, &inner, "x:", &[five]));
        let edit = add_class(
            "public class Inner = (| public x ::= 1. public y ::= 3. |) (\n  public a = ( ^9 )\n)",
        );
        let prepared = interpreter.enter(|mutator| {
            mutator.prepare_entries(&[InstallEntry::Edit(outer_id, edit.clone())])
        });
        match prepared {
            Ok(prepared) => assert!(prepared.added.is_empty() && prepared.removed.is_empty()),
            Err(diagnostic) => panic!("the install was refused: {diagnostic}"),
        }
        install(&mut interpreter, outer_id, &edit);
        let again = send(&mut interpreter, &outer_instance, "Inner");
        let same = interpreter.enter(|mutator| {
            let key = |root: &Root| {
                let object = mutator.fetch(root).as_object().expect("a class object");
                mutator.heap.object_identity_key(object)
            };
            key(&again) == key(&inner_class)
        });
        assert!(
            same,
            "the accessor answers the class object it answered before"
        );
        let answer = send(&mut interpreter, &inner, "a");
        assert_eq!(integer(&mut interpreter, &answer), Some(9));
        let kept = send(&mut interpreter, &inner, "x");
        assert_eq!(integer(&mut interpreter, &kept), Some(5));
        let added = send(&mut interpreter, &inner, "y");
        assert!(is_nil(&mut interpreter, &added));
        interpreter.enter(|mutator| {
            assert_eq!(mutator.heap.nested_of(outer_id), &[inner_id]);
            assert!(!mutator.heap.is_obsolete(inner_id));
        });
    }

    /// An entry whose declaration, or one it is nested in, another entry
    /// removes or replaces is refused, naming both, in either order, and
    /// nothing changes; once removed, the class's own edit is refused.
    // spec: runtime.install
    #[test]
    fn an_entry_whose_class_another_removes_or_replaces_is_refused() {
        let source = indoc! {"
            class Outer = Object new () (
              public class Inner = () (
                public class Deep = () ()
              )
            )
        "};
        let (mut interpreter, outer) = load(source);
        let instance = expect_ok(interpreter.instantiate(&outer, &[]), "an Outer");
        let inner_class = send(&mut interpreter, &instance, "Inner");
        let outer_id = root_id_of(&mut interpreter, &outer);
        let inner_id = nested_id_of(&mut interpreter, &outer, "Inner");
        let deep_id = nested_id_of(&mut interpreter, &inner_class, "Deep");
        let objects = [&instance, &inner_class];
        let before = snapshot(&mut interpreter, &[outer_id, inner_id, deep_id], &objects);
        let edit_of = |id| InstallEntry::Edit(id, add_method("public b = ( ^2 )"));
        let removes = InstallEntry::Edit(outer_id, remove_class("Inner"));
        let replaces = InstallEntry::Edit(outer_id, add_class("public class Inner = () ()"));
        for (entries, expected) in [
            (
                [edit_of(inner_id), removes.clone()],
                "the builder of Inner edits a class the builder of Outer removes",
            ),
            (
                [removes.clone(), edit_of(deep_id)],
                "the builder of Deep edits a class the builder of Outer removes",
            ),
            (
                [replaces.clone(), edit_of(inner_id)],
                "the builder of Inner edits a class the builder of Outer replaces",
            ),
        ] {
            assert_eq!(install_refusal(&mut interpreter, &entries), expected);
            assert_unchanged(
                &mut interpreter,
                &before,
                &[outer_id, inner_id, deep_id],
                &objects,
            );
        }
        install(&mut interpreter, outer_id, &remove_class("Inner"));
        for id in [inner_id, deep_id] {
            let refused = interpreter
                .enter(|mutator| mutator.install_one(id, &add_method("public b = ( ^2 )")))
                .err()
                .expect("an obsolete class is not edited");
            let name = if id == inner_id { "Inner" } else { "Deep" };
            assert_eq!(
                refused,
                format!("{name} was removed by an install, and an obsolete class is not edited")
            );
        }
    }

    /// The class an entry removes or replaces is the one its name stands
    /// for when the entry is spliced, every earlier rename in effect --
    /// the class pairing finds by that name: a class renamed to the name
    /// another entry then adds or removes is taken, and one whose old name
    /// another entry then adds again is not. An entry that replaces or
    /// removes a class before another renames it takes the class by the
    /// name it still has.
    // spec: runtime.install
    #[test]
    fn a_rename_decides_which_class_another_entry_takes() {
        let (mut interpreter, _, _, inner, inner_id) = outer_and_inner(OUTER);
        let outer_id = enclosing_of(&mut interpreter, inner_id);
        let renames = InstallEntry::Edit(
            inner_id,
            Edit {
                new_name: Some(Symbol::intern("Foo")),
                ..add_method("public b = ( ^2 )")
            },
        );
        let of_outer = |edit: Edit| InstallEntry::Edit(outer_id, edit);
        let objects = [&inner];
        let before = snapshot(&mut interpreter, &[outer_id, inner_id], &objects);
        for (entries, expected) in [
            (
                [
                    renames.clone(),
                    of_outer(add_class("public class Foo = () ()")),
                ],
                "the builder of Inner edits a class the builder of Outer replaces",
            ),
            (
                [renames.clone(), of_outer(remove_class("Foo"))],
                "the builder of Inner edits a class the builder of Outer removes",
            ),
            (
                [
                    of_outer(add_class("public class Inner = () ()")),
                    renames.clone(),
                ],
                "the builder of Inner edits a class the builder of Outer replaces",
            ),
            (
                [
                    of_outer(add_class("public class Foo = () ()")),
                    renames.clone(),
                ],
                "Outer already has a member named Foo",
            ),
        ] {
            assert_eq!(install_refusal(&mut interpreter, &entries), expected);
            assert_unchanged(&mut interpreter, &before, &[outer_id, inner_id], &objects);
        }
        // Renamed away, the old name is free: a new class takes it.
        let entries = [
            renames,
            of_outer(add_class(
                "public class Inner = () ( public fresh = ( ^3 ) )",
            )),
        ];
        if let Err(diagnostic) = interpreter.enter(|mutator| mutator.install_entries(&entries)) {
            panic!("the install was refused: {diagnostic}");
        }
        let answer = send(&mut interpreter, &inner, "b");
        assert_eq!(integer(&mut interpreter, &answer), Some(2));
        interpreter.enter(|mutator| {
            let nested = mutator.heap.nested_of(outer_id).to_vec();
            assert_eq!(nested.len(), 2);
            assert_eq!(nested[0], inner_id);
            let renamed = mutator.heap.mixin(inner_id).expect("Foo's row");
            assert_eq!(renamed.name.as_str(), "Foo");
            let added = mutator.heap.mixin(nested[1]).expect("the new Inner's row");
            assert_eq!(added.name.as_str(), "Inner");
            assert!(added.methods.contains_key(&Symbol::intern("fresh")));
        });
    }

    /// A class added to a declaration is reached through an instance of a
    /// subclass of it, made before the install.
    // spec: runtime.install
    #[test]
    fn an_added_class_is_reached_through_a_subclass_instance() {
        let source = indoc! {"
            class Outer = Object new () (
              public class Base = () ()
              public class Derived = Base () ()
            )
        "};
        let (mut interpreter, outer) = load(source);
        let instance = expect_ok(interpreter.instantiate(&outer, &[]), "an Outer");
        let derived_class = send(&mut interpreter, &instance, "Derived");
        let derived = send(&mut interpreter, &derived_class, "new");
        let base_id = nested_id_of(&mut interpreter, &outer, "Base");
        install(
            &mut interpreter,
            base_id,
            &add_class("public class Added = () ( public b = ( ^2 ) )"),
        );
        let added_class = send(&mut interpreter, &derived, "Added");
        let added = send(&mut interpreter, &added_class, "new");
        let answer = send(&mut interpreter, &added, "b");
        assert_eq!(integer(&mut interpreter, &answer), Some(2));
        let again = send(&mut interpreter, &derived, "Added");
        let same = interpreter.enter(|mutator| {
            let key = |root: &Root| {
                let object = mutator.fetch(root).as_object().expect("a class object");
                mutator.heap.object_identity_key(object)
            };
            key(&again) == key(&added_class)
        });
        assert!(same, "the subclass instance caches the added class");
    }

    /// A nested-class text of two declarations, or of a method, is
    /// refused, naming the enclosing class.
    // spec: runtime.install
    #[test]
    fn a_nested_class_text_of_two_declarations_is_refused() {
        let (mut interpreter, _, _, _, inner_id) = outer_and_inner(OUTER);
        let outer_id = enclosing_of(&mut interpreter, inner_id);
        for text in [
            "class A = ()() class B = ()()",
            "public b = ( ^2 )",
            "class A = ()() public b = ( ^2 )",
        ] {
            let refused = interpreter
                .enter(|mutator| mutator.install_one(outer_id, &add_class(text)))
                .err()
                .expect("the text is refused");
            assert_eq!(
                refused,
                "addFromSource: of Outer's nested classes takes one class declaration"
            );
        }
    }

    /// Spans are byte offsets: a class added, replaced and removed between
    /// comments of several bytes a character keeps every one of them, and
    /// a removed class goes with its comment.
    // spec: runtime.install
    #[test]
    fn a_nested_class_splice_keeps_multibyte_text() {
        let source = indoc! {"
            class Outer = Object new () (
              public before = ( ^'ä🙂' )
              /// Grüße 🙂
              public class Inner = () (
                public a = ( ^'ä' )
              )
              /// Ende ä 🙂
              public after = ( ^'🙂ü' )
              /// Weg 🙂
              public class Other = () ()
            )
        "};
        let (mut interpreter, outer) = load(source);
        let instance = expect_ok(interpreter.instantiate(&outer, &[]), "an Outer");
        let outer_id = root_id_of(&mut interpreter, &outer);
        let replaced = "public class Inner = () (\n  public a = ( ^'ö🙂' )\n)";
        install(&mut interpreter, outer_id, &add_class(replaced));
        install(
            &mut interpreter,
            outer_id,
            &add_class("public class Added = () ( public b = ( ^'é' ) )"),
        );
        install(&mut interpreter, outer_id, &remove_class("Other"));
        let module = interpreter
            .enter(|mutator| mutator.heap.module_source(outer_id))
            .expect("Outer's text");
        // The removed class leaves the indentation it stood after, and the
        // added one's line break follows it, which indoc would trim.
        let expected = indoc! {"
            class Outer = Object new () (
              public before = ( ^'ä🙂' )
              public class Inner = () (
              public a = ( ^'ö🙂' )
            )
              /// Ende ä 🙂
              public after = ( ^'🙂ü' )
            INDENT

            public class Added = () ( public b = ( ^'é' ) )
            )
        "}
        .replace("INDENT", "  ");
        assert_eq!(module, expected);
        for (selector, expected) in [("before", "ä🙂"), ("after", "🙂ü")] {
            let answer = send(&mut interpreter, &instance, selector);
            assert_eq!(string(&mut interpreter, &answer).as_deref(), Some(expected));
        }
        let inner_class = send(&mut interpreter, &instance, "Inner");
        let inner = send(&mut interpreter, &inner_class, "new");
        let answer = send(&mut interpreter, &inner, "a");
        assert_eq!(string(&mut interpreter, &answer).as_deref(), Some("ö🙂"));
    }

    // A superclass change

    /// A module whose classes' superclasses an install changes, through
    /// the VM mirror's three install leaves as `Installer>>install` sends
    /// them: each clause the preparation names evaluated, then the commit
    /// given the class objects with their answers.
    const REPARENTED: &str = indoc! {"
        class Outer = Object new (
          |
          public count ::= 0.
          |
        ) (
          public class Base = (| public base ::= 1. |) ()
          public class Other = (| public other ::= 2. |) ()
          public class Inner = Base new (| public own ::= bump. |) ()
          public class Trait = (| public trait ::= 3. |) (
            public traitMethod = ( ^#trait )
          )
          public class Mixed = Base new <: Trait.
          public bump = (
            count:: count + 1.
            ^3
          )
          public install: header of: class using: mirror = (
            ^installEdit: {header. {}. {}. nil} of: class using: mirror
          )
          public installClass: text in: class using: mirror = (
            ^installEdit: {nil. {{#addClass. text}}. {}. nil} of: class using: mirror
          )
          public installEdit: edit of: class using: mirror = (
            | entries prepared answers committed |
            entries:: {{mirror reflectionMixinOf: class ifFail: [nil]. edit}}.
            prepared:: mirror reflectionPrepareInstall: entries ifFail: [^#declined].
            (prepared at: 1) ifFalse: [^prepared at: 2].
            answers:: (prepared at: 3) collect: [:clause |
              {clause at: 1. mirror reflectionInstall: entries evaluateClause: (clause at: 1) epoch: (prepared at: 2) ifFail: [nil]}].
            committed:: mirror reflectionInstall: entries commitEpoch: (prepared at: 2) superclasses: answers ifFail: [^#declined].
            (committed at: 1) ifFalse: [^committed at: 2].
            ^#installed
          )
        )
    "};

    /// The install of `header` as `class`'s header, through `outer`'s
    /// `install:of:using:`: `installed`, or the diagnostic that refused it.
    fn install_header(
        interpreter: &mut Interpreter,
        outer: &Root,
        class: &Root,
        header: &str,
    ) -> String {
        install_through(interpreter, outer, "install:of:using:", class, header)
    }

    /// The install `selector` of `outer` makes of `text` and `class`, as
    /// `install_header` does.
    fn install_through(
        interpreter: &mut Interpreter,
        outer: &Root,
        selector: &str,
        class: &Root,
        header: &str,
    ) -> String {
        let (header, mirror) = interpreter.enter(|mutator| {
            let header = mutator.new_string(header.to_string());
            let header = mutator.stash(Value::object(header));
            let mirror = mutator.vm_mirror();
            (header, mutator.stash(mirror))
        });
        let answer = send_with(
            interpreter,
            outer,
            selector,
            &[header, class.clone(), mirror],
        );
        interpreter.enter(|mutator| {
            let object = mutator
                .fetch(&answer)
                .as_object()
                .expect("a Symbol or a String");
            mutator
                .heap
                .symbol_text(object)
                .or_else(|| mutator.heap.str_value(object))
                .expect("a Symbol or a String")
        })
    }

    /// Whether `first` and `second` are one object.
    fn same(interpreter: &mut Interpreter, first: &Root, second: &Root) -> bool {
        interpreter.enter(|mutator| {
            let key = |root: &Root| {
                let object = mutator.fetch(root).as_object().expect("an object");
                mutator.heap.object_identity_key(object)
            };
            key(first) == key(second)
        })
    }

    /// `Inner = Base` becomes `Inner = Other`: the class object stays, an
    /// instance made before loses `Base`'s slot, starts `Other`'s nil and
    /// keeps its own, and no initializer runs -- the enclosing instance's
    /// count of `Inner`'s own-slot initializers stays where it was.
    // spec: runtime.install.superclass
    #[test]
    fn a_superclass_change_keeps_the_class_object_and_migrates_by_name() {
        let (mut interpreter, outer_class) = load(REPARENTED);
        let outer = expect_ok(interpreter.instantiate(&outer_class, &[]), "an Outer");
        let inner_class = send(&mut interpreter, &outer, "Inner");
        let inner = send(&mut interpreter, &inner_class, "new");
        let epoch = interpreter.enter(|mutator| mutator.install_epoch);
        let answer = install_header(
            &mut interpreter,
            &outer,
            &inner_class,
            "public class Inner = Other new (| public own ::= bump. |)",
        );
        assert_eq!(answer, "installed");
        assert_eq!(
            interpreter.enter(|mutator| mutator.install_epoch),
            epoch + 1
        );
        let again = send(&mut interpreter, &outer, "Inner");
        assert!(same(&mut interpreter, &again, &inner_class));
        let other_class = send(&mut interpreter, &outer, "Other");
        let superclass = interpreter.enter(|mutator| {
            let class = mutator.fetch(&inner_class).as_object().expect("a class");
            let superclass = mutator.heap.class_superclass(class).expect("a superclass");
            mutator.stash(Value::object(superclass))
        });
        assert!(same(&mut interpreter, &superclass, &other_class));
        assert!(
            send_error(&mut interpreter, &inner, "base").contains("#base"),
            "Base's slot is gone"
        );
        let other = send(&mut interpreter, &inner, "other");
        assert!(is_nil(&mut interpreter, &other), "Other's slot starts nil");
        let own = send(&mut interpreter, &inner, "own");
        assert_eq!(integer(&mut interpreter, &own), Some(3));
        let count = send(&mut interpreter, &outer, "count");
        assert_eq!(
            integer(&mut interpreter, &count),
            Some(1),
            "no initializer ran"
        );
    }

    /// A clause that answers a subclass of the class it is the clause of is
    /// refused before anything changes: `Base = Object` becoming `Base =
    /// Inner` while `Inner = Base`.
    // spec: runtime.install.superclass
    #[test]
    fn a_cycle_changes_nothing() {
        let (mut interpreter, outer_class) = load(REPARENTED);
        let outer = expect_ok(interpreter.instantiate(&outer_class, &[]), "an Outer");
        let inner_class = send(&mut interpreter, &outer, "Inner");
        let inner = send(&mut interpreter, &inner_class, "new");
        let base_class = send(&mut interpreter, &outer, "Base");
        let base_id = nested_id_of(&mut interpreter, &outer_class, "Base");
        let inner_id = nested_id_of(&mut interpreter, &outer_class, "Inner");
        let ids = [base_id, inner_id];
        let before = snapshot(&mut interpreter, &ids, &[&base_class, &inner_class, &inner]);
        let epoch = interpreter.enter(|mutator| mutator.install_epoch);
        let answer = install_header(
            &mut interpreter,
            &outer,
            &base_class,
            "public class Base = Inner new (| public base ::= 1. |)",
        );
        assert_eq!(
            answer,
            "the superclass clause of Base answers a class that inherits from Base"
        );
        assert_unchanged(
            &mut interpreter,
            &before,
            &ids,
            &[&base_class, &inner_class, &inner],
        );
        assert_eq!(interpreter.enter(|mutator| mutator.install_epoch), epoch);
        let base = send(&mut interpreter, &inner, "base");
        assert_eq!(integer(&mut interpreter, &base), Some(1));
    }

    /// An install of no entries answers at once: nothing is prepared or
    /// committed, and the epoch stays, so that a clause that installs
    /// nothing leaves the install it runs in free to commit.
    // spec: runtime.install.epoch
    #[test]
    fn an_empty_install_moves_no_epoch() {
        let (mut interpreter, _) = load(REPARENTED);
        let epoch = interpreter.enter(|mutator| mutator.install_epoch);
        let installed = interpreter.enter(|mutator| mutator.install_entries(&[]));
        match installed {
            Ok(texts) => assert!(texts.is_empty()),
            Err(diagnostic) => panic!("the empty install was refused: {diagnostic}"),
        }
        assert_eq!(interpreter.enter(|mutator| mutator.install_epoch), epoch);
    }

    /// A replacement that turns a class with a body into a mixin
    /// application without one would make its class objects objects of
    /// another shape, so it is refused, and nothing changes.
    // spec: runtime.install.superclass
    #[test]
    fn a_class_that_becomes_body_less_is_refused() {
        let source = indoc! {"
            class Outer = Object new () (
              public class Trait = () ()
              public class Inner = () ()
            )
        "};
        let (mut interpreter, outer) = load(source);
        let outer_id = root_id_of(&mut interpreter, &outer);
        let edit = add_class("public class Inner = Object new <: Trait.");
        assert_eq!(
            install_refusal(&mut interpreter, &[InstallEntry::Edit(outer_id, edit)]),
            "Inner gains or loses a body of its own, which an install does not change"
        );
    }

    /// A mixin application without a body of its own, `Mixed = Base <:
    /// Trait.`, whose clause changes to `Other <: Trait` is re-parented as
    /// any class: its class object -- the level its clause adds -- keeps
    /// its identity and its factory, its superclass becomes `Other`, and
    /// an instance made before keeps the applied mixin's slot and methods,
    /// loses the slot of `Base` and starts the one of `Other` nil.
    // spec: runtime.install.superclass
    #[test]
    fn a_body_less_class_is_reparented() {
        let (mut interpreter, outer_class) = load(REPARENTED);
        let outer = expect_ok(interpreter.instantiate(&outer_class, &[]), "an Outer");
        let mixed_class = send(&mut interpreter, &outer, "Mixed");
        let mixed = send(&mut interpreter, &mixed_class, "new");
        let answer = install_through(
            &mut interpreter,
            &outer,
            "installClass:in:using:",
            &outer_class,
            "public class Mixed = Other new <: Trait.",
        );
        assert_eq!(answer, "installed");
        let again = send(&mut interpreter, &outer, "Mixed");
        assert!(same(&mut interpreter, &again, &mixed_class));
        let other_class = send(&mut interpreter, &outer, "Other");
        let superclass = interpreter.enter(|mutator| {
            let class = mutator.fetch(&mixed_class).as_object().expect("a class");
            let superclass = mutator.heap.class_superclass(class).expect("a superclass");
            mutator.stash(Value::object(superclass))
        });
        assert!(same(&mut interpreter, &superclass, &other_class));
        let method = send(&mut interpreter, &mixed, "traitMethod");
        let symbol = interpreter.enter(|mutator| {
            let object = mutator.fetch(&method).as_object().expect("a Symbol");
            mutator.heap.symbol_text(object)
        });
        assert_eq!(symbol.as_deref(), Some("trait"));
        let trait_slot = send(&mut interpreter, &mixed, "trait");
        assert_eq!(integer(&mut interpreter, &trait_slot), Some(3));
        let other = send(&mut interpreter, &mixed, "other");
        assert!(
            is_nil(&mut interpreter, &other),
            "the slot of Other starts nil"
        );
        assert!(send_error(&mut interpreter, &mixed, "base").contains("#base"));
        let fresh = send(&mut interpreter, &mixed_class, "new");
        let fresh_other = send(&mut interpreter, &fresh, "other");
        assert_eq!(integer(&mut interpreter, &fresh_other), Some(2));
    }

    /// A root's header from `Object` to `Value`, then to `Object <: Value`,
    /// re-parents its class object over the builtin the header names,
    /// without evaluating anything: the instance made before keeps its
    /// class object, which answers its methods.
    // spec: runtime.install.superclass
    #[test]
    fn a_root_is_reparented_over_its_new_static_superclass() {
        let source = indoc! {"
            class Root = Object new () (
              public a = ( ^1 )
            )
        "};
        let (mut interpreter, root) = load(source);
        let instance = expect_ok(interpreter.instantiate(&root, &[]), "a Root");
        let root_id = root_id_of(&mut interpreter, &root);
        let superclass_of = |interpreter: &mut Interpreter| {
            interpreter.enter(|mutator| {
                let class = mutator.fetch(&root).as_object().expect("a class");
                let superclass = mutator.heap.class_superclass(class).expect("a superclass");
                let named = mutator
                    .heap
                    .class_named(superclass)
                    .and_then(|level| mutator.heap.class_builtin(level));
                let below = mutator
                    .heap
                    .class_superclass(superclass)
                    .and_then(|level| mutator.heap.class_builtin(level));
                (mutator.heap.class_builtin(superclass), named, below)
            })
        };
        let class_of_instance_is_root = |interpreter: &mut Interpreter| {
            interpreter.enter(|mutator| {
                let object = mutator.fetch(&instance).as_object().expect("an instance");
                let class = mutator.heap.object_class(object);
                let root_class = mutator.fetch(&root).as_object().expect("a class");
                class.same(root_class)
            })
        };
        install(
            &mut interpreter,
            root_id,
            &header_edit("class Root = Value new ()"),
        );
        assert!(class_of_instance_is_root(&mut interpreter));
        let (superclass, _, _) = superclass_of(&mut interpreter);
        assert_eq!(superclass, Some(BuiltinClass::Value));
        let answer = send(&mut interpreter, &instance, "a");
        assert_eq!(integer(&mut interpreter, &answer), Some(1));
        install(
            &mut interpreter,
            root_id,
            &header_edit("class Root = Object new <: Value ()"),
        );
        assert!(class_of_instance_is_root(&mut interpreter));
        let (superclass, named, below) = superclass_of(&mut interpreter);
        assert_eq!(
            superclass, None,
            "the superclass is the level `<: Value` adds"
        );
        assert_eq!(named, Some(BuiltinClass::Value));
        assert_eq!(below, Some(BuiltinClass::Object));
        let answer = send(&mut interpreter, &instance, "a");
        assert_eq!(integer(&mut interpreter, &answer), Some(1));
    }
}
