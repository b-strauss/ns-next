//! The optimizing compiler's input when it inlines: the snapshot of the
//! body it compiles and, below it, what the caches recorded at each of its
//! sends -- the receivers seen, in the cache's order, and the callee each
//! reached, snapshotted in turn -- and a snapshot of every block literal
//! of a snapshotted body (`docs/design/execution.md`, "Planned: the
//! optimizing JIT", "Inlining"). Taken at the run loop's safepoint, as
//! the snapshot is, so that the inlining plan and the graph read nothing
//! the interpreter can change.
//!
//! The tree is taken eagerly, down to whatever an inlining plan with the
//! callee limit `weight_limit` could take: a callee heavier than it is
//! recorded as `CalleeFacts::Other` and not walked into, and every other
//! body is walked once, whatever number of sites reach it, so that
//! recursion ends.
#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the inlining plan reads the tree; no compile takes it yet"
    )
)]

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::builtins::primitive::Primitive;
use crate::bytecode::{CompiledCode, Instruction};
use crate::heap::{ApplicationIndex, CodeId, Heap};
use crate::interpreter::inline_cache::{
    CacheEntry, FeedbackBase, InlineCaches, SiteEntries, Target,
};
use crate::mixin::{AccessorKind, DispatchId, MethodFastPath};

use super::snapshot::{Customization, Snapshot};

/// What a cache entry's guard says of the receivers it was filled for:
/// the guard and the application it names, with the own-slot base of
/// each level of that application's chain, the application's own first,
/// so that an inlined method entered at any level -- the entry's `level`
/// steps up -- reads its receiver's own slots at fixed offsets. For a
/// public send the application is the receiver's; for a `super` or a
/// private lexical send, whose entries are keyed by the class the send
/// starts its lookup at, it is that class's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReceiverFacts {
    pub(crate) dispatch_id: DispatchId,
    pub(crate) application: ApplicationIndex,
    pub(crate) own_slot_bases: Vec<u32>,
}

/// What a cache entry's target says the send reached.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum CalleeFacts {
    /// A method with an activation, found `level` superclass steps up
    /// from the chain's start, snapshotted against its family's range.
    Method { snapshot: Box<Snapshot>, level: u16 },
    /// A slot accessor of the application `level`, `field` the absolute
    /// index of its slot.
    Slot {
        field: u32,
        kind: AccessorKind,
        level: ApplicationIndex,
    },
    /// A method a send answers without an activation, or activates only
    /// when its fast path declines.
    FastPath(MethodFastPath),
    /// A primitive.
    Primitive(Primitive),
    /// What no inlining plan takes: a method heavier than the tree's
    /// limit, a nested class's accessor, a factory.
    Other,
}

/// What a site's cache recorded, as candidates for inlining: one arm per
/// cached receiver in the cache's order -- the order the receivers were
/// first seen -- and none for a megamorphic site, which records none, nor
/// for a vacant one. A site with an entry whose receiver no registered
/// application carries -- which a live entry never has, since a
/// collection vacates every entry naming an id it gives back -- is
/// recorded as megamorphic, so that no plan takes a type-case that would
/// leave out a receiver the site saw.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SiteCandidates {
    pub(crate) site: u32,
    pub(crate) megamorphic: bool,
    pub(crate) arms: Vec<(ReceiverFacts, CalleeFacts)>,
}

/// The compiled body's snapshot and what lies below it: the candidates of
/// every send of every body walked, keyed by the body's code id and the
/// send's call-site number, and the snapshot of every block literal of a
/// walked body, by its code id.
#[derive(Debug, Clone)]
pub(crate) struct SnapshotTree {
    pub(crate) root: Snapshot,
    pub(crate) sites: HashMap<(CodeId, u32), SiteCandidates>,
    pub(crate) blocks: HashMap<CodeId, Snapshot>,
}

/// The walk's state: the tree so far, the bodies still to walk with the
/// range their sends' cells lie in, the code ids of the bodies walked or
/// queued, and each method callee's snapshot, taken once.
struct Walk<'caches> {
    caches: &'caches InlineCaches,
    weight_limit: u32,
    tree: SnapshotTree,
    work: Vec<(Rc<CompiledCode>, FeedbackBase)>,
    walked: HashSet<CodeId>,
    callees: HashMap<CodeId, Snapshot>,
}

impl SnapshotTree {
    /// The tree of `code`, its sends' cells found from `feedback_base`,
    /// for the copy `customization` names: the root's snapshot is
    /// `Snapshot::take`'s, every callee is snapshotted against the range
    /// its entry names and every block against its home's, and a method
    /// callee of more than `weight_limit` instructions -- the measure the
    /// inlining plan's weight is to replace -- is `CalleeFacts::Other`.
    /// Every body and block is registered with `heap` if it was not yet.
    pub(crate) fn take(
        code: &Rc<CompiledCode>,
        heap: &mut Heap,
        caches: &InlineCaches,
        feedback_base: FeedbackBase,
        customization: Option<Customization>,
        weight_limit: u32,
    ) -> SnapshotTree {
        let root = Snapshot::take(code, heap, caches, feedback_base, customization);
        let mut walk = Walk {
            caches,
            weight_limit,
            walked: HashSet::from([root.code_id]),
            tree: SnapshotTree {
                root,
                sites: HashMap::new(),
                blocks: HashMap::new(),
            },
            work: vec![(Rc::clone(code), feedback_base)],
            callees: HashMap::new(),
        };
        while let Some((body, base)) = walk.work.pop() {
            walk.body(&body, base, heap);
        }
        walk.tree
    }
}

impl Walk<'_> {
    /// Records the candidates of each send of `body`, whose cells lie in
    /// the range at `base`, and snapshots each of its block literals,
    /// queueing every body below it not walked yet.
    fn body(&mut self, body: &Rc<CompiledCode>, base: FeedbackBase, heap: &mut Heap) {
        let body_id = heap.code_id(body);
        for instruction in body.instructions() {
            if let Instruction::MakeClosure { code: block, .. } = &instruction {
                let block_id = heap.code_id(block);
                if self.walked.insert(block_id) {
                    // A block's sends are sites of its home's family.
                    let snapshot = Snapshot::take(block, heap, self.caches, base, None);
                    self.tree.blocks.insert(block_id, snapshot);
                    self.work.push((Rc::clone(block), base));
                }
            } else if let Some(site) = instruction.call_site() {
                let arms = match self.caches.entries_of(base, body.family_size, site) {
                    SiteEntries::Vacant => Some(Vec::new()),
                    SiteEntries::Megamorphic => None,
                    SiteEntries::Monomorphic(entry) => self.arm(entry, heap).map(|arm| vec![arm]),
                    SiteEntries::Polymorphic(slots) => slots
                        .iter()
                        .filter(|slot| !matches!(slot.target, Target::Vacant))
                        .map(|slot| self.arm(slot, heap))
                        .collect(),
                };
                let candidates = SiteCandidates {
                    site: site.number,
                    megamorphic: arms.is_none(),
                    arms: arms.unwrap_or_default(),
                };
                self.tree.sites.insert((body_id, site.number), candidates);
            }
        }
    }

    /// The arm of `entry`: its receiver and its callee; `None` when no
    /// registered application carries its guard.
    fn arm(&mut self, entry: &CacheEntry, heap: &mut Heap) -> Option<(ReceiverFacts, CalleeFacts)> {
        let receiver = receiver_facts(heap, entry.guard)?;
        let callee = match &entry.target {
            Target::Method {
                method,
                feedback_base,
                level,
                ..
            } => match method.fast_path {
                Some(fast_path) => CalleeFacts::FastPath(fast_path),
                None if method.code.instruction_count() > self.weight_limit as usize => {
                    CalleeFacts::Other
                }
                None => CalleeFacts::Method {
                    snapshot: Box::new(self.callee(&method.code, *feedback_base, heap)),
                    level: *level,
                },
            },
            Target::Slot {
                field, kind, level, ..
            } => CalleeFacts::Slot {
                field: *field,
                kind: *kind,
                level: *level,
            },
            Target::Primitive { primitive, .. } => CalleeFacts::Primitive(*primitive),
            Target::NestedClass { .. }
            | Target::Factory { .. }
            | Target::Vacant
            | Target::Polymorphic { .. }
            | Target::Megamorphic { .. } => CalleeFacts::Other,
        };
        Some((receiver, callee))
    }

    /// The snapshot of the method body `code`, whose family's range is at
    /// `base`, taken once whatever number of sites reach it, the body
    /// queued for the walk.
    fn callee(&mut self, code: &Rc<CompiledCode>, base: FeedbackBase, heap: &mut Heap) -> Snapshot {
        let code_id = heap.code_id(code);
        if let Some(snapshot) = self.callees.get(&code_id) {
            return snapshot.clone();
        }
        let snapshot = Snapshot::take(code, heap, self.caches, base, None);
        self.callees.insert(code_id, snapshot.clone());
        if self.walked.insert(code_id) {
            self.work.push((Rc::clone(code), base));
        }
        snapshot
    }
}

/// What the guard `guard` says of its receivers (`ReceiverFacts`); `None`
/// when no registered application carries it, or a level's own-slot base
/// passes `u32`, which no field index reaches.
fn receiver_facts(heap: &Heap, guard: DispatchId) -> Option<ReceiverFacts> {
    let index = heap.application_of_dispatch_id(guard)?;
    let mut own_slot_bases = Vec::new();
    let mut level = heap.application(index);
    while let Some(application) = level {
        own_slot_bases.push(u32::try_from(application.own_slot_base).ok()?);
        level = application.superclass.clone();
    }
    Some(ReceiverFacts {
        dispatch_id: guard,
        application: index,
        own_slot_bases,
    })
}

#[cfg(test)]
mod tests {
    use indoc::indoc;

    use super::{CalleeFacts, SiteCandidates, SnapshotTree};
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

    /// `source` loaded and its `run` sent once, so that the caches hold
    /// the feedback of one run.
    fn run(source: &str) -> (Interpreter, Root) {
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
        (interpreter, class)
    }

    /// The tree of `selector`'s body on `class`'s instance side, against
    /// its family's range, every callee up to `weight_limit` snapshotted.
    fn tree_of(
        interpreter: &mut Interpreter,
        class: &Root,
        selector: &str,
        weight_limit: u32,
    ) -> SnapshotTree {
        interpreter.enter(|mutator| {
            let code = crate::interpreter::method_code(
                mutator,
                class,
                MethodSide::Instance,
                Symbol::intern(selector),
            )
            .expect("the method");
            let base = mutator.caches.known_base(&code).expect("it ran");
            SnapshotTree::take(
                &code,
                &mut mutator.heap,
                &mutator.caches,
                base,
                None,
                weight_limit,
            )
        })
    }

    /// The candidates of the root's sites that cached something, in the
    /// order of their site numbers.
    fn root_sites_with_arms(tree: &SnapshotTree) -> Vec<&SiteCandidates> {
        let mut sites: Vec<&SiteCandidates> = tree
            .sites
            .iter()
            .filter(|((code_id, _), candidates)| {
                *code_id == tree.root.code_id && !candidates.arms.is_empty()
            })
            .map(|(_, candidates)| candidates)
            .collect();
        sites.sort_by_key(|candidates| candidates.site);
        sites
    }

    /// The name of the mixin of the application `index` names.
    fn mixin_name(interpreter: &mut Interpreter, index: crate::heap::ApplicationIndex) -> Symbol {
        interpreter.enter(|mutator| {
            mutator
                .heap
                .application(index)
                .expect("a live application")
                .mixin
                .name
        })
    }

    const MONOMORPHIC: &str = indoc! {"
        class Monomorphic = () (
          public run = (
            ^self helper: 3
          )
          public helper: number = (
            ^number + 1
          )
        )
    "};

    #[test]
    fn a_monomorphic_site_s_callee_is_snapshotted_with_its_receiver() {
        let (mut interpreter, class) = run(MONOMORPHIC);
        let tree = tree_of(&mut interpreter, &class, "run", 1_000);
        let sites = root_sites_with_arms(&tree);
        let [candidates] = sites.as_slice() else {
            panic!("one site of the root cached its callee");
        };
        assert!(!candidates.megamorphic);
        assert_eq!(candidates.arms.len(), 1);
        let (receiver, callee) = &candidates.arms[0];
        let CalleeFacts::Method { snapshot, level } = callee else {
            panic!("the callee is a method");
        };
        assert_eq!(*level, 0, "the receiver's own class defines it");
        let helper = interpreter.enter(|mutator| {
            let code = crate::interpreter::method_code(
                mutator,
                &class,
                MethodSide::Instance,
                Symbol::intern("helper:"),
            )
            .expect("the method");
            mutator.heap.code_id(&code)
        });
        assert_eq!(snapshot.code_id, helper);
        assert_eq!(
            mixin_name(&mut interpreter, receiver.application),
            Symbol::intern("Monomorphic")
        );
        assert!(
            !receiver.own_slot_bases.is_empty(),
            "a base per level of the receiver's chain"
        );
        // The callee's own sites are in the tree, keyed by its code id.
        assert!(tree.sites.keys().any(|(code_id, _)| *code_id == helper));
    }

    /// Five classes answering `answer`, sent at one site of `probe:`: the
    /// first two of them make it polymorphic, all five megamorphic.
    fn receivers(count: usize) -> String {
        let names = ["A", "B", "C", "D", "E"];
        let classes: String = names
            .iter()
            .enumerate()
            .map(|(index, name)| {
                format!(
                    "  class {name} = () (\n    public answer = (\n      ^{index}\n    )\n  )\n"
                )
            })
            .collect();
        let probes: String = names[..count]
            .iter()
            .map(|name| format!("    probe: {name} new.\n"))
            .collect();
        format!(
            "class Receivers = () (\n{classes}  public probe: object = (\n    ^object answer\n  )\n  public run = (\n{probes}    ^0\n  )\n)\n"
        )
    }

    /// The arms of `probe:`'s one send.
    fn probe_candidates(tree: &SnapshotTree) -> &SiteCandidates {
        let sites: Vec<&SiteCandidates> = tree
            .sites
            .iter()
            .filter(|((code_id, _), _)| *code_id == tree.root.code_id)
            .map(|(_, candidates)| candidates)
            .collect();
        let [candidates] = sites.as_slice() else {
            panic!("probe: has one send");
        };
        candidates
    }

    #[test]
    fn a_polymorphic_site_keeps_its_cache_order() {
        let (mut interpreter, class) = run(&receivers(2));
        let tree = tree_of(&mut interpreter, &class, "probe:", 1_000);
        let candidates = probe_candidates(&tree);
        assert!(!candidates.megamorphic);
        let names: Vec<Symbol> = candidates
            .arms
            .iter()
            .map(|(receiver, _)| mixin_name(&mut interpreter, receiver.application))
            .collect();
        assert_eq!(names, vec![Symbol::intern("A"), Symbol::intern("B")]);
        for (receiver, callee) in &candidates.arms {
            assert_eq!(
                interpreter.application_of_dispatch_id(receiver.dispatch_id),
                Some(receiver.application)
            );
            assert!(matches!(callee, CalleeFacts::Method { .. }));
        }
    }

    #[test]
    fn a_megamorphic_site_has_no_arms() {
        let (mut interpreter, class) = run(&receivers(5));
        let tree = tree_of(&mut interpreter, &class, "probe:", 1_000);
        let candidates = probe_candidates(&tree);
        assert!(candidates.megamorphic);
        assert!(candidates.arms.is_empty());
    }

    /// `twice:`'s body holds a block literal, and `run` sends it at two
    /// sites: the block is snapshotted once, under its own code id.
    const ONE_BLOCK: &str = indoc! {"
        class OneBlock = () (
          public twice: number = (
            ^[:each | each + number] value: number
          )
          public run = (
            ^(self twice: 1) + (self twice: 2)
          )
        )
    "};

    #[test]
    fn a_block_literal_is_snapshotted_once() {
        let (mut interpreter, class) = run(ONE_BLOCK);
        let tree = tree_of(&mut interpreter, &class, "run", 1_000);
        let callees: Vec<_> = root_sites_with_arms(&tree)
            .into_iter()
            .filter_map(|candidates| match &candidates.arms[..] {
                [(_, CalleeFacts::Method { snapshot, .. })] => Some(snapshot.code_id),
                _ => None,
            })
            .collect();
        assert_eq!(callees.len(), 2, "both sends snapshotted `twice:`");
        assert_eq!(callees[0], callees[1]);
        assert_eq!(tree.blocks.len(), 1, "one block literal, snapshotted once");
        let (block_id, block) = tree.blocks.iter().next().expect("the block");
        assert_eq!(block.code_id, *block_id);
        assert_eq!(block.facts.kind, crate::bytecode::CodeKind::Block);
    }

    #[test]
    fn a_callee_past_the_limit_is_other() {
        let (mut interpreter, class) = run(MONOMORPHIC);
        let tree = tree_of(&mut interpreter, &class, "run", 1);
        let sites = root_sites_with_arms(&tree);
        let [candidates] = sites.as_slice() else {
            panic!("one site of the root cached its callee");
        };
        assert!(matches!(candidates.arms[..], [(_, CalleeFacts::Other)]));
    }
}
