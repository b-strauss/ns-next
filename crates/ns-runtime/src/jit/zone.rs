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
//! used until a quarter of the bodies' room past the stubs is free, and
//! at least what the compile that found no room needed, plans each
//! survivor's new
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
//!
//! A lean body and its twin are one body to every step here: the twin is
//! freed whenever its lean body is, by death, abandonment, a moved base or
//! usage, and never alone, so that it takes the lean body's usage; each
//! copy moves on its own, and the lean body's transfer words, which name
//! the twin's code, are written afresh once the copies moved. Optimized
//! code is freed whenever its baseline body is, and here with it, the
//! pair judged by its more used copy; only its own exits free it alone
//! (`jit::abandon_optimized`).

#[cfg(ns_jit)]
use std::rc::Rc;

use ns_memory::CodeOffset;

use crate::bytecode::CompiledCode;
use crate::hash::IdentityMap;
use crate::interpreter::inline_cache::SiteSlide;
use crate::interpreter::Mutator;

use super::{InstalledBody, JitState, BODY_ALIGNMENT, COMPILED};

/// A body a slide freed (`JitState::free_bodies_with_moved_bases`): its
/// code, while it lives, whose machine entry row names it, and its entry's
/// offset, which cells may name.
pub(crate) struct FreedBody {
    code: Option<std::rc::Rc<CompiledCode>>,
    start: CodeOffset,
}

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

/// The compiles an exhausted zone holds before it asks for a compaction
/// again, after one compaction left it exhausted; each further fruitless
/// compaction in a row doubles it.
///
/// Cog compacts at every compile that finds no room
/// (`callForCogCompiledCodeCompaction`, `cointerp.c:12068-12074`), so its
/// zone is never exhausted for longer than the next failed compile; here
/// a compaction also walks every cell, so a zone full of active code
/// would pay that walk at every held entry. Doubling bounds the fruitless
/// compactions to the logarithm of the entries held, and the wait for the
/// first compaction after the active code returned to at most twice the
/// entries held since the zone became exhausted -- counted in entries, so
/// the rule depends neither on allocation, as a full collection does, nor
/// on time. A held compile is an interpreted entry of a body that came
/// due, and a fruitless compaction costs a walk over the cells and the
/// bodies, the order of a few dozen such entries: 64 keeps the retries
/// below what the held entries cost.
pub(crate) const COMPILES_HELD_BEFORE_RETRY: u32 = 64;

/// The most doublings of `COMPILES_HELD_BEFORE_RETRY`: as many as a `u32`
/// holds, 64 << 25 being its highest power of two.
const MAXIMUM_RETRY_DOUBLINGS: u32 = u32::BITS - 1 - COMPILES_HELD_BEFORE_RETRY.trailing_zeros();

/// The compiles an exhausted zone holds before it asks for a compaction
/// again, after `fruitless_compactions` compactions in a row left it
/// exhausted.
pub(crate) fn compiles_held_before_retry(fruitless_compactions: u32) -> u32 {
    let doublings = fruitless_compactions
        .saturating_sub(1)
        .min(MAXIMUM_RETRY_DOUBLINGS);
    COMPILES_HELD_BEFORE_RETRY << doublings
}

/// Which bodies a compaction frees beyond those always freed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Freeing {
    /// Cog's: the least used first, every body whose usage is at most 0,
    /// then at most 1, and so on below `MAXIMUM_USAGE`, until a quarter of
    /// the bodies' room past the stubs is free, and at least what the
    /// compile that found no room needed (`freeOlderMethodsForCompaction`,
    /// `:12897-12925`).
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

/// Whether a body is freed by a compaction or between compactions: the
/// bytes of one freed between them count towards the quarter a full
/// collection asks for a compaction at (`bytes_freed_since_compaction`),
/// while a compaction reclaims the bytes of those it frees itself, and so
/// counts none.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum FreedBy {
    BetweenCompactions,
    TheCompaction,
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

    /// Where the bodies ended before the compaction.
    pub(crate) fn old_end(&self) -> u32 {
        self.old_end
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
    /// Whether each installed body is unfreed and what its code's slot
    /// names, by index.
    #[cfg(ns_jit)]
    fn named_bodies(&self) -> Vec<bool> {
        (0..self.bodies.len())
            .map(|index| !self.bodies[index].free && self.still_named(index))
            .collect()
    }

    /// Every live body that has optimized code here: what the zone stress
    /// mode's sparing compaction keeps (`compact_code_zone_for_stress`).
    #[cfg(debug_assertions)]
    fn codes_with_optimized_code(&self) -> Vec<std::rc::Rc<CompiledCode>> {
        self.bodies
            .iter()
            .filter(|body| !body.free)
            .filter_map(|body| body.code.upgrade())
            .filter(|code| self.has_optimized_code(code))
            .collect()
    }

    /// The bodies whose machine code a compaction freed, those `named`
    /// said were what their slots named before it and that are freed now,
    /// which still live: a copy freed before, whose body may have been
    /// compiled again, is none of them.
    #[cfg(ns_jit)]
    fn freed_codes(&self, named: &[bool]) -> Vec<Rc<CompiledCode>> {
        self.bodies
            .iter()
            .zip(named)
            .filter(|(body, named)| body.free && **named)
            .filter_map(|(body, _)| body.code.upgrade())
            .collect()
    }

    /// Marks the body at `index` freed: no lookup finds it from now on,
    /// its returns are removed, and the next compaction reclaims its
    /// bytes, which count towards the quarter a full collection asks for a
    /// compaction at. Only between entries, where no return address into
    /// it exists.
    pub(super) fn free_body(&mut self, index: u32) {
        let twin = self.bodies.get(index as usize).and_then(|body| body.twin);
        // Optimized code goes with its baseline body, as the twin does.
        let optimized = self.optimized_copies_of(index);
        for copy in std::iter::once(index).chain(twin).chain(optimized) {
            self.free_one(copy);
        }
    }

    /// Marks the body at `index` alone freed and removes its returns, as
    /// `free_body` does each copy: optimized code abandoned while its
    /// baseline code stays (`jit::abandon_optimized`). Only between
    /// entries.
    pub(super) fn free_one(&mut self, index: u32) {
        if self.mark_free(index, FreedBy::BetweenCompactions) {
            self.returns.retain(|_, site| site.body != index);
        }
    }

    /// Marks the body at `index` freed, counting it, and its bytes as
    /// `freed_by` says, and answers whether it was not already; its returns
    /// are the caller's. The one way a body is marked freed, so that every
    /// caller asserts it runs between entries.
    fn mark_free(&mut self, index: u32, freed_by: FreedBy) -> bool {
        #[cfg(debug_assertions)]
        assert_eq!(
            self.entries_in_progress, 0,
            "internal: machine code is freed only between entries"
        );
        let Some(body) = self.bodies.get_mut(index as usize) else {
            return false;
        };
        if body.free {
            return false;
        }
        body.free = true;
        let size = body.size();
        self.statistics.count_freed(body.lean.is_some());
        if freed_by == FreedBy::BetweenCompactions {
            self.bytes_freed_since_compaction =
                self.bytes_freed_since_compaction.saturating_add(size);
        }
        true
    }

    /// Whether the body at `index` is still what its code's slot names:
    /// its code lives, and it was neither abandoned nor compiled again; for
    /// a twin, whether its lean body is, unfreed, and still has it as its
    /// twin; for optimized code, whether its baseline body is, and its
    /// slot names it as the optimized code beside it -- its one copy, or
    /// the copy its key's row names.
    fn still_named(&self, index: usize) -> bool {
        if let Some(baseline) = self.bodies[index].baseline {
            return self
                .optimized_copies_of(baseline)
                .iter()
                .any(|optimized| *optimized as usize == index);
        }
        if let Some(lean) = self.bodies[index].lean {
            let lean_body = &self.bodies[lean as usize];
            return !lean_body.free
                && lean_body.twin.is_some_and(|twin| twin as usize == index)
                && self.still_named(lean as usize);
        }
        self.bodies[index]
            .code
            .upgrade()
            .is_some_and(|code| self.named_body(&code) == Some(index))
    }

    /// Frees the machine code of every installed body that died -- its
    /// code no longer upgrades: its mixin's row was freed or superseded,
    /// and nothing runs it -- or that its slot no longer names, without
    /// waiting for the zone to fill, as Cog's full collection frees the
    /// machine code of every unmarked method (`freeUnmarkedMachineCode`,
    /// `cogitX64SysV.c:7473-7505`, from `cointerp.c:54977`). Run by a full
    /// collection after the ids are reclaimed (`World::collect`), between
    /// entries. The bytes freed count towards the quarter the collection
    /// asks for a compaction at.
    ///
    /// No holder needs writing: a dead body's code is held by no cache
    /// entry, whose method would hold it, so no cell names it; its code id
    /// was freed with it, which cleared its machine entry row; and no
    /// activation runs it. A body alive but no longer named was abandoned
    /// or freed by its usage, and so freed already, with its holders
    /// written then -- but for one compiled again into another
    /// interpreter's zone, whose cells here could still name it: that
    /// rests on no body being run by two interpreters
    /// (`docs/OPEN-WORK.md`, "A body shared by two interpreters"). A frame
    /// stopped in a body its slot no longer names resumes interpreted
    /// (`jit::resumed`).
    pub(crate) fn free_dead_bodies(&mut self) {
        let mut freed = Vec::new();
        for index in 0..self.bodies.len() {
            if self.bodies[index].free || self.still_named(index) {
                continue;
            }
            let Ok(number) = u32::try_from(index) else {
                unreachable!("internal: a body's index fits its slot's map");
            };
            if self.mark_free(number, FreedBy::BetweenCompactions) {
                freed.push(number);
            }
        }
        if !freed.is_empty() {
            // Ascending, as `bodies` is walked.
            self.returns
                .retain(|_, site| freed.binary_search(&site.body).is_err());
        }
    }

    /// Frees the machine code of every installed body compiled against a
    /// feedback base `slide` moved, or against one it holds no range for:
    /// its compiled sends address their cells from that base, and its
    /// closure templates write it into every closure they make, so the
    /// code would read and fill another family's sites. A body its slot
    /// still names is put back to counting its entries, so that it is
    /// compiled again against its family's new base on demand. Run by a
    /// full collection that slid the ranges, after `free_dead_bodies`,
    /// between entries; answers the bodies freed, each with its code while
    /// it lives and its entry's offset, for the holders the caller writes
    /// (`Mutator::free_code_of_moved_bases`).
    pub(crate) fn free_bodies_with_moved_bases(&mut self, slide: &SiteSlide) -> Vec<FreedBody> {
        let mut freed = Vec::new();
        let mut indices = Vec::new();
        for index in 0..self.bodies.len() {
            let body = &self.bodies[index];
            if body.free || !slide.moves(body.feedback_base) {
                continue;
            }
            let Ok(number) = u32::try_from(index) else {
                unreachable!("internal: a body's index fits its slot's map");
            };
            // A twin, compiled against its lean body's base, is freed with
            // it; no cell and no machine entry row names it.
            if body.lean.is_some() {
                if self.mark_free(number, FreedBy::BetweenCompactions) {
                    indices.push(number);
                }
                continue;
            }
            let code = body.code.upgrade();
            let start = body.start;
            if let Some(code) = &code {
                if self.named_body(code) == Some(index) {
                    code.jit.forget();
                }
            }
            if self.mark_free(number, FreedBy::BetweenCompactions) {
                indices.push(number);
                freed.push(FreedBody {
                    code,
                    start: CodeOffset(start),
                });
            }
        }
        if !indices.is_empty() {
            // Ascending, as `bodies` is walked.
            self.returns
                .retain(|_, site| indices.binary_search(&site.body).is_err());
        }
        freed
    }

    /// Whether a compile found no room since the last compaction, a held
    /// compile found the zone exhausted long enough, or a full collection
    /// freed a quarter of the bodies' room: the run loop's safepoint
    /// compacts the zone (`Interpreter::safepoint`).
    pub(crate) fn compaction_requested(&self) -> bool {
        self.compaction_requested
    }

    /// The zone has room again, or may have: the next compile tries for
    /// itself and asks again if it finds none, and the backoff of the
    /// retries starts afresh, so that a later exhaustion waits only on the
    /// compiles held since it began.
    fn end_exhaustion(&mut self) {
        self.zone_exhausted = false;
        self.wanted_bytes = 0;
        self.held_compiles = 0;
        self.fruitless_compactions = 0;
    }

    /// Whether a body that came due, with `slot`, may be compiled now:
    /// always while the zone is not exhausted; while it is, only if no
    /// compile found the zone without room for this body since it last
    /// had room (`JitSlot::needed`), or the room left holds what it needed.
    /// So an exhausted zone still compiles every body that fits the room
    /// it has, and a body that does not fit is assembled in vain at most
    /// once while the room stays as it is -- the room grows only at a
    /// compaction -- since a compile that finds no room records the bytes
    /// it needed and, while the zone is exhausted, asks for no compaction
    /// but counts as a held compile (`hold_compile`): no livelock between
    /// compiles and compactions is possible.
    pub(super) fn may_compile_now(&self, slot: &super::JitSlot) -> bool {
        self.may_compile_needing(slot.needed.get())
    }

    /// `may_compile_now` for a copy that needed `needed` bytes when a
    /// compile last found no room for it, 0 if none did: a lean body's
    /// slot records them, the lean body its twin's and its optimized
    /// code's.
    pub(super) fn may_compile_needing(&self, needed: u32) -> bool {
        if !self.zone_exhausted {
            return true;
        }
        needed == 0
            || self
                .zone
                .as_ref()
                .is_some_and(|zone| zone.limit.saturating_sub(zone.next_offset) >= needed)
    }

    /// A compile held because the zone is exhausted: once
    /// `compiles_held_before_retry` of them were held since the last
    /// compaction, another is asked for, which frees what the active code
    /// that exhausted the zone has since left. Every compaction halves the
    /// usage counts, so each retry finds less of what ran before kept.
    pub(super) fn hold_compile(&mut self) {
        self.held_compiles = self.held_compiles.saturating_add(1);
        if self.held_compiles >= compiles_held_before_retry(self.fruitless_compactions) {
            self.compaction_requested = true;
        }
    }

    /// A full collection ran, after it freed the dead bodies' code
    /// (`free_dead_bodies`): once the bytes freed since the last
    /// compaction reach a quarter of the bodies' room -- what Cog keeps as
    /// `methodBytesFreedSinceLastCompaction` (`cogitX64SysV.c:12887`)
    /// against the quarter its compaction aims at (`:12907-12911`) -- a
    /// compaction is asked for. Cog compacts only when a compile finds no
    /// room; asking here as well keeps the zone's committed end near what
    /// lives, and a compile from finding the zone full of dead code. A
    /// zone exhausted by active code is tried again, so that the next
    /// compile that finds no room compacts it once more. Answers whether a
    /// compaction is asked for, which the caller passes on to the
    /// safepoint request.
    pub(crate) fn note_full_collection(&mut self) -> bool {
        self.end_exhaustion();
        if let Some(zone) = self.zone.as_ref() {
            let room = zone.room();
            if room > 0 && self.bytes_freed_since_compaction >= room / 4 {
                self.compaction_requested = true;
            }
        }
        self.compaction_requested
    }

    /// Gives back the zone's committed pages above the end of its last
    /// body and `shrink`'s headroom once more than its threshold lies
    /// unused there (`heap::limits::Shrink`), and answers how many bytes
    /// that was. Run between entries, after a compaction slid the
    /// survivors down and at the end of a full collection: nothing runs
    /// from past the last body, and the next install there commits the
    /// pages again (`CodeWriter::decommit_above`).
    pub(crate) fn shrink_zone(
        &mut self,
        shrink: crate::heap::limits::Shrink,
        damped: bool,
    ) -> usize {
        #[cfg(debug_assertions)]
        assert_eq!(
            self.entries_in_progress, 0,
            "internal: the zone shrinks only between entries"
        );
        let Some(zone) = self.zone.as_mut() else {
            return 0;
        };
        match shrink.keep(
            &mut self.zone_damping,
            damped,
            zone.writer.take_grown(),
            zone.next_offset as usize,
            zone.writer.committed(),
        ) {
            Some(end) => zone.writer.decommit_above(end),
            None => 0,
        }
    }

    /// The index in `bodies` of the body `code`'s slot names in this zone,
    /// whether or not the JIT is on, while it is not freed.
    pub(super) fn named_body(&self, code: &CompiledCode) -> Option<usize> {
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

    /// The zone offset of `code`'s optimized code, if it has some here:
    /// its one copy's, or its first customized copy's. For tests.
    pub(crate) fn optimized_offset(&self, code: &CompiledCode) -> Option<u32> {
        let lean = u32::try_from(self.named_body(code)?).ok()?;
        let first = *self.optimized_copies_of(lean).first()?;
        Some(self.bodies[first as usize].start)
    }

    /// How many customized copies of `code`'s optimized code it has here.
    /// For tests.
    pub(crate) fn customized_copies(&self, code: &CompiledCode) -> usize {
        code.jit
            .copies
            .borrow()
            .iter()
            .filter(|copy| self.copy_of(code, copy.key).is_some())
            .count()
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

    /// Whether the machine code `code`'s slot names reifies its frame, if
    /// it has some: never, since every body compiles lean and only its
    /// twin reifies (`baseline::BodyCopy`). For tests.
    pub(crate) fn body_reifies(&self, code: &CompiledCode) -> Option<bool> {
        let index = self.named_body(code)?;
        Some(self.bodies[index].reifies)
    }

    /// The twin of the machine code `code`'s slot names, if it has one.
    fn named_twin(&self, code: &CompiledCode) -> Option<&InstalledBody> {
        let index = self.named_body(code)?;
        let twin = self.bodies[index].twin?;
        Some(&self.bodies[twin as usize])
    }

    /// The zone offset of the twin of the machine code `code`'s slot
    /// names, if it has one. For tests.
    pub(crate) fn twin_offset(&self, code: &CompiledCode) -> Option<u32> {
        Some(self.named_twin(code)?.start)
    }

    /// The data words of the twin of the machine code `code`'s slot names
    /// -- its closure sites -- as the zone holds them. For tests.
    pub(crate) fn twin_words(&self, code: &CompiledCode) -> Option<Vec<u64>> {
        let twin = self.named_twin(code)?;
        let zone = self.zone.as_ref()?;
        let bytes = zone.writer.bytes(twin.words..twin.words_end)?;
        Some(
            bytes
                .as_chunks::<8>()
                .0
                .iter()
                .map(|word| u64::from_le_bytes(*word))
                .collect(),
        )
    }

    /// A debug build's check, after every compaction and every slide of
    /// the site ranges, that the lean bodies and their twins are paired:
    /// a live lean body's twin is live and names it back, a live twin's
    /// lean body is live and names it, and every transfer word of a live
    /// lean body names its `MakeClosure`'s code in the twin, or is 0 for a
    /// body without one.
    #[cfg(debug_assertions)]
    pub(crate) fn verify_pairs(&self) {
        let Some(zone) = self.zone.as_ref() else {
            return;
        };
        for (index, body) in self.bodies.iter().enumerate() {
            if body.free {
                continue;
            }
            if let Some(lean) = body.lean {
                let lean = &self.bodies[lean as usize];
                assert!(
                    !lean.free && lean.twin.is_some_and(|twin| twin as usize == index),
                    "internal: a live twin's lean body is freed or names another twin"
                );
                continue;
            }
            // Optimized code transfers into its baseline body's twin.
            let owner = match body.baseline {
                Some(baseline) => {
                    let owner = &self.bodies[baseline as usize];
                    assert!(
                        !owner.free && body.twin.is_none(),
                        "internal: live optimized code's baseline body is freed, or it has a twin"
                    );
                    owner
                }
                None => body,
            };
            let twin = owner.twin.map(|twin| &self.bodies[twin as usize]);
            if let Some(twin) = twin.filter(|_| body.baseline.is_none()) {
                assert!(
                    !twin.free && twin.lean.is_some_and(|lean| lean as usize == index),
                    "internal: a live lean body's twin is freed or names another body"
                );
            }
            for transfer in &body.twin_transfers {
                let start = body.start + transfer.offset;
                let Some(bytes) = zone.writer.bytes(start..start + 8) else {
                    unreachable!("internal: an installed body's words lie in the committed zone");
                };
                let mut word = [0; 8];
                word.copy_from_slice(&bytes);
                let expected = twin.map_or(0, |twin| {
                    let offset = twin
                        .map
                        .machine_offset(transfer.program_counter)
                        .unwrap_or(0);
                    zone.writer
                        .zone()
                        .executable_address(CodeOffset(twin.start + offset))
                        as u64
                });
                assert_eq!(
                    u64::from_le_bytes(word),
                    expected,
                    "internal: a transfer word does not name its twin's code"
                );
            }
        }
    }

    /// Step 1, Cog's `markActiveMethodsAndReferents` (`cointerp.c:17709-17725`):
    /// every body an activation runs, on the activation stack or suspended
    /// into its frame, is set to `MAXIMUM_USAGE`, so that freeing by usage
    /// keeps it.
    fn mark_active(&self, active: &[std::rc::Rc<CompiledCode>]) {
        for code in active {
            if let Some(index) = self.named_body(code) {
                let body = &self.bodies[index];
                body.usage.set(MAXIMUM_USAGE);
                let optimized = u32::try_from(index)
                    .map(|index| self.optimized_copies_of(index))
                    .unwrap_or_default();
                for copy in body.twin.into_iter().chain(optimized) {
                    self.bodies[copy as usize].usage.set(MAXIMUM_USAGE);
                }
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
            if self.bodies[index].free || self.still_named(index) {
                continue;
            }
            let Ok(number) = u32::try_from(index) else {
                unreachable!("internal: a body's index fits its slot's map");
            };
            // The slide drops the returns of every freed body.
            self.mark_free(number, FreedBy::TheCompaction);
        }
    }

    /// Step 3, Cog's `freeOlderMethodsForCompaction` (`:12897-12925`):
    /// frees bodies by their usage as `freeing` says, each put back to
    /// counting its entries, until the freed bodies and the room past the
    /// last body count `wanted` bytes. Answers whether it freed any, and
    /// the bytes so counted.
    fn free_by_usage(&mut self, freeing: Freeing, wanted: u32) -> (bool, u32) {
        let Some(zone) = self.zone.as_ref() else {
            return (false, 0);
        };
        let mut freed_any = false;
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
            for index in 0..self.bodies.len() {
                if freeing == Freeing::ToAQuarter && free_bytes >= wanted {
                    return (freed_any, free_bytes);
                }
                let body = &self.bodies[index];
                // A twin, and optimized code, are freed here with their
                // lean body, never alone.
                if body.free || body.lean.is_some() || body.baseline.is_some() {
                    continue;
                }
                let Ok(number) = u32::try_from(index) else {
                    unreachable!("internal: a body's index fits its slot's map");
                };
                // The body is as used as its most used copy: once a body
                // is optimized, its cells and its row name the optimized
                // code, whose links raise that copy's usage alone, so that
                // the lean body's would make a hot optimized body the
                // first a compaction frees.
                let optimized = self.optimized_copies_of(number);
                let usage = optimized
                    .iter()
                    .map(|optimized| self.bodies[*optimized as usize].usage.get())
                    .fold(body.usage.get(), u8::max);
                if usage > freeable {
                    continue;
                }
                let (size, code, twin) = (body.size(), body.code.upgrade(), body.twin);
                self.mark_free(number, FreedBy::TheCompaction);
                freed_any = true;
                free_bytes += size;
                // Alive: a body whose code died was freed in step 2.
                if let Some(code) = code {
                    code.jit.forget();
                }
                for copy in twin.into_iter().chain(optimized) {
                    let copy_size = self.bodies[copy as usize].size();
                    if self.mark_free(copy, FreedBy::TheCompaction) {
                        free_bytes += copy_size;
                    }
                }
            }
            if freeing == Freeing::EveryInactiveBody {
                return (freed_any, free_bytes);
            }
            freeable += 1;
        }
        (freed_any, free_bytes)
    }

    /// Steps 3 and 4: frees by usage until the plan of the survivors'
    /// new places leaves `wanted` bytes past the last, and answers that
    /// plan. Step 3 counts the freed bodies' sizes, which lie apart, while
    /// the room the plan leaves is one gap past the re-aligned survivors,
    /// which can be less: so step 3 runs again for the shortfall until the
    /// plan leaves enough or nothing more is freed, and a compaction that
    /// reached its aim by its own count never leaves the zone exhausted.
    fn plan_freeing(&mut self, freeing: Freeing, wanted: u32) -> Option<CompactionPlan> {
        let mut counted = wanted;
        let mut first = true;
        loop {
            let (freed_any, free_bytes) = self.free_by_usage(freeing, counted);
            let plan = self.plan()?;
            let left = self
                .zone
                .as_ref()
                .map_or(0, |zone| zone.limit.saturating_sub(plan.new_end()));
            if freeing == Freeing::EveryInactiveBody || left >= wanted || !(freed_any || first) {
                return Some(plan);
            }
            // The shortfall past what step 3 counted, so that it frees at
            // least one more body, or finds none left to free.
            counted = free_bytes.saturating_add(wanted - left);
            first = false;
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
    /// installed bodies, where each survivor's slot says its code lies (its
    /// exits kept, `JitSlot::relocated`), and the return sites keyed by
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
        for (old_index, mut body) in old_bodies.into_iter().enumerate() {
            if body.free {
                // A row naming a freed copy would name whichever body
                // takes its index now.
                if let (Some(key), Some(code)) = (body.key, body.code.upgrade()) {
                    if code
                        .jit
                        .copy_row(key)
                        .is_some_and(|row| row.body as usize == old_index)
                    {
                        code.jit.set_copy_body(key, super::NO_BODY);
                    }
                }
                renumbered.push(None);
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
                let Some(bytes) = zone.writer.bytes(body.start..body.end) else {
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
            // A twin's slot is its lean body's, which names the lean body;
            // optimized code's slot names it beside the lean body, as its
            // one copy or in its key's row.
            let code = body.code.upgrade();
            if let Some(code) = code.as_ref().filter(|_| body.baseline.is_some()) {
                let named = match body.key {
                    Some(key) => code.jit.copy_row(key).map(|row| row.body),
                    None => Some(code.jit.optimized.get()),
                };
                debug_assert_eq!(
                    named.map(|named| named as usize),
                    Some(old_index),
                    "internal: a compaction moves only optimized code its slot names"
                );
                match body.key {
                    Some(key) => code.jit.set_copy_body(key, index),
                    None => code.jit.optimized.set(index),
                }
            } else if let Some(code) = code.filter(|_| body.lean.is_none()) {
                let first_instruction = code.jit.first_instruction.get() - delta;
                debug_assert_eq!(
                    code.jit.zone.get(),
                    identity,
                    "internal: a survivor's slot names this zone"
                );
                code.jit.relocated(
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
        // The pairs renumbered: a twin survives exactly when its lean body
        // does, since the two are freed together.
        let renumber = |old: Option<u32>| {
            old.and_then(|index| renumbered.get(index as usize).copied().flatten())
                .map(|(index, _)| index)
        };
        for body in &mut self.bodies {
            body.twin = renumber(body.twin);
            body.lean = renumber(body.lean);
            body.baseline = renumber(body.baseline);
        }
        // Every byte the survivors no longer cover -- the gaps their
        // alignment leaves and the room freed past the last -- is filled
        // with stops, as Cog fills the freed end of its zone
        // (`stopsFromto`, `compactCogCompiledCode`, `:5738`): an address a
        // holder failed to remap traps there rather than running stale code.
        let mut covered = zone.bodies_start;
        for body in &self.bodies {
            fill_with_stops(zone, covered, body.start);
            covered = body.end;
        }
        fill_with_stops(zone, covered, plan.old_end());
        zone.next_offset = plan.new_end();
        self.statistics.zone_compactions += 1;
        self.statistics.moved_bytes += moved_bytes;
        // The transfer words name the twin's code, which may have moved
        // apart from the lean body and the optimized code beside it: each
        // written afresh at its body's new place.
        for index in 0..self.bodies.len() {
            let body = &self.bodies[index];
            let owner = body
                .baseline
                .map_or(body, |baseline| &self.bodies[baseline as usize]);
            if body.lean.is_none() && owner.twin.is_some() {
                let Ok(index) = u32::try_from(index) else {
                    unreachable!("internal: a body's index fits its slot's map");
                };
                self.write_transfer_words(index);
            }
        }
    }
}

/// Writes stops over the zone's bytes from `start` to `end`.
#[cfg(ns_jit)]
fn fill_with_stops(zone: &mut super::Zone, start: u32, end: u32) {
    if start >= end {
        return;
    }
    let stops = super::macro_assembler::stop_bytes(
        super::macro_assembler::Abi::host(),
        (end - start) as usize,
    );
    if zone.writer.write(CodeOffset(start), &stops).is_err() {
        unreachable!("internal: the room a body left lies in the committed zone");
    }
}

#[cfg(not(ns_jit))]
fn fill_with_stops(_zone: &mut super::Zone, _start: u32, _end: u32) {}

impl Mutator<'_> {
    /// Compacts the code zone at the run loop's safepoint, once a compile
    /// found no room (`JitState::compaction_requested`): Cog's
    /// `compactCogCompiledCode` (`:5723-5744`), freeing the least used
    /// bodies until a quarter of the bodies' room past the stubs is free,
    /// and at least what the compile that found no room needed.
    pub(crate) fn compact_code_zone(&mut self) {
        self.compact_code_zone_freeing(Freeing::ToAQuarter, Vec::new());
    }

    /// The compaction the zone stress mode makes (`NS_NEXT_STRESS_ZONE`):
    /// every body no activation runs freed -- but, when
    /// `spare_optimized`, every body that has optimized code, which the
    /// compaction slides and remaps with the rest. The mode alternates
    /// the two: freed at every compaction, a callee's optimized code,
    /// installed while the callee still ran its baseline code, would be
    /// freed at its caller's next poll, before any call entered it, and
    /// never run at an address a compaction gave it; spared at every
    /// other one, it is moved, then run where it lies now, then freed and
    /// compiled again.
    #[cfg(debug_assertions)]
    pub(crate) fn compact_code_zone_for_stress(&mut self, spare_optimized: bool) {
        let spared = if spare_optimized {
            self.jit.codes_with_optimized_code()
        } else {
            Vec::new()
        };
        self.compact_code_zone_freeing(Freeing::EveryInactiveBody, spared);
    }

    /// The compaction, in Cog's order: the bodies marked, freed and
    /// planned; every address outside the bodies remapped before the bytes
    /// move, as Cog updates the stack zone and relocates its methods'
    /// references before it copies
    /// (`updateStackZoneReferencesToCompiledCodePreCompaction`,
    /// `cointerp.c:21546-21596`; `relocateMethodsPreCompaction`,
    /// `:13400-13417`); the bytes slid and the records rewritten; the
    /// request cleared, and the exhaustion once that much is free.
    fn compact_code_zone_freeing(
        &mut self,
        freeing: Freeing,
        spared: Vec<std::rc::Rc<CompiledCode>>,
    ) {
        #[cfg(debug_assertions)]
        assert_eq!(
            self.jit.entries_in_progress, 0,
            "internal: the zone is compacted only between entries"
        );
        self.jit.compaction_requested = false;
        self.jit.held_compiles = 0;
        self.jit.bytes_freed_since_compaction = 0;
        let Some(wanted) = self
            .jit
            .zone
            .as_ref()
            .map(|zone| self.jit.wanted_free_bytes(zone))
        else {
            self.jit.zone_exhausted = false;
            return;
        };
        // The bodies still what their slots name before this compaction
        // frees any: only their rows are cleared below, never that of a
        // body compiled again since an older copy of it was freed.
        #[cfg(ns_jit)]
        let named_before = self.jit.named_bodies();
        let mut active = self.heap.active_codes();
        active.extend(spared);
        self.jit.mark_active(&active);
        drop(active);
        self.jit.free_unnamed();
        let Some(plan) = self.jit.plan_freeing(freeing, wanted) else {
            return;
        };
        self.caches.remap_cell_entries(&plan);
        #[cfg(ns_jit)]
        self.heap.remap_machine_entries(&plan);
        // A freed body's next compile arms its budget afresh, as its
        // machine entry row is written afresh; and the slide renumbers the
        // bodies a word still to be pointed names, which its first spend
        // points instead.
        #[cfg(ns_jit)]
        for code in self.jit.freed_codes(&named_before) {
            self.heap.forget_optimization_budget(&code);
        }
        self.jit.budget_words_to_point.clear();
        self.jit.slide(&plan);
        // After the slide, which lowered the zone's next offset: what the
        // freed bodies left past it is given back by the code zone's pair.
        let pairs = self.heap.shrink_pairs();
        let bytes = self.jit.shrink_zone(pairs.code_zone, pairs.damped);
        self.heap.count_decommit(bytes);
        let enough = self
            .jit
            .zone
            .as_ref()
            .is_some_and(|zone| zone.limit.saturating_sub(zone.next_offset) >= wanted);
        if enough {
            self.jit.end_exhaustion();
        } else if self.jit.zone_exhausted {
            // The next retry waits for twice the compiles this one did,
            // and aims at what the refused compile needed still: a retry
            // that freed a quarter but not that would end the exhaustion
            // only for the compile to find no room again at once.
            self.jit.fruitless_compactions = self.jit.fruitless_compactions.saturating_add(1);
        } else {
            self.jit.wanted_bytes = 0;
        }
        #[cfg(all(ns_jit, debug_assertions))]
        self.verify_code_zone_holders();
    }

    /// Step 4 of a slide (`docs/design/dispatch.md`, "Code families"):
    /// frees the machine code of every body compiled against a feedback
    /// base `slide` moved (`JitState::free_bodies_with_moved_bases`), then
    /// clears each one's machine entry row, whose `FeedbackBase` word names
    /// the old base, and writes every cell that named one afresh from its
    /// entry, which names the miss stub now -- so that no closure stub and
    /// no cached send enters freed code. A frame stopped in a freed body
    /// resumes interpreted (`jit::resumed`). Run by `World::collect` after
    /// the caches and the heap took the new bases, between entries.
    pub(crate) fn free_code_of_moved_bases(&mut self, slide: &SiteSlide) {
        self.free_moved_code(slide);
        // As after a compaction: every slide of the collect stress mode
        // checks that no cell and no machine entry row names freed code.
        #[cfg(all(ns_jit, debug_assertions))]
        self.verify_code_zone_holders();
    }

    /// `free_code_of_moved_bases`'s work, before its check.
    fn free_moved_code(&mut self, slide: &SiteSlide) {
        let freed = self.jit.free_bodies_with_moved_bases(slide);
        if freed.is_empty() {
            return;
        }
        let Some(zone) = self.jit.zone.as_ref() else {
            return;
        };
        let mut entries: Vec<u64> = freed
            .iter()
            .map(|body| zone.writer.zone().executable_address(body.start) as u64)
            .collect();
        entries.sort_unstable();
        for body in &freed {
            if let Some(code) = &body.code {
                #[cfg(ns_jit)]
                self.heap.forget_machine_entry(code);
                #[cfg(not(ns_jit))]
                let _ = code;
            }
        }
        self.caches.refresh_cells_entering_any(&entries, &self.jit);
    }

    /// A debug build's check, after every compaction and every slide of the
    /// site ranges, that every holder of an address into the zone was
    /// remapped: every cell agrees with its entry, which names a live
    /// body's start, a stub, or the miss stub (`InlineCaches::verify_cells`),
    /// and every machine entry row names the start of a lean body or of
    /// optimized code the zone holds and has not freed; no cell names a
    /// twin's start. The zone and collect stress modes so check every
    /// compaction and every slide they make.
    #[cfg(all(ns_jit, debug_assertions))]
    fn verify_code_zone_holders(&self) {
        if let Err(message) = self.caches.verify_cells(&self.heap, &self.jit) {
            panic!("internal: a cell names stale code after a compaction or a slide: {message}");
        }
        let Some(zone) = self.jit.zone.as_ref() else {
            return;
        };
        let start_of = |body: &InstalledBody| {
            zone.writer
                .zone()
                .executable_address(CodeOffset(body.start)) as u64
        };
        // Only a lean body or the optimized code beside it is ever named:
        // a twin is entered by its lean body's and its optimized code's
        // transfers and by a resumption alone.
        let starts: Vec<u64> = self
            .jit
            .bodies
            .iter()
            .filter(|body| !body.free && body.lean.is_none())
            .map(start_of)
            .collect();
        let twins: Vec<u64> = self
            .jit
            .bodies
            .iter()
            .filter(|body| !body.free && body.lean.is_some())
            .map(start_of)
            .collect();
        for entry in self.heap.recorded_machine_entries() {
            assert!(
                starts.contains(&entry),
                "internal: a machine entry row names {entry:#x}, no live lean body's start, \
                 after a compaction or a slide"
            );
        }
        for entry in self.caches.cell_entries() {
            assert!(
                !twins.contains(&entry),
                "internal: a cell names {entry:#x}, a twin's start, after a compaction or a slide"
            );
        }
        self.jit.verify_pairs();
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

    /// A hot optimized body's pair is judged by its more used copy: its
    /// cells and row link its optimized code, whose usage they raise,
    /// while its lean body's is halved by every slide, so that a freeing
    /// by usage frees a colder body first.
    #[cfg(ns_jit)]
    #[test]
    fn a_hot_optimized_body_outlives_a_colder_one_at_a_usage_compaction() {
        use crate::jit::tests::machine::{body_with_a_loop, enter_twice};
        let mut interpreter =
            crate::interpreter::Interpreter::bootstrap_with(crate::Lowering::Optimized);
        interpreter.set_jit_enabled(true);
        interpreter.enter(|mutator| {
            let cold = body_with_a_loop();
            let hot = body_with_a_loop();
            for code in [&cold, &hot] {
                enter_twice(&mut mutator.jit, code);
            }
            crate::jit::after_compile_trigger(mutator);
            mutator
                .jit
                .optimization_requests
                .push(crate::jit::OptimizationRequest::uncustomized(&hot));
            crate::jit::optimize_requested(mutator);
            let (cold_lean, hot_lean) = (cold.jit.map.get(), hot.jit.map.get());
            let optimized = hot.jit.optimized.get();
            assert_ne!(optimized, crate::jit::NO_BODY, "the hot body is optimized");
            // Linked since its optimization, its optimized code is the
            // copy whose usage went up, while slides halved its lean
            // body's; the cold body is used, a little.
            mutator.jit.bodies[hot_lean as usize].usage.set(0);
            mutator.jit.bodies[optimized as usize]
                .usage
                .set(super::LINKED_USAGE_CEILING);
            mutator.jit.bodies[cold_lean as usize].usage.set(1);
            let (_, free) = mutator.jit.free_by_usage(super::Freeing::ToAQuarter, 0);
            let (freed_any, _) = mutator
                .jit
                .free_by_usage(super::Freeing::ToAQuarter, free + 1);
            assert!(freed_any);
            assert!(
                mutator.jit.bodies[cold_lean as usize].free,
                "the colder body is freed"
            );
            assert!(!mutator.jit.bodies[hot_lean as usize].free);
            assert!(!mutator.jit.bodies[optimized as usize].free);
            assert!(mutator.jit.optimized(&hot).is_some());
        });
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
