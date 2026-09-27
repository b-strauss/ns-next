//! The shipped core-library modules beyond the kernel
//! (`docs/design/core-library.md`, "The module table and the boot"):
//! source compiled into the binary, resolved with `resolve_kernel` because
//! it is shipped, and booted at every start.

use std::rc::Rc;

use crate::interpreter::{HostResult, HostSignal, Mutator, Root};
use crate::types::{ResolvedModule, SourceOrigin, Value};
use crate::Symbol;

pub(crate) const PLATFORM_FACTORY_SOURCE: &str =
    include_str!("../../core-library/PlatformFactory.ns");

/// One shipped module. Every entry here is shipped source; a user module
/// joins the table, marked as such by the host, with the step that loads
/// one.
pub(crate) struct ShippedModule {
    pub(crate) name: &'static str,
    pub(crate) source: &'static str,
}

pub(crate) const SHIPPED_MODULES: &[ShippedModule] = &[ShippedModule {
    name: "PlatformFactory",
    source: PLATFORM_FACTORY_SOURCE,
}];

thread_local! {
    /// Every shipped module, parsed and resolved once per thread, in
    /// `SHIPPED_MODULES`' order, for the reason `interpreter::kernel`'s
    /// `RESOLVED_KERNEL` is: the AST and its bindings are immutable, and the
    /// test suite bootstraps well over a hundred interpreters. Each
    /// interpreter still compiles them, because compiled code is numbered
    /// against one heap's inline caches.
    static RESOLVED_SHIPPED_MODULES: Vec<Rc<ResolvedModule>> = SHIPPED_MODULES
        .iter()
        .map(|shipped| Rc::new(resolve_shipped_source(shipped)))
        .collect();
}

/// The shipped module `name`, resolved. A name the table does not list is a
/// defect in the binary, as a module that does not parse or resolve is, so
/// both panic.
pub(crate) fn resolved_shipped_module(name: &str) -> Rc<ResolvedModule> {
    let position = SHIPPED_MODULES
        .iter()
        .position(|shipped| shipped.name == name)
        .unwrap_or_else(|| panic!("internal: no shipped module is named {name}"));
    RESOLVED_SHIPPED_MODULES.with(|resolved| Rc::clone(&resolved[position]))
}

/// A shipped module that does not parse or resolve is a defect in the
/// binary, not in a program, so it panics, as the kernel's does
/// (`interpreter::kernel::resolve_kernel_source`); every test that
/// bootstraps catches it before a release could.
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
        Ok(resolved) => resolved,
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
    /// The platform phase of the bootstrap, right after the kernel's
    /// (`docs/design/core-library.md`, "The factory pattern"):
    /// `PlatformFactory` from the table, instantiated with the VM mirror and
    /// the kernel instance through the ordinary `instantiate`, then sent
    /// `Platform` and `new` as ordinary host sends -- runs like the kernel's
    /// own, entered from the outside, so the stress modes see them -- and
    /// the answer kept as the platform every program is handed. The factory
    /// instance is kept by nothing but the platform's class, whose enclosing
    /// object it is: no program is handed it, and nothing a program can name
    /// answers it. The platform's members are lazy slots, so nothing past
    /// `new` runs here. `image::load` relies on more than the boot, though:
    /// it resolves an image's platform paths -- running each member's lazy
    /// initializer, each class accessor, and each nested class's superclass
    /// clause, that a path names -- before it restores the image's identity
    /// hashes, so none of that code may assign an identity hash either. None
    /// does; a debug build asserts it at every load.
    pub(crate) fn install_platform(&mut self) {
        let resolved = resolved_shipped_module("PlatformFactory");
        let factory_class = self.load_module(&resolved);
        let (mirror, kernel) = self.enter(|mutator| {
            let mirror = mutator.vm_mirror();
            let kernel = mutator
                .kernel
                .expect("internal: the kernel is wired in before the platform boots");
            (mutator.stash(mirror), mutator.stash(kernel))
        });
        let factory = booted(
            self.instantiate(&factory_class, &[mirror, kernel]),
            "PlatformFactory usingVmMirror:kernel:",
        );
        self.enter(|mutator| {
            let factory = mutator.fetch(&factory);
            check_factory_holds_the_mirror(mutator, factory);
        });
        let platform_class = booted(
            self.send(&factory, Symbol::intern("Platform"), &[]),
            "PlatformFactory Platform",
        );
        let platform = booted(
            self.send(&platform_class, Symbol::NEW, &[]),
            "PlatformFactory Platform new",
        );
        self.enter(|mutator| {
            let platform = mutator.fetch(&platform);
            mutator.platform_singleton = Some(platform);
        });
    }
}

/// What a boot step answered. A shipped module that fails to boot is a
/// defect in the binary, not in a program, so it panics, naming the step.
fn booted(result: HostResult, step: &str) -> Root {
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
/// VM mirror is checked here for the platform's module, as
/// `interpreter::kernel` checks it for the kernel. Otherwise a mirror send's
/// fast path and its `SendMirror` fallback could disagree about the
/// receiver. The one instance checked is the only one there is: no program
/// reaches the root's class to make another.
fn check_factory_holds_the_mirror<'gc>(mutator: &Mutator<'gc>, factory: Value<'gc>) {
    let Value::Object(factory) = factory else {
        panic!("internal: PlatformFactory's instance is an object");
    };
    let heap = mutator.heap();
    let application = heap
        .class_application(heap.class_of_object(factory))
        .expect("internal: the factory's class is a class object");
    let position = application
        .mixin
        .slots
        .iter()
        .position(|slot| slot.name.as_str() == "vmMirror" && !slot.mutable && !slot.lazy)
        .expect(
            "internal: PlatformFactory.ns's root class must declare the immutable, eager slot \
             vmMirror",
        );
    let holds_the_mirror = heap
        .instance_field(factory, application.own_slot_base + position)
        .is_some_and(|value| mutator.is_vm_mirror(value));
    assert!(
        holds_the_mirror,
        "internal: PlatformFactory's vmMirror slot must hold the VM mirror"
    );
}
