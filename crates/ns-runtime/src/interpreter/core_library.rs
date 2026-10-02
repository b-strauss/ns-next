//! The shipped core-library modules beyond the kernel
//! (`docs/design/core-library.md`, "The module table and the boot"):
//! source compiled into the binary, resolved with `resolve_kernel` because
//! it is shipped, each on its first request: `PlatformFactory` at every
//! boot, any other when a program first asks for it.

use std::cell::OnceCell;
use std::rc::Rc;

use crate::interpreter::{HostResult, HostSignal, Mutator, Root};
use crate::types::{ResolvedModule, SourceOrigin, Value};
use crate::Symbol;

pub(crate) const PLATFORM_FACTORY_SOURCE: &str =
    include_str!("../../core-library/PlatformFactory.ns");

pub(crate) const COLLECTIONS_FACTORY_SOURCE: &str =
    include_str!("../../core-library/CollectionsFactory.ns");

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
}

pub(crate) const SHIPPED_MODULES: &[ShippedModule] = &[
    ShippedModule {
        name: "PlatformFactory",
        source: PLATFORM_FACTORY_SOURCE,
    },
    ShippedModule {
        name: "CollectionsFactory",
        source: COLLECTIONS_FACTORY_SOURCE,
    },
];

thread_local! {
    /// One cell per `SHIPPED_MODULES` entry, in its order, each filled with
    /// that module parsed and resolved the first time anything on this thread
    /// asks for it. Per thread, for the reason `interpreter::kernel`'s
    /// `RESOLVED_KERNEL` is: the AST and its bindings are immutable, and the
    /// test suite bootstraps well over a hundred interpreters. On first
    /// request rather than all at once, because a program pays for a module
    /// only when it asks for one: the boot needs `PlatformFactory` alone, and
    /// `CollectionsFactory` is resolved the first time a program reaches
    /// `platform collections` (`docs/design/execution.md`, "What the
    /// collections module cost"). Resolving is pure -- a parse and
    /// `resolve_kernel`, no heap and no handle -- and never asks for another
    /// module, so filling a cell cannot re-enter it. Each interpreter still
    /// compiles what it asks for, because compiled code is numbered against
    /// one heap's inline caches.
    static RESOLVED_SHIPPED_MODULES: Vec<OnceCell<Rc<ResolvedModule>>> =
        SHIPPED_MODULES.iter().map(|_| OnceCell::new()).collect();
}

/// The shipped module `name`, resolved on this thread's first request for
/// it, or `None` when the table lists no such module: what the mirror's
/// `moduleNamed:` answers from, since the name there is a program value,
/// even if only shipped code can send it.
pub(crate) fn shipped_module_named(name: &str) -> Option<Rc<ResolvedModule>> {
    let position = SHIPPED_MODULES
        .iter()
        .position(|shipped| shipped.name == name)?;
    Some(RESOLVED_SHIPPED_MODULES.with(|resolved| {
        Rc::clone(
            resolved[position]
                .get_or_init(|| Rc::new(resolve_shipped_source(&SHIPPED_MODULES[position]))),
        )
    }))
}

/// The shipped module `name`, resolved, for the boot, which names it in
/// the binary. A name the table does not list is a defect in the binary, as
/// a module that does not parse or resolve is, so both panic.
pub(crate) fn resolved_shipped_module(name: &str) -> Rc<ResolvedModule> {
    shipped_module_named(name)
        .unwrap_or_else(|| panic!("internal: no shipped module is named {name}"))
}

/// A shipped module that does not parse or resolve is a defect in the
/// binary, not in a program, so it panics, as the kernel's does
/// (`interpreter::kernel::resolve_kernel_source`). Since a module is
/// resolved only on request, a boot no longer resolves every entry;
/// `every_shipped_module_resolves` does, so the suite still catches one
/// before a release could.
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
    /// `newPlatform`, its one public member, as an ordinary host send --
    /// runs like the kernel's own, entered from the outside, so the stress
    /// modes see them -- and the answer kept as the platform every program
    /// is handed. The factory's nested classes are private, so this is the
    /// one send it answers. The factory instance is kept by nothing but the
    /// platform's class, whose enclosing object it is: no program is handed
    /// it, and nothing a program can name answers it. The platform's
    /// members are lazy slots, so nothing past `Platform new` runs here:
    /// the first `platform collections` builds the collections module,
    /// `CollectionsFactory` from the same table, through the mirror's
    /// `moduleNamed:` and the factory's lazy `collectionsModule`, and
    /// `image::load` may do so while it resolves a path.
    /// `image::load` relies on more than the boot, though:
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
        let platform = booted(
            self.send(&factory, Symbol::intern("newPlatform"), &[]),
            "PlatformFactory newPlatform",
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
    let Some(factory) = factory.as_object() else {
        panic!("internal: PlatformFactory's instance is an object");
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

#[cfg(test)]
mod tests {
    use super::*;

    fn module_named<'gc>(
        mutator: &mut Mutator<'gc>,
        subject: Value<'gc>,
    ) -> Option<Result<Value<'gc>, crate::types::NewspeakError<'gc>>> {
        crate::builtins::module::execute(
            mutator,
            crate::builtins::primitive::ModulePrimitive::Named,
            subject,
        )
    }

    /// A boot resolves only the modules it asks for, so this is what
    /// catches a shipped module that does not parse or resolve: it resolves
    /// every table entry, panicking on the first that fails.
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

    /// The leaf answers a shipped module's root class, and declines a name
    /// the table does not list and a subject that is no symbol, so the
    /// forwarder's failure block runs.
    #[test]
    fn module_named_answers_a_shipped_root_class_and_declines_anything_else() {
        let mut interpreter = crate::Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let name = mutator.new_symbol(Symbol::intern("CollectionsFactory"));
            let Some(Ok(answer)) = module_named(mutator, name) else {
                panic!("moduleNamed: #CollectionsFactory answers no class");
            };
            let Some(class) = answer.as_object() else {
                panic!("moduleNamed: #CollectionsFactory answers no class");
            };
            assert!(mutator.heap().is_class(class));
            assert!(mutator.heap().class_is_shipped(class));
            assert_eq!(
                mutator.heap().class_display_name(class),
                "CollectionsFactory"
            );
            let unknown = mutator.new_symbol(Symbol::intern("NoSuchModule"));
            assert!(module_named(mutator, unknown).is_none());
            let string = Value::object(mutator.new_string("CollectionsFactory".to_string()));
            assert!(module_named(mutator, string).is_none());
        });
    }
}
