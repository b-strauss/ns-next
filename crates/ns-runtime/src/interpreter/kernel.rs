//! The bootstrap's kernel phase (`docs/design/core-library.md`, "The
//! bootstrap: placeholders first, kernel second", which lists its steps):
//! `Kernel.ns`, compiled, fills the builtin class objects `World::new` built
//! as placeholders, and is then instantiated with the VM mirror.

use std::rc::Rc;

use ns_syntax::ast;

use crate::heap::Root;
use crate::interpreter::{HostSignal, Interpreter, Mutator};
use crate::mixin::{mixin_key, Mixin};
use crate::types::*;

/// `Kernel.ns`, compiled into the binary: the library is source the runtime
/// ships (`docs/design/core-library.md`, "Where the library source lives,
/// and how modules are found").
pub(crate) const KERNEL_SOURCE: &str = include_str!("../../core-library/Kernel.ns");

thread_local! {
    /// The kernel, parsed and resolved once per thread: the AST and its
    /// bindings are immutable, and the test suite bootstraps well over a
    /// hundred interpreters. Each interpreter still compiles it, because
    /// compiled code is numbered against one heap's inline caches.
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
        Ok(resolved) => resolved,
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
        let resolved = resolved_kernel();
        let (kernel_class, mirror) = self.enter(|mutator| {
            let kernel_class = mutator.fill_builtin_classes(&resolved);
            let mirror = mutator.vm_mirror();
            (
                mutator.stash(Value::object(kernel_class)),
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
        self.enter(|mutator| mutator.wire_kernel_in(&kernel_class, &kernel));
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
    /// `Object`. Then the kernel's own class is built over the filled
    /// `Object`, every class object built so far is given its metaclass over
    /// the filled `Class`, and the kernel's class is answered for the host
    /// to instantiate.
    ///
    /// Nothing has run yet, so no inline cache or instance built before
    /// this phase can still hold a placeholder application.
    fn fill_builtin_classes(&mut self, resolved: &ResolvedModule) -> ClassReference<'gc> {
        let declaration = &resolved.module.root_class;
        let root_mixin = self.build_root_mixin(
            declaration,
            &resolved.bindings,
            super::instantiate::ModuleSource::Kernel,
        );
        check_kernel_shape(&root_mixin, resolved);
        for kind in fill_order(&root_mixin) {
            let class = self.builtin_class(kind);
            let mixin = if kind == BuiltinClass::VmMirror {
                self.heap
                    .class_application(class)
                    .map(|application| application.mixin.clone())
                    .expect("internal: the mirror's class has its placeholder application")
            } else {
                nested_named(&root_mixin, kernel_class_name(kind)).clone()
            };
            let superclass = superclass_of_kind(&root_mixin, kind);
            let super_application = superclass
                .and_then(|superclass| self.heap.class_application(self.builtin_class(superclass)));
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
            let application = self.apply_mixin(&mixin, super_application.as_ref());
            if !self.heap.class_set_application(class, application) {
                unreachable!("internal: a builtin class is a class object");
            }
            let linked = match superclass {
                Some(superclass) => {
                    let superclass_object = self.builtin_class(superclass);
                    self.heap.class_set_superclass(class, superclass_object)
                }
                None => self.heap.class_clear_superclass(class),
            };
            if !linked {
                unreachable!("internal: a builtin class is a class object");
            }
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
        // `Object <: Value`: the level is built as a user root's is, from
        // the filled `Value`, and its lexical context is set once the kernel
        // instance exists (`wire_kernel_in`), as every builtin class's is.
        assert!(
            root_mixin.root_clause_defect.is_none() && root_mixin.mixin_applications.len() == 1,
            "internal: Kernel.ns's root must be `Object <: Value`"
        );
        let object = self.builtin_class(BuiltinClass::Object);
        let value = self.builtin_class(BuiltinClass::Value);
        let object_application = self.heap.class_application(object);
        let (level, level_application) = match super::nested::build_applied_levels(
            self,
            &root_mixin,
            object_application.map(|application| (object, application)),
            &[(0, Value::object(value))],
        ) {
            Ok(Some(level)) => level,
            Ok(None) | Err(_) => panic!("internal: the kernel's Value level is a class object"),
        };
        let application = self.apply_mixin(&root_mixin, Some(&level_application));
        let kernel_class = self.new_class_object(application, None, None, Some(level));
        // `Class` is filled, so every class object built so far -- each
        // builtin, the `Value` level and the kernel's own class -- can get
        // its metaclass now, and every one built from here on gets it with
        // the class object.
        self.give_existing_class_objects_metaclasses(&[level, kernel_class]);
        kernel_class
    }

    /// The kernel instance becomes every kernel class's enclosing object,
    /// and each of its hidden nested-class slots holds the builtin class it
    /// names -- so the nested-class accessor in kernel code answers the
    /// builtin class itself rather than building a second class object of
    /// the same mixin.
    fn wire_kernel_in(&mut self, kernel_class: &Root, kernel: &Root) {
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
                self.heap.instance_field(kernel, field).is_some_and(|value| value.is_nil()),
                "internal: Kernel's initializer touched the nested class {kind:?} before the bootstrap could fill it"
            );
            if !self
                .heap
                .instance_set_field(kernel, field, Value::object(class))
            {
                unreachable!("internal: the kernel instance has a field per nested class");
            }
            if !self
                .heap
                .class_set_lexical_context(class, Some(kernel), Some(kernel_class))
            {
                unreachable!("internal: a builtin class is a class object");
            }
            // The metaclass was built at the end of the fill, when the
            // builtin's lexical context was still the placeholder's; §6.3
            // puts a class side in its class's declaration, so it takes the
            // same context.
            self.set_metaclass_lexical_context(class, kernel, kernel_class);
        }
        // The kernel's own `Value` level was built before the kernel
        // instance existed, over `Value`'s then-empty context.
        let mut level = self.heap.class_superclass(kernel_class);
        while let Some(class) = level {
            if !self.heap.class_is_applied_level(class) {
                break;
            }
            if !self
                .heap
                .class_set_lexical_context(class, Some(kernel), Some(kernel_class))
            {
                unreachable!("internal: an applied level is a class object");
            }
            self.set_metaclass_lexical_context(class, kernel, kernel_class);
            level = self.heap.class_superclass(class);
        }
        self.kernel = Some(Value::object(kernel));
    }

    /// Gives `class`'s metaclass the lexical context `wire_kernel_in` just
    /// gave `class`: the kernel instance and the kernel's class.
    fn set_metaclass_lexical_context(
        &mut self,
        class: ClassReference<'gc>,
        kernel: ObjectReference<'gc>,
        kernel_class: ClassReference<'gc>,
    ) {
        if let Some(metaclass) = self.heap.class_metaclass(class) {
            if !self
                .heap
                .class_set_lexical_context(metaclass, Some(kernel), Some(kernel_class))
            {
                unreachable!("internal: a metaclass is a class object");
            }
        }
    }
}

/// The kernel's nested class called `name`. `check_kernel_shape` has made
/// sure there is exactly one.
fn nested_named<'mixin>(root: &'mixin Rc<Mixin>, name: &str) -> &'mixin Rc<Mixin> {
    root.nested
        .iter()
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
fn fill_order(root: &Rc<Mixin>) -> Vec<BuiltinClass> {
    let mut order: Vec<BuiltinClass> = BuiltinClass::ALL.into_iter().collect();
    order.sort_by_cached_key(|kind| depth_below_top(root, *kind));
    order
}

/// How many superclass links separate `kind` from `Top`, which is 0 links
/// from itself. A chain that repeats a class or ends anywhere but `Top` is a
/// defect in `Kernel.ns`, not in a program, so it panics naming where the
/// walk went wrong: a subclass whose chain does not reach the root would be
/// filled over a placeholder, and a cycle would not terminate.
fn depth_below_top(root: &Rc<Mixin>, kind: BuiltinClass) -> usize {
    let mut walked: Vec<BuiltinClass> = Vec::with_capacity(BuiltinClass::ALL.len());
    let mut current = kind;
    while let Some(superclass) = superclass_of_kind(root, current) {
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
fn superclass_of_kind(root: &Rc<Mixin>, kind: BuiltinClass) -> Option<BuiltinClass> {
    if kind == BuiltinClass::VmMirror {
        return Some(BuiltinClass::Value);
    }
    superclass_kind(nested_named(root, kernel_class_name(kind)))
}

/// The builtin a kernel class's clause names, read off the declaration and
/// never run; `None` for `Top`.
fn superclass_kind(nested: &Rc<Mixin>) -> Option<BuiltinClass> {
    let declaration = nested.declaration.as_ref()?;
    let clause = declaration.superclass.as_ref()?;
    Some(kernel_kind_named(&clause.receiver.name).unwrap_or_else(|| {
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
/// each declaring no slots -- but the exception classes and `Message` --
/// and the default factory `new`; a root class with
/// exactly one slot, the immutable `vmMirror`; a clause on every class but
/// `Top`, a bare name with no message arguments -- it is read, never run --
/// naming a kernel class, from which the chain reaches `Top` without
/// repeating, so that `fill_order` can sort by depth. And every mirror send
/// passes a literal block only as its failure block
/// (`check_mirror_send_arguments`).
fn check_kernel_shape(root: &Rc<Mixin>, resolved: &ResolvedModule) {
    check_mirror_send_arguments(resolved);
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
    assert_eq!(
        root.nested.len(),
        kinds.len(),
        "internal: Kernel.ns must declare exactly one nested class per builtin class"
    );
    for kind in kinds.iter().copied() {
        let name = kernel_class_name(kind);
        let matching = root
            .nested
            .iter()
            .filter(|nested| nested.name.as_str() == name)
            .count();
        assert_eq!(
            matching, 1,
            "internal: Kernel.ns must declare the class {name} exactly once"
        );
        let nested = nested_named(root, name);
        // Rust builds a builtin's instances with a payload and no fields, so
        // a slot would have nowhere to live -- except on the exception
        // classes, whose instances `decide_instantiation` gives an ordinary
        // field vector like any other class's (`docs/design/core-library.md`,
        // "Exceptions"). `Activation` declares no slots of its own: an
        // activation's state lives in the running activation it wraps.
        // `FutureImplementation`'s two slots are an ordinary field vector
        // too, which `interpreter::future` allocates and alone reads and
        // writes.
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
                ),
            "internal: Kernel.ns's {name} declares a slot; a builtin's instances have none"
        );
        let declaration = nested
            .declaration
            .as_ref()
            .unwrap_or_else(|| panic!("internal: Kernel.ns's {name} was built from no source"));
        // `FutureImplementation` alone takes its computation through its
        // factory, so that `Future computing:` stores it untouched; a
        // simultaneous slot clause makes its futures without sending it.
        assert!(
            declaration.factory.is_none() || kind == BuiltinClass::FutureImplementation,
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
    }
    // Every chain ends at `Top` without repeating, which is what lets
    // `fill_order` sort the kinds by depth. Checked once the clauses are
    // known to be readable, so a malformed one is reported as itself rather
    // than as a broken chain.
    for kind in kinds {
        depth_below_top(root, kind);
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
/// `MixinApplication::nested_slot`, keyed by the nested mixin's identity.
pub(super) fn hidden_slot_of<'gc>(
    mutator: &Mutator<'gc>,
    kernel: ObjectReference<'gc>,
    kind: BuiltinClass,
) -> usize {
    let application = kernel_application(mutator, kernel);
    let nested = nested_named(&application.mixin, kernel_class_name(kind));
    application
        .nested_slot
        .get(&mixin_key(nested))
        .copied()
        .unwrap_or_else(|| panic!("internal: the kernel's application has no slot for {kind:?}"))
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

/// A mirror send in the kernel passes a literal block only as its final,
/// `ifFail:` argument. A literal block anywhere before it would be a block
/// the leaf itself activates (as `whileTrue:`'s would be, were it passed
/// as a literal), and the reference lowering, which marks the whole send
/// `SendInlinable` because its failure block is inlined by the optimized
/// lowering, would exempt that block's activation from the depth count too
/// -- while the optimized lowering counts it. The two lowerings would then count activations
/// differently (`docs/design/compiler.md`, "How the limit counts"), so the
/// shape is refused rather than the count special-cased. Such a block is
/// passed through a local or an argument instead, which neither lowering
/// marks.
fn check_mirror_send_arguments(resolved: &ResolvedModule) {
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
                "internal: Kernel.ns passes a literal block to the mirror's {} before its \
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
            for slot in &side.slots {
                if let Some(initializer) = &slot.initializer {
                    expression(initializer, visit);
                }
            }
            statements(&side.initializer_expressions, visit);
            for method in &side.methods {
                body(&method.body, visit);
            }
            for nested in &side.nested_classes {
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
            match statement {
                ast::Statement::Expression(inner) | ast::Statement::Return(inner) => {
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
    use crate::{BuiltinClass, Interpreter};

    /// Every builtin class but the mirror's is the kernel class of its name.
    #[test]
    fn every_builtin_class_is_filled_from_the_kernel() {
        let mut interpreter = Interpreter::bootstrap();
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
    /// an entry is (class, selector).
    const NOT_FORWARDERS: &[(BuiltinClass, &str)] = &[
        (BuiltinClass::True, "hash"),
        (BuiltinClass::False, "hash"),
        // §3.1.1's identity method on a value: a send of `=`, which is what
        // makes the two agree for every subclass. No leaf could answer it
        // without sending `=` back into Newspeak.
        (BuiltinClass::Value, "=="),
        // The Newspeak side of the derivation, which runs when a leaf
        // declines: both send `=` or `hash` to a slot, which a leaf may not.
        (BuiltinClass::Value, "valueEquals:"),
        (BuiltinClass::Value, "valueHashOfSlots"),
        // A send of `hash`, which no leaf could answer for a value.
        (BuiltinClass::Value, "identityHash"),
        (BuiltinClass::Behavior, "identityHash"),
        // Forwarders in shape, but of their subject rather than of `self`:
        // §5.9 reaches a private member only through a lexical send, so the
        // other side's state has to be read by naming it as an argument,
        // which a primitive method may not do.
        (BuiltinClass::Value, "valueSlotCountOf:"),
        (BuiltinClass::Value, "valueSlotOf:at:"),
        // A string literal is no quick answer: an ordinary method, like
        // `hash` above, whose integer literal is none either.
        (BuiltinClass::True, "printString"),
        (BuiltinClass::False, "printString"),
        (BuiltinClass::UndefinedObject, "printString"),
        // `cull:` tests its own arity, which no quick method does; the quick
        // methods of the nil protocol do what it says without sending it.
        (BuiltinClass::Closure, "cull:"),
        // A native loop's send of `value:` to an action that is no block,
        // pushed as a method so that its return carries the loop on
        // (`Mutator::send_value_then`): a send, no leaf.
        (BuiltinClass::Closure, "valueOf:with:"),
        // Iteration is Newspeak, after Primordial Soup's `Number`: a loop that
        // sends `<=`, `>=` and `+` to its receiver, which no leaf could do.
        (BuiltinClass::Number, "to:do:"),
        (BuiltinClass::Number, "to:by:do:"),
        // Primordial Soup's Newspeak body, `-1 - self`: a send of `-` to
        // `self`, which no leaf could do, and which `LargeInteger` inherits.
        (BuiltinClass::Integer, "bitInvert"),
        // Integer's methods written in Newspeak: `&` and `|` send `bitAnd:`
        // and `bitOr:`, the modular pair loops over sends of arithmetic, and
        // `denominator` answers the integer literal 1, which is no quick
        // constant.
        (BuiltinClass::Integer, "&"),
        (BuiltinClass::Integer, "|"),
        (BuiltinClass::Integer, "raisedTo:modulo:"),
        (BuiltinClass::Integer, "reciprocalModulo:"),
        (BuiltinClass::Integer, "denominator"),
        // The class side: `parse:` sends `parse:radix:` to `self`.
        (BuiltinClass::Integer, "parse:"),
        // Primordial Soup's Newspeak bodies: `timesRepeat:` sends `to:do:`
        // and `value`, and `max:` sends `>` to `self`, which no leaf could
        // do.
        (BuiltinClass::Integer, "timesRepeat:"),
        (BuiltinClass::Number, "max:"),
        (BuiltinClass::Number, "min:"),
        (BuiltinClass::Number, "between:and:"),
        (BuiltinClass::Number, "sqrt"),
        (BuiltinClass::Number, "sin"),
        (BuiltinClass::Number, "cos"),
        (BuiltinClass::Number, "tan"),
        (BuiltinClass::Number, "asin"),
        (BuiltinClass::Number, "acos"),
        (BuiltinClass::Number, "atan"),
        (BuiltinClass::Number, "atan:"),
        (BuiltinClass::Number, "exp"),
        (BuiltinClass::Number, "ln"),
        (BuiltinClass::Number, "log"),
        (BuiltinClass::Number, "negated"),
        (BuiltinClass::Number, "abs"),
        (BuiltinClass::Number, "raisedTo:"),
        // A send to `self` is no leaf: `printString` reads the class's own
        // `name`, and a metaclass's `name` reads its own `thisClass` first.
        (BuiltinClass::Class, "printString"),
        (BuiltinClass::Metaclass, "name"),
        (BuiltinClass::Metaclass, "printString"),
        // The Newspeak side of a value class object's equality, which runs
        // when the leaf declines: it sends `=` to an enclosing object and a
        // superclass, which a leaf may not.
        (BuiltinClass::Behavior, "behaviorEquals:"),
        // Forwarders in shape, but of their subject rather than of `self`,
        // for the reason `Value`'s slot readers above are.
        (BuiltinClass::Behavior, "enclosingOf:"),
        (BuiltinClass::Behavior, "superclassOf:"),
        (BuiltinClass::Metaclass, "thisClassOf:"),
        // The marked methods (`UNWIND_MARKERS`): each keeps a local the
        // unwinder or `signal` reads, and sends `value` to `self`.
        (BuiltinClass::Closure, "on:do:"),
        (BuiltinClass::Closure, "ensure:"),
        (BuiltinClass::Closure, "ifCurtailed:"),
        // A forwarder in shape, but of its argument rather than of `self`,
        // for the reason `Value`'s slot readers above are: the leaf's
        // subject is the exception, and the class is its argument.
        (BuiltinClass::Class, "handles:"),
        // `Object>>halt` signals a `Halt`: sends, no leaf.
        (BuiltinClass::Object, "halt"),
        // `Object>>doesNotUnderstand:` signals a `MessageNotUnderstood` and
        // may send the message again: sends, no leaf.
        (BuiltinClass::Object, "doesNotUnderstand:"),
        // `Object>>subclassResponsibility` makes several mirror sends and
        // signals an `Error`: sends, no leaf.
        (BuiltinClass::Object, "subclassResponsibility"),
        // `Collection`'s protocol is written over `do:`, which a subclass
        // defines: every method sends, none forwards to a leaf.
        (BuiltinClass::Collection, "do:"),
        (BuiltinClass::Collection, "newForCollectUsingAdd:"),
        (BuiltinClass::Collection, "allSatisfy:"),
        (BuiltinClass::Collection, "anySatisfy:"),
        (BuiltinClass::Collection, "noneSatisfy:"),
        (BuiltinClass::Collection, "detect:"),
        (BuiltinClass::Collection, "detect:ifNone:"),
        (BuiltinClass::Collection, "do:separatedBy:"),
        (BuiltinClass::Collection, "includes:"),
        (BuiltinClass::Collection, "inject:into:"),
        (BuiltinClass::Collection, "isEmpty"),
        (BuiltinClass::Collection, "reduce:"),
        (BuiltinClass::Collection, "reduce:ifEmpty:"),
        (BuiltinClass::Collection, "collect:"),
        (BuiltinClass::Collection, "select:"),
        (BuiltinClass::Collection, "reject:"),
        (BuiltinClass::Collection, "size"),
        // A fraction's text is built from its parts' texts: sends, no leaf.
        (BuiltinClass::Fraction, "printString"),
        (BuiltinClass::Fraction, "asString"),
        (BuiltinClass::Fraction, "floor"),
        (BuiltinClass::Fraction, "ceiling"),
        (BuiltinClass::Fraction, "truncated"),
        (BuiltinClass::Fraction, "asInteger"),
        (BuiltinClass::Fraction, "rounded"),
        // `Array`'s sequence protocol is Newspeak over its leaves --
        // `at:`, `at:put:`, `size`, `copyFrom:to:` and
        // `replaceFrom:to:with:startingAt:` -- and `new:` on its class side:
        // every method sends, none forwards to a leaf.
        (BuiltinClass::Array, "isEmpty"),
        (BuiltinClass::Array, "first"),
        (BuiltinClass::Array, "last"),
        (BuiltinClass::Array, "indexOf:"),
        (BuiltinClass::Array, "keysAndValuesDo:"),
        (
            BuiltinClass::Array,
            "replaceFrom:to:withElementsOf:startingAt:",
        ),
        (BuiltinClass::Array, ","),
        (BuiltinClass::Array, "copyWith:"),
        (BuiltinClass::Array, "copyWithSize:"),
        (BuiltinClass::Array, "collect:"),
        (BuiltinClass::Array, "select:"),
        (BuiltinClass::Array, "reject:"),
        (BuiltinClass::Array, "sort:"),
        (BuiltinClass::Array, "sortFrom:to:by:"),
        (BuiltinClass::Array, "merge:into:from:width:count:by:"),
        (BuiltinClass::Array, "withAll:"),
        // `String`'s code-point protocol is Newspeak over its leaves --
        // `size`, `at:`, `copyFrom:to:` and the two searches: every method
        // sends, none forwards to a leaf.
        (BuiltinClass::String, "do:"),
        (BuiltinClass::String, "isEmpty"),
        (BuiltinClass::String, "first"),
        (BuiltinClass::String, "last"),
        (BuiltinClass::String, "stringAt:"),
        (BuiltinClass::String, "copyStringFrom:to:"),
        (BuiltinClass::String, "indexOf:"),
        (BuiltinClass::String, "lastIndexOf:"),
        (BuiltinClass::String, "startsWith:"),
        (BuiltinClass::String, "endsWith:"),
        // What a `MessageNotUnderstood` hands a host: its message's
        // selector, a send, and its receiver, a slot.
        (BuiltinClass::MessageNotUnderstood, "hostSelector"),
        (BuiltinClass::MessageNotUnderstood, "hostReceiver"),
        // The exception protocol (`docs/design/core-library.md`, "Exceptions")
        // is Newspeak over the activation leaves: each method sends, keeps
        // state in the exception's slots, or makes several mirror sends, none
        // of them a forwarder of `self` alone.
        (BuiltinClass::Exception, "description"),
        (BuiltinClass::Exception, "printString"),
        (BuiltinClass::Exception, "reachedDefaultHandler"),
        (BuiltinClass::Exception, "signal"),
        (BuiltinClass::Exception, "signal:"),
        (BuiltinClass::Exception, "pass"),
        (BuiltinClass::Exception, "return:"),
        (BuiltinClass::Exception, "return"),
        (BuiltinClass::Exception, "retry"),
        (BuiltinClass::Exception, "resume:"),
        (BuiltinClass::Exception, "resume"),
        (BuiltinClass::Exception, "defaultAction"),
        (BuiltinClass::Exception, "hostSelector"),
        (BuiltinClass::Exception, "hostReceiver"),
        (BuiltinClass::Exception, "invokeNextHandler"),
        (BuiltinClass::Exception, "isHandledAt:"),
        (BuiltinClass::Exception, "invokeHandlerAt:disabling:"),
        (BuiltinClass::Exception, "notInAHandler:"),
        // The class sides' `signal` and `signal:`, `^self new signal`: an
        // instantiation and a send. §3.2's metaclasses are flat, so each
        // kernel exception class declares its own.
        (BuiltinClass::Error, "signal"),
        (BuiltinClass::Error, "signal:"),
        (BuiltinClass::MessageNotUnderstood, "signal"),
        (BuiltinClass::MessageNotUnderstood, "signal:"),
        (BuiltinClass::Halt, "signal"),
        (BuiltinClass::Halt, "signal:"),
        (BuiltinClass::ArgumentError, "signal"),
        (BuiltinClass::ArgumentError, "signal:"),
        (BuiltinClass::CannotReturn, "signal"),
        (BuiltinClass::CannotReturn, "signal:"),
        (BuiltinClass::NonBooleanReceiver, "signal"),
        (BuiltinClass::NonBooleanReceiver, "signal:"),
        (BuiltinClass::CannotInstantiate, "signal"),
        (BuiltinClass::CannotInstantiate, "signal:"),
        // The secondary factories of the error classes that carry a
        // subject, `^self new argument: offending` and its siblings: an
        // instantiation and a send.
        (BuiltinClass::ArgumentError, "value:"),
        (BuiltinClass::CannotReturn, "result:"),
        (BuiltinClass::NonBooleanReceiver, "receiver:"),
        (BuiltinClass::CannotInstantiate, "behavior:"),
        // Their `description`: a test of `messageText`, and a send of
        // `printString` to the subject.
        (BuiltinClass::ArgumentError, "description"),
        (BuiltinClass::CannotReturn, "description"),
        (BuiltinClass::NonBooleanReceiver, "description"),
        (BuiltinClass::CannotInstantiate, "description"),
        // A forwarder in shape, but of its argument rather than of `self`:
        // the row's subject is the computation the future will force.
        (BuiltinClass::Future, "computing:"),
    ];

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
        // Quick in shape, though only the unwinder activates it, pushing it
        // as a method so that its return carries the unwind on
        // (`Mutator::start_unwind_block`).
        (
            BuiltinClass::Closure,
            "valueOnUnwind:",
            MethodFastPath::ValueOfParameter(0),
        ),
        // Quick in shape too, though only a native loop activates it, pushing
        // it as a method so that its return carries the loop on
        // (`Mutator::send_value_then`).
        (
            BuiltinClass::Closure,
            "valueOf:",
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

    /// §3.2's accessors for the predefined classes, which `Object` declares:
    /// every builtin a program can name, which is every one but `Top`, the
    /// mirror's class, `Behavior`, `Class`, `Metaclass`, `Activation`,
    /// `FutureImplementation`, `LargeInteger`, which a program meets only as
    /// an integer's class, and the exception classes, `Message`, `Proxy`,
    /// `Future`, `Collection` and `Fraction`, which a program reaches
    /// through `platform kernel`.
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
            for kind in BuiltinClass::ALL {
                // `Proxy`'s methods forward nothing to a leaf: each is a
                // protected override of one of `Object`'s public members,
                // there only to stop §5.5's lookupPublic, and none is ever
                // found by a send a program writes. `tests/future.rs`'s
                // `a_proxy_overrides_every_public_member_of_object_as_protected`
                // checks them instead. `FutureImplementation`'s one method,
                // `doesNotUnderstand:`, forces the future and sends the
                // message on: a send, no leaf.
                if matches!(
                    kind,
                    BuiltinClass::VmMirror | BuiltinClass::Proxy | BuiltinClass::FutureImplementation
                ) {
                    continue;
                }
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
                    if NOT_FORWARDERS.contains(&(kind, selector)) {
                        assert!(
                            method.fast_path.is_none(),
                            "{kind:?}>>{selector} reaches a leaf after all; take it off NOT_FORWARDERS"
                        );
                        continue;
                    }
                    if let Some((_, _, expected)) = QUICK_METHODS
                        .iter()
                        .find(|(owner, name, _)| *owner == kind && *name == selector)
                    {
                        assert_eq!(
                            method.fast_path,
                            Some(*expected),
                            "{kind:?}>>{selector} is not the quick method it should be"
                        );
                        continue;
                    }
                    // §3.2's accessors on `Object` are a fast path of their
                    // own kind, which reaches no row;
                    // `objects_class_accessors_are_exactly_the_nameable_builtins`
                    // checks each one.
                    if kind == BuiltinClass::Object
                        && OBJECT_CLASS_ACCESSORS
                            .iter()
                            .any(|accessor| super::kernel_class_name(*accessor) == selector)
                    {
                        assert!(
                            matches!(
                                method.fast_path,
                                Some(crate::mixin::MethodFastPath::BuiltinClass(_))
                            ),
                            "Object>>{selector} is not recognized as a class accessor"
                        );
                        continue;
                    }
                    let Some(crate::mixin::MethodFastPath::Mirror(row)) = method.fast_path else {
                        panic!("{kind:?}>>{selector} is not recognized as a primitive method");
                    };
                    let reported = REPORTED_UNDER_ANOTHER_SELECTOR
                        .iter()
                        .find(|(owner, name, _)| *owner == kind && *name == selector)
                        .map_or(selector, |(_, _, reported)| *reported);
                    assert_eq!(
                        mirror_entry(row).reported,
                        reported,
                        "{kind:?}>>{selector} forwards to a row reported under another selector"
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
    /// but only the image code reads that recognition, to step a platform
    /// path through the accessor; a send runs the method like any other.
    /// `KernelNamespace`'s members send to the kernel instance the platform was
    /// handed, so they are neither. Each costs one activation more than a
    /// kernel accessor; a namespace member is asked for once, typically in a
    /// slot initializer, not in a loop. `Transcript`'s two and `Stopwatch`'s
    /// clock reading are mirror forwarders like the kernel's:
    /// `PlatformFactory.ns` is resolved with `resolve_kernel`. `Time`'s
    /// `Stopwatch` is a class accessor, like the platform's.
    #[test]
    fn the_platform_modules_members_have_the_fast_path_they_should() {
        let mut interpreter = Interpreter::bootstrap();
        let platform = interpreter.platform();
        let mut class_named =
            |name: &str| match interpreter.send(&platform, crate::Symbol::intern(name), &[]) {
                Ok(class) => class,
                Err(_) => panic!("platform {name} failed"),
            };
        let kernel_namespace = class_named("KernelNamespace");
        let transcript = class_named("Transcript");
        let time = class_named("Time");
        let stopwatch = class_named("Stopwatch");
        let collections = class_named("collections");
        let collections = match interpreter.send(&collections, crate::Symbol::intern("class"), &[])
        {
            Ok(class) => class,
            Err(_) => panic!("platform collections class failed"),
        };
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
                method.fast_path
            };
            assert_eq!(fast_path(&collections, "List"), None, "Collections>>List");
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
            ] {
                assert_eq!(
                    fast_path(&kernel_namespace, selector),
                    None,
                    "KernelNamespace>>{selector}"
                );
            }
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
                method.fast_path,
                Some(crate::mixin::MethodFastPath::Mirror(row))
            );
        });
    }

    /// The platform hands out its module's classes by methods the compiler
    /// recognizes as class accessors (`MixinMethod::class_accessor`), and the
    /// collections facet its module's the same way; only the image code
    /// reads them: none carries a fast path, so no send pays for the
    /// recognition. `Object`'s accessors are `Kernel.ns`'s, whose classes an
    /// image names by their builtin tags, so none of them is one.
    #[test]
    fn the_platforms_class_accessors_are_recognized_without_a_fast_path() {
        let mut interpreter = Interpreter::bootstrap();
        let platform = interpreter.platform();
        let collections =
            match interpreter.send(&platform, crate::Symbol::intern("collections"), &[]) {
                Ok(collections) => collections,
                Err(_) => panic!("platform collections failed"),
            };
        let collections_class =
            match interpreter.send(&collections, crate::Symbol::intern("class"), &[]) {
                Ok(class) => class,
                Err(_) => panic!("platform collections class failed"),
            };
        interpreter.enter(|mutator| {
            let Some(collections_class) = mutator.fetch(&collections_class).as_object() else {
                panic!("the facet's class is an object");
            };
            let facet_application = mutator
                .heap()
                .class_application(collections_class)
                .expect("Collections has an application");
            for name in ["Collections", "List"] {
                let method = facet_application
                    .mixin
                    .compiled_method(crate::Symbol::intern(name))
                    .unwrap_or_else(|| panic!("Collections declares no {name}"));
                assert_eq!(
                    method.class_accessor,
                    Some(crate::mixin::ClassAccessor {
                        depth: 1,
                        class_name: crate::Symbol::intern(name),
                    }),
                    "Collections>>{name}"
                );
                assert_eq!(method.fast_path, None, "Collections>>{name}");
            }
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
                let method = application
                    .mixin
                    .compiled_method(crate::Symbol::intern(name))
                    .unwrap_or_else(|| panic!("Platform declares no {name}"));
                assert_eq!(
                    method.class_accessor,
                    Some(crate::mixin::ClassAccessor {
                        depth: 1,
                        class_name: crate::Symbol::intern(name),
                    }),
                    "Platform>>{name}"
                );
                assert_eq!(method.fast_path, None, "Platform>>{name}");
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
                active_register: 2,
            },
        ),
        (
            "ensure:",
            UnwindMarker::Ensure {
                block_register: 0,
                complete_register: 1,
            },
        ),
        (
            "ifCurtailed:",
            UnwindMarker::IfCurtailed {
                block_register: 0,
                complete_register: 1,
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
                let block = application
                    .mixin
                    .nested
                    .iter()
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
    /// which it reaches there too, nor `Activation` and
    /// `FutureImplementation`, which it does not reach at all, nor
    /// `LargeInteger`, which a
    /// program meets only as an integer's class. `PlatformFactory.ns`'s class names are listed
    /// too: `Object` must never grow an accessor for one of them, since a
    /// program reaches them through the platform it is handed.
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
                    method.fast_path,
                    Some(crate::mixin::MethodFastPath::BuiltinClass(kind)),
                    "{kind:?}"
                );
            }
            for absent in [
                "Top",
                "PlatformFactory",
                "Platform",
                "Transcript",
                "Collections",
                "CollectionsFactory",
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

    /// §6.3.1 defaults an undeclared member to protected, and a protected
    /// nested class is unreachable from outside the module that declares it,
    /// so every kernel class has to say `public` for a program to name it --
    /// every one but `Top`, which §3.2 lets only `Object` name,
    /// `Activation` and `FutureImplementation`, which no program may reach
    /// at all, and
    /// `NonBooleanReceiver`, which a program catches as an `Error`
    /// (`docs/design/core-library.md`, "Exceptions";
    /// `docs/design/object-model.md`, "Simultaneous slots").
    #[test]
    fn only_top_activation_future_implementation_and_non_boolean_receiver_are_private() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let Some(kernel) = mutator.kernel.and_then(|value| value.as_object()) else {
                panic!("the kernel is instantiated at bootstrap");
            };
            let application = super::kernel_application(mutator, kernel);
            assert_eq!(
                application.mixin.nested.len(),
                BuiltinClass::ALL.len() - 1,
                "the kernel declares one class per builtin but the mirror's"
            );
            let top = super::kernel_class_name(BuiltinClass::Top);
            let activation = super::kernel_class_name(BuiltinClass::Activation);
            let future = super::kernel_class_name(BuiltinClass::FutureImplementation);
            let non_boolean_receiver = super::kernel_class_name(BuiltinClass::NonBooleanReceiver);
            for nested in &application.mixin.nested {
                let name = nested.name.as_str();
                let expected = if [top, activation, future, non_boolean_receiver].contains(&name) {
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
                (BuiltinClass::Array, Some(BuiltinClass::Collection)),
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
        });
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

    /// The kernel's `Value` level is an applied level like any other, and
    /// it too is enclosed by the kernel instance: a method of `Value` run on
    /// the kernel reaches `vmMirror` through it.
    #[test]
    fn the_kernels_value_level_is_enclosed_by_the_kernel() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let Some(kernel) = mutator.kernel.and_then(|value| value.as_object()) else {
                panic!("the kernel is instantiated at bootstrap");
            };
            let kernel_class = mutator.heap().object_class(kernel);
            let level = mutator
                .heap()
                .class_superclass(kernel_class)
                .expect("the kernel class has its Value level");
            assert!(mutator.heap().class_is_applied_level(level));
            assert!(mutator
                .heap()
                .class_enclosing(level)
                .is_some_and(|enclosing| enclosing.same(kernel)));
            assert!(mutator
                .heap()
                .class_declaring(level)
                .is_some_and(|declaring| declaring.same(kernel_class)));
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
        super::check_mirror_send_arguments(&resolved);
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
        super::check_mirror_send_arguments(&resolved);
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
