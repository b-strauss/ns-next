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
pub(crate) mod plan;
pub(crate) mod snapshot;
pub(crate) mod tree;
pub(crate) mod verify;

/// How far a compile speculates (`docs/design/execution.md`, "Speculation",
/// "Guards and traps", "Versions and the refusal for good").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Speculation {
    /// Every site the feedback says never ran is a guard that traps: an
    /// integer fast path with no fallback, a conditional with no third
    /// way, a send that exits before it is made (`build::build`).
    Traps,
    /// No guard: every fast path keeps its fallback send, every
    /// conditional its third way and every send its call, and no value
    /// is of a known kind -- stage 1's code, which can never trap, for a
    /// copy past its last version.
    None,
}

impl Speculation {
    /// How far a copy at `version` speculates: with traps up to
    /// `jit::LAST_VERSION`, without past it, so that the copy a key ends
    /// with can never be made not entrant (`docs/design/execution.md`,
    /// "Speculation", "Versions and the refusal for good").
    pub(crate) fn at_version(version: u8) -> Speculation {
        if version <= crate::jit::LAST_VERSION {
            Speculation::Traps
        } else {
            Speculation::None
        }
    }
}

/// The optimized code of `snapshot`'s body alone, inlining nothing
/// (`compile_tree`): the passes' tests' compile of a hand-written body.
#[cfg(all(test, ns_jit))]
pub(crate) fn compile(
    snapshot: &snapshot::Snapshot,
    macro_assembler: crate::jit::macro_assembler::MacroAssembler,
    role: crate::jit::baseline::BodyRole,
    futures_exit: bool,
    speculation: Speculation,
) -> Result<emit::OptimizedBody, crate::jit::Refusal> {
    compile_tree(
        &tree::SnapshotTree::alone(snapshot.clone()),
        macro_assembler,
        role,
        futures_exit,
        speculation,
    )
}

/// The optimized code of `tree`'s root, compiled as `role` with
/// `macro_assembler` (`docs/design/execution.md`, "Planned: the
/// optimizing JIT"): the callees the inlining plan takes inlined
/// (`plan::plan`), its graph built (and, in a debug build, verified),
/// its values allocated to registers and its interrupt points described
/// (the allocation verified in a debug build), then emitted. A refusal is
/// one the baseline makes of the same body: too many instructions, or
/// code the assembler cannot lay out. `futures_exit` is the setting its
/// baseline code was compiled under (`JitState::set_futures_exit`), so
/// that both tiers are the same lowering. `speculation` says which sites
/// the feedback says never ran become guards.
#[cfg(ns_jit)]
pub(crate) fn compile_tree(
    tree: &tree::SnapshotTree,
    macro_assembler: crate::jit::macro_assembler::MacroAssembler,
    role: crate::jit::baseline::BodyRole,
    futures_exit: bool,
    speculation: Speculation,
) -> Result<emit::OptimizedBody, crate::jit::Refusal> {
    use crate::jit::baseline::{BodyRole, ScopeFacts};
    use crate::jit::Refusal;

    let snapshot = &tree.root;
    let roles = macro_assembler.roles();
    let mut body = crate::jit::baseline::BodyAssembler::for_optimized(
        macro_assembler,
        &snapshot.facts,
        snapshot.feedback_base,
        role,
        futures_exit,
    )?;
    let plan = plan::plan(tree, speculation);
    body.deepest_chain = plan.deepest_chain;
    body.deepest_extent = plan.deepest_extent;
    let graph = build::build_inlined(tree, &plan, speculation);
    let (allocation, metadata) = metadata::allocate_and_describe(&graph, &roles);
    // Each scope's facts, as its own baseline code would read them: an
    // inlined scope is a method's, as many scopes deep as its chain.
    let scopes: Vec<emit::EmittedScope<'_>> = graph
        .scopes
        .iter()
        .enumerate()
        .map(|(index, scope)| {
            let scope_snapshot = if index == 0 {
                snapshot
            } else {
                tree.snapshot_of(scope.code_id)
                    .expect("internal: every inlined scope's body is snapshotted")
            };
            let mut depth = 0_u16;
            let mut parent = scope.parent;
            while let Some((sender, _)) = parent {
                depth += 1;
                parent = graph.scope(sender).parent;
            }
            // An inlined block's scope is lowered as a method's too: its
            // templates never return, reach an outer local or make a
            // closure, which are what a block's role changes -- its return
            // and its `^` are jumps, its outer locals its homes' registers
            // and its `MakeClosure` an exit (`build`, `emit`).
            let scope_role = if index == 0 { role } else { BodyRole::Method };
            emit::EmittedScope {
                snapshot: scope_snapshot,
                facts: ScopeFacts::of(
                    &scope_snapshot.facts,
                    scope_snapshot.feedback_base,
                    scope_role,
                    depth,
                ),
                offset: scope.offset,
            }
        })
        .collect();
    let (push_entry, polls) = emit::emit(&scopes, &graph, &allocation, &metadata, &mut body)?;
    // The one answer to which scopes the copy inlined: what it keeps of
    // them, what it names in a collection and what it depends on are all
    // read off it, so that none of them can name a scope the others miss.
    let laid_out = laid_out_scopes(&graph);
    let eliminated = eliminated_blocks(&plan, tree);
    let returns = body.take_call_returns();
    let words = body.macro_assembler.body_words_label();
    let word_bytes =
        u32::try_from(body.macro_assembler.body_word_count() * 8).map_err(|_| Refusal::ZoneFull)?;
    let twin_transfers = std::mem::take(&mut body.twin_transfers);
    #[cfg(test)]
    let small_integer_tag_tests = body.macro_assembler.small_integer_tag_tests();
    let (code, site_labels) = body
        .macro_assembler
        .finish_with_sites()
        .map_err(|error| Refusal::Unassembled(Some(error)))?;
    let words = words
        .and_then(|label| code.label_offsets[label.index()])
        .map(|start| start..start + word_bytes);
    let push_entry =
        code.label_offsets[push_entry.index()].expect("internal: the first block is bound");
    let sites = site_labels
        .iter()
        .map(|(label, point)| {
            let offset =
                code.label_offsets[label.index()].expect("internal: every site's label is bound");
            (offset, *point)
        })
        .collect();
    let bound = |label: ns_assembler::Label| {
        code.label_offsets[label.index()].expect("internal: every poll's labels are bound")
    };
    let polls = polls
        .iter()
        .map(|poll| emit::PollEntry {
            exit: bound(poll.exit),
            entry: bound(poll.entry),
            point: poll.point,
            #[cfg(test)]
            rewrites_lost_words: poll.rewrites_lost_words,
            #[cfg(test)]
            machine_sources: poll.machine_sources.clone(),
        })
        .collect();
    Ok(emit::OptimizedBody {
        code,
        returns,
        words,
        twin_transfers,
        metadata,
        sites,
        scopes: scopes
            .iter()
            .zip(&laid_out)
            .zip(&graph.scopes)
            .map(|((scope, laid_out), graph_scope)| {
                laid_out.then_some(emit::LaidOutScope {
                    code_id: scope.snapshot.code_id,
                    feedback_base: scope.snapshot.feedback_base,
                    block: graph_scope.block,
                })
            })
            .collect(),
        deepest_chain: plan.deepest_chain,
        deepest_extent: plan.deepest_extent,
        relied_on: relied_on(&plan, &graph, &scopes, &laid_out, &eliminated, tree),
        eliminated,
        push_entry,
        polls,
        type_case_arms: type_case_arms(&graph),
        #[cfg(test)]
        small_integer_tag_tests,
    })
}

/// The arms of every type-case `graph` lays out.
#[cfg(ns_jit)]
fn type_case_arms(graph: &graph::Graph) -> u32 {
    graph
        .blocks
        .iter()
        .map(|block| match &block.end {
            graph::End::TypeCase { arms, .. } => u32::try_from(arms.len()).unwrap_or(u32::MAX),
            _ => 0,
        })
        .fold(0, u32::saturating_add)
}

/// Whether `graph` lays out code for each of its scopes, by its index:
/// the root always, an inlined scope only where some block holds its
/// instructions. A scope the plan made whose site the graph sends instead
/// -- a lexical send past depth 0, an arm the graph does not express --
/// is no scope of the copy: it has no interrupt point, and the copy
/// relies on its body no more than on any send's.
#[cfg(ns_jit)]
fn laid_out_scopes(graph: &graph::Graph) -> Vec<bool> {
    let mut laid_out = vec![false; graph.scopes.len()];
    if let Some(root) = laid_out.first_mut() {
        *root = true;
    }
    for block in &graph.blocks {
        laid_out[block.scope.0 as usize] = true;
    }
    laid_out
}

/// Every block `plan` eliminates, by its `EliminatedId`, as the exit makes
/// it (`emit::EliminatedBlock`): its body, its home and the registers its
/// `MakeClosure` copies.
#[cfg(ns_jit)]
fn eliminated_blocks(
    plan: &plan::InliningPlan,
    tree: &tree::SnapshotTree,
) -> Vec<emit::EliminatedBlock> {
    let mut eliminated: Vec<(plan::EliminatedId, emit::EliminatedBlock)> = plan
        .blocks
        .iter()
        .filter_map(|((home, program_counter), planned)| {
            let plan::BlockPlan::Eliminated(id) = planned else {
                return None;
            };
            let snapshot = tree
                .snapshot_of(plan.scopes[home.0 as usize].code_id)
                .expect("internal: the tree holds every planned scope's snapshot");
            let snapshot::SnapshotInstruction::MakeClosure { block, copied, .. } =
                &snapshot.instructions[*program_counter as usize]
            else {
                panic!("internal: an eliminated block is a `MakeClosure`'s");
            };
            Some((
                *id,
                emit::EliminatedBlock {
                    code_id: *block,
                    home: *home,
                    copied: copied.clone(),
                },
            ))
        })
        .collect();
    eliminated.sort_unstable_by_key(|(id, _)| *id);
    debug_assert!(
        eliminated
            .iter()
            .enumerate()
            .all(|(index, (id, _))| id.0 as usize == index),
        "internal: the eliminated blocks are numbered from 0 without a gap"
    );
    eliminated.into_iter().map(|(_, block)| block).collect()
}

/// What the code `graph` lays out relied on (`emit::ReliedOn`): the
/// guards its type-cases test, the applications its arms always inlined
/// were decided for, of every inlined scope `laid_out` says it lays out
/// the receiver's and entered applications `plan` customized it to and
/// its body and feedback base, as `scopes` emitted it -- a block's scope
/// among them -- and the body of every block it eliminated, `eliminated`,
/// whose closure an exit makes, as `tree` snapshotted it.
#[cfg(ns_jit)]
fn relied_on(
    plan: &plan::InliningPlan,
    graph: &graph::Graph,
    scopes: &[emit::EmittedScope<'_>],
    laid_out: &[bool],
    eliminated: &[emit::EliminatedBlock],
    tree: &tree::SnapshotTree,
) -> emit::ReliedOn {
    let mut relied_on = emit::ReliedOn::default();
    for block in &graph.blocks {
        if let graph::End::TypeCase { arms, .. } = &block.end {
            for (guard, _) in arms {
                if !relied_on.guards.contains(guard) {
                    relied_on.guards.push(*guard);
                }
            }
        }
    }
    // A field an accessor's node addresses is its arm's application's.
    for application in &graph.arm_applications {
        if !relied_on.applications.contains(application) {
            relied_on.applications.push(*application);
        }
    }
    for (index, scope_plan) in plan.scopes.iter().enumerate().skip(1) {
        if !laid_out[index] {
            continue;
        }
        for application in [scope_plan.receiver, scope_plan.entered]
            .into_iter()
            .flatten()
        {
            if !relied_on.applications.contains(&application) {
                relied_on.applications.push(application);
            }
        }
        let body = (
            scopes[index].snapshot.code_id,
            scopes[index].snapshot.feedback_base,
        );
        if !relied_on.bodies.contains(&body) {
            relied_on.bodies.push(body);
        }
    }
    for block in eliminated {
        let snapshot = tree
            .snapshot_of(block.code_id)
            .expect("internal: the tree holds every eliminated block's snapshot");
        let body = (block.code_id, snapshot.feedback_base);
        if !relied_on.bodies.contains(&body) {
            relied_on.bodies.push(body);
        }
    }
    relied_on
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
        sites: vec![None; code.instructions().len()],
        customization: None,
    }
}

/// Records every site `snapshot`'s speculations rest on as never run, so
/// that under `Speculation::Traps` each is a guard: the passes' tests
/// build every trap the corpus can have.
#[cfg(test)]
pub(crate) fn unrun_every_site(snapshot: &mut snapshot::Snapshot) {
    for facts in snapshot.sites.iter_mut().flatten() {
        facts.executed = false;
        facts.trapped = false;
    }
}

/// Records the site the speculation of `snapshot`'s instruction at
/// `index` rests on as `site`, never run.
#[cfg(test)]
pub(crate) fn unrun_site(snapshot: &mut snapshot::Snapshot, index: usize, site: u32) {
    snapshot.sites[index] = Some(snapshot::SiteFacts {
        site,
        executed: false,
        trapped: false,
    });
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

/// Records the site of `snapshot`'s instruction at `index` as `site`, run
/// and never trapped: a send the plan may inline.
#[cfg(test)]
pub(crate) fn ran_site(snapshot: &mut snapshot::Snapshot, index: usize, site: u32) {
    snapshot.sites[index] = Some(snapshot::SiteFacts {
        site,
        executed: true,
        trapped: false,
    });
}

/// What a cache entry of the passes' tests records of a receiver: the
/// guard `guard`, of the application `application`, whose own slots
/// start at `own_slot_base` at its own level and its superclass's -- the
/// same application again, so that a `super` send of one of its methods
/// finds a cached arm.
#[cfg(test)]
pub(crate) fn test_receiver(
    guard: u32,
    application: u32,
    own_slot_base: u32,
) -> tree::ReceiverFacts {
    let application = crate::heap::ApplicationIndex::from_raw(application);
    tree::ReceiverFacts {
        dispatch_id: crate::mixin::DispatchId::from_counter(
            crate::mixin::DispatchId::FIRST_MINTED + guard,
        ),
        application,
        applications: vec![application, application],
        own_slot_bases: vec![own_slot_base, own_slot_base],
    }
}

/// A site of `test_tree`: its body's code id, its number and its arms.
#[cfg(test)]
pub(crate) type TestSite = (
    crate::heap::CodeId,
    u32,
    Vec<(tree::ReceiverFacts, tree::CalleeFacts)>,
);

/// The tree of `root` whose sites hold `sites` -- each the body's code
/// id, the site's number and its arms -- over the method bodies
/// `callees`, as the passes' tests make one by hand.
#[cfg(test)]
pub(crate) fn test_tree(
    root: snapshot::Snapshot,
    sites: Vec<TestSite>,
    callees: Vec<snapshot::Snapshot>,
) -> tree::SnapshotTree {
    tree::SnapshotTree {
        root,
        sites: sites
            .into_iter()
            .map(|(code_id, site, arms)| {
                (
                    (code_id, site),
                    tree::SiteCandidates {
                        site,
                        megamorphic: false,
                        private: false,
                        arms,
                    },
                )
            })
            .collect(),
        blocks: std::collections::HashMap::new(),
        callees: callees
            .into_iter()
            .map(|callee| (callee.code_id, callee))
            .collect(),
        budgets: std::collections::HashMap::new(),
        root_receiver: None,
        root_entered: None,
    }
}

/// The tree of `selector`'s body on the instance side of the class
/// `source` declares, taken after one run of its `run`, so that the
/// caches hold that run's feedback.
#[cfg(test)]
pub(crate) fn tree_of_run(source: &str, selector: &str) -> tree::SnapshotTree {
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

    let module = ns_syntax::parse(source).expect("parse");
    let resolved = crate::resolve(module, source.into(), SourceOrigin::File("tree.ns".into()))
        .unwrap_or_else(|_| panic!("the module resolves"));
    let mut interpreter = Interpreter::bootstrap_with(Lowering::Optimized);
    // The run's feedback is what the tree reads: the cache stress mode
    // would vacate it at the run's safepoints.
    #[cfg(debug_assertions)]
    interpreter.set_stress_caches(false);
    let class = interpreter.load_module(&resolved);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    expect_ok(
        interpreter.send(&instance, Symbol::intern("run"), &[]),
        "run",
    );
    interpreter.enter(|mutator| {
        let code = crate::interpreter::method_code(
            mutator,
            &class,
            MethodSide::Instance,
            Symbol::intern(selector),
        )
        .expect("the method");
        let base = mutator.caches.known_base(&code).expect("it ran");
        tree::SnapshotTree::take(
            &code,
            &mut mutator.heap,
            &mutator.caches,
            base,
            None,
            &plan::Limits::STANDARD,
        )
    })
}

/// A tree over every body of `corpus_of_bodies()` as its root, with
/// synthetic feedback: each send of each body walked a site whose cache
/// recorded a callee of the corpus that takes as many arguments -- one,
/// or for every third site two, and for every fifth of at most two
/// arguments two arms always inlined beside them: a unary one's slot
/// getter and `class` or a lazy slot's getter, a one-argument one's
/// setter and `==` or an `Array`'s `at:` and a `SmallInteger`'s `+`, and
/// a two-argument one's `at:put:` of an `Array` and of a `ByteArray` --
/// each its own guard's, all of one application, so that a `super` or
/// lexical send in an inlined scope is taken without a test; a
/// `value`-family send a closure's leaf, as a send to a block records it,
/// every block a walked body makes walked as a callee is; and every
/// seventh site left never run; walked from the root three callees deep. The inlining passes' sweeps
/// hold every scope, return, type-case and trap such trees make to their
/// checks.
#[cfg(test)]
pub(crate) fn inlining_corpus() -> Vec<tree::SnapshotTree> {
    use std::collections::{HashMap, HashSet};

    use crate::builtins::primitive::{
        BlockPrimitive, BytePrimitive, CollectionPrimitive, IntegerPrimitive, ObjectPrimitive,
        Primitive,
    };
    use crate::bytecode::CodeKind;

    // The mark of a `value`-family send, whose one arm is a closure's leaf.
    const BLOCK_VALUE: u16 = u16::MAX;
    use crate::heap::CodeId;
    use crate::mixin::AccessorKind;
    use snapshot::Snapshot;
    use tree::CalleeFacts;

    let bodies = corpus_of_bodies();
    // A lazy slot's getter, its own slot 0 filled with 1 when empty: a
    // callee the plan always inlines as a load and a test.
    let mut lazy_getter = test_snapshot(vec![
        crate::bytecode::Instruction::ReturnSlotIfFilled { own_index: 0 },
        crate::bytecode::Instruction::LoadLiteral {
            destination: 13,
            literal: crate::bytecode::Literal::SmallInteger(1),
        },
        crate::bytecode::Instruction::InitializeSlot {
            source: 13,
            own_index: 0,
        },
        crate::bytecode::Instruction::Return { register: 13 },
    ]);
    lazy_getter.code_id = CodeId::from_raw(u32::MAX - 1);
    let by_id: HashMap<CodeId, &Snapshot> =
        bodies.iter().map(|body| (body.code_id, body)).collect();
    let mut methods: HashMap<u16, Vec<CodeId>> = HashMap::new();
    for body in &bodies {
        if body.facts.kind == CodeKind::Method {
            methods
                .entry(body.facts.argument_count)
                .or_default()
                .push(body.code_id);
        }
    }
    let raw = |code_id: CodeId| {
        u32::try_from(code_id.index()).expect("internal: a test's code ids are few")
    };
    // Each body's sends, each with the callees its synthetic cache
    // recorded, and the body with those sites run.
    let fed = |body: &Snapshot| {
        let mut fed = body.clone();
        let mut sites = Vec::new();
        for (index, instruction) in body.instructions.iter().enumerate() {
            let Some((_, sources)) = instruction.argument_slots() else {
                continue;
            };
            let Some(facts) = body.sites[index] else {
                continue;
            };
            if matches!(
                instruction,
                snapshot::SnapshotInstruction::SendMirror { .. }
                    | snapshot::SnapshotInstruction::InitializeSuperclass { .. }
                    | snapshot::SnapshotInstruction::InitializeAppliedLevel { .. }
            ) {
                continue;
            }
            let seed = raw(body.code_id) * 31 + facts.site;
            if seed % 7 == 6 {
                continue;
            }
            let arity = u16::try_from(sources.len()).expect("a send's arguments fit a u16");
            // A `value`-family send reached a closure's leaf, as every send
            // of one to a block records it: the plan inlines the block
            // where the receiver can only be one the body makes.
            let block_value = matches!(
                instruction,
                snapshot::SnapshotInstruction::SendPublic { selector, .. }
                    if ["value", "value:", "value:value:", "value:value:value:"]
                        .contains(&selector.as_str())
            );
            if block_value {
                ran_site(&mut fed, index, facts.site);
                sites.push((facts.site, Vec::new(), Some((BLOCK_VALUE, false))));
                continue;
            }
            let Some(pool) = methods.get(&arity) else {
                continue;
            };
            let pick = |turn: u32| pool[(seed + turn) as usize % pool.len()];
            let mut arms = vec![pick(0)];
            if seed % 3 == 0 {
                arms.push(pick(1));
            }
            ran_site(&mut fed, index, facts.site);
            sites.push((
                facts.site,
                arms,
                (seed % 5 == 0).then_some((arity, seed % 2 == 0)),
            ));
        }
        (fed, sites)
    };
    bodies
        .iter()
        .map(|root| {
            let (fed_root, root_sites) = fed(root);
            let mut tree_sites = Vec::new();
            let mut callees: Vec<Snapshot> = Vec::new();
            let mut walked: HashSet<CodeId> = HashSet::from([root.code_id]);
            let mut pending = vec![(root.code_id, root_sites, 0)];
            let mut blocks = HashMap::new();
            while let Some((code_id, sites, depth)) = pending.pop() {
                // Its blocks, walked as its own sends are.
                for instruction in &by_id[&code_id].instructions {
                    if let snapshot::SnapshotInstruction::MakeClosure { block, .. } = instruction {
                        if walked.insert(*block) {
                            let (fed_block, block_sites) = fed(by_id[block]);
                            blocks.insert(*block, fed_block);
                            pending.push((*block, block_sites, depth));
                        }
                    }
                }
                for (site, arms, accessor) in sites {
                    let mut facts = Vec::new();
                    for (turn, callee) in (0_u32..).zip(&arms) {
                        let (fed_callee, callee_sites) = fed(by_id[callee]);
                        if depth < 3 && walked.insert(*callee) {
                            pending.push((*callee, callee_sites, depth + 1));
                            callees.push(fed_callee.clone());
                        }
                        facts.push((
                            test_receiver(raw(*callee) * 4 + turn, 0, 2),
                            CalleeFacts::Method {
                                snapshot: Box::new(fed_callee),
                                level: 0,
                            },
                        ));
                    }
                    let slot = |kind| CalleeFacts::Slot {
                        field: 2,
                        kind,
                        level: crate::heap::ApplicationIndex::from_raw(0),
                    };
                    let always = match accessor {
                        Some((BLOCK_VALUE, _)) => vec![CalleeFacts::Primitive(Primitive::Block(
                            BlockPrimitive::Value,
                        ))],
                        Some((0, true)) => vec![
                            slot(AccessorKind::Getter),
                            CalleeFacts::Primitive(Primitive::Object(ObjectPrimitive::Class)),
                        ],
                        Some((0, false)) => vec![
                            slot(AccessorKind::Getter),
                            CalleeFacts::Method {
                                snapshot: Box::new(lazy_getter.clone()),
                                level: 0,
                            },
                        ],
                        Some((1, true)) => vec![
                            slot(AccessorKind::Setter),
                            CalleeFacts::Primitive(Primitive::Object(ObjectPrimitive::Identical)),
                        ],
                        Some((1, false)) => vec![
                            CalleeFacts::Primitive(Primitive::Collection(
                                CollectionPrimitive::ElementAt,
                            )),
                            CalleeFacts::Primitive(Primitive::Integer(IntegerPrimitive::Add)),
                        ],
                        Some((2, _)) => vec![
                            CalleeFacts::Primitive(Primitive::Collection(
                                CollectionPrimitive::ElementAtPut,
                            )),
                            CalleeFacts::Primitive(Primitive::Bytes(BytePrimitive::ElementAtPut)),
                        ],
                        _ => Vec::new(),
                    };
                    for (turn, callee) in (0_u32..).zip(always) {
                        facts.push((
                            test_receiver(raw(code_id) * 4 + 3 + turn * (1 << 20), 0, 2),
                            callee,
                        ));
                    }
                    tree_sites.push((code_id, site, facts));
                }
            }
            // An arm whose callee lies past the walk is snapshotted but not
            // walked: the plan takes it, and its own sends trap.
            for (_, _, arms) in &tree_sites {
                for (_, callee) in arms {
                    if let CalleeFacts::Method { snapshot, .. } = callee {
                        if walked.insert(snapshot.code_id) {
                            callees.push((**snapshot).clone());
                        }
                    }
                }
            }
            let mut tree = test_tree(fed_root, tree_sites, callees);
            tree.blocks = blocks;
            tree
        })
        .collect()
}

/// The limits the inlining sweeps plan under: room for more of the
/// corpus's callees than the standard method limit leaves, under the
/// standard compile limit.
#[cfg(test)]
pub(crate) const SWEEP_LIMITS: plan::Limits = plan::Limits {
    method: 200,
    ..plan::Limits::STANDARD
};

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
                    None,
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
            super::snapshot::Snapshot::take(&code, &mut mutator.heap, &mutator.caches, base, None)
        });
        let taken = snapshot
            .fallback_taken
            .iter()
            .filter(|taken| **taken)
            .count();
        assert_eq!(taken, 9, "the nine float operations, not the counter's two");
        let graph = super::build::build(&snapshot, super::Speculation::Traps);
        let integer_paths = graph
            .blocks
            .iter()
            .flat_map(|block| &block.nodes)
            .filter(|node| matches!(node, Node::IntegerBinary { .. }))
            .count();
        assert_eq!(integer_paths, 2, "`iteration < 50` and `iteration + 1`");
        let roles =
            crate::jit::macro_assembler::Roles::of(crate::jit::macro_assembler::Abi::host());
        let (allocation, _) = super::metadata::allocate_and_describe(&graph, &roles);
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
                Node::IntegerBinary {
                    fallback: Some(fallback),
                    ..
                } => Some(allocation.positions.end_definition(*fallback)),
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
                // A trap's write-back lies on its cold way alone.
                .filter(|point| !matches!(point.kind, super::allocate::PointKind::Trap { .. }))
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
