//! The bootstrap's kernel phase (`docs/design/core-library.md`, "The
//! bootstrap: placeholders first, kernel second", which lists its steps):
//! `Kernel.ns`, compiled, fills the builtin class objects `World::new` built
//! as placeholders, and is then instantiated with the VM mirror.

use std::rc::Rc;

use ns_syntax::ast;

use crate::heap::{Heap, Root};
use crate::interpreter::core_library::KERNEL_MODULE;
use crate::interpreter::{HostSignal, Interpreter, Mutator};
use crate::mixin::Mixin;
use crate::types::*;

/// `Kernel.ns`, compiled into the binary: the library is source the runtime
/// ships (`docs/design/core-library.md`, "Where the library source lives,
/// and how modules are found").
pub(crate) const KERNEL_SOURCE: &str = include_str!("../../core-library/Kernel.ns");

/// `Kernel.ns`'s path below `core-library/`, beside its `include_str!`.
pub(crate) const KERNEL_PATH: &str = "Kernel.ns";

thread_local! {
    /// The kernel, parsed and resolved once per thread: the AST and its
    /// bindings are immutable, and the test suite bootstraps well over a
    /// hundred interpreters. Each interpreter still compiles it: compiling
    /// needs no heap, but the boot builds its world from its own mixins.
    static RESOLVED_KERNEL: Rc<ResolvedModule> = Rc::new(resolve_kernel_source());
}

pub(crate) fn resolved_kernel() -> Rc<ResolvedModule> {
    RESOLVED_KERNEL.with(Rc::clone)
}

/// A kernel that does not parse or resolve is a defect in the binary, not
/// in a program, so it panics; `every_send_to_vm_mirror_in_the_kernel_is_marked`
/// and every other test that bootstraps catch it before a release could.
fn resolve_kernel_source() -> ResolvedModule {
    let module = match ns_syntax::parse(KERNEL_SOURCE) {
        Ok(module) => module,
        Err(diagnostics) => panic!(
            "internal: Kernel.ns does not parse: {}",
            diagnostics
                .iter()
                .map(|diagnostic| diagnostic.message.clone())
                .collect::<Vec<_>>()
                .join("; ")
        ),
    };
    match crate::resolver::resolve_kernel(
        module,
        Rc::from(KERNEL_SOURCE),
        SourceOrigin::File(std::path::PathBuf::from("Kernel.ns")),
    ) {
        Ok(mut resolved) => {
            resolved.identity = Some(super::core_library::checked_shipped_identity(
                &resolved,
                KERNEL_MODULE,
            ));
            resolved
        }
        Err(errors) => panic!(
            "internal: Kernel.ns does not resolve: {}",
            errors
                .iter()
                .map(|error| error.message.clone())
                .collect::<Vec<_>>()
                .join("; ")
        ),
    }
}

/// The name of `kind`'s class in `Kernel.ns`. Every builtin but the mirror's
/// class has one, `Top` included, unlike the narrower list of
/// names a *program* may write bare -- `Object`'s own accessor methods, which
/// `tests::OBJECT_CLASS_ACCESSORS` pins.
pub(crate) fn kernel_class_name(kind: BuiltinClass) -> &'static str {
    match kind {
        BuiltinClass::Object => "Object",
        BuiltinClass::Boolean => "Boolean",
        BuiltinClass::True => "True",
        BuiltinClass::False => "False",
        BuiltinClass::Integer => "Integer",
        BuiltinClass::Float => "Float",
        BuiltinClass::String => "String",
        BuiltinClass::Symbol => "Symbol",
        BuiltinClass::Array => "Array",
        BuiltinClass::UndefinedObject => "UndefinedObject",
        BuiltinClass::Closure => "Closure",
        BuiltinClass::Top => "Top",
        BuiltinClass::Value => "Value",
        BuiltinClass::Number => "Number",
        BuiltinClass::Behavior => "Behavior",
        BuiltinClass::Class => "Class",
        BuiltinClass::Metaclass => "Metaclass",
        BuiltinClass::Activation => "Activation",
        BuiltinClass::Exception => "Exception",
        BuiltinClass::Error => "Error",
        BuiltinClass::MessageNotUnderstood => "MessageNotUnderstood",
        BuiltinClass::Message => "Message",
        BuiltinClass::Halt => "Halt",
        BuiltinClass::FutureImplementation => "FutureImplementation",
        BuiltinClass::LargeInteger => "LargeInteger",
        BuiltinClass::ArgumentError => "ArgumentError",
        BuiltinClass::CannotReturn => "CannotReturn",
        BuiltinClass::NonBooleanReceiver => "NonBooleanReceiver",
        BuiltinClass::CannotInstantiate => "CannotInstantiate",
        BuiltinClass::Proxy => "Proxy",
        BuiltinClass::Future => "Future",
        BuiltinClass::Collection => "Collection",
        BuiltinClass::Fraction => "Fraction",
        BuiltinClass::ByteArray => "ByteArray",
        BuiltinClass::WeakArray => "WeakArray",
        BuiltinClass::Ephemeron => "Ephemeron",
        BuiltinClass::Mixin => "Mixin",
        BuiltinClass::VmMirror => {
            unreachable!("internal: the mirror's class is not in Kernel.ns")
        }
    }
}

impl Interpreter {
    /// The kernel phase of the bootstrap: fill the placeholders, instantiate
    /// the kernel with the mirror, wire it in. The instantiation is a run
    /// like any other, entered from the outside (`Interpreter::instantiate`),
    /// so the stress modes see it too.
    pub(crate) fn install_kernel(&mut self) {
        let (kernel_class, applied_classes, mirror) = self.enter(|mutator| {
            let root_mixin = mutator.shipped_root_mixin(KERNEL_MODULE);
            let (kernel_class, applied_classes) = mutator.fill_builtin_classes(&root_mixin);
            let applied_classes: Vec<(Rc<Mixin>, Root)> = applied_classes
                .into_iter()
                .map(|(mixin, class)| (mixin, mutator.stash(Value::object(class))))
                .collect();
            let mirror = mutator.vm_mirror();
            (
                mutator.stash(Value::object(kernel_class)),
                applied_classes,
                mutator.stash(mirror),
            )
        });
        let kernel = match self.instantiate(&kernel_class, &[mirror]) {
            Ok(kernel) => kernel,
            Err(HostSignal::Error(error)) => {
                panic!(
                    "internal: Kernel.ns failed to instantiate: {}",
                    error.message
                )
            }
            Err(HostSignal::Return) => {
                panic!("internal: a return escaped Kernel.ns's instantiation")
            }
        };
        self.enter(|mutator| mutator.wire_kernel_in(&kernel_class, &kernel, &applied_classes));
    }
}

/// Test support: reading a kernel class's own method table, which no
/// program can reflect on.
impl<'gc> Mutator<'gc> {
    /// Every selector the kernel class `kind`'s own mixin declares public.
    #[doc(hidden)]
    pub fn public_selectors_of(&self, kind: BuiltinClass) -> Vec<crate::Symbol> {
        let Some(application) = self.heap.class_application(self.builtin_class(kind)) else {
            return Vec::new();
        };
        application
            .mixin
            .methods
            .iter()
            .filter(|(_, entry)| entry.access() == ast::AccessModifier::Public)
            .map(|(selector, _)| *selector)
            .collect()
    }

    /// The access of `selector` in the kernel class `kind`'s own mixin, if
    /// it declares one.
    #[doc(hidden)]
    pub fn access_of(
        &self,
        kind: BuiltinClass,
        selector: crate::Symbol,
    ) -> Option<ast::AccessModifier> {
        let application = self.heap.class_application(self.builtin_class(kind))?;
        application
            .mixin
            .methods
            .get(&selector)
            .map(|entry| entry.access())
    }
}

impl<'gc> Mutator<'gc> {
    /// Every placeholder gets the application of its kernel class, over its
    /// superclass's -- `fill_order` puts every superclass ahead of its
    /// subclasses, so that one is filled already -- and the mirror's class,
    /// which `Kernel.ns` does not declare, keeps its mixin over the filled
    /// `Value`. A builtin whose clause applies kernel mixins (`Array =
    /// Collection <: IndexedCollection`) is applied over the levels §6.2
    /// adds for them, built here from the classes those clauses name
    /// (`applied_kernel_class`). Then the kernel's own class is built over
    /// the filled `Value`, every class object built so far is given its
    /// metaclass over the filled `Class`, and the kernel's class is answered
    /// for the host to instantiate, with each applied kernel class and its
    /// mixin, for `wire_kernel_in` to cache in the kernel instance.
    ///
    /// Nothing has run yet, so no inline cache or instance built before
    /// this phase can still hold a placeholder application.
    fn fill_builtin_classes(
        &mut self,
        root_mixin: &Rc<Mixin>,
    ) -> (ClassReference<'gc>, Vec<(Rc<Mixin>, ClassReference<'gc>)>) {
        let mut applied_classes: Vec<(Rc<Mixin>, ClassReference<'gc>)> = Vec::new();
        let mut levels: Vec<ClassReference<'gc>> = Vec::new();
        let mut filled: Vec<BuiltinClass> = Vec::new();
        for kind in fill_order(&self.heap, root_mixin) {
            let class = self.builtin_class(kind);
            let mixin = if kind == BuiltinClass::VmMirror {
                self.heap
                    .class_application(class)
                    .map(|application| application.mixin.clone())
                    .expect("internal: the mirror's class has its placeholder application")
            } else {
                nested_named(&self.heap, root_mixin, kernel_class_name(kind))
            };
            let superclass = superclass_of_kind(&self.heap, root_mixin, kind)
                .map(|superclass| self.builtin_class(superclass));
            let super_application =
                superclass.and_then(|superclass| self.heap.class_application(superclass));
            // Recorded before `Value`'s own application is built, not after
            // the loop: `apply_mixin` asks whether the mixin it is applying
            // is this one, so that `Value`'s own application carries
            // §3.1.1's "its class inherits from class `Value`" too (§3.2's
            // "subclass of" is inclusive). `fill_order` puts `Value` ahead
            // of every class below it, and each of those reads the flag off
            // its superclass; recorded after the loop, `String` and every
            // other value class would answer that it does not inherit
            // `Value`.
            if kind == BuiltinClass::Value {
                self.value_mixin = Some(mixin.clone());
            }
            // Recorded before `Behavior`'s own application is built, for the
            // reason `value_mixin` is: `apply_mixin` refuses to instantiate
            // `Behavior` and everything below it
            // (`InstantiationRefusal::ClassObject`) by asking whether the
            // mixin it is applying is this one.
            if kind == BuiltinClass::Behavior {
                self.behavior_mixin = Some(mixin.clone());
            }
            // The levels a `<:` clause adds sit between the builtin and the
            // superclass its clause names, so the builtin's own mixin is
            // applied over the topmost of them (§6.2), as a nested class's is
            // (`nested::after_clause`). The clause is read, never run: each
            // class it names is a kernel class built here from builtins
            // alone, which `check_kernel_shape` has made sure of.
            let applied: Vec<(usize, Value<'gc>)> = mixin
                .mixin_applications
                .iter()
                .enumerate()
                .map(|(index, clause)| {
                    let receiver = clause.receiver.unwrap_or_else(|| {
                        panic!("internal: Kernel.ns's {kind:?} applies a clause with no receiver")
                    });
                    let named = self.applied_kernel_class(
                        root_mixin,
                        receiver.as_str(),
                        &filled,
                        &mut applied_classes,
                    );
                    (index, Value::object(named))
                })
                .collect();
            let (superclass, super_application) = if applied.is_empty() {
                (superclass, super_application)
            } else {
                let below = superclass.zip(super_application);
                let Ok(top) = super::nested::build_applied_levels(self, &mixin, below, &applied)
                else {
                    panic!(
                        "internal: Kernel.ns's {kind:?} applies a class that is no class object"
                    );
                };
                let (top, top_application) =
                    top.expect("internal: a builtin that applies a mixin has a superclass");
                let mut level = Some(top);
                while let Some(class) =
                    level.filter(|class| self.heap.class_is_applied_level(*class))
                {
                    levels.push(class);
                    level = self.heap.class_superclass(class);
                }
                (Some(top), Some(top_application))
            };
            let application = self.apply_mixin(&mixin, super_application.as_ref());
            if !self.heap.class_set_application(class, application) {
                unreachable!("internal: a builtin class is a class object");
            }
            let linked = match superclass {
                Some(superclass) => self.heap.class_set_superclass(class, superclass),
                None => self.heap.class_clear_superclass(class),
            };
            if !linked {
                unreachable!("internal: a builtin class is a class object");
            }
            filled.push(kind);
        }
        // Recorded once the classes are filled, because it is read off
        // `Value`'s own filled application: `builtins::value` tells an
        // instance that inherits this `=` from one that overrides it by the
        // method's identity, and while `Value` declares none the derivation
        // declines every instance.
        self.value_equality_method = self
            .heap
            .class_application(self.builtin_class(BuiltinClass::Value))
            .and_then(|application| {
                application
                    .mixin
                    .compiled_method(crate::Symbol::EQUALS)
                    .cloned()
            });
        self.refresh_immediate_dispatch_ids();
        // The root is built as a user root is, by the root rule's shape
        // (`load_root_class`), from the filled `Object` and `Value`; its
        // lexical context is set once the kernel instance exists
        // (`wire_kernel_in`), as every builtin class's is. `Kernel.ns`'s root
        // is `= Value`, so it adds no level, but a `<:` clause would be
        // built here the same way.
        let shape = root_mixin
            .root_shape
            .unwrap_or_else(|| panic!("internal: Kernel.ns's root breaks the root rule"));
        let superclass_kind = if shape.over_value {
            BuiltinClass::Value
        } else {
            BuiltinClass::Object
        };
        let superclass = self.builtin_class(superclass_kind);
        let superclass_application = self.heap.class_application(superclass);
        let applied: Vec<(usize, Value<'gc>)> = if shape.applies_value {
            vec![(0, Value::object(self.builtin_class(BuiltinClass::Value)))]
        } else {
            Vec::new()
        };
        let top = match super::nested::build_applied_levels(
            self,
            root_mixin,
            superclass_application.map(|application| (superclass, application)),
            &applied,
        ) {
            Ok(top) => top,
            Err(_) => panic!("internal: the kernel root's superclass is a class object"),
        };
        let (superclass, super_application) = top.unzip();
        let application = self.apply_mixin(root_mixin, super_application.as_ref());
        let kernel_class = self.new_class_object(application, None, None, superclass);
        // `Class` is filled, so every class object built so far -- each
        // builtin, each applied kernel class and the levels it adds, the
        // `Value` level and the kernel's own class -- can get its metaclass
        // now, and every one built from here on gets it with the class
        // object.
        let mut built: Vec<ClassReference<'gc>> =
            applied_classes.iter().map(|(_, class)| *class).collect();
        built.extend(levels);
        if shape.applies_value
            && let Some(level) = superclass
        {
            built.push(level);
        }
        built.push(kernel_class);
        self.give_existing_class_objects_metaclasses(&built);
        (kernel_class, applied_classes)
    }

    /// The class of the kernel's nested class `name`, which a builtin's
    /// `<:` clause applies: built once, the first time a clause names it,
    /// and answered from `built` after that, so that every builtin applying
    /// it applies one class -- the one `wire_kernel_in` caches in the
    /// kernel instance, which the class's accessor then answers.
    ///
    /// Built as its accessor would build it (`nested::after_clause`), its
    /// mixin over the builtin its clause names, which `check_kernel_shape`
    /// has made sure is a bare builtin name; the kernel instance does not
    /// exist yet, so its lexical context waits for `wire_kernel_in`.
    fn applied_kernel_class(
        &mut self,
        root_mixin: &Rc<Mixin>,
        name: &str,
        filled: &[BuiltinClass],
        built: &mut Vec<(Rc<Mixin>, ClassReference<'gc>)>,
    ) -> ClassReference<'gc> {
        if let Some((_, class)) = built.iter().find(|(mixin, _)| mixin.name.as_str() == name) {
            return *class;
        }
        let mixin = nested_named(&self.heap, root_mixin, name);
        let superclass_kind = superclass_kind(&mixin)
            .unwrap_or_else(|| panic!("internal: Kernel.ns's {name} names no superclass"));
        let superclass = self.builtin_class(superclass_kind);
        let super_application = self
            .heap
            .class_application(superclass)
            .expect("internal: a builtin class is a class object");
        // A placeholder's mixin was built from no source: applied over it,
        // the class would inherit nothing of its superclass. `fill_order`
        // ranks builtins only, so this is where a kernel that applies a
        // class before its superclass is filled is caught.
        assert!(
            filled.contains(&superclass_kind),
            "internal: Kernel.ns's {name} is applied before its superclass {superclass_kind:?} is filled"
        );
        let application = self.apply_mixin(&mixin, Some(&super_application));
        // §3.1.1's enclosing-object condition is asked of a value class as
        // its accessor builds it, against the enclosing object, which does
        // not exist yet.
        assert!(
            !application.inherits_value,
            "internal: Kernel.ns's {name} is a value class, which the kernel phase cannot build before the kernel instance exists"
        );
        let class = self.new_class_object(application, None, None, Some(superclass));
        built.push((mixin, class));
        class
    }

    /// The kernel instance becomes every kernel class's enclosing object,
    /// and each of its hidden nested-class slots holds the builtin class it
    /// names -- so the nested-class accessor in kernel code answers the
    /// builtin class itself rather than building a second class object of
    /// the same mixin. So does the slot of each kernel class a builtin
    /// applies (`applied_classes`, which `fill_builtin_classes` built), and
    /// every level built before the kernel instance existed, the kernel's
    /// own and each builtin's, takes the kernel's context too.
    fn wire_kernel_in(
        &mut self,
        kernel_class: &Root,
        kernel: &Root,
        applied_classes: &[(Rc<Mixin>, Root)],
    ) {
        let Some(kernel_class) = self.fetch(kernel_class).as_object() else {
            unreachable!("internal: the kernel class is a class object");
        };
        let Some(kernel) = self.fetch(kernel).as_object() else {
            unreachable!("internal: the kernel instance is an object");
        };
        let mirror_field = mirror_slot_of(self, kernel);
        let holds_the_mirror = self
            .heap
            .instance_field(kernel, mirror_field)
            .is_some_and(|value| self.is_vm_mirror(value));
        assert!(
            holds_the_mirror,
            "internal: Kernel's vmMirror slot must hold the VM mirror"
        );
        for kind in BuiltinClass::ALL {
            if kind == BuiltinClass::VmMirror {
                continue;
            }
            let class = self.builtin_class(kind);
            let field = hidden_slot_of(self, kernel, kind);
            debug_assert!(
                self.heap
                    .instance_field(kernel, field)
                    .is_some_and(|value| value.is_nil()),
                "internal: Kernel's initializer touched the nested class {kind:?} before the bootstrap could fill it"
            );
            if !self
                .heap
                .instance_set_field(kernel, field, Value::object(class))
            {
                unreachable!("internal: the kernel instance has a field per nested class");
            }
            self.set_kernel_lexical_context(class, kernel, kernel_class);
            // Its levels were built from classes whose context was still
            // empty.
            self.set_levels_kernel_lexical_context(class, kernel, kernel_class);
        }
        for (mixin, class) in applied_classes {
            let Some(class) = self.fetch(class).as_object() else {
                unreachable!("internal: an applied kernel class is a class object");
            };
            let field = hidden_slot_of_mixin(self, kernel, mixin);
            debug_assert!(
                self.heap
                    .instance_field(kernel, field)
                    .is_some_and(|value| value.is_nil()),
                "internal: Kernel's initializer touched the nested class {} before the bootstrap could fill it",
                mixin.name
            );
            if !self
                .heap
                .instance_set_field(kernel, field, Value::object(class))
            {
                unreachable!("internal: the kernel instance has a field per nested class");
            }
            self.set_kernel_lexical_context(class, kernel, kernel_class);
        }
        // The kernel's own `Value` level was built before the kernel
        // instance existed, over `Value`'s then-empty context.
        self.set_levels_kernel_lexical_context(kernel_class, kernel, kernel_class);
        self.kernel = Some(Value::object(kernel));
    }

    /// Gives `class` and its metaclass the kernel's lexical context: the
    /// kernel instance and the kernel's class. The metaclass was built at
    /// the end of the fill, when `class`'s context was still empty; §6.3
    /// puts a class side in its class's declaration, so it takes the same
    /// context.
    fn set_kernel_lexical_context(
        &mut self,
        class: ClassReference<'gc>,
        kernel: ObjectReference<'gc>,
        kernel_class: ClassReference<'gc>,
    ) {
        if !self
            .heap
            .class_set_lexical_context(class, Some(kernel), Some(kernel_class))
        {
            unreachable!("internal: a kernel class is a class object");
        }
        self.set_metaclass_lexical_context(class, kernel, kernel_class);
    }

    /// Gives every level `class`'s own `<:` clauses added, the levels
    /// directly below it, the kernel's lexical context: each was built from
    /// a kernel class (§6.2: the level's enclosing object is the applied
    /// class's) before the kernel instance existed.
    fn set_levels_kernel_lexical_context(
        &mut self,
        class: ClassReference<'gc>,
        kernel: ObjectReference<'gc>,
        kernel_class: ClassReference<'gc>,
    ) {
        let mut level = self.heap.class_superclass(class);
        while let Some(class) = level.filter(|class| self.heap.class_is_applied_level(*class)) {
            self.set_kernel_lexical_context(class, kernel, kernel_class);
            level = self.heap.class_superclass(class);
        }
    }

    /// Gives `class`'s metaclass the lexical context `wire_kernel_in` just
    /// gave `class`: the kernel instance and the kernel's class.
    fn set_metaclass_lexical_context(
        &mut self,
        class: ClassReference<'gc>,
        kernel: ObjectReference<'gc>,
        kernel_class: ClassReference<'gc>,
    ) {
        if let Some(metaclass) = self.heap.class_metaclass(class)
            && !self
                .heap
                .class_set_lexical_context(metaclass, Some(kernel), Some(kernel_class))
        {
            unreachable!("internal: a metaclass is a class object");
        }
    }
}

/// The kernel's nested class called `name`, its current version
/// (`Heap::nested_mixins`): the fill applies the nested classes one by one,
/// and the first application registers the root with its whole tree.
/// `check_kernel_shape` has made sure there is exactly one.
fn nested_named(heap: &Heap<'_>, root: &Rc<Mixin>, name: &str) -> Rc<Mixin> {
    heap.nested_mixins(root)
        .into_iter()
        .find(|nested| nested.name.as_str() == name)
        .unwrap_or_else(|| panic!("internal: Kernel.ns declares no class {name}"))
}

/// The order the fill walks: every superclass before its subclasses.
/// `BuiltinClass::ALL` is the wire-tag order, and a tag is assigned once and
/// never renumbered, so a class declared later is appended rather than
/// slotted in beside its siblings -- `Value` follows `Top` there while
/// standing above `Boolean`. Filling in that order would apply a mixin over
/// a superclass whose application is still the empty placeholder. Sorting by
/// depth below `Top` puts every superclass first whatever order `ALL` grows
/// in, and is stable, so classes at one depth keep `ALL`'s order.
///
/// `sort_by_cached_key`, not `sort_by_key`: `depth_below_top` walks the
/// chain and allocates, and every bootstrap runs this -- the test suite
/// bootstraps well over a hundred interpreters.
fn fill_order(heap: &Heap<'_>, root: &Rc<Mixin>) -> Vec<BuiltinClass> {
    let mut order: Vec<BuiltinClass> = BuiltinClass::ALL.into_iter().collect();
    order.sort_by_cached_key(|kind| depth_below_top(heap, root, *kind));
    order
}

/// How many superclass links separate `kind` from `Top`, which is 0 links
/// from itself. A chain that repeats a class or ends anywhere but `Top` is a
/// defect in `Kernel.ns`, not in a program, so it panics naming where the
/// walk went wrong: a subclass whose chain does not reach the root would be
/// filled over a placeholder, and a cycle would not terminate.
fn depth_below_top(heap: &Heap<'_>, root: &Rc<Mixin>, kind: BuiltinClass) -> usize {
    let mut walked: Vec<BuiltinClass> = Vec::with_capacity(BuiltinClass::ALL.len());
    let mut current = kind;
    while let Some(superclass) = superclass_of_kind(heap, root, current) {
        assert!(
            !walked.contains(&superclass),
            "internal: Kernel.ns's superclass chain from {kind:?} returns to {superclass:?}"
        );
        walked.push(current);
        current = superclass;
    }
    assert_eq!(
        current,
        BuiltinClass::Top,
        "internal: Kernel.ns's superclass chain from {kind:?} ends at {current:?}, not at Top"
    );
    walked.len()
}

/// The builtin that `kind`'s class names as its superclass; `None` for
/// `Top`, which has none. The mirror's class is not in `Kernel.ns` at all,
/// and sits under `Value`, so the mirror is a value by the ordinary rule,
/// with no slots and no `==`, as SOMns asserts (`vm/ObjectSystem.java`,
/// `assert vmMirror.isValue()`). `fill_order` sorts by depth, so `Value` is
/// still filled first.
fn superclass_of_kind(
    heap: &Heap<'_>,
    root: &Rc<Mixin>,
    kind: BuiltinClass,
) -> Option<BuiltinClass> {
    if kind == BuiltinClass::VmMirror {
        return Some(BuiltinClass::Value);
    }
    superclass_kind(&nested_named(heap, root, kernel_class_name(kind)))
}

/// The builtin a kernel class's clause names, read off its mixin and never
/// run; `None` for `Top`.
fn superclass_kind(nested: &Rc<Mixin>) -> Option<BuiltinClass> {
    let receiver = nested.superclass_receiver?;
    Some(kernel_kind_named(receiver.as_str()).unwrap_or_else(|| {
        panic!(
            "internal: Kernel.ns's {} names no kernel class",
            nested.name
        )
    }))
}

/// The builtin whose kernel class is called `name`, if any.
pub(crate) fn kernel_kind_named(name: &str) -> Option<BuiltinClass> {
    BuiltinClass::ALL
        .into_iter()
        .find(|kind| *kind != BuiltinClass::VmMirror && kernel_class_name(*kind) == name)
}

/// The shape the phase relies on and the resolver cannot check: one nested
/// class per builtin but the mirror's, named as `kernel_class_name` says,
/// each declaring no slots -- but the exception classes, `Message`,
/// `FutureImplementation` and `Ephemeron`, whose first slot is its key --
/// and the default factory `new` -- but `FutureImplementation` and
/// `WeakArray`, which name their own; a root class with
/// exactly one slot, the immutable `vmMirror`; a clause on every builtin's
/// class but `Top`, a bare name with no message arguments -- it is read,
/// never run -- naming a builtin, from which the chain reaches `Top` without
/// repeating, so that `fill_order` can sort by depth, and `<:` clauses only
/// as `check_applied_kernel_class` allows them. Every other nested class is
/// an ordinary class, unchecked here: its accessor builds it, running its
/// clause, as any class's does. And every mirror send passes a literal
/// block only as its failure block (`check_mirror_send_arguments`).
pub(super) fn check_kernel_shape(heap: &Heap<'_>, root: &Rc<Mixin>, resolved: &ResolvedModule) {
    check_mirror_send_arguments(resolved, KERNEL_MODULE);
    let kinds: Vec<BuiltinClass> = BuiltinClass::ALL
        .into_iter()
        .filter(|kind| *kind != BuiltinClass::VmMirror)
        .collect();
    assert!(
        root.slots.len() == 1
            && root.slots[0].name.as_str() == "vmMirror"
            && !root.slots[0].mutable,
        "internal: Kernel.ns's root class must declare exactly one slot, the immutable vmMirror"
    );
    for kind in kinds.iter().copied() {
        let name = kernel_class_name(kind);
        let matching = heap
            .nested_mixins(root)
            .iter()
            .filter(|nested| nested.name.as_str() == name)
            .count();
        assert_eq!(
            matching, 1,
            "internal: Kernel.ns must declare the class {name} exactly once"
        );
        let nested = nested_named(heap, root, name);
        // Rust builds a builtin's instances with a payload and no fields, so
        // a slot would have nowhere to live -- except on the exception
        // classes, whose instances `allocate_for_factory` gives an ordinary
        // field vector like any other class's (`docs/design/core-library.md`,
        // "Exceptions"). `Activation` declares no slots of its own: an
        // activation's state lives in the running activation it wraps.
        // `FutureImplementation`'s two slots are an ordinary field vector
        // too, which `interpreter::future` allocates and alone reads and
        // writes. `Ephemeron`'s three are the fields of its own format,
        // which `allocate_for_factory` allocates and only its mirror leaves
        // reach (`builtins/weak.rs`); declared with no initializer, since
        // an initializer's slot store does not reach that format.
        assert!(
            nested.slots.is_empty()
                || matches!(
                    kind,
                    BuiltinClass::Exception
                        | BuiltinClass::Error
                        | BuiltinClass::MessageNotUnderstood
                        | BuiltinClass::Message
                        | BuiltinClass::Halt
                        | BuiltinClass::ArgumentError
                        | BuiltinClass::CannotReturn
                        | BuiltinClass::NonBooleanReceiver
                        | BuiltinClass::CannotInstantiate
                        | BuiltinClass::FutureImplementation
                        | BuiltinClass::Ephemeron
                ),
            "internal: Kernel.ns's {name} declares a slot; a builtin's instances have none"
        );
        // The collectors read an ephemeron's first field as its key, and
        // `Object` above it declares none, so the key must be its first
        // slot.
        if kind == BuiltinClass::Ephemeron {
            let names: Vec<&str> = nested.slots.iter().map(|slot| slot.name.as_str()).collect();
            assert_eq!(
                names,
                ["storedKey", "storedValue", "storedFinalizer"],
                "internal: Kernel.ns's Ephemeron must declare its key first, then its value and finalizer"
            );
            // Its instantiation writes no slot: an initializer's store
            // would meet the `Ephemeron` format, which the slot accessors
            // refuse.
            assert!(
                nested.slots.iter().all(|slot| !slot.lazy
                    && slot
                        .declaration
                        .as_ref()
                        .is_some_and(|declaration| declaration.initializer.is_none())),
                "internal: Kernel.ns's Ephemeron must declare its slots without initializers"
            );
        }
        let declaration = nested
            .declaration
            .as_ref()
            .unwrap_or_else(|| panic!("internal: Kernel.ns's {name} was built from no source"));
        // `FutureImplementation` takes its computation through its
        // factory, so that `Future computing:` stores it untouched; a
        // simultaneous slot clause makes its futures without sending it.
        // `WeakArray` names its factory so that `WeakArray new` is not
        // understood: its class side's `new:` makes one with slots, and a
        // `new` would answer an instance holding none.
        assert!(
            declaration.factory.is_none()
                || matches!(
                    kind,
                    BuiltinClass::FutureImplementation | BuiltinClass::WeakArray
                ),
            "internal: Kernel.ns's {name} declares a factory; a builtin's is `new`"
        );
        match (&declaration.superclass, kind) {
            (None, BuiltinClass::Top) => {}
            (Some(_), BuiltinClass::Top) => {
                panic!("internal: Kernel.ns's Top declares a superclass clause")
            }
            (None, _) => panic!("internal: Kernel.ns's {name} declares no superclass clause"),
            (Some(clause), _) => check_clause(name, kind, clause),
        }
        for applied in &declaration.mixin_applications {
            check_applied_kernel_class(heap, root, name, applied);
        }
    }
    // Every chain ends at `Top` without repeating, which is what lets
    // `fill_order` sort the kinds by depth. Checked once the clauses are
    // known to be readable, so a malformed one is reported as itself rather
    // than as a broken chain.
    for kind in kinds {
        depth_below_top(heap, root, kind);
    }
}

/// A kernel class's clause is read, never run: a bare name, no message
/// arguments, and a kernel class. §3.2 reserves `Top` for `Object` alone,
/// so no other class may name it -- and, since `Top` is private, as only
/// the kernel classes no program may name are, no module outside the
/// kernel can either.
fn check_clause(name: &str, kind: BuiltinClass, clause: &ast::SuperclassClause) {
    assert!(
        clause.prefix.is_none()
            && clause
                .messages
                .iter()
                .all(|message| message.arguments.is_empty()),
        "internal: Kernel.ns's {name} must name its superclass by a bare name, with no message arguments"
    );
    let superclass = kernel_kind_named(&clause.receiver.name).unwrap_or_else(|| {
        panic!(
            "internal: Kernel.ns's {name} names {}, which is no kernel class",
            clause.receiver.name
        )
    });
    assert!(
        superclass != BuiltinClass::Top || kind == BuiltinClass::Object,
        "internal: Kernel.ns's {name} names Top, which only Object may have as its superclass"
    );
}

/// A builtin's `<:` clause is read, never run, like its superclass clause:
/// a bare name with no message, naming a kernel class that is no builtin
/// -- a builtin is a class Rust fills, never a mixin the bootstrap builds
/// a class of. The bootstrap builds that class before any kernel code
/// runs, so its own clause must be readable the same way, a bare builtin
/// name, and it may apply no mixin of its own.
fn check_applied_kernel_class(
    heap: &Heap<'_>,
    root: &Rc<Mixin>,
    name: &str,
    applied: &ast::SuperclassClause,
) {
    let applied_name = applied.receiver.name.as_str();
    assert!(
        applied.prefix.is_none() && applied.messages.is_empty(),
        "internal: Kernel.ns's {name} must apply {applied_name} by a bare name, with no message"
    );
    assert!(
        kernel_kind_named(applied_name).is_none(),
        "internal: Kernel.ns's {name} applies the builtin {applied_name}; only a kernel class that is no builtin may be applied"
    );
    let nested = heap
        .nested_mixins(root)
        .into_iter()
        .find(|nested| nested.name.as_str() == applied_name)
        .unwrap_or_else(|| {
            panic!("internal: Kernel.ns's {name} applies {applied_name}, which is no kernel class")
        });
    let declaration = nested
        .declaration
        .as_ref()
        .unwrap_or_else(|| panic!("internal: Kernel.ns's {applied_name} was built from no source"));
    let clause = declaration.superclass.as_ref().unwrap_or_else(|| {
        panic!("internal: Kernel.ns's {applied_name} declares no superclass clause")
    });
    assert!(
        clause.prefix.is_none()
            && clause.messages.is_empty()
            && declaration.mixin_applications.is_empty()
            && kernel_kind_named(&clause.receiver.name)
                .is_some_and(|superclass| superclass != BuiltinClass::Top),
        "internal: Kernel.ns's {applied_name}, which {name} applies, must name a builtin other than Top as its superclass by a bare name, with no message and no mixin of its own"
    );
}

/// The kernel instance's field holding `vmMirror`: the root application's
/// own slot base plus `vmMirror`'s position among the root mixin's own
/// slots -- `InitializeSlot`'s rule (`bytecode.rs`, `own_index`).
pub(super) fn mirror_slot_of<'gc>(mutator: &Mutator<'gc>, kernel: ObjectReference<'gc>) -> usize {
    let application = kernel_application(mutator, kernel);
    let position = application
        .mixin
        .slots
        .iter()
        .position(|slot| slot.name.as_str() == "vmMirror")
        .expect("internal: check_kernel_shape found the vmMirror slot");
    application.own_slot_base + position
}

/// The kernel instance's hidden slot caching the nested class `kind`:
/// `MixinApplication::nested_slot`, keyed by the nested mixin's id.
pub(super) fn hidden_slot_of<'gc>(
    mutator: &Mutator<'gc>,
    kernel: ObjectReference<'gc>,
    kind: BuiltinClass,
) -> usize {
    let application = kernel_application(mutator, kernel);
    let nested = nested_named(mutator.heap(), &application.mixin, kernel_class_name(kind));
    hidden_slot_of_mixin(mutator, kernel, &nested)
}

/// The kernel instance's hidden slot caching the nested class whose mixin
/// is `nested`, a builtin's or any other kernel class's.
fn hidden_slot_of_mixin<'gc>(
    mutator: &Mutator<'gc>,
    kernel: ObjectReference<'gc>,
    nested: &Rc<Mixin>,
) -> usize {
    mutator
        .heap
        .known_mixin_id(nested)
        .and_then(|id| {
            kernel_application(mutator, kernel)
                .nested_slot
                .get(&id)
                .copied()
        })
        .unwrap_or_else(|| {
            panic!(
                "internal: the kernel's application has no slot for {}",
                nested.name
            )
        })
}

/// The application of the kernel instance's class, the root mixin's.
fn kernel_application<'gc>(
    mutator: &Mutator<'gc>,
    kernel: ObjectReference<'gc>,
) -> Rc<crate::mixin::MixinApplication> {
    let heap = mutator.heap();
    heap.class_application(heap.object_class(kernel))
        .expect("internal: the kernel's class is a class object")
}

/// A mirror send in shipped source -- the kernel's and every shipped module's,
/// `module` naming it in the panic -- passes a literal block only as its final,
/// `ifFail:` argument. A literal block anywhere before it would be a block the
/// leaf itself activates (as `whileTrue:`'s would be, were it passed as a
/// literal), and the reference lowering, which marks the whole send
/// `SendInlinable` because its failure block is inlined by the optimized
/// lowering, would exempt that block's activation from the depth count too --
/// while the optimized lowering counts it. The two lowerings would then count
/// activations differently (`docs/design/compiler.md`, "How the limit counts"),
/// so the shape is refused rather than the count special-cased. Such a block is
/// passed through a local or an argument instead, which neither lowering
/// marks.
pub(super) fn check_mirror_send_arguments(resolved: &ResolvedModule, module: &str) {
    for_each_send(resolved, |send| {
        if !matches!(resolved.bindings.get(&send.span), Some(Binding::MirrorSend)) {
            return;
        }
        let arguments = &send.message.arguments;
        let leading = &arguments[..arguments.len().saturating_sub(1)];
        if leading
            .iter()
            .any(|argument| matches!(argument.kind, ast::ExpressionKind::Block(_)))
        {
            panic!(
                "internal: {module}.ns passes a literal block to the mirror's {} before its \
                 failure block; only the final ifFail: argument may be a literal block, since \
                 the reference lowering would exempt that block's activation from the depth \
                 count and the optimized lowering would count it \
                 (docs/design/compiler.md, \"How the limit counts\")",
                crate::compiler::lower::selector_of(&send.message)
            );
        }
    });
}

/// Calls `visit` on every explicit-receiver send in `resolved` whose
/// receiver is the bare identifier `vmMirror`, for the test that checks the
/// resolver marked them all.
#[cfg(test)]
pub(super) fn for_each_send_to_vm_mirror(
    resolved: &ResolvedModule,
    mut visit: impl FnMut(&ast::Send),
) {
    for_each_send(resolved, |send| {
        let to_the_mirror = matches!(
            &send.receiver.kind,
            ast::ExpressionKind::Identifier(identifier) if identifier.name == "vmMirror"
        );
        if to_the_mirror {
            visit(send);
        }
    });
}

/// Calls `visit` on every send in `resolved`, blocks and every nested class
/// included.
fn for_each_send(resolved: &ResolvedModule, mut visit: impl FnMut(&ast::Send)) {
    fn class(declaration: &ast::ClassDeclaration, visit: &mut dyn FnMut(&ast::Send)) {
        let sides = std::iter::once(&declaration.instance_side).chain(&declaration.class_side);
        for side in sides {
            for slot in side.slots() {
                if let Some(initializer) = &slot.initializer {
                    expression(initializer, visit);
                }
            }
            statements(&side.initializer_expressions, visit);
            for method in side.methods() {
                body(&method.body, visit);
            }
            for nested in side.nested_classes() {
                class(nested, visit);
            }
        }
    }
    fn body(code: &ast::CodeBody, visit: &mut dyn FnMut(&ast::Send)) {
        for temporary in &code.temporaries {
            if let Some(initializer) = &temporary.initializer {
                expression(initializer, visit);
            }
        }
        statements(&code.statements, visit);
    }
    fn statements(list: &[ast::Statement], visit: &mut dyn FnMut(&ast::Send)) {
        for statement in list {
            match &statement.kind {
                ast::StatementKind::Expression(inner) | ast::StatementKind::Return(inner) => {
                    expression(inner, visit)
                }
            }
        }
    }
    fn expression(node: &ast::Expression, visit: &mut dyn FnMut(&ast::Send)) {
        match &node.kind {
            ast::ExpressionKind::Identifier(_) | ast::ExpressionKind::Literal(_) => {}
            ast::ExpressionKind::Send(send) => {
                visit(send);
                expression(&send.receiver, visit);
                for argument in &send.message.arguments {
                    expression(argument, visit);
                }
            }
            ast::ExpressionKind::Cascade(cascade) => {
                expression(&cascade.receiver, visit);
                for message in std::iter::once(&cascade.first).chain(&cascade.rest) {
                    for argument in &message.arguments {
                        expression(argument, visit);
                    }
                }
            }
            ast::ExpressionKind::Assign { value, .. } => expression(value, visit),
            ast::ExpressionKind::Block(block) => body(&block.body, visit),
            ast::ExpressionKind::DynamicArray { elements, .. } => {
                for element in elements {
                    expression(element, visit);
                }
            }
        }
    }
    class(&resolved.module.root_class, &mut visit);
}

#[cfg(test)]
mod tests {
    use crate::builtins::primitive::mirror_entry;
    use crate::bytecode::UnwindMarker;
    use crate::mixin::{MethodFastPath, QuickConstant};
    use crate::{BuiltinClass, Interpreter, Symbol};

    /// Compiles the kernel's root mixin from its source and hands it to
    /// `check` with the resolved kernel it was compiled from.
    fn check_kernel_root_from_source(
        check: impl FnOnce(&crate::mixin::Mixin, &crate::types::ResolvedModule),
    ) {
        let resolved = super::resolved_kernel();
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let root = mutator.build_root_mixin(
                &resolved.module.root_class,
                &resolved.bindings,
                crate::interpreter::instantiate::ModuleSource::Kernel,
                &resolved.source,
                resolved.identity.clone(),
            );
            check(&root, &resolved);
        });
    }

    #[test]
    fn every_kernel_class_records_the_receiver_its_clauses_name() {
        check_kernel_root_from_source(|root, _| {
            for nested in root.declared_nested().iter() {
                let declaration = nested.declaration.as_ref().expect("built from source");
                let written = declaration
                    .superclass
                    .as_ref()
                    .map(|clause| Symbol::intern(&clause.receiver.name));
                assert_eq!(nested.superclass_receiver, written, "{}", nested.name);
                let applied: Vec<Option<Symbol>> = declaration
                    .mixin_applications
                    .iter()
                    .map(|clause| Some(Symbol::intern(&clause.receiver.name)))
                    .collect();
                let recorded: Vec<Option<Symbol>> = nested
                    .mixin_applications
                    .iter()
                    .map(|clause| clause.receiver)
                    .collect();
                assert_eq!(recorded, applied, "{}", nested.name);
            }
        });
    }

    #[test]
    fn the_kernel_root_records_its_shape() {
        check_kernel_root_from_source(|root, resolved| {
            assert_eq!(
                root.root_shape,
                crate::resolver::root_shape(&resolved.module.root_class)
            );
            assert!(root.root_shape.is_some());
        });
    }

    /// Every builtin class but the mirror's is the kernel class of its name.
    #[test]
    fn every_builtin_class_is_filled_from_the_kernel() {
        // The source path: only a compiled mixin keeps its declaration.
        let mut interpreter = Interpreter::bootstrap_on(
            crate::compiler::Lowering::Optimized,
            crate::shipped_code::ShippedCodePath::Source,
        );
        interpreter.enter(|mutator| {
            for kind in BuiltinClass::ALL {
                if kind == BuiltinClass::VmMirror {
                    continue;
                }
                let class = mutator.builtin_class(kind);
                let application = mutator
                    .heap()
                    .class_application(class)
                    .expect("a builtin class has an application");
                let declaration = application
                    .mixin
                    .declaration
                    .as_ref()
                    .unwrap_or_else(|| panic!("{kind:?} is still a placeholder"));
                assert_eq!(declaration.name.name, super::kernel_class_name(kind));
            }
        });
    }

    /// The kernel methods that answer without reaching Rust at all, and so
    /// are ordinary methods rather than forwarders: the value each one
    /// answers is already at hand, and a leaf for it would be a mirror row
    /// that only repeats what `Kernel.ns` can say. Listed here so that a
    /// forwarder the recognizer silently missed still fails the test below;
    /// an entry is (the class's name in `Kernel.ns`, selector), so that a
    /// kernel class that is no builtin is listed as a builtin is.
    const NOT_FORWARDERS: &[(&str, &str)] = &[
        ("True", "hash"),
        ("False", "hash"),
        // §3.1.1's identity method on a value: a send of `=`, which is what
        // makes the two agree for every subclass. No leaf could answer it
        // without sending `=` back into Newspeak.
        ("Value", "=="),
        // The Newspeak side of the derivation, which runs when a leaf
        // declines: both send `=` or `hash` to a slot, which a leaf may not.
        ("Value", "valueEquals:"),
        ("Value", "valueHashOfSlots"),
        // A send of `hash`, which no leaf could answer for a value.
        ("Value", "identityHash"),
        ("Behavior", "identityHash"),
        // Forwarders in shape, but of their subject rather than of `self`:
        // §5.9 reaches a private member only through a lexical send, so the
        // other side's state has to be read by naming it as an argument,
        // which a primitive method may not do.
        ("Value", "valueSlotCountOf:"),
        ("Value", "valueSlotOf:at:"),
        // A string literal is no quick answer: an ordinary method, like
        // `hash` above, whose integer literal is none either.
        ("True", "printString"),
        ("False", "printString"),
        ("True", "asString"),
        ("False", "asString"),
        ("UndefinedObject", "printString"),
        // `cull:` tests its own arity, which no quick method does; the quick
        // methods of the nil protocol do what it says without sending it.
        ("Closure", "cull:"),
        ("Closure", "cull:cull:"),
        ("Closure", "cull:cull:cull:"),
        // A loop is Newspeak, its sends visible to the JIT tiers: the
        // inlined loop of `[self value] whileTrue: [body value]`.
        ("Closure", "whileTrue:"),
        ("Closure", "whileFalse:"),
        ("Closure", "whileTrue"),
        ("Closure", "whileFalse"),
        ("Closure", "repeat"),
        // A walk in Newspeak, over the size its leaf reads: a loop sending
        // `at:` and `value:`.
        ("Array", "do:"),
        // Iteration is Newspeak, after Primordial Soup's `Number`: a loop that
        // sends `<=`, `>=` and `+` to its receiver, which no leaf could do.
        ("Number", "to:do:"),
        ("Number", "to:by:do:"),
        // Primordial Soup's Newspeak body, `-1 - self`: a send of `-` to
        // `self`, which no leaf could do, and which `LargeInteger` inherits.
        ("Integer", "bitInvert"),
        // Integer's methods written in Newspeak: `&` and `|` send `bitAnd:`
        // and `bitOr:`, the modular pair loops over sends of arithmetic, and
        // `denominator` answers the integer literal 1, which is no quick
        // constant.
        ("Integer", "&"),
        ("Integer", "|"),
        ("Integer", "raisedTo:modulo:"),
        ("Integer", "reciprocalModulo:"),
        ("Integer", "denominator"),
        // The class side: `parse:` sends `parse:radix:` to `self`.
        ("Integer", "parse:"),
        // Primordial Soup's Newspeak bodies: `timesRepeat:` sends `to:do:`
        // and `value`, and `max:` sends `>` to `self`, which no leaf could
        // do.
        ("Integer", "timesRepeat:"),
        ("Number", "max:"),
        ("Number", "min:"),
        ("Number", "between:and:"),
        ("Number", "sqrt"),
        ("Number", "sin"),
        ("Number", "cos"),
        ("Number", "tan"),
        ("Number", "asin"),
        ("Number", "acos"),
        ("Number", "atan"),
        ("Number", "atan:"),
        ("Number", "exp"),
        ("Number", "ln"),
        ("Number", "log"),
        ("Number", "negated"),
        ("Number", "abs"),
        ("Number", "raisedTo:"),
        // A send to `self` is no leaf: `printString` reads the class's own
        // `name`, and a metaclass's `name` reads its own `thisClass` first.
        ("Class", "printString"),
        ("Metaclass", "name"),
        ("Metaclass", "printString"),
        // The Newspeak side of a value class object's equality, which runs
        // when the leaf declines: it sends `=` to the enclosing objects and a
        // superclass, which a leaf may not.
        ("Behavior", "behaviorEquals:"),
        // Forwarders in shape, but of their subject rather than of `self`,
        // for the reason `Value`'s slot readers above are.
        ("Behavior", "enclosingOf:"),
        ("Behavior", "levelEnclosingOf:"),
        ("Behavior", "superclassOf:"),
        ("Metaclass", "thisClassOf:"),
        // The marked methods (`UNWIND_MARKERS`): each keeps a local the
        // unwinder or `signal` reads, and sends `value` to `self`.
        ("Closure", "on:do:"),
        ("Closure", "ensure:"),
        ("Closure", "ifCurtailed:"),
        // A forwarder in shape, but of its argument rather than of `self`,
        // for the reason `Value`'s slot readers above are: the leaf's
        // subject is the exception, and the class is its argument.
        ("Class", "handles:"),
        // `Object>>halt` signals a `Halt`: sends, no leaf.
        ("Object", "halt"),
        // `Object>>doesNotUnderstand:` signals a `MessageNotUnderstood` and
        // may send the message again: sends, no leaf.
        ("Object", "doesNotUnderstand:"),
        // `Object>>subclassResponsibility` makes several mirror sends and
        // signals an `Error`: sends, no leaf.
        ("Object", "subclassResponsibility"),
        // `Collection`'s protocol is written over `do:`, which a subclass
        // defines: every method sends, none forwards to a leaf.
        ("Collection", "do:"),
        ("Collection", "newForCollectUsingAdd:"),
        ("Collection", "allSatisfy:"),
        ("Collection", "anySatisfy:"),
        ("Collection", "noneSatisfy:"),
        ("Collection", "detect:"),
        ("Collection", "detect:ifNone:"),
        ("Collection", "do:separatedBy:"),
        ("Collection", "includes:"),
        ("Collection", "inject:into:"),
        ("Collection", "isEmpty"),
        ("Collection", "reduce:"),
        ("Collection", "reduce:ifEmpty:"),
        ("Collection", "collect:"),
        ("Collection", "select:"),
        ("Collection", "reject:"),
        ("Collection", "size"),
        // A fraction's text is built from its parts' texts: sends, no leaf.
        ("Fraction", "printString"),
        ("Fraction", "asString"),
        ("Fraction", "floor"),
        ("Fraction", "ceiling"),
        ("Fraction", "truncated"),
        ("Fraction", "asInteger"),
        ("Fraction", "rounded"),
        // The sequence protocol is Newspeak over the leaves of the class
        // applying it -- `at:`, `at:put:`, `size` and `newOfSize:`: every
        // method sends, none forwards to a leaf.
        ("IndexedCollection", "newOfSize:"),
        ("IndexedCollection", "isEmpty"),
        ("IndexedCollection", "first"),
        ("IndexedCollection", "last"),
        ("IndexedCollection", "indexOf:"),
        ("IndexedCollection", "keysAndValuesDo:"),
        ("IndexedCollection", "copyFrom:to:"),
        ("IndexedCollection", "replaceFrom:to:with:startingAt:"),
        ("IndexedCollection", ","),
        ("IndexedCollection", "copyWith:"),
        ("IndexedCollection", "copyWithSize:"),
        ("IndexedCollection", "collect:"),
        ("IndexedCollection", "select:"),
        ("IndexedCollection", "reject:"),
        ("IndexedCollection", "sort:"),
        ("IndexedCollection", "sortFrom:to:by:"),
        ("IndexedCollection", "merge:into:from:width:count:by:"),
        (
            "IndexedCollection",
            "checkRangeFrom:to:keyword:ifNoInteger:",
        ),
        ("IndexedCollection", "checkSourceStart:count:sourceSize:"),
        ("IndexedCollection", "asArray"),
        // `Array`'s new instance of its own kind, and its class side's
        // `withAll:`: sends, no leaf.
        ("Array", "newOfSize:"),
        ("Array", "withAll:"),
        // `ByteArray`'s new instance of its own kind, its `do:`, a loop
        // over the size leaf's answer that sends `value:`, the two searches
        // from the first and the last start, and its class side's
        // `withAll:`: sends, no leaf.
        ("ByteArray", "newOfSize:"),
        ("ByteArray", "do:"),
        ("ByteArray", "indexOf:"),
        ("ByteArray", "lastIndexOf:"),
        ("ByteArray", "withAll:"),
        // `WeakArray`'s `isEmpty` over its `size`, its `do:`, a loop over
        // the size leaf's answer that sends `value:`, and the loops of
        // `keysAndValuesDo:` and `copyWith:`: sends, no leaf.
        ("WeakArray", "isEmpty"),
        ("WeakArray", "do:"),
        ("WeakArray", "keysAndValuesDo:"),
        ("WeakArray", "copyWith:"),
        // `Ephemeron>>key:` asks the value leaf and signals its refusal
        // before it sends the store on: no leaf answers for it.
        ("Ephemeron", "key:"),
        // `WeakMap` is Newspeak over an `Array` of `Ephemeron`s: each
        // member probes its table by sends, and `at:put:` asks the value
        // leaf before it searches. No leaf answers for any of them.
        ("WeakMap", "at:"),
        ("WeakMap", "includesKey:"),
        ("WeakMap", "removeKey:"),
        ("WeakMap", "at:put:"),
        ("WeakMap", "entryAt:"),
        ("WeakMap", "rehash"),
        // `String`'s code-point protocol is Newspeak over its leaves --
        // `size`, `at:`, `copyFrom:to:` and the two searches: every method
        // sends, none forwards to a leaf.
        ("String", "do:"),
        ("String", "isEmpty"),
        ("String", "first"),
        ("String", "last"),
        ("String", "stringAt:"),
        ("String", "copyStringFrom:to:"),
        ("String", "indexOf:"),
        ("String", "lastIndexOf:"),
        ("String", "startsWith:"),
        ("String", "endsWith:"),
        // The class side's `fromUtf8:` checks its argument before it asks
        // the argument's size and sends `fromUtf8:from:to:`: no leaf.
        ("String", "fromUtf8:"),
        // `StringBuilder` is Newspeak over `ByteArray`'s and `String`'s
        // leaves: each member checks its argument, keeps its counts in its
        // slots, or sends, and none forwards to a leaf. Its class side's
        // `new` sends `new:`.
        ("StringBuilder", "add:"),
        ("StringBuilder", "addCodePoint:"),
        ("StringBuilder", "writeln:"),
        ("StringBuilder", "size"),
        ("StringBuilder", "isEmpty"),
        ("StringBuilder", "asString"),
        ("StringBuilder", "asUtf8"),
        ("StringBuilder", "ensureRoom:"),
        ("StringBuilder", "checkState:"),
        ("StringBuilder", "new"),
        // What a `MessageNotUnderstood` hands a host: its message's
        // selector, a send, and its receiver, a slot.
        ("MessageNotUnderstood", "hostSelector"),
        ("MessageNotUnderstood", "hostReceiver"),
        // The exception protocol (`docs/design/core-library.md`, "Exceptions")
        // is Newspeak over the activation leaves: each method sends, keeps
        // state in the exception's slots, or makes several mirror sends, none
        // of them a forwarder of `self` alone.
        ("Exception", "description"),
        ("Exception", "printString"),
        ("Exception", "reachedDefaultHandler"),
        ("Exception", "signal"),
        ("Exception", "signal:"),
        ("Exception", "pass"),
        ("Exception", "return:"),
        ("Exception", "return"),
        ("Exception", "retry"),
        ("Exception", "resume:"),
        ("Exception", "resume"),
        ("Exception", "defaultAction"),
        ("Exception", "hostSelector"),
        ("Exception", "hostReceiver"),
        ("Exception", "invokeNextHandler"),
        ("Exception", "isHandledAt:"),
        ("Exception", "invokeHandlerAt:disabling:"),
        ("Exception", "notInAHandler:"),
        // The class sides' `signal` and `signal:`, `^self new signal`: an
        // instantiation and a send. §3.2's metaclasses are flat, so each
        // kernel exception class declares its own.
        ("Error", "signal"),
        ("Error", "signal:"),
        ("MessageNotUnderstood", "signal"),
        ("MessageNotUnderstood", "signal:"),
        ("Halt", "signal"),
        ("Halt", "signal:"),
        ("ArgumentError", "signal"),
        ("ArgumentError", "signal:"),
        ("CannotReturn", "signal"),
        ("CannotReturn", "signal:"),
        ("NonBooleanReceiver", "signal"),
        ("NonBooleanReceiver", "signal:"),
        ("CannotInstantiate", "signal"),
        ("CannotInstantiate", "signal:"),
        // The secondary factories of the error classes that carry a
        // subject, `^self new argument: offending` and its siblings: an
        // instantiation and a send.
        ("ArgumentError", "value:"),
        ("CannotReturn", "result:"),
        ("NonBooleanReceiver", "receiver:"),
        ("CannotInstantiate", "behavior:"),
        // Their `description`: a test of `messageText`, and a send of
        // `printString` to the subject.
        ("ArgumentError", "description"),
        ("CannotReturn", "description"),
        ("NonBooleanReceiver", "description"),
        ("CannotInstantiate", "description"),
        // A forwarder in shape, but of its argument rather than of `self`:
        // the row's subject is the computation the future will force.
        ("Future", "computing:"),
        // Float's class side: a division of two literals, whose answer the
        // arithmetic already makes; a leaf would only repeat it.
        ("Float", "nan"),
        ("Float", "infinity"),
        ("Float", "negativeInfinity"),
    ];

    /// What `TypedArray` declares, none a forwarder but
    /// `isKindOfTypedArray`, a quick method answering true: it is Newspeak
    /// over its subclasses' accessors -- its abstract members send
    /// `subclassResponsibility`, `size` and `do:` send, the slot's
    /// validation and the crafted-slot refusal signal, and its class side's
    /// helpers allocate and fill. Each of the ten typed arrays' own members
    /// is on `TYPED_ARRAY_MEMBERS`.
    const TYPED_ARRAY_ROOT_MEMBERS: &[&str] = &[
        "bytesPerElement",
        "size",
        "do:",
        "at:",
        "at:put:",
        "newOfSize:",
        "noByteArray:",
        "viewable:",
        "bytesFor:width:",
        "fill:from:",
    ];

    /// The ten typed arrays of `TypedArraysFactory.ns`, whose members are
    /// the same in each.
    const TYPED_ARRAYS: &[&str] = &[
        "Int8Array",
        "Uint8Array",
        "Int16Array",
        "Uint16Array",
        "Int32Array",
        "Uint32Array",
        "Int64Array",
        "Uint64Array",
        "Float32Array",
        "Float64Array",
    ];

    /// What each typed array declares, none a forwarder: `at:` and
    /// `at:put:` send to the mirror with the `bytes` slot as the subject,
    /// not `self` -- or, in `Uint8Array`, to the ByteArray itself -- so a
    /// view stays an ordinary object over its ByteArray; `bytesPerElement`
    /// answers a literal on both sides; `newOfSize:` and the class side's
    /// `new`, `new:` and `withAll:` send.
    const TYPED_ARRAY_MEMBERS: &[&str] = &[
        "bytesPerElement",
        "at:",
        "at:put:",
        "newOfSize:",
        "new",
        "new:",
        "withAll:",
    ];

    /// `DataView` and its two byte orders, none of whose members is a
    /// forwarder but `isKindOfDataView`, a quick method: the accessors send
    /// to the mirror with the `bytes` slot as the subject, not `self`, so a
    /// view stays an ordinary object over its ByteArray, and the rest is
    /// Newspeak around them.
    const DATA_VIEWS: &[&str] = &["DataView", "LittleEndianDataView", "BigEndianDataView"];

    /// The kernel forwarders whose row is reported under a selector other
    /// than their own, each with that row's reported selector. `Behavior>>==`
    /// and `Metaclass>>==` forward to `value:sameAs:ifFail:`, reported `=`,
    /// so that `self class == other class` stays one leaf and no activation;
    /// §3.1.1 asks a value's identity method to answer what its `=` answers,
    /// and the one leaf is what makes them agree. The reported selector only
    /// ever names an error text, and that row raises none -- it answers or
    /// declines.
    const REPORTED_UNDER_ANOTHER_SELECTOR: &[(BuiltinClass, &str, &str)] = &[
        (BuiltinClass::Behavior, "==", "="),
        (BuiltinClass::Metaclass, "==", "="),
        // `objectHash:ifFail:` reports `hash`, the selector `identityHash`
        // asks for an object without an `=` of its own.
        (BuiltinClass::Object, "identityHash", "hash"),
    ];

    /// The kernel's quick methods: `True`'s and `False`'s, the nil
    /// protocol on `Object` and `UndefinedObject`, and `Integer>>hash`. A
    /// missed one would run correctly as an ordinary method and nothing else
    /// would notice, except that it would cost an activation -- and for
    /// `True` and `False`, that the reference lowering would count that
    /// activation against the depth limit and the optimized lowering would
    /// not.
    const QUICK_METHODS: &[(BuiltinClass, &str, MethodFastPath)] = &[
        (
            BuiltinClass::True,
            "ifTrue:ifFalse:",
            MethodFastPath::ValueOfParameter(0),
        ),
        (
            BuiltinClass::True,
            "ifFalse:ifTrue:",
            MethodFastPath::ValueOfParameter(1),
        ),
        (
            BuiltinClass::True,
            "ifTrue:",
            MethodFastPath::ValueOfParameter(0),
        ),
        (
            BuiltinClass::True,
            "ifFalse:",
            MethodFastPath::Constant(QuickConstant::Nil),
        ),
        (
            BuiltinClass::True,
            "not",
            MethodFastPath::Constant(QuickConstant::False),
        ),
        (BuiltinClass::True, "&", MethodFastPath::Parameter(0)),
        (
            BuiltinClass::True,
            "and:",
            MethodFastPath::ValueOfParameter(0),
        ),
        (
            BuiltinClass::True,
            "|",
            MethodFastPath::Constant(QuickConstant::True),
        ),
        (
            BuiltinClass::True,
            "or:",
            MethodFastPath::Constant(QuickConstant::True),
        ),
        (
            BuiltinClass::False,
            "ifTrue:ifFalse:",
            MethodFastPath::ValueOfParameter(1),
        ),
        (
            BuiltinClass::False,
            "ifFalse:ifTrue:",
            MethodFastPath::ValueOfParameter(0),
        ),
        (
            BuiltinClass::False,
            "ifTrue:",
            MethodFastPath::Constant(QuickConstant::Nil),
        ),
        (
            BuiltinClass::False,
            "ifFalse:",
            MethodFastPath::ValueOfParameter(0),
        ),
        (
            BuiltinClass::False,
            "not",
            MethodFastPath::Constant(QuickConstant::True),
        ),
        (
            BuiltinClass::False,
            "&",
            MethodFastPath::Constant(QuickConstant::False),
        ),
        (
            BuiltinClass::False,
            "and:",
            MethodFastPath::Constant(QuickConstant::False),
        ),
        (BuiltinClass::False, "|", MethodFastPath::Parameter(0)),
        (
            BuiltinClass::False,
            "or:",
            MethodFastPath::ValueOfParameter(0),
        ),
        (
            BuiltinClass::Object,
            "isNil",
            MethodFastPath::Constant(QuickConstant::False),
        ),
        (
            BuiltinClass::Object,
            "notNil",
            MethodFastPath::Constant(QuickConstant::True),
        ),
        (BuiltinClass::Object, "ifNil:", MethodFastPath::Receiver),
        (
            BuiltinClass::Object,
            "ifNotNil:",
            MethodFastPath::CullParameter(0),
        ),
        (
            BuiltinClass::Object,
            "ifNil:ifNotNil:",
            MethodFastPath::CullParameter(1),
        ),
        (
            BuiltinClass::Object,
            "ifNotNil:ifNil:",
            MethodFastPath::CullParameter(0),
        ),
        (BuiltinClass::Object, "yourself", MethodFastPath::Receiver),
        (BuiltinClass::Float, "asFloat", MethodFastPath::Receiver),
        (BuiltinClass::Integer, "floor", MethodFastPath::Receiver),
        (BuiltinClass::Integer, "ceiling", MethodFastPath::Receiver),
        (BuiltinClass::Integer, "rounded", MethodFastPath::Receiver),
        (BuiltinClass::Integer, "truncated", MethodFastPath::Receiver),
        (BuiltinClass::Integer, "asInteger", MethodFastPath::Receiver),
        (BuiltinClass::Integer, "numerator", MethodFastPath::Receiver),
        (
            BuiltinClass::Integer,
            "isKindOfFraction",
            MethodFastPath::Constant(QuickConstant::True),
        ),
        (
            BuiltinClass::Fraction,
            "isKindOfFraction",
            MethodFastPath::Constant(QuickConstant::True),
        ),
        (
            BuiltinClass::Number,
            "isNaN",
            MethodFastPath::Constant(QuickConstant::False),
        ),
        (
            BuiltinClass::Number,
            "isFinite",
            MethodFastPath::Constant(QuickConstant::True),
        ),
        (
            BuiltinClass::Number,
            "isInfinite",
            MethodFastPath::Constant(QuickConstant::False),
        ),
        (
            BuiltinClass::Array,
            "isKindOfArray",
            MethodFastPath::Constant(QuickConstant::True),
        ),
        (
            BuiltinClass::Collection,
            "isKindOfCollection",
            MethodFastPath::Constant(QuickConstant::True),
        ),
        (
            BuiltinClass::Behavior,
            "isKindOfBehavior",
            MethodFastPath::Constant(QuickConstant::True),
        ),
        (
            BuiltinClass::Boolean,
            "isKindOfBoolean",
            MethodFastPath::Constant(QuickConstant::True),
        ),
        (
            BuiltinClass::Closure,
            "isKindOfClosure",
            MethodFastPath::Constant(QuickConstant::True),
        ),
        (
            BuiltinClass::Number,
            "isKindOfNumber",
            MethodFastPath::Constant(QuickConstant::True),
        ),
        (
            BuiltinClass::Number,
            "isKindOfFloat",
            MethodFastPath::Constant(QuickConstant::False),
        ),
        (
            BuiltinClass::Number,
            "isKindOfInteger",
            MethodFastPath::Constant(QuickConstant::False),
        ),
        (
            BuiltinClass::Number,
            "isKindOfFraction",
            MethodFastPath::Constant(QuickConstant::False),
        ),
        (
            BuiltinClass::Integer,
            "isKindOfInteger",
            MethodFastPath::Constant(QuickConstant::True),
        ),
        (
            BuiltinClass::Float,
            "isKindOfFloat",
            MethodFastPath::Constant(QuickConstant::True),
        ),
        (
            BuiltinClass::String,
            "isKindOfString",
            MethodFastPath::Constant(QuickConstant::True),
        ),
        (
            BuiltinClass::UndefinedObject,
            "isNil",
            MethodFastPath::Constant(QuickConstant::True),
        ),
        (
            BuiltinClass::UndefinedObject,
            "notNil",
            MethodFastPath::Constant(QuickConstant::False),
        ),
        (
            BuiltinClass::UndefinedObject,
            "ifNil:",
            MethodFastPath::ValueOfParameter(0),
        ),
        (
            BuiltinClass::UndefinedObject,
            "ifNotNil:",
            MethodFastPath::Constant(QuickConstant::Nil),
        ),
        (
            BuiltinClass::UndefinedObject,
            "ifNil:ifNotNil:",
            MethodFastPath::ValueOfParameter(0),
        ),
        (
            BuiltinClass::UndefinedObject,
            "ifNotNil:ifNil:",
            MethodFastPath::ValueOfParameter(1),
        ),
        (BuiltinClass::Integer, "hash", MethodFastPath::Receiver),
        (BuiltinClass::String, "asString", MethodFastPath::Receiver),
        (BuiltinClass::Array, "asArray", MethodFastPath::Receiver),
        (
            BuiltinClass::ByteArray,
            "isKindOfByteArray",
            MethodFastPath::Constant(QuickConstant::True),
        ),
        (
            BuiltinClass::ByteArray,
            "asByteArray",
            MethodFastPath::Receiver,
        ),
        (
            BuiltinClass::WeakArray,
            "isKindOfWeakArray",
            MethodFastPath::Constant(QuickConstant::True),
        ),
        (
            BuiltinClass::WeakArray,
            "isKindOfArray",
            MethodFastPath::Constant(QuickConstant::True),
        ),
        (
            BuiltinClass::Ephemeron,
            "isKindOfEphemeron",
            MethodFastPath::Constant(QuickConstant::True),
        ),
        // Quick in shape, though only the unwinder activates it, pushing it
        // as a method so that its return carries the unwind on
        // (`Mutator::start_unwind_block`).
        (
            BuiltinClass::Closure,
            "valueOnUnwind:",
            MethodFastPath::ValueOfParameter(0),
        ),
        // Whether a handler may resume the exception.
        (
            BuiltinClass::Exception,
            "isResumable",
            MethodFastPath::Constant(QuickConstant::True),
        ),
        (
            BuiltinClass::Error,
            "isResumable",
            MethodFastPath::Constant(QuickConstant::False),
        ),
        (
            BuiltinClass::MessageNotUnderstood,
            "isResumable",
            MethodFastPath::Constant(QuickConstant::True),
        ),
    ];

    /// The quick methods of the kernel's ordinary classes, those that are no
    /// `BuiltinClass` and so cannot be named in `QUICK_METHODS`: the class's
    /// name, the selector, and the fast path it must have.
    const ORDINARY_CLASS_QUICK_METHODS: &[(&str, &str, MethodFastPath)] = &[
        (
            "StringBuilder",
            "isKindOfStringBuilder",
            MethodFastPath::Constant(QuickConstant::True),
        ),
        (
            "DataView",
            "isKindOfDataView",
            MethodFastPath::Constant(QuickConstant::True),
        ),
        (
            "WeakMap",
            "isKindOfWeakMap",
            MethodFastPath::Constant(QuickConstant::True),
        ),
    ];

    /// §3.2's accessors for the predefined classes, which `Object` declares:
    /// every builtin a program can name, which is every one but `Top`, the
    /// mirror's class, `Behavior`, `Class`, `Metaclass`, `Activation`,
    /// `FutureImplementation`, `LargeInteger`, which a program meets only as
    /// an integer's class, and the exception classes, `Message`, `Proxy`,
    /// `Future`, `Collection`, `Fraction`, `WeakArray` and `Ephemeron`,
    /// which a program reaches through `platform kernel`.
    const OBJECT_CLASS_ACCESSORS: &[BuiltinClass] = &[
        BuiltinClass::Object,
        BuiltinClass::Value,
        BuiltinClass::Boolean,
        BuiltinClass::True,
        BuiltinClass::False,
        BuiltinClass::Number,
        BuiltinClass::Integer,
        BuiltinClass::Float,
        BuiltinClass::Fraction,
        BuiltinClass::String,
        BuiltinClass::Symbol,
        BuiltinClass::UndefinedObject,
        BuiltinClass::Array,
        BuiltinClass::ByteArray,
        BuiltinClass::Closure,
        BuiltinClass::Exception,
        BuiltinClass::Error,
    ];

    /// Every kernel method is a forwarder the compiler recognized, and is
    /// named after its row's reported selector -- every one but the handful
    /// `NOT_FORWARDERS` names, which reach no leaf. A method the recognizer
    /// misses -- a swapped parameter, a temporary, a failure block that is
    /// not a literal -- still runs correctly as an ordinary method, so
    /// nothing else would notice: a leaf would lose its fast path, and a
    /// missed `ifTrue:`, `and:` or `whileTrue:` would count against the depth
    /// limit under one lowering and not the other. The fast path's error
    /// texts name the row's reported selector, so they would also drift from
    /// the selector the program sent if the two differed. `True`'s and
    /// `False`'s methods are quick methods instead, each checked against
    /// `QUICK_METHODS`.
    #[test]
    fn every_kernel_method_has_the_fast_path_it_should() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let Some(kernel) = mutator.kernel.and_then(|value| value.as_object()) else {
                panic!("the kernel is instantiated at bootstrap");
            };
            let kernel_mixin = super::kernel_application(mutator, kernel).mixin.clone();
            for mixin in &mutator.heap().nested_mixins(&kernel_mixin) {
                let class_name = mixin.name.as_str();
                // `Proxy`'s methods forward nothing to a leaf: each is a
                // protected override of one of `Object`'s public members,
                // there only to stop §5.5's lookupPublic, and none is ever
                // found by a send a program writes. `tests/future.rs`'s
                // `a_proxy_overrides_every_public_member_of_object_as_protected`
                // checks them instead. `FutureImplementation`'s one method,
                // `doesNotUnderstand:`, forces the future and sends the
                // message on: a send, no leaf.
                if matches!(class_name, "Proxy" | "FutureImplementation") {
                    continue;
                }
                let owned_by = |owner: BuiltinClass| super::kernel_class_name(owner) == class_name;
                let class_side = mixin
                    .class_side
                    .iter()
                    .flat_map(|side| side.compiled_methods());
                for method in mixin.compiled_methods().chain(class_side) {
                    let selector = method.selector.as_str();
                    let data_view_member =
                        DATA_VIEWS.contains(&class_name) && selector != "isKindOfDataView";
                    if data_view_member || NOT_FORWARDERS.contains(&(class_name, selector)) {
                        assert!(
                            method.fast_path().is_none(),
                            "{class_name}>>{selector} reaches a leaf after all; take it off NOT_FORWARDERS"
                        );
                        continue;
                    }
                    if let Some((_, _, expected)) = QUICK_METHODS
                        .iter()
                        .find(|(owner, name, _)| owned_by(*owner) && *name == selector)
                    {
                        assert_eq!(
                            method.fast_path(),
                            Some(*expected),
                            "{class_name}>>{selector} is not the quick method it should be"
                        );
                        continue;
                    }
                    if let Some((_, _, expected)) = ORDINARY_CLASS_QUICK_METHODS
                        .iter()
                        .find(|(owner, name, _)| *owner == class_name && *name == selector)
                    {
                        assert_eq!(
                            method.fast_path(),
                            Some(*expected),
                            "{class_name}>>{selector} is not the quick method it should be"
                        );
                        continue;
                    }
                    // §3.2's accessors on `Object` are a fast path of their
                    // own kind, which reaches no row;
                    // `objects_class_accessors_are_exactly_the_nameable_builtins`
                    // checks each one.
                    if class_name == "Object"
                        && OBJECT_CLASS_ACCESSORS
                            .iter()
                            .any(|accessor| super::kernel_class_name(*accessor) == selector)
                    {
                        assert!(
                            matches!(
                                method.fast_path(),
                                Some(crate::mixin::MethodFastPath::BuiltinClass(_))
                            ),
                            "Object>>{selector} is not recognized as a class accessor"
                        );
                        continue;
                    }
                    let Some(crate::mixin::MethodFastPath::Mirror(row)) = method.fast_path() else {
                        panic!("{class_name}>>{selector} is not recognized as a primitive method");
                    };
                    let reported = REPORTED_UNDER_ANOTHER_SELECTOR
                        .iter()
                        .find(|(owner, name, _)| owned_by(*owner) && *name == selector)
                        .map_or(selector, |(_, _, reported)| *reported);
                    assert_eq!(
                        mirror_entry(row).reported,
                        reported,
                        "{class_name}>>{selector} forwards to a row reported under another selector"
                    );
                }
            }
        });
    }

    /// `Platform` declares no accessor for a predefined class: it inherits
    /// `Object`'s, which answer the same classes (§3.2), so a copy of its own
    /// would only be a second way to the same answer.
    #[test]
    fn the_platform_declares_no_accessor_for_a_predefined_class() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let Some(platform) = mutator.platform_value().as_object() else {
                panic!("the platform is an object");
            };
            let platform_class = mutator.heap().object_class(platform);
            let application = mutator
                .heap()
                .class_application(platform_class)
                .expect("Platform has an application");
            for kind in OBJECT_CLASS_ACCESSORS.iter().copied() {
                let name = super::kernel_class_name(kind);
                assert!(
                    application
                        .mixin
                        .compiled_method(crate::Symbol::intern(name))
                        .is_none(),
                    "Platform declares its own {name}"
                );
            }
        });
    }

    /// No class accessor outside the kernel carries a fast path. The one
    /// that does, `MethodFastPath::BuiltinClass`, is recognized only in the
    /// form `^outer Kernel <Name>`, which no module but the kernel can write.
    /// Every other shipped module's pure class accessors, such as
    /// `Collections>>List`, are recognized too, as `MixinMethod::class_accessor`,
    /// but only the image's rule of what may be named where reads that
    /// recognition; a send runs the method like any other.
    /// `KernelNamespace`'s members answer a slot of the root, the kernel's
    /// class it took at its instantiation, so they are neither. Each costs one activation more than a
    /// kernel accessor; a namespace member is asked for once, typically in a
    /// slot initializer, not in a loop. `Transcript`'s two and `Stopwatch`'s
    /// clock reading are mirror forwarders like the kernel's:
    /// `PlatformFactory.ns` is resolved with `resolve_kernel`. `Time`'s
    /// `Stopwatch` is a class accessor, as is each of `TypedArrays`'
    /// accessors. The typed arrays' own members reach no leaf through a
    /// forwarder (`TYPED_ARRAY_ROOT_MEMBERS`, `TYPED_ARRAY_MEMBERS`), but
    /// `TypedArray>>isKindOfTypedArray`, a quick method. The classes are
    /// read on the host side, off the members the platform answers, since
    /// the platform hands out none of its module's classes.
    #[test]
    fn the_platform_modules_members_have_the_fast_path_they_should() {
        let mut interpreter = Interpreter::bootstrap();
        let platform = interpreter.platform();
        let mut member = |receiver: &crate::Root, name: &str| match interpreter.send(
            receiver,
            crate::Symbol::intern(name),
            &[],
        ) {
            Ok(answer) => answer,
            Err(_) => panic!("{name} failed"),
        };
        let kernel = member(&platform, "kernel");
        let transcript = member(&platform, "transcript");
        let time = member(&platform, "time");
        let stopwatch = member(&time, "Stopwatch");
        let collections = member(&platform, "collections");
        let typed_arrays = member(&platform, "typedArrays");
        let typed_array_classes: Vec<(&str, crate::Root)> = std::iter::once("TypedArray")
            .chain(TYPED_ARRAYS.iter().copied())
            .map(|name| (name, member(&typed_arrays, name)))
            .collect();
        let class_of = |interpreter: &mut Interpreter, subject: &crate::Root| {
            interpreter.enter(|mutator| {
                let class = mutator.class_of(mutator.fetch(subject));
                mutator.stash(crate::Value::object(class))
            })
        };
        let kernel_namespace = class_of(&mut interpreter, &kernel);
        let transcript = class_of(&mut interpreter, &transcript);
        let time = class_of(&mut interpreter, &time);
        let collections = class_of(&mut interpreter, &collections);
        let typed_arrays = class_of(&mut interpreter, &typed_arrays);
        interpreter.enter(|mutator| {
            let fast_path = |class: &crate::Root, selector: &str| {
                let Some(class) = mutator.fetch(class).as_object() else {
                    panic!("a nested class accessor answers a class object");
                };
                let application = mutator
                    .heap()
                    .class_application(class)
                    .expect("a class object has an application");
                let method = application
                    .mixin
                    .compiled_method(crate::Symbol::intern(selector))
                    .unwrap_or_else(|| panic!("no method {selector}"))
                    .clone();
                method.fast_path()
            };
            assert_eq!(fast_path(&collections, "List"), None, "Collections>>List");
            assert_eq!(
                fast_path(&collections, "WeakArray"),
                None,
                "Collections>>WeakArray"
            );
            for selector in [
                "Exception",
                "Error",
                "MessageNotUnderstood",
                "Halt",
                "Message",
                "ArgumentError",
                "CannotReturn",
                "CannotInstantiate",
                "Closure",
                "Future",
                "Proxy",
                "StringBuilder",
                "DataView",
                "WeakArray",
                "Ephemeron",
            ] {
                assert_eq!(
                    fast_path(&kernel_namespace, selector),
                    None,
                    "KernelNamespace>>{selector}"
                );
            }
            // The collection the kernel's `garbageCollect` asks for is served
            // at the safepoint of this method's return: as a fast path it
            // would have none, and a weak slot read right after it would
            // still name the object
            // (`goldens/library/weak/a_weak_array_forgets_an_object_after_a_collection.ns`).
            assert_eq!(
                fast_path(&kernel_namespace, "garbageCollect"),
                None,
                "KernelNamespace>>garbageCollect must be activated"
            );
            for selector in ["show:", "newLine"] {
                assert!(
                    matches!(
                        fast_path(&transcript, selector),
                        Some(crate::mixin::MethodFastPath::Mirror(_))
                    ),
                    "Transcript>>{selector} is not recognized as a primitive method"
                );
            }
            assert!(
                matches!(
                    fast_path(&stopwatch, "currentMonotonicNanoseconds"),
                    Some(crate::mixin::MethodFastPath::Mirror(_))
                ),
                "Stopwatch>>currentMonotonicNanoseconds is not recognized as a primitive method"
            );
            assert_eq!(fast_path(&time, "Stopwatch"), None, "Time>>Stopwatch");
            for (name, class) in &typed_array_classes {
                assert_eq!(fast_path(&typed_arrays, name), None, "TypedArrays>>{name}");
                let Some(class) = mutator.fetch(class).as_object() else {
                    panic!("TypedArrays>>{name} answers a class object");
                };
                let application = mutator
                    .heap()
                    .class_application(class)
                    .expect("a class object has an application");
                let members = if *name == "TypedArray" {
                    TYPED_ARRAY_ROOT_MEMBERS
                } else {
                    TYPED_ARRAY_MEMBERS
                };
                let class_side = application
                    .mixin
                    .class_side
                    .iter()
                    .flat_map(|side| side.compiled_methods());
                for method in application.mixin.compiled_methods().chain(class_side) {
                    let selector = method.selector.as_str();
                    if *name == "TypedArray" && selector == "isKindOfTypedArray" {
                        assert_eq!(
                            method.fast_path(),
                            Some(MethodFastPath::Constant(QuickConstant::True)),
                            "TypedArray>>isKindOfTypedArray is not the quick method it should be"
                        );
                        continue;
                    }
                    assert!(
                        members.contains(&selector),
                        "{name}>>{selector} is no member the test knows; list it"
                    );
                    assert!(
                        method.fast_path().is_none(),
                        "{name}>>{selector} reaches a leaf after all; take it off the members"
                    );
                }
            }
        });
    }

    /// `Array new:` is a class-side mirror forwarder, so a send of it runs
    /// the leaf with no activation. The handler search depends on that: it
    /// sends `Array new:` on every handled exception, and one activation
    /// more per search would move where the depth limit's goldens overflow.
    #[test]
    fn array_new_is_a_mirror_forwarder() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let array = mutator.builtin_class(BuiltinClass::Array);
            let metaclass = mutator
                .heap()
                .class_metaclass(array)
                .expect("Array has a metaclass");
            let application = mutator
                .heap()
                .class_application(metaclass)
                .expect("a metaclass has an application");
            let method = application
                .mixin
                .compiled_method(crate::Symbol::intern("new:"))
                .expect("Array's class side declares new:")
                .clone();
            let row = crate::builtins::primitive::mirror_primitive_named("arrayClass:new:ifFail:")
                .expect("the row exists");
            assert_eq!(
                method.fast_path(),
                Some(crate::mixin::MethodFastPath::Mirror(row))
            );
        });
    }

    /// `ByteArray new:` is a class-side mirror forwarder, as `Array new:`
    /// is, so a send of it runs the leaf with no activation.
    #[test]
    fn byte_array_new_is_a_mirror_forwarder() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let byte_array = mutator.builtin_class(BuiltinClass::ByteArray);
            let metaclass = mutator
                .heap()
                .class_metaclass(byte_array)
                .expect("ByteArray has a metaclass");
            let application = mutator
                .heap()
                .class_application(metaclass)
                .expect("a metaclass has an application");
            let method = application
                .mixin
                .compiled_method(crate::Symbol::intern("new:"))
                .expect("ByteArray's class side declares new:")
                .clone();
            let row =
                crate::builtins::primitive::mirror_primitive_named("byteArrayClass:new:ifFail:")
                    .expect("the row exists");
            assert_eq!(
                method.fast_path(),
                Some(crate::mixin::MethodFastPath::Mirror(row))
            );
        });
    }

    /// The collections facet hands out its module's classes by methods the
    /// compiler recognizes as class accessors (`MixinMethod::class_accessor`),
    /// which only the image's rule of what may be named where reads: none
    /// carries a fast path, so no send pays for the recognition. The platform
    /// hands out none of its module's classes, and the facet not its own, so
    /// a program can make no second platform, transcript or facet. `Object`'s
    /// accessors are `Kernel.ns`'s, recognized as `MethodFastPath::BuiltinClass`
    /// instead, so none of them is one.
    #[test]
    fn the_platforms_class_accessors_are_recognized_without_a_fast_path() {
        let mut interpreter = Interpreter::bootstrap();
        let platform = interpreter.platform();
        let collections =
            match interpreter.send(&platform, crate::Symbol::intern("collections"), &[]) {
                Ok(collections) => collections,
                Err(_) => panic!("platform collections failed"),
            };
        interpreter.enter(|mutator| {
            let collections_class = mutator.class_of(mutator.fetch(&collections));
            let facet_application = mutator
                .heap()
                .class_application(collections_class)
                .expect("Collections has an application");
            let method = facet_application
                .mixin
                .compiled_method(crate::Symbol::intern("List"))
                .expect("Collections declares List");
            assert_eq!(
                method.class_accessor,
                Some(crate::mixin::ClassAccessor {
                    depth: 1,
                    class_name: crate::Symbol::intern("List"),
                }),
                "Collections>>List"
            );
            assert_eq!(method.fast_path(), None, "Collections>>List");
            assert!(
                facet_application
                    .mixin
                    .compiled_method(crate::Symbol::intern("Collections"))
                    .is_none(),
                "the facet hands out its own class"
            );
            let Some(platform) = mutator.platform_value().as_object() else {
                panic!("the platform is an object");
            };
            let platform_class = mutator.heap().object_class(platform);
            let application = mutator
                .heap()
                .class_application(platform_class)
                .expect("Platform has an application");
            for name in [
                "Platform",
                "Transcript",
                "KernelNamespace",
                "Time",
                "Stopwatch",
            ] {
                assert!(
                    application
                        .mixin
                        .compiled_method(crate::Symbol::intern(name))
                        .is_none(),
                    "Platform hands out its module's class {name}"
                );
            }
            let object = mutator.builtin_class(BuiltinClass::Object);
            let object_application = mutator
                .heap()
                .class_application(object)
                .expect("Object has an application");
            for kind in OBJECT_CLASS_ACCESSORS.iter().copied() {
                let method = object_application
                    .mixin
                    .compiled_method(crate::Symbol::intern(super::kernel_class_name(kind)))
                    .unwrap_or_else(|| panic!("Object declares no accessor {kind:?}"));
                assert_eq!(method.class_accessor, None, "Object>>{kind:?}");
            }
        });
    }

    /// The kernel methods that carry an unwind marker, each with the
    /// marker: its registers are its parameters from 0, then its named
    /// temporaries in declaration order (`docs/design/compiler.md`,
    /// "Registers").
    const UNWIND_MARKERS: &[(&str, UnwindMarker)] = &[
        (
            "on:do:",
            UnwindMarker::Handler {
                class_register: 0,
                block_register: 1,
                active_register: 15,
            },
        ),
        (
            "ensure:",
            UnwindMarker::Ensure {
                block_register: 0,
                complete_register: 14,
            },
        ),
        (
            "ifCurtailed:",
            UnwindMarker::IfCurtailed {
                block_register: 0,
                complete_register: 14,
            },
        ),
    ];

    crate::for_each_lowering! {
        /// `Closure>>on:do:`, `ensure:` and `ifCurtailed:` carry the markers
        /// `UNWIND_MARKERS` says, under both lowerings, and no other kernel
        /// method's code carries one: the unwinder would run a block for it.
        fn only_the_three_block_methods_carry_an_unwind_marker(lowering) {
            let mut interpreter = Interpreter::bootstrap_with(lowering);
            interpreter.enter(|mutator| {
                for kind in BuiltinClass::ALL {
                    let class = mutator.builtin_class(kind);
                    let application = mutator
                        .heap()
                        .class_application(class)
                        .expect("a builtin class has an application");
                    let mixin = &application.mixin;
                    let class_side = mixin
                        .class_side
                        .iter()
                        .flat_map(|side| side.compiled_methods());
                    for method in mixin.compiled_methods().chain(class_side) {
                        let selector = method.selector.as_str();
                        let expected = if kind == BuiltinClass::Closure {
                            UNWIND_MARKERS
                                .iter()
                                .find(|(name, _)| *name == selector)
                                .map(|(_, marker)| *marker)
                        } else {
                            None
                        };
                        assert_eq!(
                            method.code.unwind_marker, expected,
                            "{lowering:?}: {kind:?}>>{selector}"
                        );
                    }
                }
                let block = mutator.builtin_class(BuiltinClass::Closure);
                let application = mutator
                    .heap()
                    .class_application(block)
                    .expect("Closure has an application");
                for (selector, _) in UNWIND_MARKERS {
                    assert!(
                        application
                            .mixin
                            .compiled_method(crate::Symbol::intern(selector))
                            .is_some(),
                        "Closure declares no {selector}"
                    );
                }
            });
        }
    }

    crate::for_each_lowering! {
        /// Only `Kernel.ns`'s own `Closure` may carry unwind markers: a class of
        /// that name in another shipped module, with the same bodies, gets none,
        /// since the unwinder trusts a marked method's registers. Other shipped
        /// modules are resolved with `resolve_kernel` too, so the mark cannot
        /// rest on that alone.
        fn a_block_class_in_another_shipped_module_carries_no_unwind_marker(lowering) {
            let source = indoc::indoc! {"
                class Shipped = Object new () (
                    public class Closure = Object new () (
                        public on: exceptionClass do: handler = (
                            | handlerActive ::= true. |
                            ^self value
                        )
                        public ensure: onUnwind = (
                            | complete ::= nil. result |
                            result:: self value.
                            complete isNil ifTrue: [complete:: true. onUnwind value].
                            ^result
                        )
                    )
                ) : ()
            "};
            let module = ns_syntax::parse(source).expect("parse");
            let Ok(resolved) = crate::resolver::resolve_kernel(
                module,
                std::rc::Rc::from(source),
                crate::types::SourceOrigin::File(std::path::PathBuf::from("Shipped.ns")),
            ) else {
                panic!("the module resolves");
            };
            let mut interpreter = Interpreter::bootstrap_with(lowering);
            let class = interpreter.load_module(&resolved);
            interpreter.enter(|mutator| {
                let Some(class) = mutator.fetch(&class).as_object() else {
                    panic!("load_module answers a class object");
                };
                let application = mutator
                    .heap()
                    .class_application(class)
                    .expect("the root class has an application");
                let block = mutator
                    .heap()
                    .nested_mixins(&application.mixin)
                    .into_iter()
                    .find(|nested| nested.name.as_str() == "Closure")
                    .expect("the module declares Closure");
                assert!(block.shipped, "the module is shipped source");
                for method in block.compiled_methods() {
                    assert_eq!(
                        method.code.unwind_marker,
                        None,
                        "{lowering:?}: Shipped's Closure>>{}",
                        method.selector.as_str()
                    );
                }
            });
        }
    }

    /// Every row of `QUICK_METHODS` names a method the kernel declares.
    #[test]
    fn every_quick_method_the_table_names_exists() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            for (kind, selector, _) in QUICK_METHODS {
                let class = mutator.builtin_class(*kind);
                let application = mutator
                    .heap()
                    .class_application(class)
                    .expect("a builtin class has an application");
                assert!(
                    application
                        .mixin
                        .compiled_method(crate::Symbol::intern(selector))
                        .is_some(),
                    "{kind:?} declares no {selector}"
                );
            }
        });
    }

    /// The accessor set on `Object` is exactly §3.2's nameable builtins,
    /// each recognized as a fast path answering its own class; `Top` and the
    /// mirror's class have none, which is one of the locks that keep the
    /// mirror a capability. Neither have the exception classes, which a
    /// program reaches through `platform kernel`, nor `Proxy` and `Future`,
    /// which it reaches there too, nor `Activation`, `FutureImplementation`
    /// and `Mixin`, which it does not reach at all, nor
    /// `LargeInteger`, which no program reaches either: `class` is
    /// protected and `Object` declares no accessor for it.
    /// `PlatformFactory.ns`'s class names are listed too: `Object` must
    /// never grow an accessor for one of them, since a program reaches them
    /// through the platform it is handed.
    #[test]
    fn objects_class_accessors_are_exactly_the_nameable_builtins() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let object = mutator.builtin_class(BuiltinClass::Object);
            let application = mutator
                .heap()
                .class_application(object)
                .expect("Object has an application");
            for kind in OBJECT_CLASS_ACCESSORS.iter().copied() {
                let selector = crate::Symbol::intern(super::kernel_class_name(kind));
                let method = application
                    .mixin
                    .compiled_method(selector)
                    .unwrap_or_else(|| panic!("Object declares no accessor {kind:?}"));
                assert_eq!(
                    method.fast_path(),
                    Some(crate::mixin::MethodFastPath::BuiltinClass(kind)),
                    "{kind:?}"
                );
                assert_eq!(
                    method.access,
                    ns_syntax::ast::AccessModifier::Protected,
                    "{kind:?}: an object hands out no class through another's accessor"
                );
            }
            for absent in [
                "Top",
                "PlatformFactory",
                "Platform",
                "Transcript",
                "Collections",
                "CollectionsFactory",
                "TypedArrays",
                "TypedArraysFactory",
                "MirrorsFactory",
                "Mirrors",
                "ObjectMirror",
                "ClassMirror",
                "MirrorGroup",
                "SlotMirror",
                "AbstractClassMirror",
                "NavigatedClassMirror",
                "MixinMirror",
                "ClassDeclarationMirror",
                "MethodMirror",
                "SlotDeclarationMirror",
                "OrderedCollection",
                "List",
                "VmMirror",
                "MessageNotUnderstood",
                "Halt",
                "Message",
                "ArgumentError",
                "CannotReturn",
                "NonBooleanReceiver",
                "CannotInstantiate",
                "Activation",
                "FutureImplementation",
                "Proxy",
                "Future",
                "KernelNamespace",
                "Time",
                "Stopwatch",
                "LargeInteger",
                "Collection",
                "WeakArray",
                "Ephemeron",
                "Mixin",
            ] {
                assert!(
                    application
                        .mixin
                        .compiled_method(crate::Symbol::intern(absent))
                        .is_none(),
                    "Object must declare no accessor {absent}"
                );
            }
        });
    }

    /// The kernel's private classes: `Top`, which §3.2 lets only `Object`
    /// name, `Activation`, `FutureImplementation` and `Mixin`, which no
    /// program may reach at all, `NonBooleanReceiver`, which a program
    /// catches as an `Error` (`docs/design/core-library.md`, "Exceptions";
    /// `docs/design/object-model.md`, "Simultaneous slots"), and the two
    /// byte-order classes of the data view.
    const PRIVATE_KERNEL_CLASSES: &[&str] = &[
        "Top",
        "Activation",
        "FutureImplementation",
        "Mixin",
        "NonBooleanReceiver",
        "LittleEndianDataView",
        "BigEndianDataView",
    ];

    /// §6.3.1 defaults an undeclared member to protected, and a protected
    /// nested class is unreachable from outside the module that declares it,
    /// so every kernel class has to say `public` for a program to name it --
    /// every one but `PRIVATE_KERNEL_CLASSES`. The count is of the classes
    /// named after a builtin -- one per builtin but the mirror's; the
    /// kernel's ordinary classes are no builtin's, and the access check
    /// below, which reads every nested class, covers them.
    #[test]
    fn only_the_kernels_private_classes_are_private() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let Some(kernel) = mutator.kernel.and_then(|value| value.as_object()) else {
                panic!("the kernel is instantiated at bootstrap");
            };
            let application = super::kernel_application(mutator, kernel);
            let declared = mutator.heap().nested_mixins(&application.mixin);
            let builtins = declared
                .iter()
                .filter(|nested| super::kernel_kind_named(nested.name.as_str()).is_some())
                .count();
            assert_eq!(
                builtins,
                BuiltinClass::ALL.len() - 1,
                "the kernel declares one class per builtin but the mirror's"
            );
            for private in PRIVATE_KERNEL_CLASSES {
                assert!(
                    declared
                        .iter()
                        .any(|nested| nested.name.as_str() == *private),
                    "the kernel declares {private}"
                );
            }
            for nested in &declared {
                let name = nested.name.as_str();
                let expected = if PRIVATE_KERNEL_CLASSES.contains(&name) {
                    ns_syntax::ast::AccessModifier::Private
                } else {
                    ns_syntax::ast::AccessModifier::Public
                };
                assert_eq!(
                    nested.access, expected,
                    "the kernel class {} must be {expected:?}",
                    nested.name
                );
            }
        });
    }

    /// §3.2: `Top` is the empty class, and only `Object` may have it as its
    /// superclass. §3.1.1's value classes hang under `Value`.
    #[test]
    fn the_hierarchy_follows_the_specification() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let expected = [
                (BuiltinClass::Object, Some(BuiltinClass::Top)),
                (BuiltinClass::Value, Some(BuiltinClass::Object)),
                (BuiltinClass::Boolean, Some(BuiltinClass::Value)),
                (BuiltinClass::True, Some(BuiltinClass::Boolean)),
                (BuiltinClass::False, Some(BuiltinClass::Boolean)),
                (BuiltinClass::Number, Some(BuiltinClass::Value)),
                (BuiltinClass::Integer, Some(BuiltinClass::Number)),
                (BuiltinClass::LargeInteger, Some(BuiltinClass::Integer)),
                (BuiltinClass::Float, Some(BuiltinClass::Number)),
                (BuiltinClass::Fraction, Some(BuiltinClass::Number)),
                (BuiltinClass::String, Some(BuiltinClass::Value)),
                (BuiltinClass::Symbol, Some(BuiltinClass::String)),
                (BuiltinClass::UndefinedObject, Some(BuiltinClass::Value)),
                (BuiltinClass::Collection, Some(BuiltinClass::Object)),
                (BuiltinClass::Closure, Some(BuiltinClass::Object)),
                (BuiltinClass::Proxy, Some(BuiltinClass::Object)),
                (
                    BuiltinClass::FutureImplementation,
                    Some(BuiltinClass::Proxy),
                ),
                (BuiltinClass::Future, Some(BuiltinClass::Object)),
                (BuiltinClass::Top, None),
            ];
            for (kind, superclass) in expected {
                let actual = mutator.heap().class_superclass(mutator.builtin_class(kind));
                match superclass {
                    // The class object itself, not just a class reporting
                    // that builtin: a second class object of the same mixin
                    // would answer the same kind and be the wrong class.
                    Some(expected) => {
                        let expected_class = mutator.builtin_class(expected);
                        assert!(
                            actual.is_some_and(|class| class.same(expected_class)),
                            "{kind:?}'s superclass must be {expected:?}"
                        );
                    }
                    None => assert!(actual.is_none(), "{kind:?} has no superclass"),
                }
            }
            // `Array = Collection <: IndexedCollection` (§6.2), and
            // `ByteArray` likewise: each one's superclass is the level its
            // clause adds, the kernel's `IndexedCollection` mixin applied
            // over `Collection`.
            let heap = mutator.heap();
            let Some(kernel) = mutator.kernel.and_then(|value| value.as_object()) else {
                panic!("the kernel is instantiated at bootstrap");
            };
            let kernel_mixin = super::kernel_application(mutator, kernel).mixin.clone();
            let collection = mutator.builtin_class(BuiltinClass::Collection);
            for kind in [BuiltinClass::Array, BuiltinClass::ByteArray] {
                let class = mutator.builtin_class(kind);
                let level = heap
                    .class_superclass(class)
                    .unwrap_or_else(|| panic!("{kind:?} has a superclass"));
                let origin = heap.class_applied_origin(level).unwrap_or_else(|| {
                    panic!("{kind:?}'s superclass is the level its clause adds")
                });
                let class_mixin = heap
                    .class_application(class)
                    .unwrap_or_else(|| panic!("{kind:?} is a class"))
                    .mixin
                    .clone();
                assert!(std::rc::Rc::ptr_eq(&origin.declaration, &class_mixin));
                assert_eq!(origin.index, 0);
                let level_mixin = heap
                    .class_application(level)
                    .expect("a level is a class")
                    .mixin
                    .clone();
                assert!(std::rc::Rc::ptr_eq(
                    &level_mixin,
                    &super::nested_named(heap, &kernel_mixin, "IndexedCollection")
                ));
                assert!(
                    heap.class_superclass(level)
                        .is_some_and(|superclass| superclass.same(collection)),
                    "{kind:?}'s level sits on Collection"
                );
            }
        });
    }

    /// The level `IndexedCollection` adds under `Array` takes its lexical
    /// context from the class it applies (§6.2), and that class is the
    /// kernel's: enclosed by the kernel instance, declared by the kernel's
    /// class, as is each one's metaclass (§6.3). A level built before the
    /// kernel instance existed would otherwise keep an empty context, and
    /// `outer` from a method of the mixin would answer nothing.
    #[test]
    fn an_applied_level_of_a_builtin_has_the_kernels_lexical_context() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let Some(kernel) = mutator.kernel.and_then(|value| value.as_object()) else {
                panic!("the kernel is instantiated at bootstrap");
            };
            let heap = mutator.heap();
            let kernel_class = heap.object_class(kernel);
            let array = mutator.builtin_class(BuiltinClass::Array);
            let level = heap
                .class_superclass(array)
                .expect("Array has a superclass");
            assert!(heap.class_is_applied_level(level));
            let indexed_collection = indexed_collection_class(mutator, kernel)
                .expect("the kernel's hidden slot holds IndexedCollection's class");
            for class in [level, indexed_collection] {
                let metaclass = heap
                    .class_metaclass(class)
                    .expect("a class has a metaclass");
                for each in [class, metaclass] {
                    assert!(
                        heap.class_enclosing(each)
                            .is_some_and(|enclosing| enclosing.same(kernel)),
                        "enclosed by the kernel instance"
                    );
                    assert!(
                        heap.class_declaring(each)
                            .is_some_and(|declaring| declaring.same(kernel_class)),
                        "declared by the kernel's class"
                    );
                }
            }
        });
    }

    /// The kernel phase builds `IndexedCollection`'s class itself, before
    /// any shipped module boots, and leaves it in the kernel's hidden slot:
    /// the accessor answers that class, so `complete_shipped_world` builds
    /// no second one beside the class the level was applied from.
    #[test]
    fn the_kernel_phase_fills_the_hidden_slot_of_an_applied_kernel_class() {
        assert_the_kernel_phase_fills_the_hidden_slot(crate::shipped_code::ShippedCodePath::Source);
    }

    /// As `the_kernel_phase_fills_the_hidden_slot_of_an_applied_kernel_class`,
    /// over the kernel decoded from the embedded shipped code: the decoded
    /// mixin carries no declaration, so the phase reads the clauses off the
    /// mixin alone.
    #[test]
    fn the_kernel_phase_fills_the_hidden_slot_from_the_embedded_code() {
        assert!(
            crate::shipped_code::usable(crate::shipped_code::embedded()),
            "run `cargo xtask shipped-code`"
        );
        assert_the_kernel_phase_fills_the_hidden_slot(
            crate::shipped_code::ShippedCodePath::Embedded,
        );
    }

    fn assert_the_kernel_phase_fills_the_hidden_slot(path: crate::shipped_code::ShippedCodePath) {
        let mut interpreter = Interpreter::with_kernel(crate::Lowering::Optimized, path);
        interpreter.enter(|mutator| {
            let Some(kernel) = mutator.kernel.and_then(|value| value.as_object()) else {
                panic!("the kernel is instantiated by its phase");
            };
            let class = indexed_collection_class(mutator, kernel)
                .expect("the kernel phase fills IndexedCollection's hidden slot");
            let collection = mutator.builtin_class(BuiltinClass::Collection);
            assert!(
                mutator
                    .heap()
                    .class_superclass(class)
                    .is_some_and(|superclass| superclass.same(collection))
            );
        });
    }

    /// The class in the kernel's hidden slot for `IndexedCollection`, if
    /// one is there.
    fn indexed_collection_class<'gc>(
        mutator: &crate::interpreter::Mutator<'gc>,
        kernel: crate::types::ObjectReference<'gc>,
    ) -> Option<crate::types::ClassReference<'gc>> {
        let application = super::kernel_application(mutator, kernel);
        let nested = super::nested_named(mutator.heap(), &application.mixin, "IndexedCollection");
        let id = mutator.heap().known_mixin_id(&nested)?;
        let slot = application.nested_slot.get(&id).copied()?;
        mutator
            .heap()
            .instance_field(kernel, slot)
            .and_then(|value| value.as_object())
    }

    /// §3.1.1's "its class inherits from class `Value`", as the flag the two
    /// refusals read. It is recorded as `Value` itself is filled, so every
    /// class filled after it in the same pass sees it -- recorded after the
    /// pass, `String` and every other value class would answer `false` and a
    /// `String` in a value's slot would be refused as if it were mutable.
    #[test]
    fn every_value_classs_application_inherits_value() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            for (kind, expected) in [
                (BuiltinClass::String, true),
                (BuiltinClass::Symbol, true),
                (BuiltinClass::Integer, true),
                (BuiltinClass::LargeInteger, true),
                (BuiltinClass::Float, true),
                (BuiltinClass::Fraction, true),
                (BuiltinClass::True, true),
                (BuiltinClass::UndefinedObject, true),
                (BuiltinClass::Number, true),
                (BuiltinClass::Array, false),
                (BuiltinClass::ByteArray, false),
                (BuiltinClass::Closure, false),
                (BuiltinClass::Object, false),
                // §3.2's "subclass of" is inclusive -- "C's superclass is S
                // or a subclass of S" -- so `Value`'s own application meets
                // §3.1.1's "its class inherits from class `Value`", and
                // `Value new` is a value. It answered `false` until
                // 2026-09-23, which made `Value new = Value new` true while
                // a slot could not hold one: `builtins::value`'s
                // `state_is_a_value`, the other predicate, had always said
                // it was one.
                (BuiltinClass::Value, true),
                (BuiltinClass::VmMirror, true),
            ] {
                let class = mutator.builtin_class(kind);
                assert_eq!(
                    mutator.heap().class_inherits_value(class),
                    expected,
                    "{kind:?}"
                );
            }
        });
    }

    /// The nested-class accessor inside kernel code answers the builtin
    /// class itself, not a second class object of the same mixin.
    #[test]
    fn the_kernels_hidden_class_slots_hold_the_builtin_classes() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let Some(kernel) = mutator.kernel.and_then(|value| value.as_object()) else {
                panic!("the kernel is instantiated at bootstrap");
            };
            for kind in BuiltinClass::ALL {
                if kind == BuiltinClass::VmMirror {
                    continue;
                }
                let class = mutator.builtin_class(kind);
                assert!(
                    mutator
                        .heap()
                        .class_enclosing(class)
                        .is_some_and(|enclosing| enclosing.same(kernel)),
                    "{kind:?}'s enclosing object is the kernel"
                );
                let field = super::hidden_slot_of(mutator, kernel, kind);
                assert!(
                    mutator
                        .heap()
                        .instance_field(kernel, field)
                        .and_then(|value| value.as_object())
                        .is_some_and(|held| held.same(class)),
                    "{kind:?}'s hidden slot holds the builtin class"
                );
            }
        });
    }

    #[test]
    fn the_kernel_holds_the_mirror() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let Some(kernel) = mutator.kernel.and_then(|value| value.as_object()) else {
                panic!("the kernel is instantiated at bootstrap");
            };
            let held = mutator
                .heap()
                .instance_field(kernel, super::mirror_slot_of(mutator, kernel));
            assert!(held.is_some_and(|value| mutator.is_vm_mirror(value)));
        });
    }

    /// §3.1.1 asks a value's enclosing objects to be values, and every
    /// builtin value class is nested in the kernel, so the kernel instance
    /// is one -- `Object <: Value`, as SOMns's `Kernel.ns` declares it --
    /// and so is the mirror its one slot holds, by the ordinary rule.
    #[test]
    fn the_kernel_and_the_mirror_are_values() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let Some(kernel) = mutator.kernel.and_then(|value| value.as_object()) else {
                panic!("the kernel is instantiated at bootstrap");
            };
            let kernel_class = mutator.heap().object_class(kernel);
            assert!(mutator.heap().class_inherits_value(kernel_class));
            let Some(mirror) = mutator.vm_mirror().as_object() else {
                panic!("the mirror is an object");
            };
            let mirror_class = mutator.heap().object_class(mirror);
            assert!(mutator.heap().class_inherits_value(mirror_class));
        });
    }

    /// `Kernel.ns`'s root is `= Value`, built by the root rule as any
    /// module's root: the kernel class's superclass is the builtin `Value`
    /// itself, no applied level, and the kernel instance is a value.
    #[test]
    fn the_kernel_class_is_built_over_value() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let Some(kernel) = mutator.kernel.and_then(|value| value.as_object()) else {
                panic!("the kernel is instantiated at bootstrap");
            };
            let kernel_class = mutator.heap().object_class(kernel);
            let superclass = mutator
                .heap()
                .class_superclass(kernel_class)
                .expect("the kernel class has a superclass");
            assert!(superclass.same(mutator.builtin_class(BuiltinClass::Value)));
            assert!(!mutator.heap().class_is_applied_level(superclass));
            assert!(mutator.heap().class_inherits_value(kernel_class));
        });
    }

    /// A kernel-shaped module resolved by `resolve_kernel`, for the checks
    /// that read only the resolved source.
    fn resolve_kernel_fixture(source: &str) -> crate::ResolvedModule {
        let module = ns_syntax::parse(source).expect("the fixture parses");
        let Ok(resolved) = crate::resolver::resolve_kernel(
            module,
            std::rc::Rc::from(source),
            crate::SourceOrigin::File(std::path::PathBuf::from("fixture.ns")),
        ) else {
            panic!("the fixture resolves");
        };
        resolved
    }

    #[test]
    fn a_mirror_send_whose_only_literal_block_is_its_failure_block_is_accepted() {
        let resolved = resolve_kernel_fixture(indoc::indoc! {"
            class K vmMirror: theMirror = Object new (
              | private vmMirror = theMirror. |
            ) (
              class C = Object new () (
                public ifTrue: action = ( ^vmMirror boolean: self ifTrue: action ifFail: [^nil] )
              )
            ) : ()
        "});
        super::check_mirror_send_arguments(&resolved, "Kernel");
    }

    /// A literal block before the failure block would be exempt from the
    /// depth count under the reference lowering and counted under the
    /// optimized one, so the kernel may not contain one.
    #[test]
    #[should_panic(expected = "only the final ifFail: argument may be a literal block")]
    fn a_mirror_send_with_a_literal_block_before_its_failure_block_is_refused() {
        let resolved = resolve_kernel_fixture(indoc::indoc! {"
            class K vmMirror: theMirror = Object new (
              | private vmMirror = theMirror. |
            ) (
              class C = Object new () (
                public probe = ( ^[vmMirror boolean: self ifTrue: [1] ifFail: [^nil]] value )
              )
            ) : ()
        "});
        super::check_mirror_send_arguments(&resolved, "Kernel");
    }

    /// `Kernel.ns` resolves in kernel mode, and every send to `vmMirror` in
    /// it carries the mark the recognizers read.
    #[test]
    fn every_send_to_vm_mirror_in_the_kernel_is_marked() {
        let resolved = super::resolved_kernel();
        super::for_each_send_to_vm_mirror(&resolved, |send| {
            assert!(
                matches!(
                    resolved.bindings.get(&send.span),
                    Some(crate::Binding::MirrorSend)
                ),
                "a send to vmMirror at {:?} is not marked",
                send.span
            );
        });
    }

    /// The mark follows the rule `resolver::is_the_mirror_slot` states: the
    /// bare `vmMirror` naming the root class's member, at any nesting depth,
    /// and nothing a local or a nearer class declaration shadows. A user
    /// module resolved with `resolve` carries no mark at all.
    #[test]
    fn only_a_send_to_the_root_classs_vm_mirror_is_marked() {
        use indoc::indoc;
        let source = indoc! {"
            class Test vmMirror: theMirror = Object new (
                | private vmMirror = theMirror. |
            ) (
                direct = ( ^vmMirror leafOne )
                shadowed: vmMirror = ( ^vmMirror leafTwo )
                class Inner = () (
                    nested = ( ^[vmMirror leafThree] value )
                )
                class Redeclares = (
                    | vmMirror = nil. |
                ) (
                    redeclared = ( ^vmMirror leafFour )
                )
            ) : ()
        "};
        let resolve = |kernel: bool| {
            let module = ns_syntax::parse(source).expect("the test module parses");
            let origin = crate::SourceOrigin::File(std::path::PathBuf::from("test.ns"));
            let resolved = if kernel {
                crate::resolver::resolve_kernel(module, std::rc::Rc::from(source), origin)
            } else {
                crate::resolve(module, std::rc::Rc::from(source), origin)
            };
            match resolved {
                Ok(resolved) => resolved,
                Err(_) => panic!("the test module resolves"),
            }
        };
        let marked = |resolved: &crate::ResolvedModule| {
            let mut marked = Vec::new();
            super::for_each_send_to_vm_mirror(resolved, |send| {
                if matches!(
                    resolved.bindings.get(&send.span),
                    Some(crate::Binding::MirrorSend)
                ) {
                    let ns_syntax::ast::Selector::Unary(selector) = &send.message.selector else {
                        panic!("every send in the test module is unary");
                    };
                    marked.push(selector.clone());
                }
            });
            marked
        };
        assert_eq!(marked(&resolve(true)), ["leafOne", "leafThree"]);
        assert!(marked(&resolve(false)).is_empty());
    }
}
