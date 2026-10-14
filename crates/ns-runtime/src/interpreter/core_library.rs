//! The shipped core-library modules beyond the kernel
//! (`docs/design/core-library.md`, "The module table and the boot"):
//! source compiled into the binary, resolved with `resolve_kernel` because
//! it is shipped, and every one of them compiled and instantiated once at
//! every boot.

use std::cell::OnceCell;
use std::rc::Rc;

use crate::interpreter::{HostResult, HostSignal, Mutator, Root};
use crate::mixin::Mixin;
use crate::types::{ResolvedModule, SourceOrigin, Value};
use crate::Symbol;

pub(crate) const PLATFORM_FACTORY_SOURCE: &str =
    include_str!("../../core-library/PlatformFactory.ns");

pub(crate) const TRANSCRIPT_FACTORY_SOURCE: &str =
    include_str!("../../core-library/platform/TranscriptFactory.ns");

pub(crate) const TIME_FACTORY_SOURCE: &str =
    include_str!("../../core-library/platform/TimeFactory.ns");

pub(crate) const COLLECTIONS_FACTORY_SOURCE: &str =
    include_str!("../../core-library/platform/CollectionsFactory.ns");

pub(crate) const TYPED_ARRAYS_FACTORY_SOURCE: &str =
    include_str!("../../core-library/platform/TypedArraysFactory.ns");

/// Not one of the shipped modules above, and never in their table: the
/// manifest a project's root receives is compiled into the program like a
/// user module, holds no mirror, and may be carried by an image.
pub const MANIFEST_SOURCE: &str = include_str!("../../core-library/Manifest.ns");

/// One shipped module. Every entry here is shipped source; a user module
/// joins the table, marked as such by the host, with the step that loads
/// one.
pub(crate) struct ShippedModule {
    pub(crate) name: &'static str,
    pub(crate) source: &'static str,
    pub(crate) factory: ShippedFactory,
}

/// What a shipped module's factory takes: `PlatformFactory` and
/// `TypedArraysFactory` the mirror and the kernel
/// (`usingVmMirror:kernel:`), `TranscriptFactory` and
/// `TimeFactory` the mirror alone (`usingVmMirror:`), since they need none
/// of the kernel's classes, and `CollectionsFactory` the kernel alone
/// (`usingKernel:`), since it reaches no leaf and so is given no mirror.
pub(crate) enum ShippedFactory {
    MirrorAndKernel,
    Mirror,
    Kernel,
}

pub(crate) const SHIPPED_MODULES: &[ShippedModule] = &[
    ShippedModule {
        name: "PlatformFactory",
        source: PLATFORM_FACTORY_SOURCE,
        factory: ShippedFactory::MirrorAndKernel,
    },
    ShippedModule {
        name: "TranscriptFactory",
        source: TRANSCRIPT_FACTORY_SOURCE,
        factory: ShippedFactory::Mirror,
    },
    ShippedModule {
        name: "TimeFactory",
        source: TIME_FACTORY_SOURCE,
        factory: ShippedFactory::Mirror,
    },
    ShippedModule {
        name: "CollectionsFactory",
        source: COLLECTIONS_FACTORY_SOURCE,
        factory: ShippedFactory::Kernel,
    },
    ShippedModule {
        name: "TypedArraysFactory",
        source: TYPED_ARRAYS_FACTORY_SOURCE,
        factory: ShippedFactory::MirrorAndKernel,
    },
];

thread_local! {
    /// One cell per `SHIPPED_MODULES` entry, in its order, each filled with
    /// that module parsed and resolved the first time a boot on this thread
    /// asks for it. Per thread, for the reason `interpreter::kernel`'s
    /// `RESOLVED_KERNEL` is: the AST and its bindings are immutable, and the
    /// test suite bootstraps well over a hundred interpreters. Every boot
    /// asks for every entry, because every shipped module is instantiated
    /// at every boot: an image names the shipped world's objects, and the
    /// loader runs nothing the image chooses, so they must all exist before
    /// it decodes (`docs/design/image-format.md`, "Shipped names"). Resolving
    /// is pure -- a parse and `resolve_kernel`, no
    /// heap and no handle -- and never asks for another module, so filling
    /// a cell cannot re-enter it. Each interpreter still compiles every
    /// module: compiling needs no heap, but the boot builds its world from
    /// its own mixins.
    static RESOLVED_SHIPPED_MODULES: Vec<OnceCell<Rc<ResolvedModule>>> =
        SHIPPED_MODULES.iter().map(|_| OnceCell::new()).collect();
}

/// The shipped module `name`, resolved, for the boot, which names it in
/// the binary. A name the table does not list is a defect in the binary, as
/// a module that does not parse or resolve is, so both panic.
pub(crate) fn resolved_shipped_module(name: &str) -> Rc<ResolvedModule> {
    let position = SHIPPED_MODULES
        .iter()
        .position(|shipped| shipped.name == name)
        .unwrap_or_else(|| panic!("internal: no shipped module is named {name}"));
    RESOLVED_SHIPPED_MODULES.with(|resolved| {
        Rc::clone(
            resolved[position]
                .get_or_init(|| Rc::new(resolve_shipped_source(&SHIPPED_MODULES[position]))),
        )
    })
}

/// A shipped module that does not parse or resolve is a defect in the
/// binary, not in a program, so it panics, as the kernel's does
/// (`interpreter::kernel::resolve_kernel_source`); so does one whose mirror
/// send passes a literal block before its failure block, as the kernel's
/// may not either (`interpreter::kernel::check_mirror_send_arguments`).
fn resolve_shipped_source(shipped: &ShippedModule) -> ResolvedModule {
    let name = shipped.name;
    let module = match ns_syntax::parse(shipped.source) {
        Ok(module) => module,
        Err(diagnostics) => panic!(
            "internal: {name}.ns does not parse: {}",
            diagnostics
                .iter()
                .map(|diagnostic| diagnostic.message.clone())
                .collect::<Vec<_>>()
                .join("; ")
        ),
    };
    match crate::resolver::resolve_kernel(
        module,
        Rc::from(shipped.source),
        SourceOrigin::File(std::path::PathBuf::from(format!("{name}.ns"))),
    ) {
        Ok(resolved) => {
            super::kernel::check_mirror_send_arguments(&resolved, name);
            resolved
        }
        Err(errors) => panic!(
            "internal: {name}.ns does not resolve: {}",
            errors
                .iter()
                .map(|error| error.message.clone())
                .collect::<Vec<_>>()
                .join("; ")
        ),
    }
}

impl crate::interpreter::Interpreter {
    /// The shipped-module phase of the bootstrap, right after the kernel's
    /// (`docs/design/core-library.md`, "The factory pattern"). It records the
    /// kernel's instance as the first shipped module's, then compiles every
    /// `SHIPPED_MODULES` entry in table order and instantiates its root through
    /// the ordinary `instantiate` -- `PlatformFactory` and `TypedArraysFactory`
    /// with the VM mirror and the kernel instance, `TranscriptFactory` and
    /// `TimeFactory` with the mirror alone, `CollectionsFactory` with the
    /// kernel instance alone -- and records each instance as that module's one
    /// instance (`interpreter::shipped_world`). Every shipped module is built
    /// at every boot, because an image names the shipped world's objects and
    /// the loader runs nothing the image chooses
    /// (`docs/design/image-format.md`, "Shipped names"); the mirror's
    /// `moduleInstanceNamed:` answers these instances and builds nothing. Then
    /// `complete_shipped_world` builds every class the roots declare and
    /// records every shipped object under its name.
    ///
    /// Last, `PlatformFactory`'s instance is sent `build`, its one
    /// public member, as an ordinary host send -- runs like the kernel's
    /// own, entered from the outside, so the stress modes see them -- and
    /// the answer kept as the platform every program is handed. The
    /// factory's nested classes are private, so this is the one send it
    /// answers. No program is handed a factory instance, and nothing a
    /// program can name answers one. The platform's members are lazy
    /// slots, so nothing past `Platform new` runs here.
    pub(crate) fn install_shipped_modules(&mut self) {
        let (mirror, kernel) = self.enter(|mutator| {
            let mirror = mutator.vm_mirror();
            let kernel = mutator
                .kernel
                .expect("internal: the kernel is wired in before the shipped modules boot");
            let kernel_object = kernel
                .as_object()
                .expect("internal: the kernel's instance is an object");
            mutator
                .shipped_world
                .add_module_instance(Symbol::intern("Kernel"), kernel_object);
            (mutator.stash(mirror), mutator.stash(kernel))
        });
        for shipped in SHIPPED_MODULES {
            let name = shipped.name;
            let factory_class = self.enter(|mutator| {
                let mixin = mutator.shipped_root_mixin_from_source(name);
                let class = mutator.root_class_of_mixin(&mixin);
                mutator.stash(Value::object(class))
            });
            let instance = match shipped.factory {
                ShippedFactory::MirrorAndKernel => booted(
                    self.instantiate(&factory_class, &[mirror.clone(), kernel.clone()]),
                    &format!("{name} usingVmMirror:kernel:"),
                ),
                ShippedFactory::Mirror => booted(
                    self.instantiate(&factory_class, std::slice::from_ref(&mirror)),
                    &format!("{name} usingVmMirror:"),
                ),
                ShippedFactory::Kernel => booted(
                    self.instantiate(&factory_class, std::slice::from_ref(&kernel)),
                    &format!("{name} usingKernel:"),
                ),
            };
            self.enter(|mutator| {
                let instance = mutator.fetch(&instance);
                if let ShippedFactory::MirrorAndKernel | ShippedFactory::Mirror = shipped.factory {
                    check_factory_holds_the_mirror(mutator, name, instance);
                }
                let instance = instance
                    .as_object()
                    .unwrap_or_else(|| panic!("internal: {name}'s instance is an object"));
                mutator
                    .shipped_world
                    .add_module_instance(Symbol::intern(name), instance);
            });
        }
        self.complete_shipped_world();
        let platform_factory = self.enter(|mutator| {
            let factory = mutator
                .shipped_world
                .module_instance(Symbol::intern("PlatformFactory"))
                .expect("internal: PlatformFactory is a shipped module");
            mutator.stash(Value::object(factory))
        });
        let platform = booted(
            self.send(&platform_factory, Symbol::intern("build"), &[]),
            "PlatformFactory build",
        );
        self.enter(|mutator| {
            let platform = mutator.fetch(&platform);
            mutator.platform_singleton = Some(platform);
        });
    }
}

impl<'gc> Mutator<'gc> {
    /// The root mixin of `Kernel` or of the `SHIPPED_MODULES` entry `name`,
    /// compiled from its source, which is parsed, resolved and checked
    /// (the mirror sends, and for the kernel its shape) on the way. A name
    /// that is neither is a defect in the binary, so it panics.
    pub(crate) fn shipped_root_mixin_from_source(&mut self, name: &str) -> Rc<Mixin> {
        if name == "Kernel" {
            let resolved = super::kernel::resolved_kernel();
            let root_mixin = self.build_root_mixin(
                &resolved.module.root_class,
                &resolved.bindings,
                super::instantiate::ModuleSource::Kernel,
            );
            super::kernel::check_kernel_shape(&root_mixin, &resolved);
            root_mixin
        } else {
            let resolved = resolved_shipped_module(name);
            self.compile_root_mixin(&resolved)
        }
    }
}

impl crate::interpreter::Interpreter {
    /// The root mixin of every shipped module after a boot, `Kernel` first
    /// and then the `SHIPPED_MODULES` in order: the one each module
    /// instance's class applies.
    // Read by the tests now, and by the precompiling step that follows.
    #[cfg_attr(not(test), expect(dead_code))]
    pub(crate) fn shipped_root_mixins(&mut self) -> Vec<(&'static str, Rc<Mixin>)> {
        self.enter(|mutator| {
            let names =
                std::iter::once("Kernel").chain(SHIPPED_MODULES.iter().map(|shipped| shipped.name));
            names
                .map(|name| {
                    let instance = mutator
                        .shipped_world
                        .module_instance(Symbol::intern(name))
                        .unwrap_or_else(|| panic!("internal: {name} has no module instance"));
                    let class = mutator.class_of(Value::object(instance));
                    let application = mutator
                        .heap()
                        .class_application(class)
                        .unwrap_or_else(|| panic!("internal: {name}'s class is a class object"));
                    (name, Rc::clone(&application.mixin))
                })
                .collect()
        })
    }
}

/// What a boot step answered. A shipped module that fails to boot is a
/// defect in the binary, not in a program, so it panics, naming the step.
pub(super) fn booted(result: HostResult, step: &str) -> Root {
    match result {
        Ok(answer) => answer,
        Err(HostSignal::Error(error)) => {
            panic!("internal: {step} failed at bootstrap: {}", error.message)
        }
        Err(HostSignal::Return) => {
            panic!("internal: a return escaped {step} at bootstrap")
        }
    }
}

/// The resolver marks every send to the root class's `vmMirror` in shipped
/// source as a mirror send (`resolver::is_the_mirror_slot`) without knowing
/// what that member holds; that it is an eager, immutable slot holding the
/// VM mirror is checked here for every shipped module given the mirror, as
/// `interpreter::kernel` checks it for the kernel. Otherwise a mirror send's
/// fast path and its `SendMirror` fallback could disagree about the
/// receiver. The one instance checked is the only one there is: no program
/// reaches the root's class to make another.
fn check_factory_holds_the_mirror<'gc>(mutator: &Mutator<'gc>, name: &str, factory: Value<'gc>) {
    let Some(factory) = factory.as_object() else {
        panic!("internal: {name}'s instance is an object");
    };
    let heap = mutator.heap();
    let application = heap
        .class_application(heap.object_class(factory))
        .expect("internal: the factory's class is a class object");
    let position = application
        .mixin
        .slots
        .iter()
        .position(|slot| slot.name.as_str() == "vmMirror" && !slot.mutable && !slot.lazy)
        .unwrap_or_else(|| {
            panic!(
                "internal: {name}.ns's root class must declare the immutable, eager slot vmMirror"
            )
        });
    let holds_the_mirror = heap
        .instance_field(factory, application.own_slot_base + position)
        .is_some_and(|value| mutator.is_vm_mirror(value));
    assert!(
        holds_the_mirror,
        "internal: {name}'s vmMirror slot must hold the VM mirror"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module_instance_named<'gc>(
        mutator: &mut Mutator<'gc>,
        subject: Value<'gc>,
    ) -> Option<Result<Value<'gc>, crate::types::NewspeakError<'gc>>> {
        crate::builtins::module::execute(
            mutator,
            crate::builtins::primitive::ModulePrimitive::InstanceNamed,
            subject,
        )
    }

    #[test]
    fn the_boot_keeps_every_shipped_root_mixin_in_table_order() {
        let mut interpreter = crate::Interpreter::bootstrap();
        let names: Vec<&str> = interpreter
            .shipped_root_mixins()
            .iter()
            .map(|(name, mixin)| {
                assert_eq!(mixin.name.as_str(), *name);
                *name
            })
            .collect();
        let mut expected = vec!["Kernel"];
        expected.extend(SHIPPED_MODULES.iter().map(|shipped| shipped.name));
        assert_eq!(names, expected);
    }

    /// Every table entry parses and resolves to something, panicking on the
    /// first that fails, independently of the boot that also resolves them.
    #[test]
    fn every_shipped_module_resolves() {
        for shipped in SHIPPED_MODULES {
            let resolved = resolve_shipped_source(shipped);
            assert!(
                !resolved.bindings.is_empty(),
                "{}.ns resolved to no bindings",
                shipped.name
            );
        }
    }

    /// The leaf answers the canonical instance the boot built, the same object
    /// on every request, and declines a name the table does not list and a
    /// subject that is no symbol, so the forwarder's failure block runs.
    #[test]
    fn module_instance_named_answers_the_boots_instance_and_declines_anything_else() {
        let mut interpreter = crate::Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let name = mutator.new_symbol(Symbol::intern("CollectionsFactory"));
            let Some(Ok(first)) = module_instance_named(mutator, name) else {
                panic!("moduleInstanceNamed: #CollectionsFactory answers no instance");
            };
            let Some(Ok(second)) = module_instance_named(mutator, name) else {
                panic!("moduleInstanceNamed: #CollectionsFactory answers no instance");
            };
            let (Some(first), Some(second)) = (first.as_object(), second.as_object()) else {
                panic!("moduleInstanceNamed: answers an object");
            };
            assert!(first.same(second));
            assert!(!mutator.heap().is_class(first));
            assert_eq!(
                mutator
                    .heap()
                    .class_display_name(mutator.heap().object_class(first)),
                "CollectionsFactory"
            );
            let unknown = mutator.new_symbol(Symbol::intern("NoSuchModule"));
            assert!(module_instance_named(mutator, unknown).is_none());
            let string = Value::object(mutator.new_string("CollectionsFactory".to_string()));
            assert!(module_instance_named(mutator, string).is_none());
        });
    }
}
