//! The shipped world: every object the binary owns and an image may name
//! (`docs/design/image-format.md`, "Shipped names").
//! Filled as the shipped world is built -- each module instance and its
//! root's names when the instance is recorded (`record_shipped_module`),
//! each class a recorded root declares and its names when its accessor
//! builds it (`nested::after_clause`), and at boot every class of the
//! modules recorded so far, by `complete_shipped_world` -- and an entry once
//! recorded is never changed.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use ns_syntax::ast::AccessModifier;

use crate::heap::{Heap, Root};
use crate::interpreter::Mutator;
use crate::mixin::{mixin_key, MethodEntry, Mixin};
use crate::trace::{Trace, Tracer};
use crate::types::{BuiltinClass, ClassReference, ObjectReference, Value};
use crate::Symbol;

/// What a shipped name names, given its module and its declaration.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ShippedKind {
    /// The module's one instance; its declaration is the module's root.
    ModuleInstance,
    /// The class object the declaration evaluates to.
    Class,
    /// That class's metaclass.
    Metaclass,
    /// The level the declaration's `<:` clause of this index adds.
    Level(u32),
    /// That level's metaclass.
    LevelMetaclass(u32),
}

/// A shipped object's name: the module, the declaration in the module's
/// root -- for the root itself, the module's own name -- and what of that
/// declaration is named.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ShippedName {
    pub module: Symbol,
    pub declaration: Symbol,
    pub kind: ShippedKind,
}

impl ShippedName {
    /// The name as a refusal or a test shows it: `Kernel Array`,
    /// `Kernel Array class`, `Kernel Kernel level 0`,
    /// `Kernel Kernel level 0 class`, `CollectionsFactory instance`.
    pub fn describe(&self) -> String {
        let module = self.module.as_str();
        let declaration = self.declaration.as_str();
        match self.kind {
            ShippedKind::ModuleInstance => format!("{module} instance"),
            ShippedKind::Class => format!("{module} {declaration}"),
            ShippedKind::Metaclass => format!("{module} {declaration} class"),
            ShippedKind::Level(index) => format!("{module} {declaration} level {index}"),
            ShippedKind::LevelMetaclass(index) => {
                format!("{module} {declaration} level {index} class")
            }
        }
    }
}

/// Where an image may put a shipped object (`docs/design/image-format.md`,
/// "Shipped names", the rule of what may be named
/// where): `value` as a slot's, a register's or an element's value;
/// `superclass` as a class record's superclass or the mixin it applies;
/// `header` as a record's class; `frame_class` as a frame's
/// `current_class` or a class record's declaring class; `frame_register`
/// as a frame record's receiver or register.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Positions {
    pub value: bool,
    pub superclass: bool,
    pub header: bool,
    pub frame_class: bool,
    pub frame_register: bool,
}

impl Positions {
    /// A module root, its metaclass, a level and a level's metaclass: a
    /// frame's class or a class record's declaring class, nothing else.
    const FRAME_CLASS_ONLY: Positions = Positions {
        value: false,
        superclass: false,
        header: false,
        frame_class: true,
        frame_register: false,
    };
}

/// One shipped object under its name, with where an image may put it.
pub(crate) struct ShippedEntry<'gc> {
    pub(crate) name: ShippedName,
    pub(crate) object: ObjectReference<'gc>,
    pub(crate) positions: Positions,
}

/// The shipped modules' instances, in the order they were built -- the
/// kernel and `PlatformFactory` at boot, every other module on its first
/// demand -- and every shipped object under its name.
#[derive(Default)]
pub(crate) struct ShippedWorld<'gc> {
    module_instances: Vec<(Symbol, ObjectReference<'gc>)>,
    /// In the order of recording: per module as it is recorded, the
    /// instance, the root class, its metaclass and its levels; per class a
    /// root declares as it is built and recorded, the class, its metaclass
    /// and its levels.
    entries: Vec<ShippedEntry<'gc>>,
    /// Each entry's name to its position in `entries`.
    by_name: HashMap<ShippedName, usize>,
    /// Per recorded module, the classes a public class accessor of its
    /// root answers (`accessed_classes`): walked once when the module is
    /// recorded rather than once for each class it declares.
    accessed: HashMap<Symbol, HashSet<Symbol>>,
    /// Per lazy shipped module whose root class `shippedModuleClassNamed:`
    /// built and whose instance is not yet recorded, that class: a demand
    /// an unwind interrupted gets the same class again rather than another.
    pending: Vec<(Symbol, ClassReference<'gc>)>,
}

impl<'gc> ShippedWorld<'gc> {
    /// Records `instance` as the one instance of the shipped module `name`.
    /// Each module is recorded once; a second instance is a defect in
    /// the binary, and so is one instance recorded for two modules, which
    /// would leave `module_of_instance` two answers.
    fn add_module_instance(&mut self, name: Symbol, instance: ObjectReference<'gc>) {
        assert!(
            self.module_instance(name).is_none(),
            "internal: the shipped module {} is instantiated twice",
            name.as_str()
        );
        if let Some(recorded) = self.module_of_instance(instance) {
            panic!(
                "internal: the instance of the shipped module {} is recorded again as {}",
                recorded.as_str(),
                name.as_str()
            );
        }
        self.module_instances.push((name, instance));
    }

    /// The one instance of the shipped module `name`, or `None` when no
    /// shipped module has that name.
    pub(crate) fn module_instance(&self, name: Symbol) -> Option<ObjectReference<'gc>> {
        self.module_instances()
            .iter()
            .find(|(module, _)| *module == name)
            .map(|(_, instance)| *instance)
    }

    /// Every shipped module's name and instance recorded so far, in the
    /// order they were built.
    pub(crate) fn module_instances(&self) -> &[(Symbol, ObjectReference<'gc>)] {
        &self.module_instances
    }

    /// The name of the shipped module whose one instance `instance` is, or
    /// `None` when it is no shipped module's instance.
    pub(crate) fn module_of_instance(&self, instance: ObjectReference<'gc>) -> Option<Symbol> {
        self.module_instances
            .iter()
            .find(|(_, recorded)| recorded.same(instance))
            .map(|(module, _)| *module)
    }

    /// Every shipped object named so far, in the order it was recorded
    /// (`ShippedWorld::entries`' field comment); empty only before the
    /// first module is recorded.
    pub(crate) fn entries(&self) -> &[ShippedEntry<'gc>] {
        &self.entries
    }

    /// The position in `entries` of the object `name` names, or `None` when
    /// the shipped world has no such object.
    pub(crate) fn index_of_name(&self, name: &ShippedName) -> Option<usize> {
        self.by_name.get(name).copied()
    }

    /// Each entry's object, by `Heap::object_identity_key` -- the key
    /// `image::encode::GraphTable` gives an object -- to its position in
    /// `entries`. The key is an offset, which a collection changes, so the
    /// map is built for each encoding and never kept past the entry that
    /// built it.
    pub(crate) fn index_by_object(&self, heap: &Heap<'gc>) -> HashMap<usize, usize> {
        self.entries
            .iter()
            .enumerate()
            .map(|(index, entry)| (heap.object_identity_key(entry.object), index))
            .collect()
    }

    /// The mixin of each `Class` entry, by `mixin_key`, to the entry's
    /// position in `entries`: which shipped declaration a mixin is. A
    /// level applies the mixin of some class, so only classes are keyed.
    pub(crate) fn index_by_mixin(&self, heap: &Heap<'gc>) -> HashMap<usize, usize> {
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.name.kind == ShippedKind::Class)
            .filter_map(|(index, entry)| {
                let application = heap.class_application(entry.object)?;
                Some((mixin_key(&application.mixin), index))
            })
            .collect()
    }

    /// The pending root class of the shipped module `name`, or `None` when
    /// none was built or its instance is recorded.
    pub(crate) fn pending_class(&self, name: Symbol) -> Option<ClassReference<'gc>> {
        self.pending
            .iter()
            .find(|(module, _)| *module == name)
            .map(|(_, class)| *class)
    }

    /// Keeps `class` as the pending root class of the shipped module `name`.
    /// The world builds one per module, so a second is a defect.
    pub(crate) fn add_pending(&mut self, name: Symbol, class: ClassReference<'gc>) {
        assert!(
            self.pending_class(name).is_none() && self.module_instance(name).is_none(),
            "internal: the shipped module {} has a root class already",
            name.as_str()
        );
        self.pending.push((name, class));
    }

    /// The shipped module whose pending root class `class` is, or `None`
    /// when it is none.
    pub(crate) fn module_of_pending_class(&self, class: ClassReference<'gc>) -> Option<Symbol> {
        self.pending
            .iter()
            .find(|(_, pending)| pending.same(class))
            .map(|(module, _)| *module)
    }

    /// Drops the pending root class of the shipped module `name`, whose
    /// instance is about to be recorded.
    pub(crate) fn remove_pending(&mut self, name: Symbol) {
        self.pending.retain(|(module, _)| *module != name);
    }

    /// Records `entries` under their names. A name recorded twice is a
    /// defect in the binary: it would name two objects.
    fn record_entries(&mut self, entries: Vec<ShippedEntry<'gc>>) {
        for entry in entries {
            let previous = self.by_name.insert(entry.name, self.entries.len());
            assert!(
                previous.is_none(),
                "internal: the shipped name {} is recorded twice",
                entry.name.describe()
            );
            self.entries.push(entry);
        }
    }
}

impl Trace for ShippedWorld<'_> {
    fn trace<T: Tracer>(&mut self, tracer: &mut T) {
        for (_, instance) in &mut self.module_instances {
            tracer.object(instance);
        }
        for entry in &mut self.entries {
            tracer.object(&mut entry.object);
        }
        for (_, class) in &mut self.pending {
            tracer.object(class);
        }
    }
}

impl crate::interpreter::Interpreter {
    /// The last phase of the boot before the platform is made: every class
    /// a recorded shipped root -- the kernel's and `PlatformFactory`'s --
    /// declares is built, in declaration order, and recorded under its name
    /// with the positions it may stand in (`docs/design/image-format.md`,
    /// "Shipped names") by `name_built_shipped_classes`, as a module
    /// recorded later names the classes built before its record. After its
    /// record, a module names its classes one by one, as their accessors
    /// build them (`nested::after_clause`).
    /// The kernel's own phase built the kernel's classes, so building one of
    /// them answers the class it cached. Only the binary's own code runs,
    /// so the world is a function of the binary.
    pub(crate) fn complete_shipped_world(&mut self) {
        let modules: Vec<(Symbol, Root, Rc<Mixin>)> = self.enter(|mutator| {
            mutator
                .shipped_world
                .module_instances()
                .iter()
                .map(|(module, instance)| {
                    let heap = mutator.heap();
                    let mixin = root_application(heap, *instance).mixin.clone();
                    (*module, mutator.stash(Value::object(*instance)), mixin)
                })
                .collect()
        });
        for (module, instance, mixin) in &modules {
            for nested in &mixin.nested {
                super::core_library::booted(
                    self.build_nested_class(instance, nested),
                    &format!("building {} {}", module.as_str(), nested.name.as_str()),
                );
            }
            self.enter(|mutator| {
                let instance = mutator.fetch(instance).as_object().unwrap_or_else(|| {
                    panic!("internal: {}'s instance is an object", module.as_str())
                });
                mutator.name_built_shipped_classes(*module, instance);
            });
        }
    }
}

impl Mutator<'_> {
    /// How many objects the shipped world names: what a test reads to see
    /// that something built, or did not build, a shipped module or class.
    #[doc(hidden)]
    pub fn shipped_name_count(&self) -> usize {
        self.shipped_world.entries().len()
    }
}

impl<'gc> Mutator<'gc> {
    /// Records `instance` as the one instance of the shipped module
    /// `module` and names it, its class -- the module's root --, the root's
    /// metaclass and its levels.
    pub(crate) fn record_shipped_module(&mut self, module: Symbol, instance: ObjectReference<'gc>) {
        self.shipped_world.add_module_instance(module, instance);
        let heap = self.heap();
        let accessed = accessed_classes(&root_application(heap, instance).mixin);
        let mut entries = vec![ShippedEntry {
            name: ShippedName {
                module,
                declaration: module,
                kind: ShippedKind::ModuleInstance,
            },
            object: instance,
            positions: Positions {
                frame_register: true,
                ..Positions::default()
            },
        }];
        push_class(
            heap,
            &mut entries,
            module,
            module,
            heap.object_class(instance),
            Positions::FRAME_CLASS_ONLY,
        );
        self.shipped_world.accessed.insert(module, accessed);
        self.shipped_world.record_entries(entries);
    }

    /// Records under its names every class the root of the recorded shipped
    /// module `module` declares that `instance`, its one instance, has
    /// already built, in declaration order; a class not built yet is left
    /// for its accessor to name when it builds it. A class can be built
    /// before its module is recorded -- by the module's own initializer, or
    /// by the kernel's phase of the boot -- when the accessor's hook does
    /// not name it, since the module is not yet a recorded one; without
    /// this, such a class would stay without a name, and an encoder that
    /// met it would find no shipped name for an object the binary owns.
    pub(crate) fn name_built_shipped_classes(
        &mut self,
        module: Symbol,
        instance: ObjectReference<'gc>,
    ) {
        let application = root_application(self.heap(), instance);
        for nested in &application.mixin.nested {
            let built = application
                .nested_slot
                .get(&mixin_key(nested))
                .and_then(|index| self.heap().instance_field(instance, *index))
                .and_then(Value::as_object)
                .filter(|class| self.heap().is_class(*class));
            if let Some(class) = built {
                self.record_shipped_class(module, nested, class);
            }
        }
    }

    /// Records `class`, the class object the declaration `nested` in the
    /// root of the recorded shipped module `module` evaluated to, under its
    /// name with the positions it may stand in, then its metaclass and its
    /// levels. A class already named is left as it is: a shipped module has
    /// one instance and its accessor builds the class once, so recording it
    /// again names the same object.
    pub(crate) fn record_shipped_class(
        &mut self,
        module: Symbol,
        nested: &Rc<Mixin>,
        class: ClassReference<'gc>,
    ) {
        let declaration = nested.name;
        let class_name = ShippedName {
            module,
            declaration,
            kind: ShippedKind::Class,
        };
        if let Some(index) = self.shipped_world.index_of_name(&class_name) {
            // Two paths record a class -- its accessor's build and
            // `name_built_shipped_classes`, which `complete_shipped_world`
            // and `recordShippedModule:` both run -- and both must name the
            // one class object the module caches: another object dropped
            // here would leave a shipped class no name names.
            debug_assert!(
                self.shipped_world.entries()[index].object.same(class),
                "internal: {} is recorded as two class objects",
                class_name.describe()
            );
            return;
        }
        let accessed = self.shipped_world.accessed.get(&module).unwrap_or_else(|| {
            panic!(
                "internal: {} is recorded before its classes",
                module.as_str()
            )
        });
        let heap = self.heap();
        let positions = class_positions(heap, module, accessed, nested, class);
        let mut entries = Vec::new();
        push_class(heap, &mut entries, module, declaration, class, positions);
        self.shipped_world.record_entries(entries);
    }
}

/// The application of a shipped module instance's class, its root class.
fn root_application<'gc>(
    heap: &Heap<'gc>,
    instance: ObjectReference<'gc>,
) -> Rc<crate::mixin::MixinApplication> {
    heap.class_application(heap.object_class(instance))
        .expect("internal: a shipped module instance's class is a class object")
}

/// The kernel classes that are public only so that another shipped
/// module's root, which holds the kernel instance, may send `kernel <Name>`
/// for them -- `TypedArraysFactory.ns` applies `IndexedCollection` -- and
/// that neither `Object`'s class accessors nor `KernelNamespace`'s hand
/// out. No program reaches one by running, so an image may name it only as
/// a private class without an accessor stands: as a record's class or a
/// frame's, never as a value or a superclass.
///
/// A named list, because the two facets' accessors are not data the boot
/// reads here: `Object`'s are recognized as `MethodFastPath::BuiltinClass`,
/// not as class accessors of a declaration, and `KernelNamespace`'s answer
/// `PlatformFactory`'s slots, which hold what the kernel answered.
/// `tests::the_kernel_classes_for_shipped_modules_are_exactly_those_no_facet_hands_out`
/// holds the list to the two facets.
const KERNEL_CLASSES_FOR_SHIPPED_MODULES: &[&str] = &["IndexedCollection"];

/// Where an image may put `class`, the class object the declaration
/// `nested` in the root of the shipped module `module` evaluated to;
/// `accessed` is that root's `accessed_classes`.
fn class_positions<'gc>(
    heap: &Heap<'gc>,
    module: Symbol,
    accessed: &HashSet<Symbol>,
    nested: &Mixin,
    class: ClassReference<'gc>,
) -> Positions {
    let public = nested.access == AccessModifier::Public
        && !(module == Symbol::intern("Kernel")
            && KERNEL_CLASSES_FOR_SHIPPED_MODULES.contains(&nested.name.as_str()));
    let value = public || accessed.contains(&nested.name);
    // A class object is made only by evaluating a declaration, so
    // nothing may inherit from what every class object inherits.
    let superclass = value
        && !matches!(
            heap.class_builtin(class),
            Some(BuiltinClass::Behavior | BuiltinClass::Class | BuiltinClass::Metaclass)
        );
    // No object is an instance of `Top`: §3.2 gives it no subclass
    // but `Object`, which every class a program reaches inherits, so
    // a record of it is refused as `Top` is refused as a value (lock
    // (4) of the VM mirror).
    let header = heap.class_builtin(class) != Some(BuiltinClass::Top);
    Positions {
        value,
        superclass,
        header,
        frame_class: true,
        frame_register: false,
    }
}

/// Records `class` as `(module, declaration, Class)` with `positions`,
/// then its metaclass and the levels its own `<:` clauses add, lowest
/// first, with their metaclasses. A metaclass may be a value and a frame's
/// class when its class is a value, and is never a record's class: no
/// record is an instance of a metaclass but its class.
fn push_class<'gc>(
    heap: &Heap<'gc>,
    entries: &mut Vec<ShippedEntry<'gc>>,
    module: Symbol,
    declaration: Symbol,
    class: ClassReference<'gc>,
    positions: Positions,
) {
    let name = |kind| ShippedName {
        module,
        declaration,
        kind,
    };
    let metaclass_of = |class| {
        heap.class_metaclass(class).unwrap_or_else(|| {
            panic!(
                "internal: {} has a metaclass at bootstrap",
                name(ShippedKind::Class).describe()
            )
        })
    };
    entries.push(ShippedEntry {
        name: name(ShippedKind::Class),
        object: class,
        positions,
    });
    entries.push(ShippedEntry {
        name: name(ShippedKind::Metaclass),
        object: metaclass_of(class),
        positions: Positions {
            value: positions.value,
            ..Positions::FRAME_CLASS_ONLY
        },
    });
    for level in own_levels(heap, class) {
        let applied = heap
            .class_applied_origin(level)
            .expect("internal: own_levels answers only applied levels");
        let index = u32::try_from(applied.index)
            .expect("internal: a shipped declaration has fewer than 2^32 clauses");
        entries.push(ShippedEntry {
            name: name(ShippedKind::Level(index)),
            object: level,
            positions: Positions::FRAME_CLASS_ONLY,
        });
        entries.push(ShippedEntry {
            name: name(ShippedKind::LevelMetaclass(index)),
            object: metaclass_of(level),
            positions: Positions::FRAME_CLASS_ONLY,
        });
    }
}

/// The levels `class`'s own `<:` clauses added, lowest first -- the order
/// the clauses are written in, and the order their names number them by
/// clause.
///
/// Walked down the superclass chain from `class` for as long as each class
/// is a level one of `class`'s own clauses added -- the applying declaration
/// is `class`'s mixin itself, not merely a mixin applied somewhere.
fn own_levels<'gc>(heap: &Heap<'gc>, class: ClassReference<'gc>) -> Vec<ClassReference<'gc>> {
    let Some(mixin) = heap
        .class_application(class)
        .map(|application| application.mixin.clone())
    else {
        return Vec::new();
    };
    let mut levels = Vec::new();
    let mut current = heap.class_superclass(class);
    while let Some(level) = current {
        let is_own_level = heap
            .class_applied_origin(level)
            .is_some_and(|applied| Rc::ptr_eq(&applied.declaration, &mixin));
        if !is_own_level {
            break;
        }
        levels.push(level);
        current = heap.class_superclass(level);
    }
    levels.reverse();
    levels
}

/// The names of the classes a public method of `root` or of any class it
/// declares, instance or class side, is a recognized class accessor of
/// (`MixinMethod::class_accessor`): a private class such a method answers
/// may be a value, since any holder of that method's receiver reaches it.
fn accessed_classes(root: &Rc<Mixin>) -> HashSet<Symbol> {
    let mut accessed = HashSet::new();
    let mut pending: Vec<&Rc<Mixin>> = vec![root];
    while let Some(mixin) = pending.pop() {
        for entry in mixin.methods.values() {
            if let MethodEntry::Compiled(method) = entry {
                if method.access == AccessModifier::Public {
                    if let Some(accessor) = &method.class_accessor {
                        accessed.insert(accessor.class_name);
                    }
                }
            }
        }
        pending.extend(mixin.nested.iter());
        pending.extend(mixin.class_side.iter());
    }
    accessed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(module: &str, declaration: &str, kind: ShippedKind) -> ShippedName {
        ShippedName {
            module: Symbol::intern(module),
            declaration: Symbol::intern(declaration),
            kind,
        }
    }

    /// A boot that has also built the shipped classes `classes` names, each
    /// a module and a declaration of it -- the module itself for its root --
    /// as an image naming them would have them built: the boot builds only
    /// the kernel and the platform, every other module on first demand.
    fn bootstrap_with(classes: &[(&str, &str)]) -> crate::Interpreter {
        let mut interpreter = crate::Interpreter::bootstrap();
        let names: Vec<ShippedName> = classes
            .iter()
            .map(|(module, declaration)| name(module, declaration, ShippedKind::Class))
            .collect();
        if let Err(error) = interpreter.build_shipped_names(&names, false) {
            panic!("the shipped classes build: {error}");
        }
        interpreter
    }

    fn positions_of(world: &ShippedWorld<'_>, shipped: ShippedName) -> Positions {
        let index = world
            .index_of_name(&shipped)
            .unwrap_or_else(|| panic!("the shipped world has no {}", shipped.describe()));
        world.entries()[index].positions
    }

    /// The names the image format relies on all exist once their modules
    /// and classes are built, and each may stand exactly where the rule of
    /// `image-format.md` allows.
    #[test]
    fn every_shipped_object_is_named_with_the_positions_the_rule_allows() {
        let mut interpreter = bootstrap_with(&[
            ("TranscriptFactory", "Transcript"),
            ("TimeFactory", "Time"),
            ("CollectionsFactory", "List"),
            ("CollectionsFactory", "Collections"),
            ("TypedArraysFactory", "TypedArray"),
            ("TypedArraysFactory", "Int16Array"),
            ("TypedArraysFactory", "TypedArrays"),
        ]);
        interpreter.enter(|mutator| {
            let world = &mutator.shipped_world;
            let array = positions_of(world, name("Kernel", "Array", ShippedKind::Class));
            assert!(array.value && array.superclass && array.header && array.frame_class);
            assert!(!array.frame_register);
            let array_class = positions_of(world, name("Kernel", "Array", ShippedKind::Metaclass));
            assert!(array_class.value && !array_class.header && array_class.frame_class);
            for private in ["Top", "Activation", "FutureImplementation"] {
                let positions = positions_of(world, name("Kernel", private, ShippedKind::Class));
                assert!(
                    !positions.value && !positions.superclass,
                    "{private} is no value"
                );
                assert!(positions.frame_class);
                assert_eq!(
                    positions.header,
                    private != "Top",
                    "{private} as a record's class"
                );
            }
            // The VM signals it into a program, which catches an instance
            // but, `class` being protected, never its class: a record's
            // class only, as any private class.
            let signalled = positions_of(
                world,
                name("Kernel", "NonBooleanReceiver", ShippedKind::Class),
            );
            assert!(!signalled.value && !signalled.superclass && signalled.header);
            let signalled_class = positions_of(
                world,
                name("Kernel", "NonBooleanReceiver", ShippedKind::Metaclass),
            );
            assert!(!signalled_class.value && !signalled_class.header);
            for meta in ["Behavior", "Class", "Metaclass"] {
                let positions = positions_of(world, name("Kernel", meta, ShippedKind::Class));
                assert!(
                    positions.value && !positions.superclass,
                    "{meta} is no superclass"
                );
            }
            // The byte storage, its builder and its view are public
            // classes a program names, subclasses and keeps instances of.
            for public in ["ByteArray", "StringBuilder", "DataView"] {
                let positions = positions_of(world, name("Kernel", public, ShippedKind::Class));
                assert!(
                    positions.value && positions.superclass && positions.header,
                    "{public} is a value, a superclass and a record's class"
                );
            }
            // `DataView on:byteOrder:` answers instances of the two byte
            // orders and `IndexedCollection`, public only for the shipped
            // module that applies it, is only mixed in: no facet hands the
            // classes out, so a record's class only.
            for private in [
                "LittleEndianDataView",
                "BigEndianDataView",
                "IndexedCollection",
            ] {
                let positions = positions_of(world, name("Kernel", private, ShippedKind::Class));
                assert!(
                    !positions.value && !positions.superclass && positions.header,
                    "{private} is a record's class only"
                );
            }
            // The typed arrays are private classes of their module whose
            // facet's class accessors answer them: a program names,
            // subclasses and keeps instances of them as of a public class.
            for typed_array in ["TypedArray", "Int16Array"] {
                let positions = positions_of(
                    world,
                    name("TypedArraysFactory", typed_array, ShippedKind::Class),
                );
                assert!(
                    positions.value && positions.superclass && positions.header,
                    "TypedArraysFactory {typed_array} is a value, a superclass and a record's class"
                );
            }
            // The level `<: IndexedCollection` adds is where a frame of its
            // methods runs, and nothing else.
            for (module, applying) in [
                ("TypedArraysFactory", "TypedArray"),
                ("Kernel", "ByteArray"),
            ] {
                assert_eq!(
                    positions_of(world, name(module, applying, ShippedKind::Level(0))),
                    Positions::FRAME_CLASS_ONLY,
                    "{module} {applying} level 0 is a frame's class only"
                );
            }
            let list = positions_of(
                world,
                name("CollectionsFactory", "List", ShippedKind::Class),
            );
            assert!(list.value && list.superclass && list.header);
            // The platform hands out none of its module's classes and the
            // collections and typed arrays facets not their own: a program
            // holds their instances, records of them, but never the classes.
            for (module, declaration) in [
                ("PlatformFactory", "Platform"),
                ("TranscriptFactory", "Transcript"),
                ("PlatformFactory", "KernelNamespace"),
                ("TimeFactory", "Time"),
                ("CollectionsFactory", "Collections"),
                ("TypedArraysFactory", "TypedArrays"),
            ] {
                let positions = positions_of(world, name(module, declaration, ShippedKind::Class));
                assert!(
                    !positions.value && !positions.superclass && positions.header,
                    "{module} {declaration} is a record's class only"
                );
            }
            for root in [
                "Kernel",
                "PlatformFactory",
                "TranscriptFactory",
                "TimeFactory",
                "CollectionsFactory",
                "TypedArraysFactory",
            ] {
                let positions = positions_of(world, name(root, root, ShippedKind::Class));
                assert!(!positions.value && !positions.header && positions.frame_class);
                // Every root is `= Value`, so it applies no mixin and adds
                // no level to name.
                assert!(world
                    .index_of_name(&name(root, root, ShippedKind::Level(0)))
                    .is_none());
                let instance = positions_of(world, name(root, root, ShippedKind::ModuleInstance));
                assert_eq!(
                    instance,
                    Positions {
                        frame_register: true,
                        ..Positions::default()
                    }
                );
            }
        });
    }

    /// `KERNEL_CLASSES_FOR_SHIPPED_MODULES` is exactly the public kernel
    /// classes that are no builtin and that neither an accessor of `Object`
    /// nor one of `KernelNamespace` answers: a kernel class made public
    /// for a shipped module joins it, and one a facet starts handing out
    /// leaves it.
    #[test]
    fn the_kernel_classes_for_shipped_modules_are_exactly_those_no_facet_hands_out() {
        let mut interpreter = crate::Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let heap = mutator.heap();
            let world = &mutator.shipped_world;
            let root_mixin = |module: &str| {
                let instance = world
                    .module_instance(Symbol::intern(module))
                    .unwrap_or_else(|| panic!("{module} is a shipped module"));
                root_application(heap, instance).mixin.clone()
            };
            // The mirror's class has no declaration, so no name.
            let builtins: HashSet<&str> = BuiltinClass::ALL
                .iter()
                .filter(|kind| **kind != BuiltinClass::VmMirror)
                .map(|kind| super::super::kernel::kernel_class_name(*kind))
                .collect();
            let object = mutator.builtin_class(BuiltinClass::Object);
            let object_mixin = heap
                .class_application(object)
                .expect("Object has an application")
                .mixin
                .clone();
            let mut handed_out: HashSet<String> = HashSet::new();
            for entry in object_mixin.methods.values() {
                if let MethodEntry::Compiled(method) = entry {
                    if let Some(crate::mixin::MethodFastPath::BuiltinClass(kind)) = method.fast_path
                    {
                        handed_out.insert(super::super::kernel::kernel_class_name(kind).into());
                    }
                }
            }
            let platform = root_mixin("PlatformFactory");
            let kernel_namespace = platform
                .nested
                .iter()
                .find(|nested| nested.name.as_str() == "KernelNamespace")
                .expect("PlatformFactory declares KernelNamespace");
            for entry in kernel_namespace.methods.values() {
                if let MethodEntry::Compiled(method) = entry {
                    if method.access == AccessModifier::Public {
                        handed_out.insert(method.selector.as_str().to_string());
                    }
                }
            }
            let mut for_shipped_modules: Vec<String> = root_mixin("Kernel")
                .nested
                .iter()
                .filter(|nested| nested.access == AccessModifier::Public)
                .map(|nested| nested.name.as_str().to_string())
                .filter(|name| !builtins.contains(name.as_str()) && !handed_out.contains(name))
                .collect();
            for_shipped_modules.sort();
            let mut listed: Vec<String> = KERNEL_CLASSES_FOR_SHIPPED_MODULES
                .iter()
                .map(|name| (*name).to_string())
                .collect();
            listed.sort();
            assert_eq!(for_shipped_modules, listed);
        });
    }

    /// A private class a public class accessor of its module answers is a
    /// value and a superclass, as a public one is.
    #[test]
    fn a_private_class_a_public_class_accessor_answers_is_a_value() {
        let mut interpreter =
            bootstrap_with(&[("TimeFactory", "Stopwatch"), ("CollectionsFactory", "Map")]);
        interpreter.enter(|mutator| {
            let world = &mutator.shipped_world;
            for (module, declaration) in
                [("TimeFactory", "Stopwatch"), ("CollectionsFactory", "Map")]
            {
                let positions = positions_of(world, name(module, declaration, ShippedKind::Class));
                assert!(
                    positions.value && positions.superclass && positions.header,
                    "{module} {declaration} is a value"
                );
                let metaclass =
                    positions_of(world, name(module, declaration, ShippedKind::Metaclass));
                assert!(metaclass.value && !metaclass.header);
            }
        });
    }

    /// Each name answers the object the boot built: the builtin `Array`
    /// and its metaclass.
    #[test]
    fn a_shipped_name_answers_the_boots_own_object() {
        let mut interpreter = crate::Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let world = &mutator.shipped_world;
            let object_of =
                |shipped| world.entries()[world.index_of_name(&shipped).expect("named")].object;
            let array = mutator.builtin_class(crate::types::BuiltinClass::Array);
            assert!(object_of(name("Kernel", "Array", ShippedKind::Class)).same(array));
            let metaclass = mutator
                .heap()
                .class_metaclass(array)
                .expect("Array has a metaclass");
            assert!(object_of(name("Kernel", "Array", ShippedKind::Metaclass)).same(metaclass));
        });
    }

    /// A boot runs every shipped class's superclass clause and every shipped
    /// module's initializer, and draws no identity hash while it does: the
    /// first hash a program draws is 1, under `run` and `load` alike
    /// (`goldens/library/object/identity_hash.ns`).
    #[test]
    fn a_boot_draws_no_identity_hash() {
        let mut interpreter = crate::Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let object = mutator.new_string(String::new());
            assert_eq!(mutator.heap_mut().object_ensure_identity_hash(object), 1);
        });
    }

    /// What a host send answered, or a panic naming `step`.
    fn expect_ok(result: crate::HostResult, step: &str) -> Root {
        match result {
            Ok(answer) => answer,
            Err(crate::HostSignal::Return) => panic!("{step}: a non-local return escaped"),
            Err(crate::HostSignal::Error(error)) => {
                panic!("{step}: unexpected Newspeak error: {}", error.message)
            }
        }
    }

    /// Building each lazy module on demand, and a class of each, runs
    /// their superclass clauses and initializers and draws no identity
    /// hash either: the first hash a program draws is 1 whichever modules
    /// it used, under `run` and `load` alike.
    #[test]
    fn building_a_module_on_demand_draws_no_identity_hash() {
        let mut interpreter = crate::Interpreter::bootstrap();
        let platform = interpreter.platform();
        for (member, class) in [
            ("collections", "List"),
            ("time", "Stopwatch"),
            ("typedArrays", "Int16Array"),
        ] {
            let facet = expect_ok(
                interpreter.send(&platform, Symbol::intern(member), &[]),
                member,
            );
            expect_ok(interpreter.send(&facet, Symbol::intern(class), &[]), class);
        }
        // The transcript is itself an instance of the module's class
        // `Transcript`, which its `build` instantiates.
        expect_ok(
            interpreter.send(&platform, Symbol::intern("transcript"), &[]),
            "transcript",
        );
        interpreter.enter(|mutator| {
            for module in super::super::core_library::LAZY_SHIPPED_MODULES {
                assert!(
                    mutator
                        .shipped_world
                        .module_instance(Symbol::intern(module))
                        .is_some(),
                    "{module} is built"
                );
            }
            let object = mutator.new_string(String::new());
            assert_eq!(mutator.heap_mut().object_ensure_identity_hash(object), 1);
        });
    }

    /// The class accessor `Other` of a module's class `Inner`, two levels
    /// below the root, as the module compiled it -- resolved with
    /// `resolve_kernel` when `shipped`, so that the accessor is recognized,
    /// and with plain `resolve` otherwise.
    fn inner_class_accessor(shipped: bool) -> Option<crate::mixin::ClassAccessor> {
        let source = indoc::indoc! {"
            class Shipped = Object new () (
              public class Middle = Object new () (
                public class Inner = Object new () (
                  public Other = ( ^outer Shipped Other )
                )
              )
              public class Other = Object new () ()
            ) : ()
        "};
        let module = ns_syntax::parse(source).expect("parse");
        let origin = crate::SourceOrigin::File(std::path::PathBuf::from("t.ns"));
        let resolved = if shipped {
            crate::resolver::resolve_kernel(module, Rc::from(source), origin)
        } else {
            crate::resolve(module, Rc::from(source), origin)
        };
        let Ok(resolved) = resolved else {
            panic!("the module resolves");
        };
        let mut interpreter = crate::Interpreter::bootstrap();
        let class = interpreter.load_module(&resolved);
        interpreter.enter(|mutator| {
            let Some(class) = mutator.fetch(&class).as_object() else {
                panic!("the root is a class object");
            };
            let root = mutator
                .heap()
                .class_application(class)
                .expect("the root has an application")
                .mixin
                .clone();
            let middle = &root.nested[0];
            let inner = &middle.nested[0];
            inner
                .compiled_method(Symbol::intern("Other"))
                .expect("Inner declares Other")
                .class_accessor
        })
    }

    /// A class accessor two levels below its root is recognized at the
    /// depth `outer Shipped` resolved to, which is what lets the class it
    /// answers be a value (`accessed_classes`).
    #[test]
    fn a_class_accessor_two_levels_down_is_recognized() {
        assert_eq!(
            inner_class_accessor(true),
            Some(crate::mixin::ClassAccessor {
                depth: 2,
                class_name: Symbol::intern("Other"),
            })
        );
    }

    /// The same text in a module plain `resolve` resolved is an ordinary
    /// method: only shipped source is recognized.
    #[test]
    fn a_user_modules_class_accessor_is_unmarked() {
        assert_eq!(inner_class_accessor(false), None);
    }

    /// The level `Array`'s `<:` clause adds is a shipped object like the
    /// class itself: named `Kernel Array level 0`, with its metaclass, a
    /// frame's class or a class record's declaring class only; the name
    /// resolves to the boot's own level, and the level leads back to its
    /// entry by its class object. A level applies another declaration's
    /// mixin, so `index_by_mixin` keys none.
    #[test]
    fn a_shipped_level_is_named_and_resolves_to_the_boots_own_object() {
        let mut interpreter = crate::Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let world = &mutator.shipped_world;
            let heap = mutator.heap();
            let array = mutator.builtin_class(BuiltinClass::Array);
            let level = heap
                .class_superclass(array)
                .expect("Array has a superclass");
            let level_metaclass = heap
                .class_metaclass(level)
                .expect("a level has a metaclass");
            let level_name = name("Kernel", "Array", ShippedKind::Level(0));
            let metaclass_name = name("Kernel", "Array", ShippedKind::LevelMetaclass(0));
            assert_eq!(level_name.describe(), "Kernel Array level 0");
            for shipped in [level_name, metaclass_name] {
                assert_eq!(
                    positions_of(world, shipped),
                    Positions::FRAME_CLASS_ONLY,
                    "{}",
                    shipped.describe()
                );
            }
            let resolved =
                crate::image::shipped_names::resolve(&[level_name, metaclass_name], world, false)
                    .unwrap_or_else(|_| panic!("the level's names resolve"));
            assert!(resolved[0].object.same(level));
            assert!(resolved[1].object.same(level_metaclass));
            let by_object = world.index_by_object(heap);
            assert_eq!(
                by_object.get(&heap.object_identity_key(level)),
                world.index_of_name(&level_name).as_ref()
            );
            assert_eq!(
                by_object.get(&heap.object_identity_key(level_metaclass)),
                world.index_of_name(&metaclass_name).as_ref()
            );
            let level_mixin = heap
                .class_application(level)
                .expect("a level is a class")
                .mixin
                .clone();
            let indexed_collection = world
                .index_of_name(&name("Kernel", "IndexedCollection", ShippedKind::Class))
                .expect("Kernel IndexedCollection is named");
            assert_eq!(
                world.index_by_mixin(heap).get(&mixin_key(&level_mixin)),
                Some(&indexed_collection),
                "the level's mixin is IndexedCollection's, keyed by its class"
            );
        });
    }

    /// An object and a class's mixin each lead back to the one entry that
    /// names them: `Array` by its class object and by its mixin. A level
    /// leads back by its class object alone
    /// (`a_shipped_level_is_named_and_resolves_to_the_boots_own_object`).
    #[test]
    fn an_object_and_a_mixin_lead_back_to_their_entry() {
        let mut interpreter = crate::Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let world = &mutator.shipped_world;
            let heap = mutator.heap();
            let by_object = world.index_by_object(heap);
            let by_mixin = world.index_by_mixin(heap);
            assert_eq!(by_object.len(), world.entries().len());
            let array = world
                .index_of_name(&name("Kernel", "Array", ShippedKind::Class))
                .expect("Kernel Array is named");
            let array_class = mutator.builtin_class(crate::types::BuiltinClass::Array);
            assert_eq!(
                by_object.get(&heap.object_identity_key(array_class)),
                Some(&array)
            );
            let application = heap
                .class_application(array_class)
                .expect("Array is a class");
            assert_eq!(by_mixin.get(&mixin_key(&application.mixin)), Some(&array));
            assert_eq!(
                name("Kernel", "Kernel", ShippedKind::LevelMetaclass(0)).describe(),
                "Kernel Kernel level 0 class"
            );
            assert_eq!(
                name(
                    "CollectionsFactory",
                    "CollectionsFactory",
                    ShippedKind::ModuleInstance
                )
                .describe(),
                "CollectionsFactory instance"
            );
        });
    }

    /// Recording a class the world already names adds no entry: a
    /// class's names are recorded once, whichever build records it first.
    #[test]
    fn a_class_is_named_once_however_often_it_is_recorded() {
        let mut interpreter = bootstrap_with(&[("CollectionsFactory", "List")]);
        interpreter.enter(|mutator| {
            let before = mutator.shipped_world.entries().len();
            let collections = Symbol::intern("CollectionsFactory");
            let instance = mutator
                .shipped_world
                .module_instance(collections)
                .expect("CollectionsFactory is built with its List");
            let application = root_application(mutator.heap(), instance);
            let list = application
                .mixin
                .nested
                .iter()
                .find(|nested| nested.name.as_str() == "List")
                .expect("CollectionsFactory declares List")
                .clone();
            let class = application
                .nested_slot
                .get(&mixin_key(&list))
                .and_then(|index| mutator.heap().instance_field(instance, *index))
                .and_then(Value::as_object)
                .expect("List was built and named");
            mutator.record_shipped_class(collections, &list, class);
            assert_eq!(mutator.shipped_world.entries().len(), before);
        });
    }

    /// A shipped module's instance leads back to its module's name; any
    /// other object to none.
    #[test]
    fn a_module_instance_leads_back_to_its_module() {
        let mut interpreter = crate::Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let world = &mutator.shipped_world;
            for &(module, instance) in world.module_instances() {
                assert_eq!(world.module_of_instance(instance), Some(module));
            }
            let array = mutator.builtin_class(BuiltinClass::Array);
            assert_eq!(world.module_of_instance(array), None);
        });
    }
}
