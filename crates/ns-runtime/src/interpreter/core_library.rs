//! The shipped core-library modules beyond the kernel
//! (`docs/design/core-library.md`, "The module table and the boot"):
//! source compiled into the binary, resolved with `resolve_kernel` because
//! it is shipped. `PlatformFactory` is instantiated at every boot, each
//! other module once, on a program's first demand or when an image names
//! it (`LAZY_SHIPPED_MODULES`).

use std::cell::OnceCell;
use std::rc::Rc;

use crate::interpreter::{HostResult, HostSignal, Mutator, Root};
use crate::mixin::Mixin;
use crate::shipped_code::ShippedCodePath;
use crate::types::{ClassReference, ObjectReference, ResolvedModule, SourceOrigin, Value};
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

pub(crate) const MIRRORS_FACTORY_SOURCE: &str =
    include_str!("../../core-library/platform/MirrorsFactory.ns");

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

/// What a shipped module's factory takes: `PlatformFactory`,
/// `TypedArraysFactory` and `MirrorsFactory` the mirror and the kernel
/// (`usingVmMirror:kernel:`), `TranscriptFactory` and
/// `TimeFactory` the mirror alone (`usingVmMirror:`), since they need none
/// of the kernel's classes, and `CollectionsFactory` the kernel alone
/// (`usingKernel:`), since it reaches no leaf and so is given no mirror.
pub(crate) enum ShippedFactory {
    MirrorAndKernel,
    Mirror,
    Kernel,
}

/// The kernel module's name. The kernel is a shipped module too, the first,
/// but booted before the others and not in `SHIPPED_MODULES`.
pub(crate) const KERNEL_MODULE: &str = "Kernel";

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
    ShippedModule {
        name: "MirrorsFactory",
        source: MIRRORS_FACTORY_SOURCE,
        factory: ShippedFactory::MirrorAndKernel,
    },
];

/// The shipped modules a program's first demand builds, through
/// `PlatformFactory.ns`'s lazy slots and the mirror's
/// `shippedModuleClassNamed:` and `recordShippedModule:`: every
/// `SHIPPED_MODULES` entry but `PlatformFactory`, whose `build` makes the
/// platform at boot.
pub(crate) const LAZY_SHIPPED_MODULES: &[&str] = &[
    "TranscriptFactory",
    "TimeFactory",
    "CollectionsFactory",
    "TypedArraysFactory",
    "MirrorsFactory",
];

/// The `SHIPPED_MODULES` entry `name`. A name the table does not list is a
/// defect in the binary, which names every module it builds.
fn shipped_module(name: &str) -> &'static ShippedModule {
    SHIPPED_MODULES
        .iter()
        .find(|shipped| shipped.name == name)
        .unwrap_or_else(|| panic!("internal: no shipped module is named {name}"))
}

thread_local! {
    /// One cell per `SHIPPED_MODULES` entry, in its order, each filled with
    /// that module parsed and resolved the first time a boot on this thread
    /// asks for it. Per thread, for the reason `interpreter::kernel`'s
    /// `RESOLVED_KERNEL` is: the AST and its bindings are immutable, and the
    /// test suite bootstraps well over a hundred interpreters. A cell is
    /// filled when the source path first compiles its module, at boot for
    /// `PlatformFactory` and on first demand for the others. Resolving
    /// is pure -- a parse and `resolve_kernel`, no
    /// heap and no handle -- and never asks for another module, so filling
    /// a cell cannot re-enter it. Each interpreter still compiles every
    /// module it builds: compiling needs no heap, but each world is built
    /// from its own mixins.
    static RESOLVED_SHIPPED_MODULES: Vec<OnceCell<Rc<ResolvedModule>>> =
        SHIPPED_MODULES.iter().map(|_| OnceCell::new()).collect();
}

thread_local! {
    /// The text of `Kernel`, then of each `SHIPPED_MODULES` entry, in that
    /// order, each made the first time a decode of the embedded shipped
    /// code on this thread asks for it: every mixin a decode builds holds
    /// its module's text (`Mixin::module_text`), and one copy per module
    /// per thread serves every boot, where one per decode would copy every
    /// module's text into every interpreter.
    static SHIPPED_MODULE_TEXTS: Vec<OnceCell<Rc<str>>> =
        std::iter::once(KERNEL_MODULE)
            .chain(SHIPPED_MODULES.iter().map(|shipped| shipped.name))
            .map(|_| OnceCell::new())
            .collect();
}

/// The text of `Kernel` or of the `SHIPPED_MODULES` entry `name`, as one
/// `Rc<str>` per thread (`SHIPPED_MODULE_TEXTS`). A name that is neither
/// is a defect in the binary, so it panics.
pub(crate) fn shipped_module_text(name: &str) -> Rc<str> {
    let (index, text) = if name == KERNEL_MODULE {
        (0, super::kernel::KERNEL_SOURCE)
    } else {
        let index = SHIPPED_MODULES
            .iter()
            .position(|shipped| shipped.name == name)
            .unwrap_or_else(|| panic!("internal: no shipped module is named {name}"));
        (index + 1, SHIPPED_MODULES[index].source)
    };
    SHIPPED_MODULE_TEXTS.with(|texts| Rc::clone(texts[index].get_or_init(|| Rc::from(text))))
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
    /// kernel's instance as the first shipped module's, then builds
    /// `PlatformFactory` (`build_shipped_module`), whose `build` makes the
    /// platform and reaches no other module. Then `complete_shipped_world`
    /// builds every class the two roots declare and records each under its
    /// name. Every other shipped module (`LAZY_SHIPPED_MODULES`) is built on
    /// first demand, through `PlatformFactory.ns`'s lazy slots, or by the
    /// loader when an image names it (`build_shipped_names`), so a program
    /// pays only for the modules it uses.
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
        self.enter(|mutator| {
            let kernel = mutator
                .kernel
                .and_then(|kernel| kernel.as_object())
                .expect("internal: the kernel's instance is wired in before the shipped modules");
            mutator.record_shipped_module(Symbol::intern(KERNEL_MODULE), kernel);
        });
        self.build_shipped_module("PlatformFactory", None);
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

    /// Builds the `SHIPPED_MODULES` entry `name` from the host, as the
    /// mirror's leaves build it from shipped Newspeak: its root class
    /// (`module_class`), instantiated through the ordinary `instantiate`
    /// with its factory's arguments -- `PlatformFactory`,
    /// `TypedArraysFactory` and `MirrorsFactory` the VM mirror and the
    /// kernel instance,
    /// `TranscriptFactory` and `TimeFactory` the mirror alone,
    /// `CollectionsFactory` the kernel instance alone -- and the instance
    /// recorded as the module's one instance (`record_pending`). A module
    /// already recorded, or one that fails to build or to record, is a
    /// defect in the binary, so it panics. `root_mixin` is the module's root
    /// mixin when the caller has already taken it from the shipped code, so
    /// that it is not decoded or compiled twice.
    pub(crate) fn build_shipped_module(&mut self, name: &str, root_mixin: Option<Rc<Mixin>>) {
        let shipped = shipped_module(name);
        let (factory_class, mirror, kernel) = self.enter(|mutator| {
            let class = module_class(mutator, shipped, root_mixin)
                .unwrap_or_else(|| panic!("internal: {name} is built twice"));
            let mirror = mutator.vm_mirror();
            let kernel = mutator
                .kernel
                .expect("internal: the kernel is wired in before the shipped modules boot");
            (
                mutator.stash(Value::object(class)),
                mutator.stash(mirror),
                mutator.stash(kernel),
            )
        });
        let instance = match shipped.factory {
            ShippedFactory::MirrorAndKernel => booted(
                self.instantiate(&factory_class, &[mirror, kernel]),
                &format!("{name} usingVmMirror:kernel:"),
            ),
            ShippedFactory::Mirror => booted(
                self.instantiate(&factory_class, &[mirror]),
                &format!("{name} usingVmMirror:"),
            ),
            ShippedFactory::Kernel => booted(
                self.instantiate(&factory_class, &[kernel]),
                &format!("{name} usingKernel:"),
            ),
        };
        self.enter(|mutator| {
            let instance = mutator.fetch(&instance);
            if let Err(reason) = record_pending(mutator, instance) {
                panic!("internal: recording {name}'s instance failed: {reason}");
            }
        });
    }
}

/// The root class of the lazy shipped module `name`, for the mirror's
/// `shippedModuleClassNamed:`: `None` when `name` is no lazy shipped module
/// (`LAZY_SHIPPED_MODULES`) or its instance is already recorded.
pub(crate) fn shipped_module_class<'gc>(
    mutator: &mut Mutator<'gc>,
    name: Symbol,
) -> Option<ClassReference<'gc>> {
    if !LAZY_SHIPPED_MODULES.contains(&name.as_str()) {
        return None;
    }
    module_class(mutator, shipped_module(name.as_str()), None)
}

/// The root class of `shipped`, or `None` when its instance is already
/// recorded: the module's pending class when one was built, otherwise a
/// new one, built from `root_mixin` or else the root mixin the boot takes
/// the shipped code from (`Mutator::shipped_root_mixin`), and kept pending.
/// One class per module, so that a demand an unwind interrupted
/// instantiates the same class again, and only an instance of it is ever
/// recorded.
fn module_class<'gc>(
    mutator: &mut Mutator<'gc>,
    shipped: &'static ShippedModule,
    root_mixin: Option<Rc<Mixin>>,
) -> Option<ClassReference<'gc>> {
    let name = Symbol::intern(shipped.name);
    if mutator.shipped_world.module_instance(name).is_some() {
        return None;
    }
    if let Some(class) = mutator.shipped_world.pending_class(name) {
        return Some(class);
    }
    let mixin = root_mixin.unwrap_or_else(|| mutator.shipped_root_mixin(shipped.name));
    let class = mutator.root_class_of_mixin(&mixin);
    mutator.shipped_world.add_pending(name, class);
    Some(class)
}

/// Records `instance` as its module's one instance, for the mirror's
/// `recordShippedModule:`, names the classes it already built
/// (`Mutator::name_built_shipped_classes`), and answers it; `None` when it
/// is no instance of a pending root class, or its module is given the
/// mirror and its `vmMirror` slot does not hold it (`record_pending`).
pub(crate) fn record_pending_module<'gc>(
    mutator: &mut Mutator<'gc>,
    instance: Value<'gc>,
) -> Option<ObjectReference<'gc>> {
    record_pending(mutator, instance).ok()
}

/// `record_pending_module`'s work, with why it declined: the host's build
/// panics with the reason, the mirror's leaf declines. Nothing is recorded
/// before the instance exists, so an unwind through a half-built module
/// leaves its class pending and records nothing.
fn record_pending<'gc>(
    mutator: &mut Mutator<'gc>,
    instance: Value<'gc>,
) -> Result<ObjectReference<'gc>, String> {
    let Some(object) = instance.as_object() else {
        return Err("it is no object".to_string());
    };
    let class = mutator.heap().object_class(object);
    let Some(module) = mutator.shipped_world.module_of_pending_class(class) else {
        return Err("it is no instance of a pending root class".to_string());
    };
    let shipped = shipped_module(module.as_str());
    if let ShippedFactory::MirrorAndKernel | ShippedFactory::Mirror = shipped.factory {
        factory_holds_the_mirror(mutator, shipped.name, object)?;
    }
    mutator.shipped_world.remove_pending(module);
    mutator.record_shipped_module(module, object);
    mutator.name_built_shipped_classes(module, object);
    Ok(object)
}

impl<'gc> Mutator<'gc> {
    /// The root mixin of `Kernel` or of the `SHIPPED_MODULES` entry `name`,
    /// taken from where this interpreter's boot takes the shipped code
    /// (`shipped_code_path`). Each boot decodes its own, as the source path
    /// compiles its own: no interpreter shares a mixin with another.
    pub(crate) fn shipped_root_mixin(&mut self, name: &str) -> Rc<Mixin> {
        match self.shipped_code_path {
            ShippedCodePath::Embedded => crate::shipped_code::decode_module(
                crate::shipped_code::embedded(),
                name,
                shipped_module_text(name),
            ),
            ShippedCodePath::Source => self.shipped_root_mixin_from_source(name),
        }
    }

    /// The root mixin of `Kernel` or of the `SHIPPED_MODULES` entry `name`,
    /// compiled from its source, which is parsed, resolved and checked
    /// (the mirror sends, and for the kernel its shape) on the way. A name
    /// that is neither is a defect in the binary, so it panics.
    pub(crate) fn shipped_root_mixin_from_source(&mut self, name: &str) -> Rc<Mixin> {
        if name == KERNEL_MODULE {
            let resolved = super::kernel::resolved_kernel();
            let root_mixin = self.build_root_mixin(
                &resolved.module.root_class,
                &resolved.bindings,
                super::instantiate::ModuleSource::Kernel,
                &resolved.source,
            );
            super::kernel::check_kernel_shape(&root_mixin, &resolved);
            root_mixin
        } else {
            let resolved = resolved_shipped_module(name);
            self.compile_root_mixin(&resolved)
        }
    }
}

/// What a boot step answered. A shipped module that fails to boot is a
/// defect in the binary, not in a program, so it panics, naming the step.
pub(crate) fn booted(result: HostResult, step: &str) -> Root {
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
/// receiver. The one instance checked is the only one recorded: no program
/// reaches the root's class to make another.
fn factory_holds_the_mirror<'gc>(
    mutator: &Mutator<'gc>,
    name: &str,
    factory: ObjectReference<'gc>,
) -> Result<(), String> {
    let heap = mutator.heap();
    let application = heap
        .class_application(heap.object_class(factory))
        .expect("internal: the factory's class is a class object");
    let Some(position) = application
        .mixin
        .slots
        .iter()
        .position(|slot| slot.name.as_str() == "vmMirror" && !slot.mutable && !slot.lazy)
    else {
        return Err(format!(
            "{name}.ns's root class must declare the immutable, eager slot vmMirror"
        ));
    };
    let holds_the_mirror = heap
        .instance_field(factory, application.own_slot_base + position)
        .is_some_and(|value| mutator.is_vm_mirror(value));
    if holds_the_mirror {
        Ok(())
    } else {
        Err(format!("{name}'s vmMirror slot must hold the VM mirror"))
    }
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

    /// The boot records the kernel's instance, then `PlatformFactory`'s,
    /// each an instance of its own root, and builds none of the lazy
    /// modules: no instance and no pending root class of one.
    #[test]
    fn the_boot_builds_only_the_kernel_and_the_platform_module() {
        let mut interpreter = crate::Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let heap = mutator.heap();
            let names: Vec<&str> = mutator
                .shipped_world
                .module_instances()
                .iter()
                .map(|(name, instance)| {
                    let application = heap
                        .class_application(heap.object_class(*instance))
                        .expect("a module instance's class is a class object");
                    assert_eq!(application.mixin.name, *name);
                    name.as_str()
                })
                .collect();
            assert_eq!(names, ["Kernel", "PlatformFactory"]);
            for name in ["Kernel", "PlatformFactory"] {
                assert!(mutator
                    .shipped_world
                    .module_instance(Symbol::intern(name))
                    .is_some());
            }
            for name in LAZY_SHIPPED_MODULES {
                let name = Symbol::intern(name);
                assert!(mutator.shipped_world.module_instance(name).is_none());
                assert!(mutator.shipped_world.pending_class(name).is_none());
            }
        });
    }

    /// `CollectionsFactory`'s root mixin holds the module's text byte for
    /// byte, and its `List` names the root as its enclosing mixin, whether
    /// the boot decodes the embedded shipped code or compiles the source.
    #[test]
    fn a_shipped_module_decoded_from_the_embedded_code_carries_its_text_and_links() {
        for path in [ShippedCodePath::Embedded, ShippedCodePath::Source] {
            let mut interpreter =
                crate::Interpreter::bootstrap_on(crate::compiler::Lowering::Optimized, path);
            interpreter.enter(|mutator| {
                let root = mutator.shipped_root_mixin("CollectionsFactory");
                let text = root
                    .module_text
                    .as_ref()
                    .expect("a shipped module has its text");
                assert_eq!(&*text.0, COLLECTIONS_FACTORY_SOURCE);
                let list = root
                    .nested
                    .iter()
                    .find(|nested| nested.name.as_str() == "List")
                    .expect("CollectionsFactory declares List");
                assert!(list
                    .enclosing_mixin()
                    .is_some_and(|enclosing| Rc::ptr_eq(&enclosing, &root)));
                assert!(list
                    .module_text
                    .as_ref()
                    .is_some_and(|list_text| Rc::ptr_eq(&list_text.0, &text.0)));
                assert!(root.enclosing_mixin().is_none());
            });
        }
    }

    /// What a host send answered, or a panic naming `step`.
    fn expect_ok(result: HostResult, step: &str) -> Root {
        match result {
            Ok(answer) => answer,
            Err(HostSignal::Return) => panic!("{step}: a non-local return escaped"),
            Err(HostSignal::Error(error)) => {
                panic!("{step}: unexpected Newspeak error: {}", error.message)
            }
        }
    }

    /// An instance of a pending root class whose factory was given the
    /// mirror is recorded only when its `vmMirror` slot holds the mirror
    /// (lock (2)): `TranscriptFactory` instantiated from the host over
    /// `nil` is declined, nothing is recorded, and the class stays pending.
    #[test]
    fn a_module_instance_over_something_that_is_not_the_mirror_is_not_recorded() {
        let mut interpreter = crate::Interpreter::bootstrap();
        let transcript = Symbol::intern("TranscriptFactory");
        let (class, not_the_mirror, entry_count) = interpreter.enter(|mutator| {
            let class = shipped_module_class(mutator, transcript)
                .expect("TranscriptFactory is lazy and not yet recorded");
            (
                mutator.stash(Value::object(class)),
                mutator.stash(Value::nil()),
                mutator.shipped_world.entries().len(),
            )
        });
        let instance = expect_ok(
            interpreter.instantiate(&class, std::slice::from_ref(&not_the_mirror)),
            "TranscriptFactory usingVmMirror: nil",
        );
        interpreter.enter(|mutator| {
            let instance = mutator.fetch(&instance);
            assert!(matches!(
                record_pending(mutator, instance),
                Err(reason) if reason.contains("vmMirror slot must hold the VM mirror")
            ));
            assert!(record_pending_module(mutator, instance).is_none());
            assert!(mutator.shipped_world.module_instance(transcript).is_none());
            assert_eq!(mutator.shipped_world.entries().len(), entry_count);
            let class = mutator
                .fetch(&class)
                .as_object()
                .expect("a root class is a class object");
            assert!(mutator
                .shipped_world
                .pending_class(transcript)
                .is_some_and(|pending| pending.same(class)));
        });
    }

    /// Whether the shipped world names the class `declaration` of
    /// `CollectionsFactory` (the module's own name for its root).
    fn collections_names(interpreter: &mut crate::Interpreter, declaration: &str) -> bool {
        interpreter.enter(|mutator| {
            let name = crate::interpreter::shipped_world::ShippedName {
                module: Symbol::intern("CollectionsFactory"),
                declaration: Symbol::intern(declaration),
                kind: crate::interpreter::shipped_world::ShippedKind::Class,
            };
            mutator.shipped_world.index_of_name(&name).is_some()
        })
    }

    /// A program's first `platform collections` builds the collections
    /// module and records it: the namespace it answers is of the instance
    /// the shipped world records. A class of the module is named once its
    /// accessor builds it, and one never asked for is not built, so
    /// `platform collections List` names `List` and not `Map`.
    #[test]
    fn a_program_builds_the_collections_module_and_only_the_classes_it_asks_for() {
        let source = indoc::indoc! {"
            class Demand = Object new () (
              public collections: platform = (
                ^platform collections
              )

              public list: platform = (
                ^platform collections List
              )
            )
        "};
        let module = ns_syntax::parse(source).expect("parse");
        let origin = SourceOrigin::File(std::path::PathBuf::from("demand.ns"));
        let Ok(resolved) = crate::resolve(module, Rc::from(source), origin) else {
            panic!("the module resolves");
        };
        let mut interpreter = crate::Interpreter::bootstrap();
        let class = interpreter.load_module(&resolved);
        let demand = expect_ok(interpreter.instantiate(&class, &[]), "Demand new");
        let platform = interpreter.platform();
        assert!(!collections_names(&mut interpreter, "CollectionsFactory"));
        let facet = expect_ok(
            interpreter.send(
                &demand,
                Symbol::intern("collections:"),
                std::slice::from_ref(&platform),
            ),
            "Demand collections:",
        );
        interpreter.enter(|mutator| {
            let recorded = mutator
                .shipped_world
                .module_instance(Symbol::intern("CollectionsFactory"))
                .expect("the first demand records the collections module");
            let heap = mutator.heap();
            let facet = mutator
                .fetch(&facet)
                .as_object()
                .expect("a facet is an object");
            let enclosing = heap
                .class_enclosing(heap.object_class(facet))
                .expect("a facet's class is nested");
            assert!(
                enclosing.same(recorded),
                "the namespace is of the recorded instance"
            );
        });
        assert!(collections_names(&mut interpreter, "CollectionsFactory"));
        assert!(!collections_names(&mut interpreter, "List"));
        expect_ok(
            interpreter.send(&demand, Symbol::intern("list:"), &[platform]),
            "Demand list:",
        );
        assert!(collections_names(&mut interpreter, "List"));
        assert!(!collections_names(&mut interpreter, "Map"));
    }

    /// The pending root class is one per module: asked twice, the leaf's
    /// function answers the same class, and an instance of it that is never
    /// recorded -- all a factory send an unwind interrupted could leave --
    /// gives the module no instance and leaves the same class pending, so
    /// the next demand instantiates it again.
    #[test]
    fn an_unrecorded_instance_leaves_the_same_root_class_pending() {
        let mut interpreter = crate::Interpreter::bootstrap();
        let name = Symbol::intern("CollectionsFactory");
        let (class, kernel) = interpreter.enter(|mutator| {
            let first = shipped_module_class(mutator, name).expect("a lazy module's class");
            let second = shipped_module_class(mutator, name).expect("a lazy module's class");
            assert!(first.same(second));
            let kernel = mutator.kernel.expect("the kernel is wired in");
            (mutator.stash(Value::object(first)), mutator.stash(kernel))
        });
        expect_ok(
            interpreter.instantiate(&class, &[kernel]),
            "CollectionsFactory usingKernel:",
        );
        interpreter.enter(|mutator| {
            assert!(mutator.shipped_world.module_instance(name).is_none());
            let pending = mutator
                .shipped_world
                .pending_class(name)
                .expect("the class is still pending");
            let class = mutator.fetch(&class).as_object().expect("a class object");
            assert!(pending.same(class));
            let again = shipped_module_class(mutator, name).expect("a lazy module's class");
            assert!(again.same(class));
        });
    }

    /// A class a module built before its instance was recorded is named by
    /// the record: the accessor's hook names no class of a module not yet
    /// recorded, so `recordShippedModule:` names whatever the instance
    /// already holds -- here `List`, built from the host before the record,
    /// as the module's own initializer could build it -- and nothing it
    /// does not, so `Map` stays unbuilt and unnamed.
    #[test]
    fn recording_a_module_names_the_classes_it_built_before() {
        let mut interpreter = crate::Interpreter::bootstrap();
        let name = Symbol::intern("CollectionsFactory");
        let (class, kernel) = interpreter.enter(|mutator| {
            let class = shipped_module_class(mutator, name).expect("a lazy module's class");
            let kernel = mutator.kernel.expect("the kernel is wired in");
            (mutator.stash(Value::object(class)), mutator.stash(kernel))
        });
        let instance = expect_ok(
            interpreter.instantiate(&class, &[kernel]),
            "CollectionsFactory usingKernel:",
        );
        let list = interpreter.enter(|mutator| {
            let instance = mutator.fetch(&instance).as_object().expect("an object");
            let heap = mutator.heap();
            heap.class_application(heap.object_class(instance))
                .expect("the root is a class object")
                .mixin
                .nested
                .iter()
                .find(|nested| nested.name.as_str() == "List")
                .expect("CollectionsFactory declares List")
                .clone()
        });
        let built = expect_ok(
            interpreter.build_nested_class(&instance, &list),
            "CollectionsFactory List",
        );
        assert!(!collections_names(&mut interpreter, "List"));
        interpreter.enter(|mutator| {
            let instance = mutator.fetch(&instance);
            assert!(record_pending_module(mutator, instance).is_some());
        });
        assert!(collections_names(&mut interpreter, "CollectionsFactory"));
        assert!(collections_names(&mut interpreter, "List"));
        assert!(!collections_names(&mut interpreter, "Map"));
        interpreter.enter(|mutator| {
            let list_name = crate::interpreter::shipped_world::ShippedName {
                module: name,
                declaration: Symbol::intern("List"),
                kind: crate::interpreter::shipped_world::ShippedKind::Class,
            };
            let world = &mutator.shipped_world;
            let index = world.index_of_name(&list_name).expect("List is named");
            let built = mutator.fetch(&built).as_object().expect("a class object");
            assert!(world.entries()[index].object.same(built));
        });
    }

    /// Every lazy module is a `SHIPPED_MODULES` entry, and the platform,
    /// which the boot makes, is the one entry that is not lazy.
    #[test]
    fn every_lazy_shipped_module_is_a_shipped_module_but_the_platform() {
        for name in LAZY_SHIPPED_MODULES {
            assert_eq!(shipped_module(name).name, *name);
        }
        let others: Vec<&str> = SHIPPED_MODULES
            .iter()
            .map(|shipped| shipped.name)
            .filter(|name| !LAZY_SHIPPED_MODULES.contains(name))
            .collect();
        assert_eq!(others, ["PlatformFactory"]);
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

    /// The leaf declines a lazy module not yet built, so the forwarder's
    /// failure block -- which builds it -- runs; once it is built, the leaf
    /// answers the canonical instance, the same object on every request.
    /// It declines a name the table does not list and a subject that is no
    /// symbol.
    #[test]
    fn module_instance_named_answers_the_built_instance_and_declines_anything_else() {
        let mut interpreter = crate::Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let name = mutator.new_symbol(Symbol::intern("CollectionsFactory"));
            assert!(module_instance_named(mutator, name).is_none());
        });
        interpreter.build_shipped_module("CollectionsFactory", None);
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
