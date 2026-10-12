//! The shipped world: every object the binary owns and an image may name
//! (`docs/design/image-format.md`, "Planned: shipped names in the image").
//! Filled during the boot -- each module instance as
//! `install_shipped_modules` builds it, then every class the roots declare
//! and every name, by `complete_shipped_world`, before the platform is
//! made -- and never changed afterwards.

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
/// "Planned: shipped names in the image", the rule of what may be named
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

/// The shipped modules' instances, in the order the boot built them, the
/// kernel first, and every shipped object under its name.
#[derive(Default)]
pub(crate) struct ShippedWorld<'gc> {
    module_instances: Vec<(Symbol, ObjectReference<'gc>)>,
    /// Per module in `module_instances`' order: the instance, the root
    /// class, its metaclass and its levels, then every class the root
    /// declares, in declaration order, each followed by its metaclass and
    /// its levels.
    entries: Vec<ShippedEntry<'gc>>,
    /// Each entry's name to its position in `entries`.
    by_name: HashMap<ShippedName, usize>,
}

impl<'gc> ShippedWorld<'gc> {
    /// Records `instance` as the one instance of the shipped module `name`.
    /// The boot names each module once; a second instance is a defect in
    /// the binary.
    pub(crate) fn add_module_instance(&mut self, name: Symbol, instance: ObjectReference<'gc>) {
        assert!(
            self.module_instance(name).is_none(),
            "internal: the shipped module {} is instantiated twice",
            name.as_str()
        );
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

    /// Every shipped module's name and instance, in the order the boot
    /// built them.
    pub(crate) fn module_instances(&self) -> &[(Symbol, ObjectReference<'gc>)] {
        &self.module_instances
    }

    /// Every shipped object under its name, in the order the boot recorded
    /// them (`ShippedWorld::entries`' field comment); empty only before
    /// `complete_shipped_world` has run.
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
    }
}

impl crate::interpreter::Interpreter {
    /// The last phase of the boot before the platform is made: every class
    /// a shipped root declares is built, in declaration order, and every
    /// shipped object is recorded under its name with the positions it may
    /// stand in (`docs/design/image-format.md`, "Planned: shipped names in
    /// the image"). An image names into this world and the loader runs
    /// nothing the image chooses, so all of it must exist before a decode.
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
        }
        self.enter(|mutator| {
            let entries = shipped_entries(mutator);
            mutator.shipped_world.record_entries(entries);
        });
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

/// Every shipped object under its name, in the order of
/// `ShippedWorld::entries`' field comment. Every class a root declares has
/// been built, so each cache slot holds its class.
fn shipped_entries<'gc>(mutator: &Mutator<'gc>) -> Vec<ShippedEntry<'gc>> {
    let heap = mutator.heap();
    let mut entries = Vec::new();
    for &(module, instance) in mutator.shipped_world.module_instances() {
        let root_class = heap.object_class(instance);
        let application = root_application(heap, instance);
        entries.push(ShippedEntry {
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
        });
        push_class(
            heap,
            &mut entries,
            module,
            module,
            root_class,
            Positions::FRAME_CLASS_ONLY,
        );
        let accessed = accessed_classes(&application.mixin);
        for nested in &application.mixin.nested {
            let class = application
                .nested_slot
                .get(&mixin_key(nested))
                .and_then(|index| heap.instance_field(instance, *index))
                .and_then(Value::as_object)
                .unwrap_or_else(|| {
                    panic!(
                        "internal: {} {} was built at bootstrap",
                        module.as_str(),
                        nested.name.as_str()
                    )
                });
            let value = nested.access == AccessModifier::Public || accessed.contains(&nested.name);
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
            let positions = Positions {
                value,
                superclass,
                header,
                frame_class: true,
                frame_register: false,
            };
            push_class(heap, &mut entries, module, nested.name, class, positions);
        }
    }
    entries
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

    fn positions_of(world: &ShippedWorld<'_>, shipped: ShippedName) -> Positions {
        let index = world
            .index_of_name(&shipped)
            .unwrap_or_else(|| panic!("the shipped world has no {}", shipped.describe()));
        world.entries()[index].positions
    }

    /// The names the image format relies on all exist, and each may stand
    /// exactly where the rule of `image-format.md` allows.
    #[test]
    fn every_shipped_object_is_named_with_the_positions_the_rule_allows() {
        let mut interpreter = crate::Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let world = &mutator.shipped_world;
            let array = positions_of(world, name("Kernel", "Array", ShippedKind::Class));
            assert!(array.value && array.superclass && array.header && array.frame_class);
            assert!(!array.frame_register);
            let array_class = positions_of(world, name("Kernel", "Array", ShippedKind::Metaclass));
            assert!(array_class.value && !array_class.header && array_class.frame_class);
            for private in [
                "Top",
                "Activation",
                "FutureImplementation",
                "NonBooleanReceiver",
            ] {
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
            for meta in ["Behavior", "Class", "Metaclass"] {
                let positions = positions_of(world, name("Kernel", meta, ShippedKind::Class));
                assert!(
                    positions.value && !positions.superclass,
                    "{meta} is no superclass"
                );
            }
            let list = positions_of(
                world,
                name("CollectionsFactory", "List", ShippedKind::Class),
            );
            assert!(list.value && list.superclass && list.header);
            let platform = positions_of(
                world,
                name("PlatformFactory", "Platform", ShippedKind::Class),
            );
            assert!(platform.value && platform.header);
            for root in ["Kernel", "PlatformFactory", "CollectionsFactory"] {
                let positions = positions_of(world, name(root, root, ShippedKind::Class));
                assert!(!positions.value && !positions.header && positions.frame_class);
                let level = positions_of(world, name(root, root, ShippedKind::Level(0)));
                assert!(!level.value && !level.header && level.frame_class);
                positions_of(world, name(root, root, ShippedKind::LevelMetaclass(0)));
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

    /// A private class a public class accessor of its module answers is a
    /// value and a superclass, as a public one is.
    #[test]
    fn a_private_class_a_public_class_accessor_answers_is_a_value() {
        let mut interpreter = crate::Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let world = &mutator.shipped_world;
            for (module, declaration) in [
                ("PlatformFactory", "Transcript"),
                ("PlatformFactory", "Stopwatch"),
                ("CollectionsFactory", "Map"),
            ] {
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

    /// An object and a class's mixin each lead back to the one entry that
    /// names them: `Array` by its class object and by its mixin, and a
    /// level by its class object alone.
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
            let level = world
                .index_of_name(&name("Kernel", "Kernel", ShippedKind::Level(0)))
                .expect("Kernel Kernel level 0 is named");
            let level_object = world.entries()[level].object;
            assert_eq!(
                by_object.get(&heap.object_identity_key(level_object)),
                Some(&level)
            );
            assert!(!by_mixin.values().any(|index| *index == level));
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
}
