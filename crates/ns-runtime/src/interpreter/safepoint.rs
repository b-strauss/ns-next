//! The run loop's safepoint (`Interpreter::safepoint`, which `vm::run`'s poll
//! calls when the heap asks for it) and, in a debug build, the stress mode:
//! a move of everything at the safepoints its schedule picks, with the
//! interpreter's side of a move -- its own roots, forwarded after the heap's,
//! and the state that must hold no handle when one happens
//! (`docs/design/memory.md`, "Checking the roots before a collector exists:
//! the stress mode").

use super::Interpreter;
#[cfg(debug_assertions)]
use crate::trace::Tracer;
#[cfg(debug_assertions)]
use crate::types::*;

impl Interpreter {
    /// The run loop's safepoint, when the heap has asked for one. In a debug
    /// build under the stress mode, a move when the schedule says one is
    /// due. A release build has nothing that asks yet: the collector's
    /// allocation counter will.
    #[cold]
    #[inline(never)]
    pub(crate) fn safepoint(&mut self) {
        #[cfg(debug_assertions)]
        {
            let size = self.heap.traced_size();
            if self.stress.as_mut().is_some_and(|stress| stress.due(size)) {
                self.move_everything();
            }
        }
    }
}

/// How much move work the stress mode spends per safepoint, on average: it
/// moves at every safepoint while the traced size is at most this, and
/// otherwise once the safepoints since the last move reach the traced size
/// over this, rounded up. A move rewrites work proportional to the traced
/// size, so moving at literally every safepoint is quadratic in a deep
/// recursion; pacing by size is what a real collector's allocation-driven
/// trigger does, and every small program keeps a move at every safepoint.
/// Fixed-frequency stress modes (HotSpot's `ScavengeALotInterval`,
/// SpiderMonkey's `JS_GC_ZEAL` frequency) would take coverage from small
/// programs to spare large ones.
#[cfg(debug_assertions)]
pub(crate) const STRESS_MOVE_BUDGET: usize = 1_000;

/// The stress mode's schedule.
#[cfg(debug_assertions)]
#[derive(Default)]
pub(crate) struct StressMoves {
    safepoints_since_move: usize,
}

#[cfg(debug_assertions)]
impl StressMoves {
    /// Counts one safepoint and answers whether it moves.
    fn due(&mut self, traced_size: usize) -> bool {
        self.safepoints_since_move += 1;
        if self
            .safepoints_since_move
            .saturating_mul(STRESS_MOVE_BUDGET)
            < traced_size
        {
            return false;
        }
        self.safepoints_since_move = 0;
        true
    }

    /// Whether `NS_NEXT_STRESS_MOVES` asks for the stress mode: set, and
    /// neither empty nor `0`.
    pub(crate) fn from_env() -> Option<StressMoves> {
        let on =
            std::env::var_os("NS_NEXT_STRESS_MOVES").is_some_and(|v| !v.is_empty() && v != "0");
        on.then(StressMoves::default)
    }
}

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
    /// tests; the stress mode moves at safepoints of the run loop instead.
    pub fn move_now(&mut self) {
        assert!(!self.running, "move_now is for between runs");
        self.move_everything();
    }

    /// Turns the stress mode on or off for this interpreter, whatever
    /// `NS_NEXT_STRESS_MOVES` says. For tests.
    pub fn set_stress_moves(&mut self, on: bool) {
        self.stress = on.then(StressMoves::default);
        self.heap.set_safepoint_requested(on);
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

#[cfg(all(test, debug_assertions))]
mod tests {
    use super::*;

    #[test]
    fn a_heap_within_the_budget_moves_at_every_safepoint() {
        let mut stress = StressMoves::default();
        for _ in 0..10 {
            assert!(stress.due(STRESS_MOVE_BUDGET));
        }
    }

    /// Above the budget, a move comes once the safepoints since the last one
    /// reach the traced size over the budget, rounded up -- 11 for a size of
    /// ten budgets and one -- so a move's cost, proportional to the size, is
    /// spread over as many safepoints.
    #[test]
    fn a_heap_past_the_budget_moves_once_per_its_size_over_the_budget() {
        let mut stress = StressMoves::default();
        let size = 10 * STRESS_MOVE_BUDGET + 1;
        let due: Vec<bool> = (0..22).map(|_| stress.due(size)).collect();
        assert_eq!(due.iter().filter(|due| **due).count(), 2);
        assert!(due[10] && due[21]);
    }
}
