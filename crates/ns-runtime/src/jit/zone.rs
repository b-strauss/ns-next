//! The code zone's compaction, as Cog compacts its method zone
//! (`docs/design/execution.md`, "The code zone"). Cog's citations here are
//! to `prior-art/opensmalltalk-vm/src/spur64.cog/` (`cogitX64SysV.c`, or
//! `cointerp.c` where named).
//!
//! A compile that finds no room asks for a compaction and puts its body
//! back to counting (`compile_into_slot`); the run loop's next safepoint
//! serves the request (`Interpreter::safepoint`), where machine code is
//! not running and so no return address into the zone exists: moving a
//! body needs no walk of the native stack. The compaction marks the
//! bodies activations run, frees what is always freed and then the least
//! used until a quarter of the zone is free, plans each survivor's new
//! place, remaps every address outside the bodies that names one -- the
//! cells and the machine entry rows -- slides the survivors' bytes down,
//! and rewrites the JIT's own records, in the order of Cog's
//! `compactCogCompiledCode` (`:5723-5744`).
//!
//! A body is position-independent: stubs and leaves are reached through
//! the entry block's words, callees through cells, its data words relative
//! to `rip`. The stubs lie below every body and never move, so the entry
//! block's lasting words stay current. A compiled frame names its body by
//! code and program counter, never by address, so a frame stopped in code
//! a compaction freed resumes interpreted (`jit::resumed`).

use ns_memory::CodeOffset;

use crate::bytecode::CompiledCode;
use crate::hash::IdentityMap;
use crate::interpreter::Mutator;

use super::{InstalledBody, JitState, BODY_ALIGNMENT, COMPILED};

/// The usage a body is born with: what Cog's `initialMethodUsageCount`
/// answers for a method whose primitive has no machine-code generator
/// (`:8837-8850`), as no body here has one.
pub(crate) const INITIAL_USAGE: u8 = 2;

/// The usage of a body an activation runs, Cog's `CMMaxUsageCount`
/// (`:85`), which `markMethodAndReferents` sets an active method to
/// (`:10073-10074`).
pub(crate) const MAXIMUM_USAGE: u8 = 7;

/// The usage a cell or a machine entry row written to name a body raises
/// it to at most: Cog raises a linked send's target only while it is below
/// `CMMaxUsageCount / 2` (`:8697-8698`), so that only an active body
/// reaches the maximum.
pub(crate) const LINKED_USAGE_CEILING: u8 = MAXIMUM_USAGE / 2;

/// Which bodies a compaction frees beyond those always freed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Freeing {
    /// Cog's: the least used first, every body whose usage is at most 0,
    /// then at most 1, and so on below `MAXIMUM_USAGE`, until a quarter of
    /// the zone is free (`freeOlderMethodsForCompaction`, `:12897-12925`).
    ToAQuarter,
    /// The zone stress mode's: every body below `MAXIMUM_USAGE`, so every
    /// body no activation runs is freed and compiled again, and every
    /// other moves whenever something below it was freed.
    EveryInactiveBody,
}

/// Where a compaction puts each surviving body: the survivors in
/// ascending order, each at the next multiple of its own alignment from
/// the end of the stubs, which yields the old start to new start table
/// Cog's `planCompaction` keeps in each method's header (`:13098-13113`).
pub(crate) struct CompactionPlan {
    /// The executable address of the zone's offset 0.
    base: u64,
    /// Where the bodies begin, past the stubs.
    bodies_start: u32,
    /// Where the bodies ended before the compaction, and so where an
    /// address the plan remaps may lie.
    old_end: u32,
    /// Where the zone's next body goes once the survivors are moved.
    new_end: u32,
    /// The executable address of the miss stub, which a cell naming a
    /// freed body names instead.
    miss_stub: u64,
    /// Every survivor, ascending by its old start.
    survivors: Vec<Survivor>,
}

/// A body that survives a compaction: its old place and its new start.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Survivor {
    pub(crate) old_start: u32,
    pub(crate) old_end: u32,
    pub(crate) new_start: u32,
}

impl Survivor {
    /// How far the body moves down.
    pub(crate) fn delta(self) -> u32 {
        self.old_start - self.new_start
    }
}

/// A body as a plan sees it: its place, its alignment, and whether it is
/// freed.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PlannedBody {
    pub(crate) start: u32,
    pub(crate) end: u32,
    pub(crate) alignment: u32,
    pub(crate) free: bool,
}

/// What a plan says of an address outside the bodies that names one.
enum Remapped {
    /// No body's: a stub's, or none.
    Outside,
    /// Inside a survivor: where it lies once the survivor moved.
    Moved(u64),
    /// Inside a freed body.
    Freed,
}

impl CompactionPlan {
    /// The plan for `bodies`, ascending and laid out from `bodies_start`
    /// to `old_end` as installs lay them out, in the zone whose offset 0
    /// lies at `base`, its miss stub at `miss_stub`.
    pub(crate) fn new(
        bodies: &[PlannedBody],
        bodies_start: u32,
        old_end: u32,
        base: u64,
        miss_stub: u64,
    ) -> CompactionPlan {
        let mut cursor = bodies_start;
        let mut survivors = Vec::new();
        for body in bodies.iter().filter(|body| !body.free) {
            // As an install places it: at its alignment, and the next one
            // past it at the next multiple of `BODY_ALIGNMENT`. So no body
            // moves up: each survivor's new start is at most its old one.
            let Some(new_start) = cursor.checked_next_multiple_of(body.alignment) else {
                unreachable!("internal: a body's new start lies below its old one");
            };
            debug_assert!(new_start <= body.start, "internal: a body never moves up");
            survivors.push(Survivor {
                old_start: body.start,
                old_end: body.end,
                new_start,
            });
            cursor = (new_start + (body.end - body.start))
                .checked_next_multiple_of(BODY_ALIGNMENT)
                .unwrap_or(old_end);
        }
        CompactionPlan {
            base,
            bodies_start,
            old_end,
            new_end: cursor,
            miss_stub,
            survivors,
        }
    }

    /// Every survivor, ascending.
    pub(crate) fn survivors(&self) -> &[Survivor] {
        &self.survivors
    }

    /// Where the zone's next body goes once the survivors are moved.
    pub(crate) fn new_end(&self) -> u32 {
        self.new_end
    }

    fn locate(&self, address: u64) -> Remapped {
        let Some(offset) = address
            .checked_sub(self.base)
            .and_then(|offset| u32::try_from(offset).ok())
            .filter(|offset| *offset >= self.bodies_start && *offset < self.old_end)
        else {
            return Remapped::Outside;
        };
        let after = self
            .survivors
            .partition_point(|survivor| survivor.old_start <= offset);
        match after.checked_sub(1).map(|index| self.survivors[index]) {
            Some(survivor) if offset < survivor.old_end => {
                Remapped::Moved(address - u64::from(survivor.delta()))
            }
            _ => Remapped::Freed,
        }
    }

    /// `address` as a cell's entry once the bodies moved: an address inside
    /// a survivor moved by its delta, one inside a freed body the miss
    /// stub's, which `cell_entry` answers for a body counting its
    /// entries; and any other, a stub's, unchanged.
    pub(crate) fn remap(&self, address: u64) -> u64 {
        match self.locate(address) {
            Remapped::Outside => address,
            Remapped::Moved(moved) => moved,
            Remapped::Freed => self.miss_stub,
        }
    }

    /// `address` as a machine entry row's entry once the bodies moved:
    /// `remap`'s, but 0, no machine entry, for a freed body's.
    pub(crate) fn remap_row(&self, address: u64) -> u64 {
        match self.locate(address) {
            Remapped::Outside => address,
            Remapped::Moved(moved) => moved,
            Remapped::Freed => 0,
        }
    }
}

impl JitState {
    /// Marks the body at `index` freed: no lookup finds it from now on,
    /// its returns are removed, and the next compaction reclaims its
    /// bytes. Only between entries, where no return address into it
    /// exists.
    pub(super) fn free_body(&mut self, index: u32) {
        debug_assert_eq!(
            self.entries_in_progress, 0,
            "internal: machine code is freed only between entries"
        );
        let Some(body) = self.bodies.get_mut(index as usize) else {
            return;
        };
        if body.free {
            return;
        }
        body.free = true;
        self.returns.retain(|_, site| site.body != index);
    }

    /// Whether a compile found no room since the last compaction: the run
    /// loop's safepoint compacts the zone (`Interpreter::safepoint`).
    pub(crate) fn compaction_requested(&self) -> bool {
        self.compaction_requested
    }

    /// A full collection ran: a zone exhausted by active code is tried
    /// again, so that the next compile that finds no room compacts it once
    /// more, now that the code the collection's run left may no longer be
    /// active.
    pub(crate) fn note_full_collection(&mut self) {
        self.zone_exhausted = false;
    }

    /// The index in `bodies` of the body `code`'s slot names in this zone,
    /// whether or not the JIT is on, while it is not freed.
    fn named_body(&self, code: &CompiledCode) -> Option<usize> {
        let slot = &code.jit;
        let index = slot.map.get() as usize;
        let named = slot.state.get() == COMPILED
            && slot.zone.get() == self.zone_identity()
            && self.bodies.get(index).is_some_and(|body| {
                !body.free && std::ptr::eq(body.code.as_ptr(), std::ptr::from_ref(code))
            });
        named.then_some(index)
    }

    /// The executable address of the machine code `code`'s slot names in
    /// this zone, if any. For tests.
    pub(crate) fn body_entry_address(&self, code: &CompiledCode) -> Option<u64> {
        let index = self.named_body(code)?;
        let zone = self.zone.as_ref()?;
        Some(
            zone.writer
                .zone()
                .executable_address(CodeOffset(self.bodies[index].start)) as u64,
        )
    }

    /// The zone offset of the machine code `code`'s slot names, if any.
    /// For tests.
    pub(crate) fn body_offset(&self, code: &CompiledCode) -> Option<u32> {
        let index = self.named_body(code)?;
        Some(self.bodies[index].start)
    }

    /// The data words of the machine code `code`'s slot names -- its
    /// closure sites among them -- as the zone holds them. For tests.
    pub(crate) fn body_words(&self, code: &CompiledCode) -> Option<Vec<u64>> {
        let index = self.named_body(code)?;
        let body = &self.bodies[index];
        let zone = self.zone.as_ref()?;
        let bytes = zone.writer.bytes(body.words..body.words_end)?;
        Some(
            bytes
                .as_chunks::<8>()
                .0
                .iter()
                .map(|word| u64::from_le_bytes(*word))
                .collect(),
        )
    }

    /// Step 1, Cog's `markActiveMethodsAndReferents` (`cointerp.c:17709-17725`):
    /// every body an activation runs, on the activation stack or suspended
    /// into its frame, is set to `MAXIMUM_USAGE`, so that freeing by usage
    /// keeps it.
    fn mark_active(&self, active: &[std::rc::Rc<CompiledCode>]) {
        for code in active {
            if let Some(index) = self.named_body(code) {
                self.bodies[index].usage.set(MAXIMUM_USAGE);
            }
        }
    }

    /// The free bytes a compaction aims at: a quarter of the bodies'
    /// room, and at least what the largest compile that found no room
    /// needed.
    fn wanted_free_bytes(&self, zone: &super::Zone) -> u32 {
        (zone.room() / 4).max(self.wanted_bytes)
    }

    /// Step 2: frees every body whose code died -- its mixin's row freed
    /// or superseded -- or that its slot no longer names: abandoned,
    /// compiled again, or compiled into another zone since.
    fn free_unnamed(&mut self) {
        for index in 0..self.bodies.len() {
            let body = &self.bodies[index];
            if body.free {
                continue;
            }
            let named = body
                .code
                .upgrade()
                .is_some_and(|code| self.named_body(&code) == Some(index));
            if !named {
                self.bodies[index].free = true;
            }
        }
    }

    /// Step 3, Cog's `freeOlderMethodsForCompaction` (`:12897-12925`):
    /// frees bodies by their usage as `freeing` says, each put back to
    /// counting its entries. A quarter is of the bodies' room, past the
    /// stubs, as Cog's zone size is of its method zone, past the
    /// trampolines.
    fn free_by_usage(&mut self, freeing: Freeing) {
        let Some(zone) = self.zone.as_ref() else {
            return;
        };
        let wanted = self.wanted_free_bytes(zone);
        let mut free_bytes = zone.limit.saturating_sub(zone.next_offset)
            + self
                .bodies
                .iter()
                .filter(|body| body.free)
                .map(InstalledBody::size)
                .sum::<u32>();
        let ceiling = match freeing {
            Freeing::ToAQuarter => 0,
            Freeing::EveryInactiveBody => MAXIMUM_USAGE - 1,
        };
        let mut freeable = ceiling;
        while freeable < MAXIMUM_USAGE {
            for body in &mut self.bodies {
                if freeing == Freeing::ToAQuarter && free_bytes >= wanted {
                    return;
                }
                if body.free || body.usage.get() > freeable {
                    continue;
                }
                body.free = true;
                free_bytes += body.size();
                // Alive: a body whose code died was freed in step 2.
                if let Some(code) = body.code.upgrade() {
                    code.jit.forget();
                }
            }
            if freeing == Freeing::EveryInactiveBody {
                return;
            }
            freeable += 1;
        }
    }

    /// Step 4: the plan of the survivors' new places.
    fn plan(&self) -> Option<CompactionPlan> {
        let zone = self.zone.as_ref()?;
        let planned: Vec<PlannedBody> = self
            .bodies
            .iter()
            .map(|body| PlannedBody {
                start: body.start,
                end: body.end,
                alignment: body.alignment,
                free: body.free,
            })
            .collect();
        let base = zone.writer.zone().executable_address(CodeOffset(0)) as u64;
        let miss_stub = zone.writer.zone().executable_address(zone.miss_stub) as u64;
        Some(CompactionPlan::new(
            &planned,
            zone.bodies_start,
            zone.next_offset,
            base,
            miss_stub,
        ))
    }

    /// Steps 6 to 8: slides each survivor's bytes, code and data words, down
    /// to its new start in ascending order, through a buffer the writer
    /// reads back (Cog's `compactCompiledCode`, `:12740-12825`); rewrites the
    /// installed bodies, each survivor's slot and the return sites keyed by
    /// zone offset, a freed body's dropped; moves the zone's next offset to
    /// the end of the last survivor; and halves every survivor's usage
    /// (`:12756-12757`, `:12806-12807`).
    fn slide(&mut self, plan: &CompactionPlan) {
        let identity = self.zone_identity();
        let Some(zone) = self.zone.as_mut() else {
            return;
        };
        let old_bodies = std::mem::take(&mut self.bodies);
        // Each old index's new index and delta, `None` for a freed body.
        let mut renumbered: Vec<Option<(u32, u32)>> = Vec::with_capacity(old_bodies.len());
        let mut survivors = plan.survivors().iter();
        let mut moved_bytes = 0u64;
        let mut freed_bodies = 0u64;
        for mut body in old_bodies {
            if body.free {
                renumbered.push(None);
                freed_bodies += 1;
                continue;
            }
            let Some(survivor) = survivors.next() else {
                unreachable!("internal: the plan has a place for every survivor");
            };
            debug_assert_eq!(survivor.old_start, body.start);
            let delta = survivor.delta();
            if delta != 0 {
                // Ascending, each body moves down onto bytes already moved
                // or its own, which the buffer keeps apart.
                let Some(bytes) = zone.writer.bytes(body.start..body.end).map(<[u8]>::to_vec)
                else {
                    unreachable!("internal: an installed body lies in the committed zone");
                };
                if zone
                    .writer
                    .write(CodeOffset(survivor.new_start), &bytes)
                    .is_err()
                {
                    unreachable!("internal: a body moves only down, inside the committed zone");
                }
                moved_bytes += u64::from(body.size());
            }
            let Ok(index) = u32::try_from(self.bodies.len()) else {
                unreachable!("internal: fewer survivors than bodies were installed");
            };
            body.start -= delta;
            body.end -= delta;
            body.words -= delta;
            body.words_end -= delta;
            body.usage.set(body.usage.get() / 2);
            if let Some(code) = body.code.upgrade() {
                let first_instruction = code.jit.first_instruction.get() - delta;
                code.jit.compiled(
                    identity,
                    [CodeOffset(body.start), CodeOffset(first_instruction)],
                    index,
                );
            }
            renumbered.push(Some((index, delta)));
            self.bodies.push(body);
        }
        let old_returns = std::mem::take(&mut self.returns);
        let mut returns = IdentityMap::default();
        for (offset, site) in old_returns {
            if let Some(Some((index, delta))) = renumbered.get(site.body as usize) {
                returns.insert(
                    offset - delta,
                    super::ReturnSite {
                        body: *index,
                        ..site
                    },
                );
            }
        }
        self.returns = returns;
        zone.next_offset = plan.new_end();
        self.statistics.zone_compactions += 1;
        self.statistics.freed_bodies += freed_bodies;
        self.statistics.moved_bytes += moved_bytes;
    }
}

impl Mutator<'_> {
    /// Compacts the code zone at the run loop's safepoint, once a compile
    /// found no room (`JitState::compaction_requested`): Cog's
    /// `compactCogCompiledCode` (`:5723-5744`), freeing the least used
    /// bodies until a quarter of the zone is free.
    pub(crate) fn compact_code_zone(&mut self) {
        self.compact_code_zone_freeing(Freeing::ToAQuarter);
    }

    /// The compaction the zone stress mode makes (`NS_NEXT_STRESS_ZONE`):
    /// every body no activation runs freed.
    #[cfg(debug_assertions)]
    pub(crate) fn compact_code_zone_for_stress(&mut self) {
        self.compact_code_zone_freeing(Freeing::EveryInactiveBody);
    }

    /// The compaction, in Cog's order: the bodies marked, freed and
    /// planned; every address outside the bodies remapped before the bytes
    /// move, as Cog updates the stack zone and relocates its methods'
    /// references before it copies
    /// (`updateStackZoneReferencesToCompiledCodePreCompaction`,
    /// `cointerp.c:21546-21596`; `relocateMethodsPreCompaction`,
    /// `:13400-13417`); the bytes slid and the records rewritten; the
    /// request cleared, and the exhaustion once a quarter is free.
    fn compact_code_zone_freeing(&mut self, freeing: Freeing) {
        debug_assert_eq!(
            self.jit.entries_in_progress, 0,
            "internal: the zone is compacted only between entries"
        );
        self.jit.compaction_requested = false;
        if self.jit.zone.is_none() {
            self.jit.zone_exhausted = false;
            return;
        }
        let active = self.heap.active_codes();
        self.jit.mark_active(&active);
        drop(active);
        self.jit.free_unnamed();
        self.jit.free_by_usage(freeing);
        let Some(plan) = self.jit.plan() else {
            return;
        };
        self.caches.remap_cell_entries(&plan);
        #[cfg(ns_jit)]
        self.heap.remap_machine_entries(&plan);
        self.jit.slide(&plan);
        if let Some(zone) = self.jit.zone.as_ref() {
            if zone.limit.saturating_sub(zone.next_offset) >= self.jit.wanted_free_bytes(zone) {
                self.jit.zone_exhausted = false;
            }
        }
        self.jit.wanted_bytes = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::{CompactionPlan, PlannedBody, Survivor};

    fn body(start: u32, end: u32, alignment: u32, free: bool) -> PlannedBody {
        PlannedBody {
            start,
            end,
            alignment,
            free,
        }
    }

    /// Each survivor goes to the next multiple of its own alignment past
    /// the last, from the end of the stubs: a body whose data words lie on
    /// lines of their own keeps them there.
    #[test]
    fn a_plan_keeps_each_body_at_its_alignment() {
        let bodies = [
            body(0x1000, 0x1030, 16, true),
            body(0x1040, 0x1070, 16, false),
            body(0x1080, 0x1100, 64, false),
            body(0x1100, 0x1110, 16, true),
            body(0x1140, 0x11c0, 64, false),
        ];
        let plan = CompactionPlan::new(&bodies, 0x1000, 0x11c0, 0x10_0000, 0x10_0040);
        assert_eq!(
            plan.survivors(),
            [
                Survivor {
                    old_start: 0x1040,
                    old_end: 0x1070,
                    new_start: 0x1000
                },
                Survivor {
                    old_start: 0x1080,
                    old_end: 0x1100,
                    new_start: 0x1040
                },
                Survivor {
                    old_start: 0x1140,
                    old_end: 0x11c0,
                    new_start: 0x10c0
                },
            ]
        );
        for survivor in plan.survivors() {
            assert!(survivor.new_start <= survivor.old_start);
        }
        assert_eq!(plan.new_end(), 0x1140);
    }

    /// An address inside a survivor moves by its delta; one inside a freed
    /// body becomes the miss stub's for a cell, and no entry for a row; one
    /// outside the bodies -- a stub's, 0 -- stays.
    #[test]
    fn a_plan_remaps_an_address_inside_a_survivor_by_its_delta() {
        let base = 0x10_0000;
        let miss_stub = base + 0x40;
        let bodies = [
            body(0x1000, 0x1030, 16, true),
            body(0x1040, 0x1070, 16, false),
            body(0x1080, 0x1100, 64, false),
        ];
        let plan = CompactionPlan::new(&bodies, 0x1000, 0x1100, base, miss_stub);
        assert_eq!(plan.remap(base + 0x1040), base + 0x1000, "an entry");
        assert_eq!(plan.remap(base + 0x1055), base + 0x1015, "inside");
        assert_eq!(plan.remap(base + 0x1080), base + 0x1040);
        assert_eq!(plan.remap(base + 0x1000), miss_stub, "a freed body's");
        assert_eq!(plan.remap_row(base + 0x1000), 0, "no machine entry");
        assert_eq!(plan.remap(miss_stub), miss_stub, "a stub's");
        assert_eq!(plan.remap(0), 0, "none");
        assert_eq!(plan.remap(base + 0x1100), base + 0x1100, "past the bodies");
    }

    /// Machine code holds no address of its own but those relative to
    /// `rip`: one body compiled at two offsets of a zone is the same
    /// bytes, so that a compaction may move it.
    #[cfg(ns_jit)]
    #[test]
    fn a_body_assembled_at_two_offsets_is_the_same_bytes() {
        use crate::jit::{install, JitState};
        let mut state = JitState::for_test(1 << 16, true);
        let first = crate::jit::tests::machine::body_making_a_closure();
        crate::jit::tests::machine::enter_twice(&mut state, &first);
        let zone = state.zone.as_mut().expect("a zone");
        install(zone, &[0x90; 40]).expect("the padding fits");
        let second = crate::jit::tests::machine::body_making_a_closure();
        crate::jit::tests::machine::enter_twice(&mut state, &second);
        assert_eq!(state.bodies.len(), 2, "both compiled");
        let (one, other) = (&state.bodies[0], &state.bodies[1]);
        assert_ne!(one.start, other.start);
        let zone = state.zone.as_ref().expect("a zone");
        let bytes = |body: &crate::jit::InstalledBody| {
            zone.writer
                .bytes(body.start..body.end)
                .expect("committed")
                .to_vec()
        };
        assert_eq!(bytes(one), bytes(other));
    }

    /// Pushes a factory's activation of `code`, as the run loop pushes
    /// one: a top-level activation is never entered in machine code.
    #[cfg(ns_jit)]
    fn push(
        mutator: &mut crate::interpreter::Mutator<'_>,
        code: &std::rc::Rc<crate::bytecode::CompiledCode>,
    ) -> crate::heap::ActivationReference {
        let class = mutator.builtin_class(crate::types::BuiltinClass::Object);
        mutator
            .heap
            .push_activation(
                crate::heap::ActivationHeader {
                    receiver: crate::Value::object(class),
                    current_class: Some(class),
                    kind: crate::types::ActivationKind::Factory(class),
                    method: None,
                    feedback_base: crate::interpreter::FeedbackBase::NOT_ASSIGNED,
                    code: std::rc::Rc::clone(code),
                    then: crate::types::Continuation::Deliver,
                    counted: true,
                },
                crate::interpreter::Arguments::NONE,
                code.registers_above_header(),
            )
            .unwrap_or_else(|_| panic!("the stack holds the frame"))
    }

    /// A frame an exit left stopped in machine code a compaction freed --
    /// active, but abandoned, which always frees -- resumes interpreted:
    /// it names its body by code and program counter, never by address,
    /// and `resumed` finds no machine code for it. A frame of a body the
    /// compaction moved resumes in its machine code at its new place.
    #[cfg(ns_jit)]
    #[test]
    fn a_frame_stopped_in_freed_code_resumes_interpreted() {
        use crate::interpreter::FeedbackBase;
        use crate::jit::tests::machine::body_with_a_loop;
        use crate::jit::{abandon, note_entry, resumed, Resumption};
        let mut interpreter =
            crate::interpreter::Interpreter::bootstrap_with(crate::Lowering::Optimized);
        interpreter.set_jit_enabled(true);
        interpreter.enter(|mutator| {
            let freed = body_with_a_loop();
            let moved = body_with_a_loop();
            for code in [&freed, &moved] {
                note_entry(&mut mutator.jit, code, FeedbackBase::NOT_ASSIGNED, None);
                note_entry(&mut mutator.jit, code, FeedbackBase::NOT_ASSIGNED, None);
            }
            let moved_before = mutator.jit.body_offset(&moved).expect("compiled");
            let stopped = push(mutator, &freed);
            abandon(mutator, &freed);
            mutator.compact_code_zone();
            assert_eq!(mutator.jit.body_offset(&freed), None, "its code is freed");
            let moved_after = mutator.jit.body_offset(&moved).expect("it survives");
            assert!(
                moved_after < moved_before,
                "it moved down over the freed code"
            );
            let register_base = mutator.heap.activation_register_base(stopped);
            let Resumption::Interpreted {
                program_counter: 0, ..
            } = resumed(mutator, stopped, &freed, register_base, 0)
            else {
                panic!("a frame stopped in freed code resumes interpreted");
            };
            mutator.heap.pop_activation(stopped);
            let running = push(mutator, &moved);
            let register_base = mutator.heap.activation_register_base(running);
            let Resumption::Returned { value } =
                resumed(mutator, running, &moved, register_base, 0)
            else {
                panic!("a frame of a moved body resumes in its machine code");
            };
            assert_eq!(value, crate::heap::Immediate::boolean(false).bits());
            mutator.heap.pop_activation(running);
        });
    }
}
