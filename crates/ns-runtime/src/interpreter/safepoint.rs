//! The interpreter's side of a move: its own roots, forwarded after the
//! heap's, and the state that must hold no handle when one happens. Debug
//! builds only. Nothing in the run loop moves yet: a test moves between runs
//! (`Interpreter::move_now`), and the run loop's safepoint will call into
//! this too.

#[cfg(debug_assertions)]
use super::Interpreter;
#[cfg(debug_assertions)]
use crate::trace::Tracer;
#[cfg(debug_assertions)]
use crate::types::*;

#[cfg(debug_assertions)]
impl Interpreter {
    /// Moves every object and every reified frame (`Heap::move_everything`)
    /// and forwards what the interpreter holds itself: the bootstrap's
    /// builtin classes and the `platform` singleton.
    pub(crate) fn move_everything(&mut self) {
        self.check_quiescent_state();
        let mut forwarder = self.heap.move_everything();
        for class in &mut self.builtin_classes {
            forwarder.object(class);
        }
        if let Some(platform) = &mut self.platform_singleton {
            forwarder.value(platform);
        }
    }

    /// Moves everything now, between runs, where only roots hold values. For
    /// tests; the stress mode will move at safepoints of the run loop instead.
    pub fn move_now(&mut self) {
        assert!(!self.running, "move_now is for between runs");
        self.move_everything();
    }

    /// How many times this interpreter's heap has moved everything.
    pub fn moves(&self) -> u32 {
        self.heap.move_epoch()
    }

    /// What must hold no handle when everything moves, because nothing
    /// traces it: the argument buffer, which a send fills and empties within
    /// one instruction, and the spare boxes finished instantiations leave
    /// for the next ones (`docs/design/memory.md`, "What the collector
    /// traces").
    fn check_quiescent_state(&self) {
        assert!(
            self.arg_buffer.is_empty(),
            "internal: the argument buffer holds values between instructions"
        );
        assert!(
            self.spare_walks.iter().all(|walk| {
                matches!(walk.instance, Value::Nil)
                    && walk.levels.is_empty()
                    && matches!(walk.phase, InstantiationPhase::Descending { .. })
            }),
            "internal: a spare instantiation box holds a handle"
        );
    }
}
