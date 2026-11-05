//! The optimizing compiler (`docs/design/execution.md`, "Planned: the
//! optimizing JIT"): it compiles from a snapshot of a body, taken at the
//! run loop's safepoint, so that its work reads nothing the interpreter can
//! change.

pub(crate) mod build;
pub(crate) mod graph;
pub(crate) mod snapshot;
pub(crate) mod verify;

/// The snapshot of every method and block body of the modules under
/// `goldens/runtime/jit/`, compiled with the optimized lowering: the
/// bodies the optimizing compiler's tests hold each of its passes to.
#[cfg(test)]
pub(crate) fn corpus_of_bodies() -> Vec<snapshot::Snapshot> {
    use std::path::Path;
    use std::rc::Rc;

    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../goldens/runtime/jit");
    let mut files: Vec<_> = std::fs::read_dir(&directory)
        .expect("the JIT's goldens")
        .map(|entry| entry.expect("an entry").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "ns"))
        .collect();
    files.sort();
    let modules: Vec<_> = files
        .iter()
        .map(|file| {
            let source = std::fs::read_to_string(file).expect("readable");
            let module = ns_syntax::parse(&source).expect("a JIT golden parses");
            let resolved = crate::resolve(
                module,
                Rc::from(source.as_str()),
                crate::SourceOrigin::File(file.clone()),
            )
            .unwrap_or_else(|_| panic!("{} resolves", file.display()));
            Rc::new(resolved)
        })
        .collect();
    snapshots_of(&modules)
}

/// The snapshot of every method and block body of the shipped core
/// library, compiled with the optimized lowering: the only bodies that
/// hold mirror sends.
#[cfg(test)]
pub(crate) fn shipped_bodies() -> Vec<snapshot::Snapshot> {
    use crate::interpreter::core_library;
    let mut modules = vec![crate::interpreter::kernel::resolved_kernel()];
    for shipped in core_library::SHIPPED_MODULES {
        modules.push(core_library::resolved_shipped_module(shipped.name));
    }
    snapshots_of(&modules)
}

/// The snapshot of every body `modules` compile to under the optimized
/// lowering, closures' blocks included.
#[cfg(test)]
fn snapshots_of(modules: &[std::rc::Rc<crate::ResolvedModule>]) -> Vec<snapshot::Snapshot> {
    use std::rc::Rc;

    use crate::bytecode::{CompiledCode, Instruction};
    use crate::interpreter::inline_cache::FeedbackBase;
    use crate::mixin::{MethodEntry, Mixin};

    fn collect(code: &Rc<CompiledCode>, into: &mut Vec<Rc<CompiledCode>>) {
        for instruction in code.instructions() {
            if let Instruction::MakeClosure { code: block, .. } = instruction {
                collect(&block, into);
            }
        }
        into.push(Rc::clone(code));
    }

    fn collect_mixin(mixin: &Rc<Mixin>, into: &mut Vec<Rc<CompiledCode>>) {
        for entry in mixin.methods.values() {
            match entry {
                MethodEntry::Compiled(method) => collect(&method.code, into),
                MethodEntry::Factory(factory) => collect(&factory.code, into),
                MethodEntry::SlotAccessor(_) | MethodEntry::Primitive(_) => {}
            }
        }
        for code in [
            &mixin.initializer_code,
            &mixin.own_initializer_code,
            &mixin.standalone_initializer_code,
            &mixin.superclass_clause_code,
        ]
        .into_iter()
        .flatten()
        {
            collect(code, into);
        }
        if let Some(class_side) = &mixin.class_side {
            collect_mixin(class_side, into);
        }
        for nested in mixin.declared_nested().iter() {
            collect_mixin(nested, into);
        }
    }

    let mut interpreter = crate::Interpreter::bootstrap_with(crate::Lowering::Optimized);
    let mut bodies = Vec::new();
    for module in modules {
        collect_mixin(&interpreter.compile_module(module), &mut bodies);
    }
    interpreter.enter(|mutator| {
        bodies
            .iter()
            .map(|code| {
                snapshot::Snapshot::take(code, &mut mutator.heap, FeedbackBase::NOT_ASSIGNED)
            })
            .collect()
    })
}
