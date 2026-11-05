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
/// code the assembler cannot lay out. `futures_exit` is the setting its
/// baseline code was compiled under (`JitState::set_futures_exit`), so
/// that both tiers are the same lowering.
#[cfg(ns_jit)]
pub(crate) fn compile(
    snapshot: &snapshot::Snapshot,
    macro_assembler: crate::jit::macro_assembler::MacroAssembler,
    role: crate::jit::baseline::BodyRole,
    futures_exit: bool,
) -> Result<emit::OptimizedBody, crate::jit::Refusal> {
    use crate::jit::Refusal;

    let roles = macro_assembler.roles();
    let mut body = crate::jit::baseline::BodyAssembler::for_optimized(
        macro_assembler,
        &snapshot.facts,
        snapshot.feedback_base,
        role,
        futures_exit,
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
        fallback_taken: vec![false; code.instructions().len()],
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
                snapshot::Snapshot::take(
                    code,
                    &mut mutator.heap,
                    &mutator.caches,
                    FeedbackBase::NOT_ASSIGNED,
                )
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

#[cfg(all(test, ns_jit))]
mod tests {
    use indoc::indoc;

    use super::allocate::Move;
    use super::graph::Node;
    use crate::interpreter::inline_cache::MethodSide;
    use crate::{HostResult, HostSignal, Interpreter, Lowering, Root, SourceOrigin, Symbol};

    fn expect_ok(result: HostResult, what: &str) -> Root {
        match result {
            Ok(root) => root,
            Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
            Err(HostSignal::Error(error)) => {
                panic!("{what}: unexpected Newspeak error: {}", error.message)
            }
        }
    }

    /// Mandelbrot's escape loop: every operation but the counter's is on
    /// `Float`s, whose `IntegerBinary` fast path fails at every pass.
    const ESCAPE: &str = indoc! {"
        class Escape = () (
          public run = (
            |
              real
              imaginary
              realSquared
              imaginarySquared
              iteration
              notDone
            |
            real:: 0.0.
            imaginary:: 0.0.
            realSquared:: 0.0.
            imaginarySquared:: 0.0.
            iteration:: 0.
            notDone:: true.
            [notDone and: [iteration < 50]] whileTrue: [
              real:: realSquared - imaginarySquared + 0.25.
              imaginary:: 2.0 * real * imaginary + 0.5.
              realSquared:: real * real.
              imaginarySquared:: imaginary * imaginary.
              realSquared + imaginarySquared > 4.0 ifTrue: [notDone:: false].
              iteration:: iteration + 1.
            ].
            ^iteration
          )
        )
    "};

    /// With the caches' feedback of a run, every float operation of the
    /// escape loop is its send alone, and the loop's locals are never
    /// reloaded on a way into a block nor written back but where the
    /// interpreter's instructions write them: each operation costs its
    /// send and nothing around it, where before each send stored and
    /// reloaded every local the loop carries.
    #[test]
    fn the_escape_loop_sends_its_float_operations_with_nothing_stored_or_reloaded_around_them() {
        let module = ns_syntax::parse(ESCAPE).expect("parse");
        let resolved = crate::resolve(
            module,
            ESCAPE.into(),
            SourceOrigin::File("escape.ns".into()),
        )
        .unwrap_or_else(|_| panic!("the escape loop resolves"));
        let mut interpreter = Interpreter::bootstrap_with(Lowering::Optimized);
        // The run's feedback is what the snapshot reads: the cache stress
        // mode would vacate it at the run's safepoints.
        #[cfg(debug_assertions)]
        interpreter.set_stress_caches(false);
        let class = interpreter.load_module(&resolved);
        let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        expect_ok(
            interpreter.send(&instance, Symbol::intern("run"), &[]),
            "run",
        );
        let snapshot = interpreter.enter(|mutator| {
            let code = crate::interpreter::method_code(
                mutator,
                &class,
                MethodSide::Instance,
                Symbol::intern("run"),
            )
            .expect("the method");
            let base = mutator.caches.known_base(&code).expect("it ran");
            super::snapshot::Snapshot::take(&code, &mut mutator.heap, &mutator.caches, base)
        });
        let taken = snapshot
            .fallback_taken
            .iter()
            .filter(|taken| **taken)
            .count();
        assert_eq!(taken, 9, "the nine float operations, not the counter's two");
        let graph = super::build::build(&snapshot);
        let integer_paths = graph
            .blocks
            .iter()
            .flat_map(|block| &block.nodes)
            .filter(|node| matches!(node, Node::IntegerBinary { .. }))
            .count();
        assert_eq!(integer_paths, 2, "`iteration < 50` and `iteration + 1`");
        let roles =
            crate::jit::macro_assembler::Roles::of(crate::jit::macro_assembler::Abi::host());
        let (allocation, _) = super::metadata::allocate_and_describe(&snapshot, &graph, &roles);
        // The four locals the float operations write: each is stored once
        // before the loop, on its way in, and once in the loop, right where
        // the interpreter's `StoreLocal` writes it, and reloaded only on
        // the way back from the counter's two fallbacks.
        let floats = 13..=16;
        let before_the_loop = allocation
            .positions
            .end_definition(super::graph::BlockId::of_index(0));
        let fallback_ways: Vec<u32> = graph
            .blocks
            .iter()
            .flat_map(|block| &block.nodes)
            .filter_map(|node| match node {
                Node::IntegerBinary { fallback, .. } => {
                    Some(allocation.positions.end_definition(*fallback))
                }
                _ => None,
            })
            .collect();
        for (position, moves) in &allocation.moves {
            for each in moves {
                match each {
                    Move::Load { register, .. } => assert!(
                        !floats.contains(register) || fallback_ways.contains(position),
                        "a float local is reloaded at {position}: {each:?}"
                    ),
                    Move::Store { register, .. } => assert!(
                        !floats.contains(register) || *position == before_the_loop,
                        "a float local is stored on a way in at {position}: {each:?}"
                    ),
                    Move::Transfer { .. } => {}
                }
            }
        }
        for register in floats {
            let write_backs = allocation
                .dirty_at
                .iter()
                .filter(|point| {
                    point
                        .stores
                        .iter()
                        .any(|(stored, _, _)| *stored == register)
                })
                .count();
            assert_eq!(write_backs, 1, "r{register} is written back once a pass");
        }
    }
}
