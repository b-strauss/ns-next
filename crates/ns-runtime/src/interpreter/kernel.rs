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

/// The name of `kind`'s class in `Kernel.ns`. Not
/// `instantiate::builtin_kind_from_name`, which is the list of names a
/// *program* may write bare and which, for that reason, has no `Platform`.
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
        BuiltinClass::Character => "Character",
        BuiltinClass::Array => "Array",
        BuiltinClass::OrderedCollection => "OrderedCollection",
        BuiltinClass::Nil => "Nil",
        BuiltinClass::Block => "Block",
        BuiltinClass::Platform => "Platform",
        BuiltinClass::Top => "Top",
        BuiltinClass::Value => "Value",
        BuiltinClass::Number => "Number",
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
                mutator.stash(Value::Object(kernel_class)),
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

impl<'gc> Mutator<'gc> {
    /// Every placeholder gets the application of its kernel class, over its
    /// superclass's -- `fill_order` puts every superclass ahead of its
    /// subclasses, so that one is filled already -- and the mirror's class,
    /// which `Kernel.ns` does not declare, keeps its mixin over the filled
    /// `Object`. Then the kernel's own class is built over the filled
    /// `Object`, and answered for the host to instantiate.
    ///
    /// Nothing has run yet, so no inline cache, metaclass or instance built
    /// before this phase can still hold a placeholder application:
    /// `wire_kernel_in` checks the metaclasses.
    fn fill_builtin_classes(&mut self, resolved: &ResolvedModule) -> ClassReference<'gc> {
        let declaration = &resolved.module.root_class;
        let root_mixin = self.build_mixin(declaration, &resolved.bindings);
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
            let application = self.apply_mixin(&mixin, super_application.as_ref());
            if !self.heap.set_class_application(class, application) {
                unreachable!("internal: a builtin class is a class object");
            }
            let linked = match superclass {
                Some(superclass) => {
                    let superclass_object = self.builtin_class(superclass);
                    self.heap.set_class_superclass(class, superclass_object)
                }
                None => self.heap.clear_class_superclass(class),
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
        let object = self.builtin_class(BuiltinClass::Object);
        let object_application = self.heap.class_application(object);
        let application = self.apply_mixin(&root_mixin, object_application.as_ref());
        self.new_class_object(application, None, None, Some(object))
    }

    /// The kernel instance becomes every kernel class's enclosing object,
    /// and each of its hidden nested-class slots holds the builtin class it
    /// names -- so the nested-class accessor in kernel code answers the
    /// builtin class itself rather than building a second class object of
    /// the same mixin.
    fn wire_kernel_in(&mut self, kernel_class: &Root, kernel: &Root) {
        let Value::Object(kernel_class) = self.fetch(kernel_class) else {
            unreachable!("internal: the kernel class is a class object");
        };
        let Value::Object(kernel) = self.fetch(kernel) else {
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
                matches!(self.heap.instance_field(kernel, field), Some(Value::Nil)),
                "internal: Kernel's initializer touched the nested class {kind:?} before the bootstrap could fill it"
            );
            if !self
                .heap
                .set_instance_field(kernel, field, Value::Object(class))
            {
                unreachable!("internal: the kernel instance has a field per nested class");
            }
            if !self
                .heap
                .set_class_lexical_context(class, Some(kernel), Some(kernel_class))
            {
                unreachable!("internal: a builtin class is a class object");
            }
            // A metaclass copies its class's lexical context when it is
            // built, so one built before this point would keep the
            // placeholder's.
            debug_assert!(
                self.heap.class_metaclass(class).is_none(),
                "internal: a metaclass built before the kernel was wired in borrowed the placeholder's context"
            );
        }
        self.kernel = Some(Value::Object(kernel));
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
/// slotted in beside its siblings -- `Value` follows `Platform` there while
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
/// and sits under `Object` (`interpreter::mod`'s `builtin_mixin`).
fn superclass_of_kind(root: &Rc<Mixin>, kind: BuiltinClass) -> Option<BuiltinClass> {
    if kind == BuiltinClass::VmMirror {
        return Some(BuiltinClass::Object);
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
fn kernel_kind_named(name: &str) -> Option<BuiltinClass> {
    BuiltinClass::ALL
        .into_iter()
        .find(|kind| *kind != BuiltinClass::VmMirror && kernel_class_name(*kind) == name)
}

/// The shape the phase relies on and the resolver cannot check: one nested
/// class per builtin but the mirror's, named as `kernel_class_name` says,
/// each declaring no slots and the default factory `new`; a root class with
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
        assert!(
            nested.slots.is_empty(),
            "internal: Kernel.ns's {name} declares a slot; a builtin's instances have none"
        );
        let declaration = nested
            .declaration
            .as_ref()
            .unwrap_or_else(|| panic!("internal: Kernel.ns's {name} was built from no source"));
        assert!(
            declaration.factory.is_none(),
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
/// so no other class may name it -- and, since `Top` is the one private
/// class here, no module outside the kernel can either.
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
    heap.class_application(heap.class_of_object(kernel))
        .expect("internal: the kernel's class is a class object")
}

/// A mirror send in the kernel passes a literal block only as its final,
/// `ifFail:` argument. A literal block anywhere before it would be a block
/// the leaf itself activates (`boolean:ifTrue:ifFail:`'s, say), and the
/// reference lowering, which marks the whole send `SendInlinable` because
/// its failure block is inlined by the optimized lowering, would exempt
/// that block's activation from the depth count too -- while the optimized
/// lowering counts it. The two lowerings would then count activations
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
            .any(|argument| matches!(argument, ast::Expression::Block(_)))
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
            &send.receiver,
            ast::Expression::Identifier(identifier) if identifier.name == "vmMirror"
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
        match node {
            ast::Expression::Identifier(_) | ast::Expression::Literal(_) => {}
            ast::Expression::Send(send) => {
                visit(send);
                expression(&send.receiver, visit);
                for argument in &send.message.arguments {
                    expression(argument, visit);
                }
            }
            ast::Expression::Cascade(cascade) => {
                expression(&cascade.receiver, visit);
                for message in std::iter::once(&cascade.first).chain(&cascade.rest) {
                    for argument in &message.arguments {
                        expression(argument, visit);
                    }
                }
            }
            ast::Expression::Assign { value, .. } => expression(value, visit),
            ast::Expression::Block(block) => body(&block.body, visit),
            ast::Expression::DynamicArray { elements, .. } => {
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
    use crate::{BuiltinClass, Interpreter, Value};

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
        (BuiltinClass::Boolean, "hash"),
        (BuiltinClass::Integer, "hash"),
        (BuiltinClass::Character, "hash"),
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
    /// the selector the program sent if the two differed.
    #[test]
    fn every_kernel_method_is_a_primitive_method_named_after_its_row() {
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
                let mixin = &application.mixin;
                let class_side = mixin
                    .class_side
                    .iter()
                    .flat_map(|side| side.compiled_methods());
                for method in mixin.compiled_methods().chain(class_side) {
                    let selector = method.selector.as_str();
                    if NOT_FORWARDERS.contains(&(kind, selector)) {
                        assert!(
                            method.primitive.is_none(),
                            "{kind:?}>>{selector} reaches a leaf after all; take it off NOT_FORWARDERS"
                        );
                        continue;
                    }
                    let Some(row) = method.primitive else {
                        panic!("{kind:?}>>{selector} is not recognized as a primitive method");
                    };
                    assert_eq!(
                        mirror_entry(row).reported,
                        selector,
                        "{kind:?}>>{selector} forwards to a row reported under another selector"
                    );
                }
            }
        });
    }

    /// §6.3.1 defaults an undeclared member to protected, and a protected
    /// nested class is unreachable from outside the module that declares it,
    /// so every kernel class has to say `public` for a program to name it --
    /// every one but `Top`, which §3.2 lets only `Object` name.
    #[test]
    fn only_top_is_private() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let Some(Value::Object(kernel)) = mutator.kernel else {
                panic!("the kernel is instantiated at bootstrap");
            };
            let application = super::kernel_application(mutator, kernel);
            assert_eq!(
                application.mixin.nested.len(),
                BuiltinClass::ALL.len() - 1,
                "the kernel declares one class per builtin but the mirror's"
            );
            for nested in &application.mixin.nested {
                let top = super::kernel_class_name(BuiltinClass::Top);
                let expected = if nested.name.as_str() == top {
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
                (BuiltinClass::Float, Some(BuiltinClass::Number)),
                (BuiltinClass::String, Some(BuiltinClass::Value)),
                (BuiltinClass::Symbol, Some(BuiltinClass::Value)),
                (BuiltinClass::Character, Some(BuiltinClass::Value)),
                (BuiltinClass::Nil, Some(BuiltinClass::Value)),
                (BuiltinClass::Array, Some(BuiltinClass::Object)),
                (BuiltinClass::OrderedCollection, Some(BuiltinClass::Object)),
                (BuiltinClass::Block, Some(BuiltinClass::Object)),
                (BuiltinClass::Platform, Some(BuiltinClass::Object)),
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

    /// The nested-class accessor inside kernel code answers the builtin
    /// class itself, not a second class object of the same mixin.
    #[test]
    fn the_kernels_hidden_class_slots_hold_the_builtin_classes() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let Some(Value::Object(kernel)) = mutator.kernel else {
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
                    matches!(
                        mutator.heap().instance_field(kernel, field),
                        Some(Value::Object(held)) if held.same(class)
                    ),
                    "{kind:?}'s hidden slot holds the builtin class"
                );
            }
        });
    }

    #[test]
    fn the_kernel_holds_the_mirror() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let Some(Value::Object(kernel)) = mutator.kernel else {
                panic!("the kernel is instantiated at bootstrap");
            };
            let held = mutator
                .heap()
                .instance_field(kernel, super::mirror_slot_of(mutator, kernel));
            assert!(held.is_some_and(|value| mutator.is_vm_mirror(value)));
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
            Newspeak3
            'T'
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
            Newspeak3
            'T'
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
            Newspeak3
            'Test'
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
