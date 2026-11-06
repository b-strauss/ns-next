//! The run loop's safepoint (`Interpreter::safepoint`, which `vm::run` calls
//! between two stretches when the heap asks for it) and, in a debug build,
//! the stress modes: a move of everything at the safepoints its schedule picks
//! (`docs/design/memory.md`, "The move stress mode"), and a full collection
//! at the safepoints its own schedule picks (`docs/design/memory.md`, "The
//! gates the collector needs") -- the mode that catches a root the marking
//! does not know about, since the move stress mode alone treats everything
//! as live -- and a scavenge at the safepoints a third schedule picks, each
//! after a check of the remembered set, the mode that catches a store that
//! missed the write barrier -- and the suspension of every activation of
//! the run into its frame at the safepoints a fourth schedule picks, the
//! mode that runs every program on from frames as a deoptimized one will
//! (`docs/design/execution.md`, "Activations") -- and a compaction of the
//! code zone at the safepoints a sixth schedule picks, every body no
//! activation runs freed (`docs/design/execution.md`, "The code zone").
//! The step state's side of a
//! move -- its own roots, forwarded after the heap's -- is
//! `World::move_everything`, in `heap/world.rs`; a collection's is
//! `World::collect`, in the same file.

use super::Interpreter;

impl Interpreter {
    /// The run loop's safepoint, when the heap has asked for one. In a debug
    /// build, the move stress mode moves everything when its schedule says
    /// one is due, the cache stress mode invalidates every inline cache on
    /// every safepoint (`docs/design/dispatch.md`, "Invalidation"), and the
    /// collect stress mode runs a full collection when its own schedule says
    /// one is due (`docs/design/memory.md`, "The gates the collector
    /// needs"), the scavenge stress mode verifies the remembered set and
    /// scavenges when its schedule says one is due, and the suspend stress
    /// mode suspends every activation of the run into its frame and
    /// resumes the top one when its own schedule says so, and the zone
    /// stress mode compacts the code zone, freeing every body no activation
    /// runs, when its own schedule says so; any one of the six keeps the
    /// request set, so that every safepoint comes here.
    ///
    /// A compaction of the code zone a compile that found it full asked
    /// for (`jit::after_compile_trigger`) is served next, before a pending
    /// collection: here, between two stretches, no return address into the
    /// zone exists (`jit::zone`).
    ///
    /// A pending collection is served after the stress modes have had their
    /// turn: `World::collect` is sound only where nothing outside the heap
    /// holds a handle, exactly the condition a safepoint of the run loop
    /// gives it, and an allocation only ever asks for one
    /// (`docs/design/memory.md`, "The trigger and the growth policy") --
    /// this is the one place that request is served. A pending scavenge is
    /// served here too, for the same reason, unless a collection is pending
    /// as well: the collection empties the nursery, which serves both.
    ///
    /// The request is then cleared, in the same call, unless serving it
    /// raised another, and the safepoint that took no request clears it
    /// too. In a release
    /// build this is now real work rather than a debug-only convenience --
    /// the collector's allocation limits ask for one -- but the same
    /// argument still applies to what is left when no request is pending: an
    /// empty slow path would let the optimizer prove the call does nothing
    /// and delete it, and the flag's load and branch with it, and the
    /// release binary that is measured would carry no poll at all.
    ///
    /// Answers whether the suspend stress mode rebuilt the activation stack
    /// above `floor`, the floor of the run `vm::run` is in: every
    /// activation there suspended into its frame and the top one resumed,
    /// so that the running activation is a different stack frame and the
    /// loop must read its state again. Always `false` in a release build.
    #[cold]
    #[inline(never)]
    pub(crate) fn safepoint(&mut self, floor: usize) -> bool {
        #[cfg(not(debug_assertions))]
        let _ = floor;
        #[cfg(debug_assertions)]
        let mut stressed = false;
        // Only the debug build's suspend stress mode rebuilds the stack.
        #[cfg_attr(
            not(debug_assertions),
            allow(
                unused_mut,
                reason = "only the debug build's suspend stress mode rebuilds the stack"
            )
        )]
        let mut rebuilt = false;
        // A body's budget ran out: its optimized code is compiled and
        // installed first, here, between two stretches, where no return
        // address into the zone exists (`jit::optimize_requested`), so
        // that the stress modes below run with it installed.
        if self.world.optimization_requested() {
            self.enter(crate::jit::optimize_requested);
        }
        // Under the deopt stress mode, one of the points optimized code
        // passes from here on leaves: the first, second, third, fifth and
        // so on up to the 987th, in turn at successive safepoints, so that
        // every point of a short stretch is met and deep points of a long
        // one too (`jit::DEOPTIMIZATION_ROTATION`).
        self.world.rotate_deoptimization_countdown();
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
            if let Some(stress) = &mut self.stress_collect {
                if stress.due(self.world.traced_size()) {
                    self.world.collect();
                }
                stressed = true;
            }
            if let Some(stress) = &mut self.stress_scavenge {
                if stress.due(self.world.traced_size()) {
                    self.world.verify_remembered_set();
                    self.world.verify_cells();
                    self.world.scavenge(false);
                }
                stressed = true;
            }
            // The zone stress mode's compaction, which frees every body no
            // activation runs -- every other one sparing each body that has
            // optimized code, which it slides and remaps with the rest:
            // it reads the stack, which it leaves as it finds it.
            let traced_size = self.world.traced_size();
            if let Some(due) = self.stress_zone.as_mut().map(|stress| {
                let due = stress.due(traced_size);
                (due, stress.fired % 2 == 0)
            }) {
                if let (true, spare_optimized) = due {
                    self.enter(|mutator| mutator.compact_code_zone_for_stress(spare_optimized));
                }
                stressed = true;
            }
            // After the other five, which leave the stack as they find it:
            // whatever this rebuilds is what the loop reads back, so the
            // floor it suspends from is the run's and nothing below it.
            let traced_size = self.world.traced_size();
            if let Some(due) = self
                .stress_suspend
                .as_mut()
                .map(|stress| stress.due(traced_size))
            {
                if due {
                    rebuilt = self.enter(|mutator| match mutator.heap.suspend_activations(floor) {
                        Some(top) => {
                            mutator.resume_frame(top);
                            true
                        }
                        None => false,
                    });
                }
                stressed = true;
            }
        }
        // A compile found the code zone full: compacted before a
        // collection is served, as Cog compacts its method zone at its next
        // event check (`cointerp.c:65051-65054`). Here, between two
        // stretches, machine code is not running and no return address
        // into the zone exists, so a body moves without a walk of the
        // native stack (`jit::zone`).
        if self.world.code_zone_compaction_requested() {
            self.enter(|mutator| mutator.compact_code_zone());
        }
        if self.world.collection_requested() {
            self.world.collect();
        } else if self.world.scavenge_requested() {
            // Only without a collection: a full collection empties the
            // nursery as well, and clears a pending scavenge's flags with it.
            let tenure_all = self.world.tenure_all_requested();
            self.world.scavenge(tenure_all);
        }
        // What was served leaves the request standing only if serving it
        // raised another -- a collection that freed a quarter of the code
        // zone asks for its compaction, a scavenge that left the old space
        // past its limit for a collection -- which the next poll serves.
        // Otherwise the request is cleared in this same call: while a real
        // one stands the depth limit is 0 and every compiled push is
        // refused (`Heap::write_safepoint_request`), so a request left for
        // the next poll to clear would cost a body that goes back into
        // machine code at its loop's head a turn whose compiled sends all
        // exit, and a second round trip through this slow path.
        if self.world.collection_requested()
            || self.world.scavenge_requested()
            || self.world.code_zone_compaction_requested()
        {
            return rebuilt;
        }
        // The request served, a stress mode holds it again, as its own.
        #[cfg(debug_assertions)]
        if stressed {
            self.world.set_safepoint_requested(true);
            return rebuilt;
        }
        self.world.clear_safepoint_request();
        rebuilt
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

/// Whether `NS_NEXT_STRESS_WIDE` asks for every instruction in a wide
/// form: set, and neither empty nor `0`. Debug builds only, like the other
/// stress modes; read once per process, since every body asks.
pub(crate) fn stress_wide_from_env() -> bool {
    static REQUESTED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    cfg!(debug_assertions)
        && *REQUESTED.get_or_init(|| {
            std::env::var_os("NS_NEXT_STRESS_WIDE")
                .is_some_and(|value| !value.is_empty() && value != "0")
        })
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
/// programs to spare large ones. `StressSchedule` paces collections and
/// scavenges by the same constant, for the same reason: a collection's
/// compaction is proportional to the traced size too, and so is the walk of
/// the old space that verifies the remembered set before each scavenge.
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

/// The schedule of the collect, the scavenge and the suspend stress
/// modes: their work at every safepoint while the traced size is within
/// `STRESS_MOVE_BUDGET`, and above it once per size-over-budget safepoints --
/// the pacing `StressMoves` uses, for the same reason.
#[cfg(debug_assertions)]
#[derive(Default)]
pub(crate) struct StressSchedule {
    safepoints_since_fired: usize,
    /// How many times this schedule has actually fired since the mode was
    /// turned on -- incremented only when `due` answers `true`, never
    /// merely on a safepoint taken. `Interpreter::moves` cannot stand in for
    /// this: an ordinary collection or scavenge advances it too, so it
    /// cannot tell "the stress mode worked" from "something else did". For
    /// tests (`Interpreter::stress_collections`,
    /// `Interpreter::stress_scavenges`): without this, a schedule whose
    /// `due` always answered `false` would still pass every existing
    /// assertion -- the gate would be a no-op that stays green.
    fired: usize,
}

#[cfg(debug_assertions)]
impl StressSchedule {
    fn due(&mut self, traced_size: usize) -> bool {
        self.safepoints_since_fired += 1;
        if self
            .safepoints_since_fired
            .saturating_mul(STRESS_MOVE_BUDGET)
            < traced_size
        {
            return false;
        }
        self.safepoints_since_fired = 0;
        self.fired += 1;
        true
    }

    /// Whether the environment variable `variable` asks for the mode: set,
    /// and neither empty nor `0`.
    pub(crate) fn from_env(variable: &str) -> Option<StressSchedule> {
        let requested =
            std::env::var_os(variable).is_some_and(|value| !value.is_empty() && value != "0");
        requested.then(StressSchedule::default)
    }
}

impl Interpreter {
    /// Tenures everything now, between entries, where only roots hold
    /// values: `image::load` makes the shipped world
    /// old before the graph pass allocates old.
    pub(crate) fn tenure_now(&mut self) {
        assert!(!self.running, "tenure_now is for between runs");
        self.world.scavenge(true);
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

    /// Scavenges now, between runs, where only roots hold values. For
    /// tests; the run loop scavenges at a safepoint instead.
    pub fn scavenge_now(&mut self) {
        assert!(!self.running, "scavenge_now is for between runs");
        self.world.scavenge(false);
    }

    /// Sets eden's occupied bytes at which an allocation asks for a
    /// scavenge, so a test can reach the trigger without filling eden
    /// first.
    pub fn set_scavenge_threshold(&mut self, bytes: usize) {
        self.world.set_scavenge_threshold(bytes);
    }

    /// Sets the safepoint request exactly when a stress mode is on, since
    /// any one alone must keep every safepoint coming back to `safepoint`:
    /// what every `set_stress_*` does after switching its own mode.
    ///
    /// Turning the last mode off this way drops `safepoint_requested` even
    /// while a collection or a scavenge is still pending. Both self-heal:
    /// the collection limits were never reset, so the next allocation that
    /// reaches one asks again, and the scavenge was never served, so the
    /// next allocation past eden's threshold, or the next old object to
    /// join the remembered set past its limit, asks again -- each only
    /// slightly later than it otherwise would have. Debug-only, since the
    /// setters do not exist in a release build; not worth coupling the
    /// flags together to close.
    pub(super) fn hold_safepoints_for_stress(&mut self) {
        let enabled = self.stress.is_some()
            || self.stress_caches
            || self.stress_collect.is_some()
            || self.stress_scavenge.is_some()
            || self.stress_suspend.is_some()
            || self.stress_zone.is_some();
        self.world.set_safepoint_requested(enabled);
    }

    /// Turns the stress mode on or off for this interpreter, whatever
    /// `NS_NEXT_STRESS_MOVES` says. For tests. Keeps the safepoint request
    /// set while any other stress mode is on (`hold_safepoints_for_stress`).
    pub fn set_stress_moves(&mut self, enabled: bool) {
        self.stress = enabled.then(StressMoves::default);
        self.hold_safepoints_for_stress();
    }

    /// Turns the collect stress mode on or off for this interpreter,
    /// whatever `NS_NEXT_STRESS_COLLECT` says. For tests. Keeps the
    /// safepoint request set while any other stress mode is on
    /// (`hold_safepoints_for_stress`).
    pub fn set_stress_collect(&mut self, enabled: bool) {
        self.stress_collect = enabled.then(StressSchedule::default);
        self.world.set_eager_slides(enabled);
        self.shrink_pairs_for_stress();
        self.hold_safepoints_for_stress();
    }

    /// Under the collect or the zone stress mode, every reservation's shrink
    /// pair is one page of threshold and one of headroom, undamped, so that
    /// each of the modes' collections and compactions in the band gives
    /// back nearly everything above a high mark at once, and every later push, row
    /// and install takes the growth again: the stress runs then check that
    /// every bound machine code reads is lowered before its pages go
    /// (`heap::limits::Shrink`). Otherwise the default pairs. Turning
    /// either mode on or off sets the pairs afresh, over any a test set.
    pub(super) fn shrink_pairs_for_stress(&mut self) {
        let pairs = if self.stress_collect.is_some() || self.stress_zone.is_some() {
            let page = ns_memory::Reservation::page_size();
            let one_page = crate::Shrink {
                threshold: page,
                headroom: page,
            };
            crate::ShrinkPairs {
                stack: one_page,
                cells: one_page,
                machine_entries: one_page,
                optimized_copies: one_page,
                application_ids: one_page,
                code_zone: one_page,
                damped: false,
            }
        } else {
            crate::ShrinkPairs::default()
        };
        self.world.set_shrink_pairs(pairs);
    }

    /// Turns the scavenge stress mode on or off for this interpreter,
    /// whatever `NS_NEXT_STRESS_SCAVENGE` says. For tests. Keeps the
    /// safepoint request set while any other stress mode is on
    /// (`hold_safepoints_for_stress`).
    pub fn set_stress_scavenge(&mut self, enabled: bool) {
        self.stress_scavenge = enabled.then(StressSchedule::default);
        self.hold_safepoints_for_stress();
    }

    /// Turns the suspend stress mode on or off for this interpreter,
    /// whatever `NS_NEXT_STRESS_SUSPEND` says. For tests. Keeps the
    /// safepoint request set while any other stress mode is on
    /// (`hold_safepoints_for_stress`).
    pub fn set_stress_suspend(&mut self, enabled: bool) {
        self.stress_suspend = enabled.then(StressSchedule::default);
        self.hold_safepoints_for_stress();
    }

    /// Turns the zone stress mode on or off for this interpreter,
    /// whatever `NS_NEXT_STRESS_ZONE` says: the code zone compacted at the
    /// safepoints its schedule picks, every body no activation runs freed.
    /// For tests. Keeps the safepoint request set while any other stress
    /// mode is on (`hold_safepoints_for_stress`).
    pub fn set_stress_zone(&mut self, enabled: bool) {
        self.stress_zone = enabled.then(StressSchedule::default);
        self.shrink_pairs_for_stress();
        self.hold_safepoints_for_stress();
    }

    /// How many compactions the zone stress mode has made since it was
    /// turned on; 0 while it is off. A compaction a full zone asked for
    /// never advances this.
    pub fn stress_zone_compactions(&self) -> usize {
        self.stress_zone.as_ref().map_or(0, |stress| stress.fired)
    }

    /// How many times this interpreter's heap has moved everything -- a
    /// move, or a collection's compaction, both of which relocate every
    /// survivor and so advance the same epoch: a handle into the
    /// arrangement before either is exactly as stale as one into the
    /// arrangement before the other. Does not tell the two apart, so a test
    /// pinning that the *collect* stress mode specifically did something
    /// uses `stress_collections` instead.
    pub fn moves(&self) -> u32 {
        self.world.move_epoch()
    }

    /// Sets the collection floor and the next limit directly, in bytes of
    /// objects allocated, so a test can reach the trigger without
    /// allocating up to `COLLECTION_FLOOR` first.
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

    /// How many collections the collect stress mode has actually caused
    /// since it was turned on; 0 while it is off. Unlike `moves`, an
    /// ordinary collection never advances this, so it is what a test pins
    /// against to show the mode did something, rather than merely being a
    /// schedule that never fires.
    pub fn stress_collections(&self) -> usize {
        self.stress_collect
            .as_ref()
            .map_or(0, |stress| stress.fired)
    }

    /// How many scavenges the scavenge stress mode has actually caused
    /// since it was turned on, each after verifying the remembered set; 0
    /// while it is off. An ordinary scavenge never advances this.
    pub fn stress_scavenges(&self) -> usize {
        self.stress_scavenge
            .as_ref()
            .map_or(0, |stress| stress.fired)
    }

    /// How many times the suspend stress mode has suspended every
    /// activation of the run and resumed the top one since it was turned
    /// on; 0 while it is off.
    pub fn stress_suspensions(&self) -> usize {
        self.stress_suspend
            .as_ref()
            .map_or(0, |stress| stress.fired)
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
