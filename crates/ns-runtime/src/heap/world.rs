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
use crate::heap::{Heap, Root};
use crate::interpreter::Mutator;
use crate::types::Value;

pub(crate) struct World {
    mutator: Mutator<'static>,
}

impl World {
    /// A bootstrapped world: every builtin class built, nothing else.
    pub(crate) fn new(lowering: Lowering) -> World {
        let mut world = World {
            mutator: Mutator {
                heap: Heap::new(),
                builtin_classes: Vec::new(),
                platform_singleton: None,
                vm_mirror: None,
                kernel: None,
                value_equality_method: None,
                value_mixin: None,
                behavior_mixin: None,
                metaclasses_ready: false,
                applications: IdentityMap::default(),
                metaclass_mixins: IdentityMap::default(),
                #[cfg(debug_assertions)]
                creations_in_progress: 0,
                spare_walks: Vec::new(),
                lowering,
                argument_buffer: Vec::new(),
                next_dispatch_id: 0,
                caches: crate::interpreter::inline_cache::InlineCaches::new(),
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

    pub(crate) fn clear_safepoint_request(&mut self) {
        self.mutator.heap.clear_safepoint_request();
    }

    /// Whether an arena has reached its limit since the last collection
    /// (`Interpreter::safepoint`). Separate from `safepoint_requested`,
    /// which a stress mode also holds set for its own reasons.
    #[inline(always)]
    pub(crate) fn collection_requested(&self) -> bool {
        self.mutator.heap.collection_requested()
    }

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

    /// The inline caches' sizes (`Interpreter::cache_statistics`).
    pub(crate) fn cache_statistics(&self) -> crate::interpreter::inline_cache::CacheStatistics {
        self.mutator.caches.statistics()
    }

    /// Moves every object and every reified frame (`Heap::move_everything`)
    /// and forwards what the step state holds beside the heap: the
    /// bootstrap's builtin classes, the `platform` singleton, the VM
    /// mirror and the kernel.
    #[cfg(debug_assertions)]
    pub(crate) fn move_everything(&mut self) {
        self.check_quiescent_state();
        let mut forwarder: crate::heap::Forwarder<crate::heap::Rotation> =
            self.mutator.heap.move_everything();
        self.forward_step_state(&mut forwarder);
    }

    /// Marks everything reachable: the heap's own roots, then the step state's
    /// -- the bootstrap's builtin classes, the `platform` singleton, the VM
    /// mirror, the kernel and the depth limit's reserve -- then the traversal.
    pub(crate) fn mark_everything_reachable(&mut self) -> crate::heap::mark::Marker {
        use crate::trace::Tracer;
        let mutator = &mut self.mutator;
        let mut marker = mutator.heap.begin_marking();
        for class in &mut mutator.builtin_classes {
            marker.object(class);
        }
        if let Some(platform) = &mut mutator.platform_singleton {
            marker.value(platform);
        }
        if let Some(mirror) = &mut mutator.vm_mirror {
            marker.value(mirror);
        }
        if let Some(kernel) = &mut mutator.kernel {
            marker.value(kernel);
        }
        if let Some(frame) = &mut mutator.depth_reserve {
            marker.frame(frame);
        }
        mutator.heap.drain_marking(&mut marker);
        marker
    }

    /// One collection: mark everything reachable, compact both arenas, and
    /// forward what the step state holds beside the heap -- the bootstrap's
    /// builtin classes, the `platform` singleton, the VM mirror and the
    /// kernel -- exactly as a move does.
    ///
    /// Timed end to end with one `Instant::now()`/`elapsed()`, folded into
    /// `Heap`'s counters by `record_collection`. That timing call costs
    /// nothing worth measuring against a mark-and-compact, and this is the
    /// only site that pays it: an allocation and the safepoint poll pay
    /// nothing towards it.
    pub(crate) fn collect(&mut self) {
        let start = std::time::Instant::now();
        self.check_quiescent_state();
        // Before marking, so that a resolved future nothing but object
        // fields reached is garbage by the time the marker looks; a program
        // that never made a future pays nothing for it. `move_everything`
        // does not replace: moving is not collecting.
        if self.mutator.futures_made {
            crate::interpreter::future::replace_resolved_futures(&mut self.mutator);
        }
        let marker = self.mark_everything_reachable();
        // Between marking and compacting: after compaction the mark bits no
        // longer line up with the arena, and before marking finishes an
        // entry cleared here could resurrect nothing but would be read
        // before it is complete (`docs/design/memory.md`, "Weak references").
        self.mutator.heap.prune_canonical_symbols(&marker);
        let mut forwarder = self.mutator.heap.compact(marker);
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

    /// Forwards what the step state holds beside the heap: the bootstrap's
    /// builtin classes, the `platform` singleton, the VM mirror, the
    /// kernel and the depth limit's reserve. Shared by `move_everything` and `collect`, which forward the
    /// same roots after the heap has forwarded its own -- one copy so a root
    /// added here later cannot land in one caller's walk and not the other's.
    fn forward_step_state<M: crate::heap::Relocation>(
        &mut self,
        forwarder: &mut crate::heap::Forwarder<M>,
    ) {
        use crate::trace::Tracer;
        let mutator = &mut self.mutator;
        for class in &mut mutator.builtin_classes {
            forwarder.object(class);
        }
        if let Some(platform) = &mut mutator.platform_singleton {
            forwarder.value(platform);
        }
        if let Some(mirror) = &mut mutator.vm_mirror {
            forwarder.value(mirror);
        }
        if let Some(kernel) = &mut mutator.kernel {
            forwarder.value(kernel);
        }
        if let Some(frame) = &mut mutator.depth_reserve {
            forwarder.frame(frame);
        }
    }

    /// What must hold no handle when everything moves, because nothing
    /// traces it: the argument buffer, which a send fills and empties within
    /// one instruction, and the spare boxes finished instantiations leave
    /// for the next ones (`docs/design/memory.md`, "What the collector
    /// traces"). As true a requirement for a release collection as for a
    /// debug-build move, so unlike `move_everything` this is not gated to
    /// debug builds.
    fn check_quiescent_state(&self) {
        use crate::types::InstantiationPhase;
        let mutator = &self.mutator;
        assert!(
            mutator.argument_buffer.is_empty(),
            "internal: the argument buffer holds values between instructions"
        );
        assert!(
            mutator.spare_walks.iter().all(|walk| {
                matches!(walk.instance, Value::Nil)
                    && walk.levels.is_empty()
                    && matches!(walk.phase, InstantiationPhase::Descending { .. })
            }),
            "internal: a spare instantiation box holds a handle"
        );
    }
}
