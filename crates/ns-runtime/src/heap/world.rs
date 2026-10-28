//! The interpreter's step state at rest: a `Mutator` no step is running in.
//! `enter` lends it out for one step and takes it back
//! (`docs/design/memory.md`, "The brand"). What a step can do is
//! `Mutator`'s; what happens between steps -- the safepoint's move, the
//! host roots of immediates -- is here and in `Interpreter`, which no step
//! can reach.
//!
//! A child of `heap`, so that the heap module keeps building and moving a
//! heap to itself: `World` builds the only one an interpreter has, and moves
//! it only while no `Mutator` is lent out.

use crate::compiler::Lowering;
use crate::hash::IdentityMap;
use crate::heap::{ForwardRefusal, Heap, Root};
use crate::interpreter::Mutator;
use crate::types::{ClassReference, FrameReference, ObjectReference, Value};

/// What the step state holds beside the heap that names objects: the
/// bootstrap's builtin classes, the `platform` singleton, the VM mirror,
/// the kernel, the shipped modules' instances and the depth limit's
/// reserve. Marking, forwarding and
/// scavenging all trace it through this one `Trace` impl, so that a root
/// added later is named once and reached by all three.
pub(crate) struct StepRoots<'state, 'gc> {
    builtin_classes: &'state mut Vec<ClassReference<'gc>>,
    platform_singleton: &'state mut Option<Value<'gc>>,
    vm_mirror: &'state mut Option<Value<'gc>>,
    kernel: &'state mut Option<Value<'gc>>,
    shipped_world: &'state mut crate::interpreter::shipped_world::ShippedWorld<'gc>,
    depth_reserve: &'state mut Option<FrameReference<'gc>>,
}

impl crate::trace::Trace for StepRoots<'_, '_> {
    fn trace<T: crate::trace::Tracer>(&mut self, tracer: &mut T) {
        let StepRoots {
            builtin_classes,
            platform_singleton,
            vm_mirror,
            kernel,
            shipped_world,
            depth_reserve,
        } = self;
        for class in builtin_classes.iter_mut() {
            tracer.object(class);
        }
        if let Some(platform) = platform_singleton.as_mut() {
            tracer.value(platform);
        }
        if let Some(mirror) = vm_mirror.as_mut() {
            tracer.value(mirror);
        }
        if let Some(kernel) = kernel.as_mut() {
            tracer.value(kernel);
        }
        shipped_world.trace(tracer);
        if let Some(frame) = depth_reserve.as_mut() {
            tracer.frame(frame);
        }
    }
}

/// The heap and the step roots, borrowed side by side out of one mutator,
/// so that a tracer the heap lends out can reach the roots it does not own.
fn split<'state, 'gc>(
    mutator: &'state mut Mutator<'gc>,
) -> (&'state mut Heap<'gc>, StepRoots<'state, 'gc>) {
    let Mutator {
        heap,
        builtin_classes,
        platform_singleton,
        vm_mirror,
        kernel,
        shipped_world,
        depth_reserve,
        ..
    } = mutator;
    (
        heap,
        StepRoots {
            builtin_classes,
            platform_singleton,
            vm_mirror,
            kernel,
            shipped_world,
            depth_reserve,
        },
    )
}

/// Forwards every object of `old` to the object of `new` at the same index
/// (`Heap::forward_identity`), then what the step state holds beside the
/// heap (`StepRoots`), then vacates every inline cache, whose entries were
/// decided for receivers that are now forwarders. Inside a step: nothing
/// moves, so no handle a caller holds goes stale but one naming an object
/// of `old`, and the VM mirror's leaf holds none.
pub(crate) fn forward_identity<'gc>(
    mutator: &mut Mutator<'gc>,
    old: &[ObjectReference<'gc>],
    new: &[ObjectReference<'gc>],
) -> Result<(), ForwardRefusal> {
    use crate::trace::Trace;
    let (heap, mut roots) = split(mutator);
    let mut forwarder = heap.forward_identity(old, new)?;
    roots.trace(&mut forwarder);
    mutator.rewrite_immediate_classes();
    mutator.caches.invalidate();
    Ok(())
}

pub(crate) struct World {
    mutator: Mutator<'static>,
}

impl World {
    /// A bootstrapped world: every builtin class built, nothing else.
    pub(crate) fn new(
        lowering: Lowering,
        shipped_code_path: crate::shipped_code::ShippedCodePath,
    ) -> World {
        let mut world = World {
            mutator: Mutator {
                heap: Heap::new(),
                builtin_classes: Vec::new(),
                platform_singleton: None,
                vm_mirror: None,
                kernel: None,
                shipped_world: crate::interpreter::shipped_world::ShippedWorld::default(),
                value_equality_method: None,
                value_mixin: None,
                behavior_mixin: None,
                metaclasses_ready: false,
                applications: IdentityMap::default(),
                metaclass_mixins: IdentityMap::default(),
                #[cfg(debug_assertions)]
                creations_in_progress: 0,
                lowering,
                shipped_code_path,
                dispatch_ids: crate::heap::DispatchIdAllocator::new(),
                next_creation_serial: 1,
                caches: crate::interpreter::inline_cache::InlineCaches::new(),
                jit: crate::jit::JitState::new(),
                unhandled_exceptions_to_resume: 0,
                depth_reserve: None,
                taking_depth_reserve: false,
                futures_made: false,
            },
        };
        world.enter(|mutator| mutator.bootstrap_builtin_classes());
        world
    }

    /// Lends the step state out for `body` as a `Mutator<'gc>`, where `'gc` is
    /// a lifetime only `body` can name -- gc-arena's `Arena::mutate`, without
    /// its cast. The state rests as `Mutator<'static>` and is handed over as
    /// it is: the compiler checks `body` once for every `'gc`, so `body` can
    /// never assume the brand is `'static`, and so can neither let a branded
    /// value out (`R` is chosen outside the `for<'gc>`) nor keep one in a
    /// host's field. That is the whole guarantee, and no conversion of the
    /// state is needed for it. The run loop makes one entry per stretch
    /// between two safepoints and a host one per read, so this must inline
    /// away.
    #[inline(always)]
    pub(crate) fn enter<R>(&mut self, body: impl for<'gc> FnOnce(&mut Mutator<'gc>) -> R) -> R {
        body(&mut self.mutator)
    }

    /// How many activations are live; `Interpreter::run_from_host` checks a
    /// run leaves it where it found it.
    pub(crate) fn activation_depth(&self) -> usize {
        self.mutator.heap.activation_depth()
    }

    /// How many activations are suspended into their frames;
    /// `Interpreter::run_from_host` checks none outlives its run.
    pub(crate) fn suspension_count(&self) -> usize {
        self.mutator.heap.suspension_count()
    }

    /// How many running activations hold a continuation other than
    /// `Continuation::Deliver`; `Interpreter::run_from_host` checks none
    /// outlives its run.
    pub(crate) fn continuation_count(&self) -> usize {
        self.mutator.heap.continuation_count()
    }

    pub(crate) fn clear_safepoint_request(&mut self) {
        self.mutator.heap.clear_safepoint_request();
    }

    /// Whether the space has reached its limit since the last collection
    /// (`Interpreter::safepoint`). Separate from `safepoint_requested`,
    /// which a stress mode also holds set for its own reasons.
    #[inline(always)]
    pub(crate) fn collection_requested(&self) -> bool {
        self.mutator.heap.collection_requested()
    }

    /// Sets the collection floor and the next limit to `floor` bytes of
    /// objects (`Interpreter::set_collection_floor`).
    #[cfg(debug_assertions)]
    pub(crate) fn set_collection_floor(&mut self, floor: usize) {
        self.mutator.heap.set_collection_floor(floor);
    }

    /// Roots an immediate a host built itself (`Interpreter::stash`).
    pub(crate) fn stash(&self, value: Value<'static>) -> Root {
        self.mutator.heap.stash(value)
    }

    #[cfg(debug_assertions)]
    pub(crate) fn set_safepoint_requested(&mut self, requested: bool) {
        self.mutator.heap.set_safepoint_requested(requested);
    }

    #[cfg(debug_assertions)]
    pub(crate) fn traced_size(&self) -> usize {
        self.mutator.heap.traced_size()
    }

    #[cfg(debug_assertions)]
    pub(crate) fn move_epoch(&self) -> u32 {
        self.mutator.heap.move_epoch()
    }

    #[cfg(debug_assertions)]
    pub(crate) fn creations_in_progress(&self) -> usize {
        self.mutator.creations_in_progress
    }

    /// Vacates every inline cache (`Interpreter::invalidate_caches`, and the
    /// cache stress mode's safepoint).
    pub(crate) fn invalidate_caches(&mut self) {
        self.mutator.caches.invalidate();
    }

    /// What the JIT has done (`Interpreter::jit_statistics`).
    pub(crate) fn jit_statistics(&self) -> crate::jit::JitStatistics {
        self.mutator.jit.statistics()
    }

    /// The inline caches' sizes (`Interpreter::cache_statistics`).
    pub(crate) fn cache_statistics(&self) -> crate::interpreter::inline_cache::CacheStatistics {
        self.mutator.caches.statistics()
    }

    /// Moves every object, every reified frame among them
    /// (`Heap::move_everything`), and forwards what the step state holds
    /// beside the heap (`StepRoots`).
    #[cfg(debug_assertions)]
    pub(crate) fn move_everything(&mut self) {
        let mut forwarder: crate::heap::Forwarder<crate::heap::compaction::Shift> =
            self.mutator.heap.move_everything();
        self.forward_step_state(&mut forwarder);
    }

    /// Marks everything reachable: the heap's own roots, then the step
    /// state's (`StepRoots`), then the traversal, then the ephemerons and
    /// weak objects it found (`Heap::finish_marking`). Answers the ids the
    /// marked objects name, for the reclamation.
    pub(crate) fn mark_everything_reachable(&mut self) -> crate::heap::reclaim::GatheredIds {
        use crate::trace::Trace;
        let (heap, mut roots) = split(&mut self.mutator);
        let mut marker = heap.begin_marking();
        roots.trace(&mut marker);
        heap.drain_marking(&mut marker);
        heap.finish_marking(&mut marker);
        marker.gathered
    }

    /// One scavenge (`Heap::scavenge`), with the step state's roots traced
    /// beside the heap's, timed like a collection.
    pub(crate) fn scavenge(&mut self, tenure_all: bool) {
        let start = std::time::Instant::now();
        let futures = crate::interpreter::future::resolved_futures(&self.mutator);
        let (heap, mut roots) = split(&mut self.mutator);
        heap.scavenge(tenure_all, futures, |scavenger| {
            use crate::trace::Trace;
            roots.trace(scavenger);
        });
        self.mutator.rewrite_immediate_classes();
        self.mutator.heap.record_scavenge(start.elapsed());
    }

    /// Whether an allocation or the remembered set has asked for a
    /// scavenge since the last one (`Interpreter::safepoint`).
    pub(crate) fn scavenge_requested(&self) -> bool {
        self.mutator.heap.scavenge_requested()
    }

    /// Whether the requested scavenge must tenure every survivor.
    pub(crate) fn tenure_all_requested(&self) -> bool {
        self.mutator.heap.tenure_all_requested()
    }

    /// Asserts the remembered-set invariant (`Heap::verify_remembered_set`):
    /// the scavenge stress mode's check before each of its scavenges.
    #[cfg(debug_assertions)]
    pub(crate) fn verify_remembered_set(&self) {
        self.mutator.heap.verify_remembered_set();
    }

    /// Asserts that every inline-cache entry's cell agrees with it
    /// (`InlineCaches::verify_cells`): the scavenge stress mode's check
    /// beside the remembered set's.
    #[cfg(debug_assertions)]
    pub(crate) fn verify_cells(&self) {
        if let Err(problem) = self
            .mutator
            .caches
            .verify_cells(&self.mutator.heap, &self.mutator.jit)
        {
            panic!("internal: an inline-cache cell disagrees with its entry: {problem}");
        }
    }

    /// Sets eden's occupied bytes at which an allocation asks for a
    /// scavenge (`Interpreter::set_scavenge_threshold`).
    #[cfg(debug_assertions)]
    pub(crate) fn set_scavenge_threshold(&mut self, bytes: usize) {
        self.mutator.heap.set_scavenge_threshold(bytes);
    }

    /// One collection: mark everything reachable, give back the ids
    /// nothing names any more (`Mutator::reclaim_ids`), compact the space,
    /// and forward what the step state holds beside the heap (`StepRoots`)
    /// exactly as a move does.
    ///
    /// Timed end to end with one `Instant::now()`/`elapsed()`, folded into
    /// `Heap`'s counters by `record_collection`. That timing call costs
    /// nothing worth measuring against a mark-and-compact, and this is the
    /// only site that pays it: an allocation and the safepoint poll pay
    /// nothing towards it.
    pub(crate) fn collect(&mut self) {
        let start = std::time::Instant::now();
        // Before marking, so that a resolved future nothing but object
        // fields reached is garbage by the time the marker looks; a program
        // that never made a future pays nothing for it. `move_everything`
        // does not replace: moving is not collecting.
        if let Some(futures) = crate::interpreter::future::resolved_futures(&self.mutator) {
            self.mutator.heap.replace_resolved_futures(futures);
        }
        let gathered = self.mark_everything_reachable();
        // Between marking and compacting: compaction clears the mark bits,
        // and before marking finishes an
        // entry cleared here could resurrect nothing but would be read
        // before it is complete (`docs/design/memory.md`, "Weak references").
        self.mutator.heap.prune_canonical_symbols();
        self.mutator.heap.prune_mixin_objects();
        // After the prune, whose survivors keep their mixins, and before
        // the compaction, while the mark bits still say what lives.
        self.mutator.reclaim_ids(gathered);
        let mut forwarder = self.mutator.heap.compact();
        self.forward_step_state(&mut forwarder);
        self.mutator.heap.reset_collection_limits();
        self.mutator.heap.record_collection(start.elapsed());
    }

    pub(crate) fn heap_statistics(&self) -> crate::HeapStatistics {
        self.mutator.heap.heap_statistics()
    }

    /// The collector's counts and pauses (`Interpreter::collector_statistics`).
    pub(crate) fn collector_statistics(&self) -> crate::CollectorStatistics {
        self.mutator.heap.collector_statistics()
    }

    /// Forwards what the step state holds beside the heap (`StepRoots`).
    /// Shared by `move_everything` and `collect`, which forward the same
    /// roots after the heap has forwarded its own.
    fn forward_step_state<M: crate::heap::Relocation>(
        &mut self,
        forwarder: &mut crate::heap::Forwarder<M>,
    ) {
        use crate::trace::Trace;
        let (_, mut roots) = split(&mut self.mutator);
        roots.trace(forwarder);
        self.mutator.rewrite_immediate_classes();
    }
}

#[cfg(test)]
mod tests {
    use crate::heap::machine_words::MachineWord;
    use crate::types::Value;

    /// The immediates' class words name the classes `class_of` answers
    /// after each mover: a scavenge that tenures everything, a full
    /// collection and the move stress mode's move.
    #[test]
    fn an_immediate_class_word_survives_a_compaction() {
        let mut interpreter = crate::Interpreter::bootstrap();
        let check = |interpreter: &mut crate::Interpreter| {
            interpreter.enter(|mutator| {
                let immediates = [
                    Value::nil(),
                    Value::boolean(false),
                    Value::boolean(true),
                    Value::small_integer(1),
                    Value::small_float(1.5),
                ];
                for (index, immediate) in immediates.into_iter().enumerate() {
                    let class = mutator.class_of(immediate);
                    let expected = mutator.heap().store(Value::object(class)).bits();
                    assert_eq!(
                        mutator
                            .heap()
                            .machine_word(MachineWord::ImmediateClass(index)),
                        expected,
                        "immediate {index}"
                    );
                }
            });
        };
        check(&mut interpreter);
        interpreter.collect_now();
        check(&mut interpreter);
        interpreter.scavenge_now();
        check(&mut interpreter);
        interpreter.enter(|mutator| {
            let guard = mutator.heap().machine_word(MachineWord::ImmediateGuard(3));
            let expected = mutator.heap().dispatch_id(Value::small_integer(1)).bits();
            assert_eq!(guard, u64::from(expected));
        });
    }

    /// What the step state holds beside the heap is forwarded with it: the
    /// `platform` singleton, here, as every `StepRoots` field is.
    #[test]
    fn the_forward_rewrites_what_the_step_state_holds() {
        let mut interpreter = crate::Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let Some(platform) = mutator.platform_singleton.and_then(Value::as_object) else {
                panic!("the bootstrap records the platform singleton")
            };
            let class = mutator.heap().object_class(platform);
            let replacement = mutator.heap_mut().allocate_instance(class, 0);
            let root = mutator.heap().stash(Value::object(platform));
            assert!(super::forward_identity(mutator, &[platform], &[replacement]).is_ok());
            assert!(mutator
                .platform_singleton
                .and_then(Value::as_object)
                .is_some_and(|object| object.same(replacement)));
            assert!(mutator
                .heap()
                .fetch(&root)
                .as_object()
                .is_some_and(|object| object.same(replacement)));
        });
    }
}
