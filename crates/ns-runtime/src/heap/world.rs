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

use std::collections::HashMap;

use crate::compiler::Lowering;
use crate::heap::{Heap, Root};
use crate::interpreter::Mutator;
use crate::types::Value;

pub(crate) struct World {
    mutator: Mutator,
}

impl World {
    /// A bootstrapped world: every builtin class built, nothing else.
    pub(crate) fn new(lowering: Lowering) -> World {
        let mut world = World {
            mutator: Mutator {
                heap: Heap::new(),
                builtin_classes: Vec::new(),
                platform_singleton: None,
                applications: HashMap::new(),
                empty_class_sides: HashMap::new(),
                #[cfg(debug_assertions)]
                creations_in_progress: 0,
                spare_walks: Vec::new(),
                lowering,
                arg_buffer: Vec::new(),
            },
        };
        world.enter(|m| m.bootstrap_builtin_classes());
        world
    }

    /// Lends the step state out for `f`, one step's worth. The run loop
    /// makes one entry per instruction and a host one per read, so this must
    /// inline away.
    #[inline(always)]
    pub(crate) fn enter<R>(&mut self, f: impl FnOnce(&mut Mutator) -> R) -> R {
        f(&mut self.mutator)
    }

    /// How many activations are live; `Interpreter::run_from_host` checks a
    /// run leaves it where it found it.
    pub(crate) fn activation_depth(&self) -> usize {
        self.mutator.heap.activation_depth()
    }

    #[inline(always)]
    pub(crate) fn safepoint_requested(&self) -> bool {
        self.mutator.heap.safepoint_requested()
    }

    pub(crate) fn clear_safepoint_request(&mut self) {
        self.mutator.heap.clear_safepoint_request();
    }

    /// Roots an immediate a host built itself (`Interpreter::stash`).
    pub(crate) fn stash(&self, value: Value) -> Root {
        self.mutator.heap.stash(value)
    }

    #[cfg(debug_assertions)]
    pub(crate) fn set_safepoint_requested(&mut self, on: bool) {
        self.mutator.heap.set_safepoint_requested(on);
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

    /// Moves every object and every reified frame (`Heap::move_everything`)
    /// and forwards what the step state holds beside the heap: the
    /// bootstrap's builtin classes and the `platform` singleton.
    #[cfg(debug_assertions)]
    pub(crate) fn move_everything(&mut self) {
        use crate::trace::Tracer;
        self.check_quiescent_state();
        let m = &mut self.mutator;
        let mut forwarder = m.heap.move_everything();
        for class in &mut m.builtin_classes {
            forwarder.object(class);
        }
        if let Some(platform) = &mut m.platform_singleton {
            forwarder.value(platform);
        }
    }

    /// What must hold no handle when everything moves, because nothing
    /// traces it: the argument buffer, which a send fills and empties within
    /// one instruction, and the spare boxes finished instantiations leave
    /// for the next ones (`docs/design/memory.md`, "What the collector
    /// traces").
    #[cfg(debug_assertions)]
    fn check_quiescent_state(&self) {
        use crate::types::InstantiationPhase;
        let m = &self.mutator;
        assert!(
            m.arg_buffer.is_empty(),
            "internal: the argument buffer holds values between instructions"
        );
        assert!(
            m.spare_walks.iter().all(|walk| {
                matches!(walk.instance, Value::Nil)
                    && walk.levels.is_empty()
                    && matches!(walk.phase, InstantiationPhase::Descending { .. })
            }),
            "internal: a spare instantiation box holds a handle"
        );
    }

    /// Transitional, for the `Deref` Task 3 of the 2c plan removes.
    pub(crate) fn mutator(&self) -> &Mutator {
        &self.mutator
    }

    /// Transitional, for the `DerefMut` Task 3 of the 2c plan removes.
    pub(crate) fn mutator_mut(&mut self) -> &mut Mutator {
        &mut self.mutator
    }
}
