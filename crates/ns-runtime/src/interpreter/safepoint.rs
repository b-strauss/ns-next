//! The run loop's safepoint (`Interpreter::safepoint`, which `vm::run`'s poll
//! calls when the heap asks for it) and, in a debug build, the stress mode:
//! a move of everything at the safepoints its schedule picks
//! (`docs/design/memory.md`, "Checking the roots before a collector exists:
//! the stress mode"). The step state's side of a move -- its own roots,
//! forwarded after the heap's, and the state that must hold no handle when
//! one happens -- is `World::move_everything` and
//! `World::check_quiescent_state`, in `heap/world.rs`.

use super::Interpreter;

impl Interpreter {
    /// The run loop's safepoint, when the heap has asked for one. In a debug
    /// build, the move stress mode moves everything when its schedule says
    /// one is due, and the cache stress mode invalidates every inline cache
    /// on every safepoint (`docs/design/dispatch.md`, "Invalidation"); either
    /// one keeps the request set, so that every safepoint comes here.
    ///
    /// A pending collection is served after the stress modes have had their
    /// turn: `World::collect` is sound only where nothing outside the heap
    /// holds a handle, exactly the condition a safepoint of the run loop
    /// gives it, and an allocation only ever asks for one
    /// (`docs/design/memory.md`, "The trigger and the growth policy") --
    /// this is the one place that request is served.
    ///
    /// Otherwise the safepoint that took no request clears it. In a release
    /// build this is now real work rather than a debug-only convenience --
    /// the collector's allocation limits ask for one -- but the same
    /// argument still applies to what is left when no request is pending: an
    /// empty slow path would let the optimizer prove the call does nothing
    /// and delete it, and the flag's load and branch with it, and the
    /// release binary that is measured would carry no poll at all.
    #[cold]
    #[inline(never)]
    pub(crate) fn safepoint(&mut self) {
        #[cfg(debug_assertions)]
        let mut stressed = false;
        #[cfg(debug_assertions)]
        {
            if self.stress_caches {
                self.world.invalidate_caches();
                self.cache_invalidations += 1;
                stressed = true;
            }
            if let Some(stress) = &mut self.stress {
                if stress.due(self.world.traced_size()) {
                    self.world.move_everything();
                }
                stressed = true;
            }
        }
        if self.world.collection_requested() {
            self.world.collect();
            // Returns without also clearing `safepoint_requested`: `collect`
            // already reset the collection limits, which is what
            // `collection_requested` is read from, so the very next poll
            // takes this same slow path once more, finds no request and no
            // stress pending, and clears it there instead. Harmless -- one
            // extra call to a `#[cold]` function per collection -- and
            // simpler than threading a second clear through this branch.
            return;
        }
        #[cfg(debug_assertions)]
        if stressed {
            return;
        }
        self.world.clear_safepoint_request();
    }
}

/// Whether `NS_NEXT_STRESS_CACHES` asks for the cache stress mode: set, and
/// neither empty nor `0`. Debug builds only. It vacates every inline cache
/// at every safepoint, so that filling, transitions and invalidation run
/// under the whole suite long before edit-and-continue needs them
/// (`docs/design/dispatch.md`, "Invalidation"). Not combined with the move
/// stress mode as a gate: an entry holds no handle for a move to break, and
/// emptied caches would leave the moves hardly a hit to run under.
#[cfg(debug_assertions)]
pub(crate) fn stress_caches_from_env() -> bool {
    std::env::var_os("NS_NEXT_STRESS_CACHES").is_some_and(|value| !value.is_empty() && value != "0")
}

/// A global-cache size the environment asks for, for the fixture run with
/// a cache small enough that every insert contends for the same group
/// (`run_goldens.rs`). Debug builds only.
#[cfg(debug_assertions)]
pub(crate) fn global_cache_entries_from_env() -> Option<usize> {
    std::env::var("NS_NEXT_GLOBAL_CACHE_ENTRIES")
        .ok()?
        .parse::<usize>()
        .ok()
        .filter(|entries| entries.is_multiple_of(2) && (entries / 2).is_power_of_two())
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
    /// Every safepoint taken since the mode was turned on, moved at or
    /// not. For tests (`Interpreter::stress_safepoints`).
    safepoints: usize,
}

#[cfg(debug_assertions)]
impl StressMoves {
    /// Counts one safepoint and answers whether it moves.
    fn due(&mut self, traced_size: usize) -> bool {
        self.safepoints += 1;
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
        let requested = std::env::var_os("NS_NEXT_STRESS_MOVES")
            .is_some_and(|value| !value.is_empty() && value != "0");
        requested.then(StressMoves::default)
    }
}

#[cfg(debug_assertions)]
impl Interpreter {
    /// Moves everything now, between runs, where only roots hold values. For
    /// tests; the stress mode moves at safepoints of the run loop instead.
    pub fn move_now(&mut self) {
        assert!(!self.running, "move_now is for between runs");
        self.world.move_everything();
    }

    /// Collects now, between runs, where only roots hold values. For tests;
    /// the run loop collects at a safepoint instead.
    pub fn collect_now(&mut self) {
        assert!(!self.running, "collect_now is for between runs");
        self.world.collect();
    }

    /// Turns the stress mode on or off for this interpreter, whatever
    /// `NS_NEXT_STRESS_MOVES` says. For tests. Keeps the safepoint request
    /// set while the cache stress mode is on, since either one alone must
    /// keep every safepoint coming back to `safepoint`.
    ///
    /// This can overwrite `safepoint_requested` from `true` to `false` while
    /// a collection is still pending -- turning both stress modes off here
    /// with `collection_requested` set does exactly that. It self-heals: the
    /// collection limits were never reset, so the next allocation that
    /// reaches one re-requests both flags, only slightly later than it
    /// otherwise would have. Debug-only, since this setter does not exist
    /// in a release build; not worth coupling the two flags together to
    /// close.
    pub fn set_stress_moves(&mut self, enabled: bool) {
        self.stress = enabled.then(StressMoves::default);
        self.world
            .set_safepoint_requested(enabled || self.stress_caches);
    }

    /// How many times this interpreter's heap has moved everything.
    pub fn moves(&self) -> u32 {
        self.world.move_epoch()
    }

    /// Sets both arenas' collection limits directly, so a test can reach the
    /// trigger without allocating up to `COLLECTION_FLOOR` first.
    pub fn set_collection_floor(&mut self, floor: usize) {
        self.world.set_collection_floor(floor);
    }

    /// How many safepoints the stress mode has taken since it was turned
    /// on, whether or not each moved; 0 while it is off. For tests: with
    /// `moves`, it pins that a heap within the budget moves at every
    /// safepoint.
    pub fn stress_safepoints(&self) -> usize {
        self.stress.as_ref().map_or(0, |stress| stress.safepoints)
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
