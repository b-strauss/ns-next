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
//! activation runs freed (`docs/design/execution.md`, "The code zone") --
//! and a stop as for a debugger's pause at the safepoints a seventh
//! schedule picks, the whole stack and every value one level deep read
//! before the run goes on (`docs/design/debugger.md`, "The pause
//! stress mode") -- and, at every safepoint, every statement start of
//! each running body not patched yet patched with a breakpoint that never
//! stops, the mode that runs every program through the `Breakpoint` arm
//! at every statement (`docs/design/debugger.md`, "The breakpoint stress
//! mode") -- and a stop at the safepoints an eighth schedule picks, from
//! which a burst of steps is taken, the stack read at every stop
//! (`docs/design/debugger.md`, "The step stress mode").
//! The step state's side of a
//! move -- its own roots, forwarded after the heap's -- is
//! `World::move_everything`, in `heap/world.rs`; a collection's is
//! `World::collect`, in the same file.

use super::Interpreter;

/// What the run loop goes on with after its safepoint
/// (`Interpreter::safepoint`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SafepointOutcome {
    /// The frame the stretch left, as it left it.
    Unchanged,
    /// The debugger or the suspend stress mode rebuilt the activation
    /// stack above the run's floor: the running activation is a different
    /// stack frame, whose state the loop reads again.
    Rebuilt,
    /// The debugger terminated the run, which the loop ends through the
    /// unwinder from the top activation (`debugger::termination`), or
    /// aborted an evaluation, whose base ends the same unwind
    /// (`debugger::Served::Abort`).
    Terminate,
    /// The debugger restarts a frame or forces it to return
    /// (`debugger::Served::Unwind`): the loop unwinds from the top
    /// activation with the signal the stop started
    /// (`Interpreter::take_started_unwind`), as for `Terminate`, and the
    /// unwind ends at that frame.
    Unwind,
}

impl Interpreter {
    /// The run loop's safepoint, when the heap has asked for one. An
    /// attached debugger is served first (`serve_debugger`), before
    /// anything below can leave early with a collection still requested
    /// and a pause would wait for a later safepoint; a termination it
    /// receives ends the safepoint there. In a debug build, the pause
    /// stress mode stops the run there too, as a pause does, when its
    /// schedule says one is due, reads the stack and continues; then the
    /// move stress mode moves everything when its schedule says
    /// one is due, the cache stress mode invalidates every inline cache on
    /// every safepoint (`docs/design/dispatch.md`, "Invalidation"), and the
    /// collect stress mode runs a full collection when its own schedule says
    /// one is due (`docs/design/memory.md`, "The gates the collector
    /// needs"), the scavenge stress mode verifies the remembered set and
    /// scavenges when its schedule says one is due, and the suspend stress
    /// mode suspends every activation of the run into its frame and
    /// resumes the top one when its own schedule says so, and the zone
    /// stress mode compacts the code zone, freeing every body no activation
    /// runs, when its own schedule says so, and the step stress mode, right
    /// after the pause stress mode, stops when its own schedule says a
    /// burst of steps is due, and the breakpoint stress mode, right after
    /// those two, patches every statement start of each running body it
    /// has not patched yet; any one of the nine keeps the request set,
    /// so that every safepoint comes here.
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
    /// Answers `SafepointOutcome::Rebuilt` when the running activation is
    /// a different stack frame and the loop must read its state again: the
    /// debugger undid an initializer's push to stop a step in its factory
    /// (`debugger::Served::Rebuilt`) or pushed an evaluation above every
    /// frame (`debugger::Served::Evaluate`), in every build, or the
    /// suspend stress mode rebuilt
    /// the activation stack above `floor`, the floor of the run `vm::run`
    /// is in, every activation there suspended into its frame and the top
    /// one resumed -- never in a release build -- and
    /// `SafepointOutcome::Terminate` when the debugger terminated the run
    /// or aborted an evaluation, and `SafepointOutcome::Unwind` when it
    /// restarts a frame or forces one to return.
    #[cold]
    #[inline(never)]
    pub(crate) fn safepoint(&mut self, floor: usize) -> SafepointOutcome {
        // The trace's events are delivered first, before a debugger's
        // stop or a collection, so that the sink hears of them in the order
        // they happened; and again last, whichever way the safepoint ends,
        // so that what it recorded -- an optimization, a compaction, a
        // collection's frees and invalidations -- never outlives it.
        self.flush_trace();
        let outcome = self.serve_safepoint(floor);
        self.flush_trace();
        outcome
    }

    /// Delivers every trace event waiting for a flush.
    fn flush_trace(&mut self) {
        self.enter(|mutator| {
            if mutator.jit.tracer.has_pending() {
                crate::trace_events::flush(mutator);
            }
        });
    }

    /// `safepoint`'s work, between its two flushes of the trace.
    fn serve_safepoint(&mut self, floor: usize) -> SafepointOutcome {
        // Whether the pause stress mode's stop is kept for an evaluation
        // its reader began, which this safepoint may serve.
        #[cfg(debug_assertions)]
        let in_stress_evaluation = self.stress_stop.is_some();
        let served = self.serve_debugger(floor);
        if matches!(
            served,
            crate::debugger::Served::Terminate | crate::debugger::Served::Abort
        ) {
            return SafepointOutcome::Terminate;
        }
        if served == crate::debugger::Served::Unwind {
            return SafepointOutcome::Unwind;
        }
        #[cfg(not(debug_assertions))]
        let _ = floor;
        #[cfg(debug_assertions)]
        let mut stressed = false;
        // Whether the pause stress mode's stop pushed an evaluation, whose
        // frame the loop reads as it reads a debugger's.
        #[cfg(debug_assertions)]
        let mut stress_evaluates = false;
        // Neither stress mode stops at a safepoint of the pause stress
        // mode's evaluations -- one within an evaluation, or the one its
        // end asked for, which serves the kept stop: a stop there would
        // nest an evaluation's stop in another's, and, where the schedule
        // falls due at every safepoint, its evaluations' safepoints would
        // make the next stop's before the program ran on, for ever.
        #[cfg(debug_assertions)]
        let at_stress_evaluation = in_stress_evaluation || self.stress_stop.is_some();
        // The pause stress mode's stop, where a debugger's pause stops:
        // first, before anything else here changes the stack or the heap.
        #[cfg(debug_assertions)]
        {
            let traced_size = self.world.traced_size();
            if self.stress_pause.is_some() {
                // A stop reads every frame of the stack: its schedule is fed
                // the stack's words beside the traced size.
                let stack_words = self.enter(|mutator| mutator.heap.activation_stack_top());
                let paced_size = size_with_stack(traced_size, stack_words, PAUSE_STACK_WEIGHT);
                if !at_stress_evaluation
                    && self
                        .stress_pause
                        .as_mut()
                        .is_some_and(|stress| stress.due(paced_size))
                {
                    stress_evaluates = self.stop_for_stress(floor);
                }
                stressed = true;
            }
            // The step stress mode's stop, which begins a burst of steps,
            // where the pause stress mode's stops.
            if self.stress_step.is_some() {
                if !at_stress_evaluation && !stress_evaluates {
                    self.step_for_stress(floor, traced_size);
                }
                stressed = true;
            }
            // The breakpoint stress mode's patches, next, before an
            // optimization or a stress mode below compiles or moves code:
            // what they find is the patched bodies' machine code freed.
            if self.stress_breakpoints {
                self.enter(crate::debugger::breakpoints::patch_running_for_stress);
                stressed = true;
            }
        }
        // The debugger rebuilds the stack when a step stops in a factory
        // by undoing its initializer's push and when it pushes an
        // evaluation, and so does the debug build's suspend stress mode.
        #[cfg_attr(
            not(debug_assertions),
            allow(
                unused_mut,
                reason = "after the debugger, only the debug build's suspend stress mode rebuilds the stack"
            )
        )]
        let mut rebuilt = matches!(
            served,
            crate::debugger::Served::Rebuilt | crate::debugger::Served::Evaluate
        );
        #[cfg(debug_assertions)]
        {
            rebuilt |= stress_evaluates;
        }
        // Under the invalidate stress mode, once optimized code spent its
        // countdown, the copies installed at earlier safepoints are freed,
        // but a backing-off body's, before this one installs the next
        // (`jit::invalidate_optimized_for_stress`).
        self.world.invalidate_optimized_for_stress();
        // A body's budget ran out: its optimized code is compiled and
        // installed first, here, between two stretches, where no return
        // address into the zone exists (`jit::optimize_requested`), so
        // that the stress modes below run with it installed.
        if self.world.optimization_requested() {
            self.enter(crate::jit::optimize_requested);
        }
        // Under the deopt stress mode, one of the points optimized code
        // passes from here on leaves: the first, second, third, fifth or
        // so on up to the 987th, drawn afresh at every safepoint, so that
        // every point of a short stretch is met and deep points of a long
        // one too (`jit::DEOPTIMIZATION_ROTATION`).
        self.world.rotate_deoptimization_countdown();
        #[cfg(debug_assertions)]
        {
            if self.stress_caches {
                self.world
                    .invalidate_caches(crate::trace_events::VacateCause::Stress);
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
                    // A frame a poll's light exit left goes on in
                    // baseline code, its blocks made while every frame is
                    // where the exit left it: no frame is after this.
                    self.enter(crate::jit::deoptimize_stopped_at_poll);
                    rebuilt |= self.enter(|mutator| {
                        let running = mutator.heap.top_activation();
                        // A step follows the frames it names to where
                        // they are resumed.
                        crate::debugger::stepping::suspending(mutator, floor);
                        match mutator.heap.suspend_activations(floor) {
                            Some(top) => {
                                mutator.resume_frame(top);
                                // A breakpoint's token, and a reach whose
                                // stop was not made, follow the frame they
                                // name to where it was resumed.
                                if let (Some(running), Some(resumed)) =
                                    (running, mutator.heap.top_activation())
                                {
                                    mutator.breakpoints.move_past_once(running, resumed);
                                }
                                true
                            }
                            None => false,
                        }
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
        // refused (`Heap::synchronize_depth_limit`), so a request left for
        // the next poll to clear would cost a body that goes back into
        // machine code at its loop's head a turn whose compiled sends all
        // exit, and a second round trip through this slow path.
        if self.world.collection_requested()
            || self.world.scavenge_requested()
            || self.world.code_zone_compaction_requested()
        {
            return rebuilt.into();
        }
        // The request served, a stress mode holds it again, as its own.
        // The hold is a bit beside the VM's request, so the request is
        // cleared first: holding alone would leave it, and the depth limit
        // it lowers, standing.
        self.world.clear_safepoint_request();
        #[cfg(debug_assertions)]
        if stressed {
            self.world.set_safepoint_requested(true);
        }
        rebuilt.into()
    }
}

impl From<bool> for SafepointOutcome {
    /// Whether the stack was rebuilt -- by the debugger's undoing of an
    /// initializer's push to stop in its factory, or by the suspend
    /// stress mode.
    fn from(rebuilt: bool) -> SafepointOutcome {
        if rebuilt {
            SafepointOutcome::Rebuilt
        } else {
            SafepointOutcome::Unchanged
        }
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

/// Whether `NS_NEXT_STRESS_BREAKPOINTS` asks for the breakpoint stress
/// mode: set, and neither empty nor `0`. Debug builds only.
#[cfg(debug_assertions)]
pub(crate) fn stress_breakpoints_from_env() -> bool {
    std::env::var_os("NS_NEXT_STRESS_BREAKPOINTS")
        .is_some_and(|value| !value.is_empty() && value != "0")
}

/// Whether `NS_NEXT_STRESS_WIDE` asks for every instruction in a wide
/// form: set, and neither empty nor `0`. Debug builds only, like the other
/// stress modes; read once per process, since every body asks.
pub(crate) fn stress_wide_from_env() -> bool {
    static REQUESTED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    cfg!(debug_assertions)
        && *REQUESTED.get_or_init(|| {
            crate::reach::record(crate::reach::Reach::Compiler);
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

/// How a stress schedule spreads its work over the safepoints: at every
/// safepoint while the size it is fed is at most `floor`, and above it once
/// the safepoints since the schedule last fired reach the size over
/// `budget`, rounded up -- about `budget` units of work per safepoint on
/// average. A mode's work at a firing is proportional to the traced size
/// (a move, a collection's compaction, the remembered set's verification)
/// or to the stack (a stop's reading), so firing at literally every
/// safepoint is quadratic in a deep recursion; pacing by size is what a
/// real collector's allocation-driven trigger does. Fixed-frequency
/// stress modes (HotSpot's `ScavengeALotInterval`, SpiderMonkey's
/// `JS_GC_ZEAL` frequency) would take coverage from small programs to spare
/// large ones (`docs/design/memory.md`, "The move stress mode").
#[cfg(debug_assertions)]
#[derive(Clone, Copy)]
pub(crate) struct Pacing {
    /// The size up to which every safepoint fires.
    pub(crate) floor: usize,
    /// The work spent per safepoint, on average, above the floor.
    pub(crate) budget: usize,
}

#[cfg(debug_assertions)]
impl Pacing {
    /// The move and the scavenge stress modes': every safepoint up to a
    /// traced size of 2,000, which a program that allocated little stays
    /// within -- the shipped world alone traces about 1,000 -- and above it
    /// a budget of 250. Measured over the goldens, 427 of 529 are then
    /// stressed at nine safepoints in ten or more, where a floor and budget
    /// of 1,000 stressed 21 so, at 0.44 of that schedule's work: the floor
    /// decoupled from the budget keeps every small program's coverage
    /// whole and takes a quarter of a large program's.
    pub(crate) const HEAP_WALK: Pacing = Pacing {
        floor: 2_000,
        budget: 250,
    };

    /// Every other paced mode's -- the collect, suspend, zone, pause and
    /// step stress modes': floor and budget alike, 1,000. The zone stress
    /// mode must not take the move's floor: its compaction costs the
    /// cells' and the bodies' count, not the traced size, and a loop that
    /// allocates nothing would compact at every one of its safepoints.
    pub(crate) const STANDARD: Pacing = Pacing {
        floor: 1_000,
        budget: 1_000,
    };

    /// Whether a schedule `safepoints_since_fired` safepoints after its last
    /// firing, this one counted, fires for a size of `size`.
    pub(crate) fn due(self, safepoints_since_fired: usize, size: usize) -> bool {
        size <= self.floor || safepoints_since_fired.saturating_mul(self.budget) >= size
    }
}

/// How many times the pause stress mode's schedule counts each word of the
/// stack beside the traced size (`size_with_stack`): a stop reads every
/// frame's variables and their values' first children, measured at 70 to
/// 180 ns per stack word in a debug build, several times a move's cost per
/// traced unit, while the traced size counts a stack word once.
#[cfg(debug_assertions)]
pub(crate) const PAUSE_STACK_WEIGHT: usize = 8;

/// The size a schedule of stops is fed: the traced size, with the stack's
/// `stack_words` counted `weight` times more, since every stop reads the
/// whole stack -- the pause stress mode's (`PAUSE_STACK_WEIGHT`) and the
/// step stress mode's, whose burst reads it at each of its steps
/// (`debugger::STEP_STRESS_BURST`). A deep stack's stops come as much
/// rarer as they cost more; a shallow program's are paced by its heap.
#[cfg(debug_assertions)]
pub(crate) fn size_with_stack(traced_size: usize, stack_words: usize, weight: usize) -> usize {
    traced_size.saturating_add(weight.saturating_mul(stack_words))
}

/// The move stress mode's schedule, paced by `Pacing::HEAP_WALK`.
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
        if !Pacing::HEAP_WALK.due(self.safepoints_since_move, traced_size) {
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

/// The schedule of the collect, the scavenge, the suspend, the zone, the
/// pause and the step stress modes, each by its `Pacing`: the scavenge
/// stress mode's `Pacing::HEAP_WALK`, since the walk of the old space that
/// verifies the remembered set before each scavenge is proportional to the
/// traced size as a move is, and the others' `Pacing::STANDARD`.
#[cfg(debug_assertions)]
pub(crate) struct StressSchedule {
    pacing: Pacing,
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
impl Default for StressSchedule {
    fn default() -> StressSchedule {
        StressSchedule::paced(Pacing::STANDARD)
    }
}

#[cfg(debug_assertions)]
impl StressSchedule {
    /// A schedule that has not fired yet, paced by `pacing`.
    pub(crate) fn paced(pacing: Pacing) -> StressSchedule {
        StressSchedule {
            pacing,
            safepoints_since_fired: 0,
            fired: 0,
        }
    }

    /// Counts one safepoint and answers whether the schedule fires there,
    /// fed `size`.
    pub(crate) fn due(&mut self, size: usize) -> bool {
        self.safepoints_since_fired += 1;
        if !self.pacing.due(self.safepoints_since_fired, size) {
            return false;
        }
        self.safepoints_since_fired = 0;
        self.fired += 1;
        true
    }

    /// Whether the environment variable `variable` asks for the mode: set,
    /// and neither empty nor `0`. Paced by `Pacing::STANDARD`, which
    /// `with_pacing` changes.
    pub(crate) fn from_env(variable: &str) -> Option<StressSchedule> {
        let requested =
            std::env::var_os(variable).is_some_and(|value| !value.is_empty() && value != "0");
        requested.then(StressSchedule::default)
    }

    /// The schedule, paced by `pacing` instead.
    pub(crate) fn with_pacing(self, pacing: Pacing) -> StressSchedule {
        StressSchedule { pacing, ..self }
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

    /// Holds the stress modes' bit of the safepoint request
    /// (`heap::STRESS_HOLD`) exactly while a stress mode is on, since any
    /// one alone must keep every safepoint coming back to `safepoint`:
    /// what every `set_stress_*` does after switching its own mode.
    ///
    /// Turning the last mode off this way drops only the hold. A
    /// collection, a scavenge or a compaction of the code zone still
    /// pending keeps its own bit (`heap::REAL_REQUEST`), and the
    /// debugger's pause and commands keep theirs, so the next poll still
    /// serves each of them, and the depth limit stays lowered while any
    /// of them stands (`Heap::synchronize_depth_limit`).
    pub(super) fn hold_safepoints_for_stress(&mut self) {
        let enabled = self.stress.is_some()
            || self.stress_caches
            || self.stress_collect.is_some()
            || self.stress_scavenge.is_some()
            || self.stress_suspend.is_some()
            || self.stress_zone.is_some()
            || self.stress_pause.is_some()
            || self.stress_breakpoints
            || self.stress_step.is_some();
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
        self.stress_scavenge = enabled.then(|| StressSchedule::paced(Pacing::HEAP_WALK));
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

    /// Turns the pause stress mode on or off for this interpreter,
    /// whatever `NS_NEXT_STRESS_PAUSE` says: the run stopped as for a
    /// debugger's pause at the safepoints its schedule picks, its stack
    /// read, and continued. For tests. Keeps the safepoint request set
    /// while any other stress mode is on (`hold_safepoints_for_stress`).
    pub fn set_stress_pause(&mut self, enabled: bool) {
        self.stress_pause = enabled.then(StressSchedule::default);
        self.hold_safepoints_for_stress();
    }

    /// Turns the breakpoint stress mode on or off for this interpreter,
    /// whatever `NS_NEXT_STRESS_BREAKPOINTS` says: at every safepoint,
    /// every statement start of each running body not patched yet patched
    /// with a breakpoint that never stops, each hit counted and gone past.
    /// Turned off, every slot only its patches held gets its original
    /// back. For tests. Keeps the safepoint request set while any other
    /// stress mode is on (`hold_safepoints_for_stress`).
    pub fn set_stress_breakpoints(&mut self, enabled: bool) {
        if self.stress_breakpoints && !enabled {
            self.enter(crate::debugger::breakpoints::end_stress);
        }
        self.stress_breakpoints = enabled;
        self.hold_safepoints_for_stress();
    }

    /// Turns the step stress mode on or off for this interpreter,
    /// whatever `NS_NEXT_STRESS_STEP` says: at the safepoints its schedule
    /// picks, the run stopped as for a debugger's pause, and from there a
    /// burst of steps taken, into, over and out in turn, the stack read at
    /// every stop. Turned off, a step it left in progress ends. For tests.
    /// Keeps the safepoint request set while any other stress mode is on
    /// (`hold_safepoints_for_stress`).
    pub fn set_stress_step(&mut self, enabled: bool) {
        if self.stress_step.is_some() && !enabled && self.debugger.is_none() {
            self.enter(crate::debugger::stepping::end);
        }
        self.stress_step = enabled.then(crate::debugger::StepStress::default);
        self.hold_safepoints_for_stress();
    }

    /// How many of the step stress mode's steps have stopped since it was
    /// switched on; 0 while it is off.
    pub fn stress_steps(&self) -> usize {
        self.stress_step.as_ref().map_or(0, |stress| stress.stopped)
    }

    /// The steps of each of the step stress mode's bursts
    /// (`debugger::STEP_STRESS_BURST`), for a test's bounds.
    #[doc(hidden)]
    pub fn stress_step_burst() -> usize {
        crate::debugger::STEP_STRESS_BURST
    }

    /// How many of the step stress mode's steps it ended itself, still in
    /// progress several periods of its schedule after a burst fell due,
    /// since it was switched on; 0 while it is off.
    pub fn stress_steps_ended(&self) -> usize {
        self.stress_step.as_ref().map_or(0, |stress| stress.ended)
    }

    /// How many times a `Breakpoint` arm went past a slot the breakpoint
    /// stress mode patched since the mode was switched on; 0 while it is
    /// off.
    pub fn stress_breakpoint_hits(&mut self) -> usize {
        self.enter(|mutator| mutator.breakpoints.stress_hits())
    }

    /// How many times a step went on as a step into from a suspended
    /// frame, the first one below the frame the program goes on in that
    /// the step filters do not filter. For tests.
    pub fn steps_from_suspended_frames(&mut self) -> usize {
        self.enter(|mutator| mutator.breakpoints.steps_from_suspended())
    }

    /// How many times the pause stress mode has stopped the run since it
    /// was turned on; 0 while it is off.
    pub fn stress_pauses(&self) -> usize {
        self.stress_pause.as_ref().map_or(0, |stress| stress.fired)
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
    fn a_heap_within_the_floor_moves_at_every_safepoint() {
        let mut stress = StressMoves::default();
        for _ in 0..10 {
            assert!(stress.due(Pacing::HEAP_WALK.floor));
        }
    }

    /// Past the floor, a move comes once the safepoints since the last one
    /// reach the traced size over the budget, rounded up -- 9 for a size of
    /// the floor and one, 11 for ten budgets and one past the floor -- so
    /// a move's cost, proportional to the size, is spread over as many
    /// safepoints.
    #[test]
    fn a_heap_past_the_floor_moves_once_per_its_size_over_the_budget() {
        let mut stress = StressMoves::default();
        let size = Pacing::HEAP_WALK.floor + 1;
        let due: Vec<bool> = (0..18).map(|_| stress.due(size)).collect();
        assert_eq!(due.iter().filter(|due| **due).count(), 2);
        assert!(due[8] && due[17]);
        let mut stress = StressMoves::default();
        let size = 10 * Pacing::HEAP_WALK.budget + 1;
        let due: Vec<bool> = (0..22).map(|_| stress.due(size)).collect();
        assert_eq!(due.iter().filter(|due| **due).count(), 2);
        assert!(due[10] && due[21]);
    }

    /// The floor and the budget are two numbers: below the floor every
    /// safepoint fires whatever the budget, and the standard pacing keeps
    /// both at 1,000, so that a size between them fires at every
    /// safepoint under the heap walk's pacing and at every second under
    /// the standard one.
    #[test]
    fn the_floor_and_the_budget_pace_apart() {
        let mut heap_walk = StressSchedule::paced(Pacing::HEAP_WALK);
        let mut standard = StressSchedule::default();
        let size = 1_500;
        let heap_walk_fired = (0..10).filter(|_| heap_walk.due(size)).count();
        let standard_fired = (0..10).filter(|_| standard.due(size)).count();
        assert_eq!((heap_walk_fired, standard_fired), (10, 5));
        assert_eq!((heap_walk.fired, standard.fired), (10, 5));
        let mut standard = StressSchedule::default();
        assert!((0..10).all(|_| standard.due(Pacing::STANDARD.floor)));
    }

    /// A stop's schedule is fed the stack's words, weighted, beside the
    /// traced size: a deep stack's stops come rarer than its heap alone
    /// would pace them.
    #[test]
    fn a_deep_stack_spaces_the_stops_out() {
        assert_eq!(size_with_stack(1_000, 0, PAUSE_STACK_WEIGHT), 1_000);
        let size = size_with_stack(1_000, 1_000, PAUSE_STACK_WEIGHT);
        assert_eq!(size, 9_000);
        let mut schedule = StressSchedule::default();
        let fired = (0..18).filter(|_| schedule.due(size)).count();
        assert_eq!(fired, 2);
        assert_eq!(
            size_with_stack(usize::MAX, 1, PAUSE_STACK_WEIGHT),
            usize::MAX
        );
    }
}
