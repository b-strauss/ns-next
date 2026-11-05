//! The optimizing compiler (`docs/design/execution.md`, "Planned: the
//! optimizing JIT"): it compiles from a snapshot of a body, taken at the
//! run loop's safepoint, so that its work reads nothing the interpreter can
//! change.

#[cfg(ns_jit)]
pub(crate) mod allocate;
pub(crate) mod build;
#[cfg(ns_jit)]
pub(crate) mod emit;
pub(crate) mod graph;
pub(crate) mod liveness;
#[cfg(ns_jit)]
pub(crate) mod metadata;
pub(crate) mod snapshot;
pub(crate) mod verify;

/// The optimized code of `snapshot`'s body, compiled as `role` with
/// `macro_assembler` (`docs/design/execution.md`, "Planned: the
/// optimizing JIT"): its graph built (and, in a debug build, verified),
/// its values allocated to registers and its interrupt points described
/// (the allocation verified in a debug build), then emitted. A refusal is
/// one the baseline makes of the same body: too many instructions, or
/// code the assembler cannot lay out.
#[cfg(ns_jit)]
pub(crate) fn compile(
    snapshot: &snapshot::Snapshot,
    macro_assembler: crate::jit::macro_assembler::MacroAssembler,
    role: crate::jit::baseline::BodyRole,
) -> Result<emit::OptimizedBody, crate::jit::Refusal> {
    use crate::jit::Refusal;

    let roles = macro_assembler.roles();
    let mut body = crate::jit::baseline::BodyAssembler::for_optimized(
        macro_assembler,
        &snapshot.facts,
        snapshot.feedback_base,
        role,
    )?;
    let graph = build::build(snapshot);
    let (allocation, metadata) = metadata::allocate_and_describe(snapshot, &graph, &roles);
    let push_entry = emit::emit(snapshot, &graph, &allocation, &mut body)?;
    let returns = body.take_call_returns();
    let words = body.macro_assembler.body_words_label();
    let word_bytes =
        u32::try_from(body.macro_assembler.body_word_count() * 8).map_err(|_| Refusal::ZoneFull)?;
    let twin_transfers = std::mem::take(&mut body.twin_transfers);
    let code = body
        .macro_assembler
        .finish()
        .map_err(|error| Refusal::Unassembled(Some(error)))?;
    let words = words
        .and_then(|label| code.label_offsets[label.index()])
        .map(|start| start..start + word_bytes);
    let push_entry =
        code.label_offsets[push_entry.index()].expect("internal: the first block is bound");
    Ok(emit::OptimizedBody {
        code,
        returns,
        words,
        twin_transfers,
        metadata,
        push_entry,
    })
}

/// The snapshot of a method of `instructions`, none of which makes a
/// closure, with registers to spare: the hand-written bodies the passes'
/// tests compile.
#[cfg(test)]
pub(crate) fn test_snapshot(instructions: Vec<crate::bytecode::Instruction>) -> snapshot::Snapshot {
    test_snapshot_with_arguments(instructions, 0)
}

/// `test_snapshot` of a method taking `argument_count` arguments.
#[cfg(test)]
pub(crate) fn test_snapshot_with_arguments(
    instructions: Vec<crate::bytecode::Instruction>,
    argument_count: u16,
) -> snapshot::Snapshot {
    let code = crate::bytecode::CompiledCode::new(
        crate::bytecode::CodeKind::Method,
        instructions,
        argument_count,
        30,
        0,
        Vec::new(),
        ns_syntax::Span::new(0, 0),
    );
    snapshot::Snapshot {
        code_id: crate::heap::CodeId::from_raw(0),
        facts: snapshot::BodyFacts::of(&code),
        feedback_base: crate::interpreter::inline_cache::FeedbackBase::NOT_ASSIGNED,
        instructions: code
            .instructions()
            .iter()
            .map(|instruction| {
                snapshot::SnapshotInstruction::of_unclosed(instruction).expect("no closure")
            })
            .collect(),
        tainted_before: snapshot::tainted_before_each(&code),
    }
}

/// `receiver new: argument` into `destination`, a send for the passes'
/// tests.
#[cfg(test)]
pub(crate) fn test_send(
    receiver: u16,
    argument: u16,
    destination: u16,
) -> crate::bytecode::Instruction {
    crate::bytecode::Instruction::SendPublic {
        selector: crate::Symbol::NEW,
        receiver_register: receiver,
        argument_base: argument,
        argument_sources: vec![argument],
        destination,
        call_site: crate::bytecode::CallSite { number: 0 },
    }
}

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
    use crate::interpreter::inline_cache::FeedbackBase;

    let mut interpreter = crate::Interpreter::bootstrap_with(crate::Lowering::Optimized);
    let bodies = bodies_of(&mut interpreter, modules);
    interpreter.enter(|mutator| {
        bodies
            .iter()
            .map(|code| {
                snapshot::Snapshot::take(code, &mut mutator.heap, FeedbackBase::NOT_ASSIGNED)
            })
            .collect()
    })
}

/// Every body `modules` compile to in `interpreter`, closures' blocks
/// included, each block before the body that makes it.
#[cfg(test)]
fn bodies_of(
    interpreter: &mut crate::Interpreter,
    modules: &[std::rc::Rc<crate::ResolvedModule>],
) -> Vec<std::rc::Rc<crate::bytecode::CompiledCode>> {
    use std::rc::Rc;

    use crate::bytecode::{CompiledCode, Instruction};
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

    let mut bodies = Vec::new();
    for module in modules {
        collect_mixin(&interpreter.compile_module(module), &mut bodies);
    }
    bodies
}
