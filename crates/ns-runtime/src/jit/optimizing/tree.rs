//! The optimizing compiler's input when it inlines: the snapshot of the
//! body it compiles and, below it, what the caches recorded at each of its
//! sends -- the receivers seen, in the cache's order, and the callee each
//! reached, snapshotted in turn -- and a snapshot of every block literal
//! of a snapshotted body (`docs/design/execution.md`, "Planned: the
//! optimizing JIT", "Inlining"). Taken at the run loop's safepoint, as
//! the snapshot is, so that the inlining plan and the graph read nothing
//! the interpreter can change.
//!
//! The tree is taken down to whatever the inlining plan under `limits`
//! could take, and no further: a method callee heavier than the most its
//! limit can grow to at its site, or whose chain's accumulated weight
//! would pass the compile's limit, or whose frame would pass the
//! register ceiling where it lies, is recorded as `CalleeFacts::Other`
//! and not walked into. The bodies are walked lightest chain first, so
//! that a body reached by several chains is walked once, from its
//! lightest, and recursion ends.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::rc::Rc;

use crate::builtins::primitive::Primitive;
use crate::bytecode::{CallSite, CompiledCode, Instruction, HEADER_REGISTERS};
use crate::heap::{ApplicationIndex, CodeId, Heap};
use crate::interpreter::inline_cache::{
    CacheEntry, FeedbackBase, InlineCaches, SiteEntries, Target,
};
use crate::interpreter::resolve::Chain;
use crate::mixin::{AccessorKind, DispatchId, MethodFastPath, MixinMethod};

use super::plan::{weight_with, Limits};
use super::snapshot::{Customization, Snapshot};

/// What a cache entry's guard says of the receivers it was filled for:
/// the guard and the application it names, with that application's
/// chain -- its own index first, then each superclass's -- and the
/// own-slot base of each level of it, so that an inlined method entered
/// at any level -- the entry's `level` steps up -- reads its receiver's
/// own slots at fixed offsets. For a public send the application is the
/// receiver's; for a `super` or a private lexical send, whose entries
/// are keyed by the class the send starts its lookup at, it is that
/// class's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReceiverFacts {
    pub(crate) dispatch_id: DispatchId,
    pub(crate) application: ApplicationIndex,
    pub(crate) applications: Vec<ApplicationIndex>,
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
    /// What no inlining plan takes: a method past what the plan's limits
    /// allow where it is sent, a nested class's accessor, a factory, and
    /// every target a `doesNotUnderstand:` lookup found, whose activation
    /// takes the message the send reifies rather than the send's
    /// arguments.
    Other,
}

/// What a site's cache recorded, as candidates for inlining: one arm per
/// cached receiver in the cache's order -- the order the receivers were
/// first seen -- and none for a megamorphic site, which records none, nor
/// for a vacant one. A site with an entry whose receiver no registered
/// application carries -- which a live entry never has, since a
/// collection vacates every entry naming an id it gives back -- is
/// recorded as megamorphic, so that no plan takes a type-case that would
/// leave out a receiver the site saw. `private` is a lexical site's §5.9
/// decision, which says whether its guard is N's application or the
/// receiver's; false for every other site.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SiteCandidates {
    pub(crate) site: u32,
    pub(crate) megamorphic: bool,
    pub(crate) private: bool,
    pub(crate) arms: Vec<(ReceiverFacts, CalleeFacts)>,
}

/// The compiled body's snapshot and what lies below it: the candidates of
/// every send of every body walked, keyed by the body's code id and the
/// send's call-site number; the snapshot of every block literal of a
/// walked body and of every method callee, by its code id; the share of
/// its last arming each of them whose row is armed spent
/// (`jit::budget_share_of`), the estimate of how often it ran (`plan`'s
/// frequency); and, for a
/// customized root, what its key's receiver and entered application are.
#[derive(Debug, Clone)]
pub(crate) struct SnapshotTree {
    pub(crate) root: Snapshot,
    pub(crate) sites: HashMap<(CodeId, u32), SiteCandidates>,
    pub(crate) blocks: HashMap<CodeId, Snapshot>,
    pub(crate) callees: HashMap<CodeId, Snapshot>,
    pub(crate) budgets: HashMap<CodeId, f64>,
    pub(crate) root_receiver: Option<ReceiverFacts>,
    pub(crate) root_entered: Option<ReceiverFacts>,
}

impl SnapshotTree {
    /// The tree of `root` alone, with no site recorded: what a compile of
    /// a body that inlines nothing plans on.
    #[cfg(test)]
    pub(crate) fn alone(root: Snapshot) -> SnapshotTree {
        SnapshotTree {
            root,
            sites: HashMap::new(),
            blocks: HashMap::new(),
            callees: HashMap::new(),
            budgets: HashMap::new(),
            root_receiver: None,
            root_entered: None,
        }
    }

    /// The snapshot of the body `code_id` names: the root's, a method
    /// callee's or a block's.
    pub(crate) fn snapshot_of(&self, code_id: CodeId) -> Option<&Snapshot> {
        if code_id == self.root.code_id {
            return Some(&self.root);
        }
        self.callees
            .get(&code_id)
            .or_else(|| self.blocks.get(&code_id))
    }
}

/// A body still to walk: the weight of the lightest chain of bodies that
/// reaches it, itself included, and the register its frame would start
/// at in the physical frame. The queue orders it by the weight, and of
/// two as heavy by the order they were queued in, so that the walk is
/// deterministic.
struct Pending {
    chain_weight: u32,
    offset: u32,
    body: Rc<CompiledCode>,
    base: FeedbackBase,
}

/// The walk's state: the tree so far, the bodies still to walk, lightest
/// chain first, the code ids of the bodies walked or queued, and each
/// method callee's snapshot, taken once.
struct Walk<'caches> {
    caches: &'caches InlineCaches,
    limits: Limits,
    tree: SnapshotTree,
    work: BinaryHeap<Reverse<(u32, u64)>>,
    pending: HashMap<u64, Pending>,
    queued: u64,
    walked: HashSet<CodeId>,
}

impl SnapshotTree {
    /// The tree of `code`, its sends' cells found from `feedback_base`,
    /// for the copy `customization` names: the root's snapshot is
    /// `Snapshot::take`'s, every callee is snapshotted against the range
    /// its entry names and every block against its home's, and the walk
    /// is bounded by what a plan under `limits` could inline (the
    /// module's comment). Every body and block is registered with `heap`
    /// if it was not yet.
    pub(crate) fn take(
        code: &Rc<CompiledCode>,
        heap: &mut Heap,
        caches: &InlineCaches,
        feedback_base: FeedbackBase,
        customization: Option<Customization>,
        limits: &Limits,
    ) -> SnapshotTree {
        let root = Snapshot::take(code, heap, caches, feedback_base, customization);
        let root_weight = cached_weight(caches, &root);
        let root_receiver = customization
            .and_then(|customization| receiver_facts(heap, customization.key.receiver));
        let root_entered =
            customization.and_then(|customization| receiver_facts(heap, customization.key.entered));
        let mut walk = Walk {
            caches,
            limits: *limits,
            walked: HashSet::from([root.code_id]),
            tree: SnapshotTree {
                root,
                sites: HashMap::new(),
                blocks: HashMap::new(),
                callees: HashMap::new(),
                budgets: HashMap::new(),
                root_receiver,
                root_entered,
            },
            work: BinaryHeap::new(),
            pending: HashMap::new(),
            queued: 0,
        };
        walk.queue(Rc::clone(code), feedback_base, root_weight, 0);
        while let Some(Reverse((_, order))) = walk.work.pop() {
            let pending = walk
                .pending
                .remove(&order)
                .expect("internal: every queued body is pending");
            walk.body(&pending, heap);
        }
        walk.tree
    }
}

impl Walk<'_> {
    fn queue(
        &mut self,
        body: Rc<CompiledCode>,
        base: FeedbackBase,
        chain_weight: u32,
        offset: u32,
    ) {
        let order = self.queued;
        self.queued += 1;
        self.work.push(Reverse((chain_weight, order)));
        self.pending.insert(
            order,
            Pending {
                chain_weight,
                offset,
                body,
                base,
            },
        );
    }

    /// Records the candidates of each send of the pending body and
    /// snapshots each of its block literals, queueing every body below it
    /// not walked yet that a plan could still inline.
    fn body(&mut self, pending: &Pending, heap: &mut Heap) {
        let body = &pending.body;
        let base = pending.base;
        let body_id = heap.code_id(body);
        let instructions = body.instructions();
        let block_registers = block_registers(body, &instructions);
        for instruction in &instructions {
            if let Instruction::MakeClosure { code: block, .. } = instruction {
                let block_id = heap.code_id(block);
                if self.walked.insert(block_id) {
                    // A block's sends are sites of its home's family.
                    let snapshot = Snapshot::take(block, heap, self.caches, base, None);
                    let block_weight = pending
                        .chain_weight
                        .saturating_add(cached_weight(self.caches, &snapshot));
                    self.record_budget(block, block_id, heap);
                    self.tree.blocks.insert(block_id, snapshot);
                    if block_weight <= self.limits.compile {
                        self.queue(Rc::clone(block), base, block_weight, pending.offset);
                    }
                }
            } else if let Some(site) = instruction.call_site() {
                let (argument_base, sources) = instruction
                    .argument_slots()
                    .expect("internal: a send names its argument slots");
                let block_arguments = sources
                    .iter()
                    .filter(|source| block_registers.contains(source))
                    .count();
                let send_site = Site {
                    chain_weight: pending.chain_weight,
                    offset: pending.offset + u32::from(argument_base),
                    block_arguments: u32::try_from(block_arguments)
                        .expect("internal: a send takes at most 255 arguments"),
                };
                let entries = self.caches.entries_of(base, body.family_size, site);
                let private = match &entries {
                    SiteEntries::Monomorphic(entry) => target_is_private(&entry.target),
                    SiteEntries::Polymorphic(slots) => {
                        slots.iter().any(|slot| target_is_private(&slot.target))
                    }
                    SiteEntries::Megamorphic | SiteEntries::Vacant => false,
                };
                let arms = match entries {
                    SiteEntries::Vacant => Some(Vec::new()),
                    SiteEntries::Megamorphic => None,
                    SiteEntries::Monomorphic(entry) => {
                        self.arm(entry, &send_site, heap).map(|arm| vec![arm])
                    }
                    SiteEntries::Polymorphic(slots) => slots
                        .iter()
                        .filter(|slot| !matches!(slot.target, Target::Vacant))
                        .map(|slot| self.arm(slot, &send_site, heap))
                        .collect(),
                };
                let candidates = SiteCandidates {
                    site: site.number,
                    megamorphic: arms.is_none(),
                    private,
                    arms: arms.unwrap_or_default(),
                };
                self.tree.sites.insert((body_id, site.number), candidates);
            }
        }
    }

    /// The arm of `entry` at `send_site`: its receiver and its callee;
    /// `None` when no registered application carries its guard.
    fn arm(
        &mut self,
        entry: &CacheEntry,
        send_site: &Site,
        heap: &mut Heap,
    ) -> Option<(ReceiverFacts, CalleeFacts)> {
        let receiver = receiver_facts(heap, entry.guard)?;
        let callee = match &entry.target {
            Target::Method { chain, .. }
            | Target::Slot { chain, .. }
            | Target::Primitive { chain, .. }
                if does_not_understand(*chain) =>
            {
                CalleeFacts::Other
            }
            Target::Method {
                method,
                feedback_base,
                level,
                ..
            } => match method.fast_path {
                Some(fast_path) => CalleeFacts::FastPath(fast_path),
                None => self.method(method, *feedback_base, *level, send_site, heap),
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

    /// The method callee `method`, whose family's range is at `base`, sent
    /// at `send_site`: its snapshot, taken once whatever number of sites reach
    /// it, and the body queued for the walk -- or `Other` when no plan
    /// could inline it there (the module's comment). Its body is
    /// registered with the method, so that an exit inside its inlined
    /// scope names the frame it materializes there by its method
    /// (`jit::materialize`).
    fn method(
        &mut self,
        method: &Rc<MixinMethod>,
        base: FeedbackBase,
        level: u16,
        send_site: &Site,
        heap: &mut Heap,
    ) -> CalleeFacts {
        let code = &method.code;
        let code_id = heap.code_id_of_method(method);
        let snapshot = match self.tree.callees.get(&code_id) {
            Some(snapshot) => snapshot.clone(),
            None => {
                let snapshot = Snapshot::take(code, heap, self.caches, base, None);
                self.record_budget(code, code_id, heap);
                self.tree.callees.insert(code_id, snapshot.clone());
                snapshot
            }
        };
        let callee_weight = cached_weight(self.caches, &snapshot);
        let limit = self.limits.method.saturating_add(
            self.limits
                .block_argument_bonus
                .saturating_mul(send_site.block_arguments),
        );
        let chain_weight = send_site.chain_weight.saturating_add(callee_weight);
        let extent = send_site
            .offset
            .saturating_add(u32::try_from(snapshot.register_count()).unwrap_or(u32::MAX));
        if callee_weight > limit
            || chain_weight > self.limits.compile
            || extent > u32::from(u16::MAX)
        {
            return CalleeFacts::Other;
        }
        if self.walked.insert(code_id) {
            self.queue(Rc::clone(code), base, chain_weight, send_site.offset);
        }
        CalleeFacts::Method {
            snapshot: Box::new(snapshot),
            level,
        }
    }

    /// Records what is left of `code`'s optimization budget, if its row
    /// is armed: how often its baseline code ran.
    #[cfg_attr(
        not(ns_jit),
        expect(unused_variables, reason = "no budget without a JIT")
    )]
    fn record_budget(&mut self, code: &Rc<CompiledCode>, code_id: CodeId, heap: &Heap) {
        #[cfg(ns_jit)]
        if let Some(share) = crate::jit::budget_share_of(heap, code) {
            self.tree.budgets.insert(code_id, share);
        }
    }
}

/// Where a send lies in the walk: the accumulated weight of its body's
/// lightest chain, the register its callee's frame would start at, and
/// how many of its arguments may be a block made in the compile.
struct Site {
    chain_weight: u32,
    offset: u32,
    block_arguments: u32,
}

/// The registers of `body` that may hold a block the compile makes: each
/// a `MakeClosure` writes, and every parameter and copied value, which
/// an inlining sender may have filled with one -- an over-estimate, so
/// that the walk never leaves out a callee the bonus for a block
/// argument would let the plan take.
fn block_registers(body: &CompiledCode, instructions: &[Instruction]) -> HashSet<u16> {
    let mut registers: HashSet<u16> = instructions
        .iter()
        .filter_map(|instruction| match instruction {
            Instruction::MakeClosure { destination, .. } => Some(*destination),
            _ => None,
        })
        .collect();
    registers.extend(0..body.argument_count);
    let copied_start = body.argument_count.saturating_add(HEADER_REGISTERS);
    registers.extend(copied_start..copied_start.saturating_add(body.copied_count));
    registers
}

/// `snapshot`'s weight as the plan weighs it (`plan::weight_in`): a send
/// whose site's cache reached only slot accessors and method fast paths
/// is cheap.
fn cached_weight(caches: &InlineCaches, snapshot: &Snapshot) -> u32 {
    let cheap = |entry: &CacheEntry| match &entry.target {
        Target::Method { method, chain, .. } => {
            method.fast_path.is_some() && !does_not_understand(*chain)
        }
        Target::Slot { chain, .. } => !does_not_understand(*chain),
        _ => false,
    };
    weight_with(snapshot, |site| {
        match caches.entries_of(
            snapshot.feedback_base,
            snapshot.facts.family_size,
            CallSite { number: site },
        ) {
            SiteEntries::Monomorphic(entry) => cheap(entry),
            SiteEntries::Polymorphic(slots) => {
                let live: Vec<&CacheEntry> = slots
                    .iter()
                    .filter(|slot| !matches!(slot.target, Target::Vacant))
                    .collect();
                !live.is_empty() && live.into_iter().all(cheap)
            }
            SiteEntries::Megamorphic | SiteEntries::Vacant => false,
        }
    })
}

/// Whether `chain` is a `doesNotUnderstand:` lookup's, whose target the
/// plan never takes.
fn does_not_understand(chain: Chain) -> bool {
    matches!(
        chain,
        Chain::DoesNotUnderstand | Chain::SuperDoesNotUnderstand
    )
}

/// Whether `target` stands at a private lexical site (§5.9), whose guard
/// is N's application.
fn target_is_private(target: &Target) -> bool {
    match target {
        Target::Polymorphic { private, .. } | Target::Megamorphic { private } => *private,
        Target::Method { chain, .. }
        | Target::Slot { chain, .. }
        | Target::NestedClass { chain, .. }
        | Target::Primitive { chain, .. }
        | Target::Factory { chain, .. } => *chain == Chain::Private,
        Target::Vacant => false,
    }
}

/// What the guard `guard` says of its receivers (`ReceiverFacts`); `None`
/// when no registered application carries it, or a level's own-slot base
/// passes `u32`, which no field index reaches.
fn receiver_facts(heap: &Heap, guard: DispatchId) -> Option<ReceiverFacts> {
    let index = heap.application_of_dispatch_id(guard)?;
    let mut own_slot_bases = Vec::new();
    let mut applications = Vec::new();
    let mut level = heap.application(index);
    while let Some(application) = level {
        own_slot_bases.push(u32::try_from(application.own_slot_base).ok()?);
        applications.push(application.index);
        level = application.superclass.clone();
    }
    Some(ReceiverFacts {
        dispatch_id: guard,
        application: index,
        applications,
        own_slot_bases,
    })
}

#[cfg(test)]
mod tests {
    use indoc::indoc;

    use super::{CalleeFacts, SiteCandidates, SnapshotTree};
    use crate::interpreter::inline_cache::MethodSide;
    use crate::jit::optimizing::plan::Limits;
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
    /// its family's range, every method callee up to the method limit
    /// `weight_limit` snapshotted.
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
                &Limits {
                    method: weight_limit,
                    ..Limits::STANDARD
                },
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
