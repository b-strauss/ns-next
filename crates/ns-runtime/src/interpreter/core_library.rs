//! The shipped core-library modules beyond the kernel
//! (`docs/design/core-library.md`, "The module table and the boot"):
//! source compiled into the binary, resolved with `resolve_kernel` because
//! it is shipped, and booted at every start.

use std::rc::Rc;

use crate::types::{ResolvedModule, SourceOrigin};

pub(crate) const PLATFORM_SOURCE: &str = include_str!("../../core-library/Platform.ns");

/// One shipped module. Every entry here is shipped source; a user module
/// joins the table, marked as such by the host, with the step that loads
/// one.
pub(crate) struct ShippedModule {
    pub(crate) name: &'static str,
    pub(crate) source: &'static str,
}

pub(crate) const SHIPPED_MODULES: &[ShippedModule] = &[ShippedModule {
    name: "Platform",
    source: PLATFORM_SOURCE,
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
    /// The platform phase of the bootstrap, right after the kernel's:
    /// `Platform` from the table, instantiated with the VM mirror and the
    /// kernel instance -- a run like the kernel's own, entered from the
    /// outside, so the stress modes see it -- and kept as the platform every
    /// program is handed. Its members are lazy slots, so nothing past the
    /// factory runs here. Nothing in the factory assigns an identity hash
    /// either, which `image::load` relies on: it resolves platform paths
    /// before it restores the image's hashes.
    pub(crate) fn install_platform(&mut self) {
        let resolved = resolved_shipped_module("Platform");
        let platform_class = self.load_module(&resolved);
        let (mirror, kernel) = self.enter(|mutator| {
            let mirror = mutator.vm_mirror();
            let kernel = mutator
                .kernel
                .expect("internal: the kernel is wired in before the platform boots");
            (mutator.stash(mirror), mutator.stash(kernel))
        });
        let platform = match self.instantiate(&platform_class, &[mirror, kernel]) {
            Ok(platform) => platform,
            Err(crate::interpreter::HostSignal::Error(error)) => {
                panic!(
                    "internal: Platform.ns failed to instantiate: {}",
                    error.message
                )
            }
            Err(crate::interpreter::HostSignal::Return) => {
                panic!("internal: a return escaped Platform.ns's instantiation")
            }
        };
        self.enter(|mutator| {
            let platform = mutator.fetch(&platform);
            mutator.platform_singleton = Some(platform);
        });
    }
}
