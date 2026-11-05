//! Inline caches (`docs/design/dispatch.md`, "3b -- The caches"): the table of
//! every call site's entry, per heap, reached from an activation's
//! `feedback_base` -- V8's split of shared code and per-context feedback,
//! reached the way HotSpot's interpreter reaches `MethodData` -- with a
//! polymorphic block pool and a global cache behind every site.
//!
//! A cache records what `resolve` decided and never decides: a hit is `act`
//! on a `Resolved` rebuilt from an entry, so it can do nothing a miss would
//! not (`resolve.rs`).

pub(crate) mod cells;

use super::Arguments;
use std::collections::{BTreeMap, BTreeSet};
use std::hash::BuildHasher;
use std::rc::Rc;

use crate::builtins::primitive::Primitive;
use crate::bytecode::{CachedSend, CachedSendKind};
use crate::bytecode::{CallSite, CompiledCode, Instruction};
use crate::dispatch::{self, LexicalOrigin};
use crate::hash::IdentityMap;
use crate::heap::limits::{GrowthLimit, SITE_FLOOR};
use crate::heap::reclaim::FreedIds;
use crate::heap::{ActivationReference, ApplicationIndex, Heap};
use crate::jit::{self, JitState};
use crate::mixin::{AccessorKind, DispatchId, Mixin, MixinMethod};
use crate::types::*;
use crate::Symbol;

use super::resolve::{Chain, Resolved};
use super::sends::{decide_lexical_send, decide_super_send};
use super::{Mutator, Signal, Step};
use cells::{Cell, Cells, ChainStart, GlobalRow, CHAIN_START_BITS, VACANT_GUARD};

/// Where an activation's code family starts in its heap's cache table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeedbackBase(u32);

impl FeedbackBase {
    /// A closure decoded from an image, or a method step the uncached path
    /// decided: the range is found or assigned when the activation is pushed.
    pub const NOT_ASSIGNED: FeedbackBase = FeedbackBase(u32::MAX);

    pub(crate) fn index(self) -> usize {
        self.0 as usize
    }

    /// The number a closure object stores in its feedback-base word.
    pub(crate) fn raw(self) -> u32 {
        self.0
    }

    /// The base a closure object's feedback-base word names.
    pub(crate) fn from_raw(raw: u32) -> FeedbackBase {
        FeedbackBase(raw)
    }
}

/// A cache entry's target: a `Resolved` with its defining class replaced by
/// the number of superclass steps from the chain's start, so that it holds no
/// handle (`docs/design/dispatch.md`, "An entry"). `Vacant`, `Polymorphic`
/// and `Megamorphic` are site states, guarded by `DispatchId::VACANT`.
#[derive(Clone)]
pub(crate) enum Target {
    Vacant,
    Method {
        method: Rc<MixinMethod>,
        /// The method's family's range, so that a hit's push need not ask
        /// the map for it.
        feedback_base: FeedbackBase,
        level: u16,
        chain: Chain,
    },
    Slot {
        field: u32,
        kind: AccessorKind,
        chain: Chain,
        /// The application whose own slot `field` is.
        level: ApplicationIndex,
        /// For a private or super site, the guard of the receiver the entry
        /// was filled for when its chain contains `level`, and
        /// `DispatchId::NONE` otherwise (`record`): the one compare a hit's
        /// `Mutator::slot_level_holds` makes before its cold check.
        recorded_guard: DispatchId,
    },
    NestedClass {
        mixin: Rc<Mixin>,
        /// The absolute index of the hidden slot `mixin`'s class is cached
        /// in, in the layout of the application the entry's guard names
        /// (`record`); `nested::NO_CACHED_FIELD` where that application
        /// reserves none or the index passes `u32`.
        field: u32,
        level: u16,
        chain: Chain,
    },
    Primitive {
        primitive: Primitive,
        level: u16,
        chain: Chain,
    },
    /// §6.3's primary factory. It carries no level, unlike every other
    /// target: §6.3 instantiates the *receiver*, a class object, and the
    /// guard -- its metaclass's instances id -- already names the one
    /// application whose own method table held the entry -- and so the
    /// entry's body, `code`, which the fill records as `Method` records its
    /// method.
    Factory {
        code: Rc<crate::bytecode::CompiledCode>,
        chain: Chain,
    },
    /// The site's entries are `block` in the pool. `private` is the site's
    /// §5.9 decision, kept across the transition.
    Polymorphic {
        block: u32,
        private: bool,
    },
    Megamorphic {
        private: bool,
    },
}

impl Target {
    /// Whether the site this target stands at is a private lexical site;
    /// `None` while the site is vacant and the decision not yet made.
    fn site_privacy(&self) -> Option<bool> {
        match self {
            Target::Vacant => None,
            Target::Polymorphic { private, .. } | Target::Megamorphic { private } => Some(*private),
            Target::Method { chain, .. }
            | Target::Slot { chain, .. }
            | Target::NestedClass { chain, .. }
            | Target::Primitive { chain, .. }
            | Target::Factory { chain, .. } => Some(*chain == Chain::Private),
        }
    }
}

#[derive(Clone)]
pub(crate) struct CacheEntry {
    pub(crate) guard: DispatchId,
    pub(crate) target: Target,
}

impl CacheEntry {
    /// Not a `const`: `Target` holds `Rc`s, which a constant cannot drop.
    fn vacant() -> CacheEntry {
        CacheEntry {
            guard: DispatchId::VACANT,
            target: Target::Vacant,
        }
    }

    fn vacant_block() -> [CacheEntry; POLYMORPHIC_WIDTH] {
        std::array::from_fn(|_| CacheEntry::vacant())
    }
}

/// The rule a global-cache key names: the same receiver id and selector
/// resolve differently under §5.5, §5.9 and §5.10. `Lexical` carries N's
/// application (its `instances` dispatch id) -- `DispatchId::NONE` where it
/// is not yet known -- so that the key does not need a separate component for
/// it: §5.9's private rule is a function of N and the selector alone, never
/// of `o`, so a `Lexical` entry recorded for one N is right to reuse only at
/// a site whose own N matches, and this is what lets a still-vacant lexical
/// site read the cache at all before it has resolved anything
/// (`docs/design/dispatch.md`, "3b -- The caches"). `LexicalPrivate` needs no
/// payload: its `guard` is already N's application alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum CacheRule {
    Public,
    Lexical(DispatchId),
    LexicalPrivate,
    Super,
}

pub(crate) const POLYMORPHIC_WIDTH: usize = 4;
pub(crate) const GLOBAL_CACHE_ENTRIES: usize = 1024;

/// The free ranges of the site table, by start, never two adjacent.
#[derive(Default)]
struct FreeRanges {
    /// Start -> size.
    ranges: BTreeMap<u32, u32>,
    /// The sites the ranges hold together, kept by every change to them:
    /// what the sites' growth limit subtracts from the table's length.
    free_sites: usize,
}

impl FreeRanges {
    /// The first range of at least `size` sites, taken; its remainder stays
    /// free.
    fn take_first_fit(&mut self, size: u32) -> Option<u32> {
        let (start, range_size) = self
            .ranges
            .iter()
            .find(|(_, range_size)| **range_size >= size)
            .map(|(start, range_size)| (*start, *range_size))?;
        self.ranges.remove(&start);
        if range_size > size {
            self.ranges.insert(start + size, range_size - size);
        }
        self.free_sites -= size as usize;
        Some(start)
    }

    /// Makes `start..start + size` free, merged with its neighbours, so that
    /// two adjacent freed families serve one larger one.
    fn free(&mut self, start: u32, size: u32) {
        // An empty range would sit at another one's start or end and break
        // the rule that no two are adjacent.
        debug_assert!(size > 0, "internal: an empty range is never freed");
        self.free_sites += size as usize;
        let mut start = start;
        let mut size = size;
        if let Some((&before_start, &before_size)) = self.ranges.range(..start).next_back() {
            if before_start + before_size == start {
                self.ranges.remove(&before_start);
                start = before_start;
                size += before_size;
            }
        }
        if let Some(after_size) = self.ranges.remove(&(start + size)) {
            size += after_size;
        }
        self.ranges.insert(start, size);
    }

    /// Removes and answers the free range that ends at `end`, if one does:
    /// what `InlineCaches` truncates `sites` by.
    fn take_ending_at(&mut self, end: u32) -> Option<(u32, u32)> {
        let (&start, &size) = self.ranges.range(..end).next_back()?;
        if start + size != end {
            return None;
        }
        self.ranges.remove(&start);
        self.free_sites -= size as usize;
        Some((start, size))
    }
}

pub(crate) struct InlineCaches {
    /// Every call site of every family that has run, `VACANT` until filled.
    sites: Vec<CacheEntry>,
    /// Polymorphic blocks, four entries each in the order first seen --
    /// V8's width, Strongtalk's shape.
    pool: Vec<[CacheEntry; POLYMORPHIC_WIDTH]>,
    /// Blocks a megamorphic transition or an invalidation gave back.
    free_blocks: Vec<u32>,
    /// Ranges of `sites` whose family died, served first by `fresh_range`.
    free_ranges: FreeRanges,
    global: GlobalCache,
    /// Every entry's machine-readable cell -- the sites', the blocks' and
    /// the global cache's -- in a reservation of its own (`cells.rs`),
    /// written wherever the entry is.
    cells: Cells,
    /// The last receiver guard a private or super slot accessor's cold
    /// check found to have a level, with that level
    /// (`Mutator::slot_level_holds`): a verification the check made, not a
    /// decision, so the next access by a receiver of that application to
    /// that level's slots passes with one compare wherever its site is.
    ///
    /// It rests on a guard naming one application whose chain does not
    /// change while the caches persist. So `DispatchId::NONE`, which names
    /// no one application -- every application once the ids run out
    /// (`Mutator::mint_dispatch_ids`), a class without one in a crafted
    /// image -- is never recorded nor answered; and whatever changes a
    /// class object's application or chain under an unchanged guard
    /// (`Heap::class_set_application` on a superclass changes every
    /// subclass's chain) must invalidate the caches after it, as road
    /// step 5b2a's atomic install does: after re-applying, itself when it
    /// migrates no instance, else through the bulk forward
    /// (`heap::forward_identity`), which vacates them.
    verified_slot_level: Option<(DispatchId, ApplicationIndex)>,
    /// Root body identity -> its family. A collection drops a family nothing
    /// runs any more (`forget_families`).
    families: IdentityMap<usize, Family>,
    /// The sites' growth limit (`heap::limits`): held here, beside the
    /// sites, since the caches have no heap to compare against.
    site_limit: GrowthLimit,
    /// Whether a new range grew `sites` to `site_limit` since the last
    /// collection (`wants_collection`).
    collection_wanted: bool,
    /// The range each closure an image decoded was given at its first
    /// activation (`fresh_loose_range`), start to size: no family's, so
    /// recorded here, that a full collection frees it as it frees a
    /// family's (`forget_families`) and the slide moves it while a closure
    /// or an activation still names it (`plan_slide`).
    loose_ranges: BTreeMap<u32, u32>,
    /// How many slides ran, and how many sites they moved
    /// (`CacheStatistics`).
    site_slides: u64,
    sites_moved: u64,
    /// Whether a full collection slides whenever any range is free, not
    /// only once a quarter of the table is: the collect stress mode's
    /// (`Interpreter::set_stress_collect`), so that its collections
    /// exercise every slide.
    #[cfg(debug_assertions)]
    eager_slides: bool,
}

/// A family that has run: its range and its root body, held so that the
/// body's address -- the key in `families` -- is never another body's
/// while the entry lives.
struct Family {
    base: FeedbackBase,
    code: Rc<CompiledCode>,
}

impl InlineCaches {
    pub(crate) fn new() -> InlineCaches {
        let mut cells = Cells::new();
        cells.vacate_global_rows(0..GLOBAL_CACHE_ENTRIES);
        InlineCaches {
            sites: Vec::new(),
            pool: Vec::new(),
            free_blocks: Vec::new(),
            free_ranges: FreeRanges::default(),
            global: GlobalCache::with_groups(GLOBAL_CACHE_ENTRIES / 2),
            cells,
            verified_slot_level: None,
            families: IdentityMap::default(),
            site_limit: GrowthLimit::at_floor(SITE_FLOOR),
            collection_wanted: false,
            loose_ranges: BTreeMap::new(),
            site_slides: 0,
            sites_moved: 0,
            #[cfg(debug_assertions)]
            eager_slides: false,
        }
    }

    /// `code`'s range, assigned the first time its family's root is
    /// activated, so that a family that never runs takes no room -- as V8
    /// allocates a feedback vector lazily. A body with no sends is given
    /// base 0 and no room: no site of it is ever indexed.
    ///
    /// A new range may grow the table past its limit; the caller asks
    /// `wants_collection` after it (`Mutator::feedback_base_for`).
    pub(crate) fn base_for(&mut self, code: &Rc<CompiledCode>) -> FeedbackBase {
        if let Some(base) = self.known_base(code) {
            return base;
        }
        let key = Rc::as_ptr(code) as usize;
        let base = self.fresh_range(code.family_size);
        self.families.insert(
            key,
            Family {
                base,
                code: Rc::clone(code),
            },
        );
        base
    }

    /// `code`'s range when its family already has one, or needs none:
    /// `base_for` without minting.
    #[inline]
    pub(crate) fn known_base(&self, code: &Rc<CompiledCode>) -> Option<FeedbackBase> {
        if code.family_size == 0 {
            return Some(FeedbackBase(0));
        }
        self.families
            .get(&(Rc::as_ptr(code) as usize))
            .map(|family| family.base)
    }

    /// Whether `site`, in the range at `feedback_base` of a family of
    /// `family_size`, has cached a receiver whose dispatch id is not
    /// `small_integer`: an entry of another guard, at the site or in its
    /// polymorphic block, or a megamorphic site, which only receivers of
    /// several ids make. The optimizing compiler reads it as the feedback
    /// of an integer fast path whose fallback the site is, so that a fast
    /// path the program's numbers fail is not compiled
    /// (`optimizing::snapshot::Snapshot::take`); it decides nothing a send
    /// runs.
    pub(crate) fn saw_receiver_other_than(
        &self,
        feedback_base: FeedbackBase,
        family_size: u32,
        site: CallSite,
        small_integer: DispatchId,
    ) -> bool {
        let Some(entry) =
            site_index(feedback_base, family_size, site).and_then(|index| self.sites.get(index))
        else {
            return false;
        };
        match entry.target {
            Target::Vacant => false,
            Target::Megamorphic { .. } => true,
            Target::Polymorphic { block, .. } => {
                self.pool.get(block as usize).is_some_and(|slots| {
                    slots.iter().any(|slot| {
                        !matches!(slot.target, Target::Vacant) && slot.guard != small_integer
                    })
                })
            }
            _ => entry.guard != small_integer,
        }
    }

    /// Whether a new range has grown the sites to their growth limit since
    /// the last collection: the caches have no heap, so the mutator that
    /// minted the range requests the collection (`Mutator::feedback_base_for`,
    /// `Mutator::fresh_feedback_range`).
    #[inline]
    pub(crate) fn wants_collection(&self) -> bool {
        self.collection_wanted
    }

    /// After a collection: the sites' next limit, from what the collection
    /// left of them (`heap::limits::GrowthLimit::reset`).
    pub(crate) fn reset_site_limit(&mut self) {
        self.collection_wanted = false;
        self.site_limit.reset(self.sites_in_use());
    }

    /// The sites no free range holds: what the sites' growth limit
    /// measures, since a freed range is minted again before the table
    /// grows, so the table's length follows the sites in use.
    fn sites_in_use(&self) -> usize {
        self.sites.len().saturating_sub(self.free_ranges.free_sites)
    }

    /// Records that a new range may have brought the sites in use to their
    /// growth limit (`wants_collection`).
    fn check_site_limit(&mut self) {
        if self.site_limit.reached(self.sites_in_use()) {
            self.collection_wanted = true;
        }
    }

    /// Sets the sites' floor and next limit (`Interpreter::set_table_floors`).
    pub(crate) fn set_site_floor(&mut self, floor: usize) {
        self.site_limit.set_floor(floor);
    }

    /// How many bytes the cells' reservation has committed
    /// (`Interpreter::table_sizes`).
    pub(crate) fn cells_committed_bytes(&self) -> usize {
        self.cells.committed_bytes()
    }

    /// Gives back the cells' committed pages above the site table's end
    /// and the pool's, by `shrink` (`Cells::shrink`), and answers how many
    /// bytes that was: run at the end of a full collection, after a slide
    /// moved the live ranges down and truncated the table.
    pub(crate) fn shrink_cells(
        &mut self,
        shrink: crate::heap::limits::Shrink,
        damped: bool,
    ) -> usize {
        self.cells
            .shrink(self.sites.len(), self.pool.len(), shrink, damped)
    }

    /// Drops every family whose root body only the family holds and whose
    /// range no live closure or activation names (`live_bases`), and every
    /// decoded closure's range none names, vacates its sites and frees its
    /// range for the next family, then gives back the table's free tail. A
    /// body nothing else holds can never be activated again, so no new
    /// activation asks for the range; a closure or activation of the family
    /// still names it by base, which is what `live_bases` keeps -- a range
    /// freed under one would be filled by another family's sends, and its
    /// hits would act on them. Run by a full collection
    /// (`Mutator::reclaim_ids`).
    pub(crate) fn forget_families(&mut self, live_bases: &BTreeSet<u32>) {
        let mut freed = Vec::new();
        self.families.retain(|_, family| {
            let held = Rc::strong_count(&family.code) > 1;
            if held || live_bases.contains(&family.base.raw()) {
                return true;
            }
            freed.push((family.base, family.code.family_size));
            false
        });
        // A decoded closure's range has no body to hold it: only the
        // closures and activations that name its base do.
        self.loose_ranges.retain(|start, size| {
            if live_bases.contains(start) {
                return true;
            }
            freed.push((FeedbackBase(*start), *size));
            false
        });
        let InlineCaches {
            sites,
            pool,
            free_blocks,
            free_ranges,
            cells,
            ..
        } = self;
        for (base, size) in freed {
            // `base_for` records no family without sends, and an empty range
            // inside another free one would break the map's coalescing. A
            // family the table had no room for holds no range to free.
            if size == 0 || base == FeedbackBase::NOT_ASSIGNED {
                continue;
            }
            let range = base.index()..base.index() + size as usize;
            for entry in &mut sites[range.clone()] {
                if let Target::Polymorphic { block, .. } = entry.target {
                    if let Some(slots) = pool.get_mut(block as usize) {
                        *slots = CacheEntry::vacant_block();
                        cells.vacate_blocks(block as usize..block as usize + 1);
                        free_blocks.push(block);
                    }
                }
                *entry = CacheEntry::vacant();
            }
            cells.vacate_sites(range);
            free_ranges.free(base.raw(), size);
        }
        let mut end = u32::try_from(sites.len()).expect("the site table fits a u32");
        while let Some((start, _)) = free_ranges.take_ending_at(end) {
            end = start;
        }
        sites.truncate(end as usize);
    }

    /// A range of `size` sites: the first free one that fits, else a new one
    /// at the end of the table. `NOT_ASSIGNED` if the table cannot grow that
    /// far -- past `cells::MAXIMUM_SITES`, the sites its cells have room
    /// for -- which leaves every site of the family uncached.
    pub(crate) fn fresh_range(&mut self, size: u32) -> FeedbackBase {
        if let Some(start) = self.free_ranges.take_first_fit(size) {
            self.check_site_limit();
            return FeedbackBase(start);
        }
        let start = self.sites.len();
        match start
            .checked_add(size as usize)
            .filter(|end| *end <= cells::MAXIMUM_SITES)
            .and_then(|end| u32::try_from(end).ok())
        {
            Some(end) if end < FeedbackBase::NOT_ASSIGNED.0 => {
                self.sites.resize(end as usize, CacheEntry::vacant());
                self.cells.vacate_sites(start..end as usize);
                self.check_site_limit();
                // `end` fits 32 bits, and `start` lies `size` below it.
                FeedbackBase(end - size)
            }
            _ => FeedbackBase::NOT_ASSIGNED,
        }
    }

    /// A range of `size` sites for a closure an image decoded, which carries
    /// none since a base is never serialized: `fresh_range`'s, recorded in
    /// `loose_ranges` so that a full collection frees it once nothing names
    /// it and a slide moves it while something does. A block without sends
    /// takes base 0 and no room, as `known_base` gives a body without sends.
    pub(crate) fn fresh_loose_range(&mut self, size: u32) -> FeedbackBase {
        if size == 0 {
            return FeedbackBase(0);
        }
        let base = self.fresh_range(size);
        if base != FeedbackBase::NOT_ASSIGNED {
            self.loose_ranges.insert(base.raw(), size);
        }
        base
    }

    /// Turns on or off the collect stress mode's slide at every collection
    /// that finds a range free (`eager_slides`).
    #[cfg(debug_assertions)]
    pub(crate) fn set_eager_slides(&mut self, eager: bool) {
        self.eager_slides = eager;
    }

    /// The slide a full collection makes after `forget_families`, or `None`
    /// while the free ranges cover less than a quarter of the table -- the
    /// code zone's fraction (`jit::zone`), so that neither table compacts
    /// for a hole of a few entries -- or none is free at all: every live
    /// range, a family's or a decoded closure's, packed from 0 in
    /// ascending order (`docs/design/dispatch.md`, "Code families").
    pub(crate) fn plan_slide(&self) -> Option<SiteSlide> {
        let free = self.free_ranges.free_sites;
        if free == 0 {
            return None;
        }
        #[cfg(debug_assertions)]
        let eager = self.eager_slides;
        #[cfg(not(debug_assertions))]
        let eager = false;
        if !eager && free.saturating_mul(4) < self.sites.len() {
            return None;
        }
        let mut live: Vec<(u32, u32)> = self
            .families
            .values()
            .filter(|family| {
                family.base != FeedbackBase::NOT_ASSIGNED && family.code.family_size > 0
            })
            .map(|family| (family.base.raw(), family.code.family_size))
            .chain(
                self.loose_ranges
                    .iter()
                    .map(|(start, size)| (*start, *size)),
            )
            .collect();
        live.sort_unstable();
        let mut ranges: Vec<MovedRange> = Vec::with_capacity(live.len());
        let mut end = 0u32;
        for (start, size) in live {
            debug_assert!(
                ranges
                    .last()
                    .is_none_or(|last| last.old + last.size <= start),
                "internal: two live ranges overlap"
            );
            ranges.push(MovedRange {
                old: start,
                size,
                new: end,
            });
            end += size;
        }
        debug_assert_eq!(
            end as usize + free,
            self.sites.len(),
            "internal: every site lies in a live range or a free one"
        );
        Some(SiteSlide { ranges, end })
    }

    /// Steps 1 and 2 of a slide (`docs/design/dispatch.md`, "Code
    /// families"): every live range's entries and their cells move down,
    /// in ascending order, onto sites already moved or vacant; the table is
    /// truncated to the packed end and no range is free any more; then
    /// every base the caches hold -- each family's, each decoded closure's
    /// range, and each method target's in a site, a polymorphic block or
    /// the global cache -- is rewritten through `slide`. A polymorphic
    /// block is named by its number, so it stays where it is. The heap's
    /// holders are the compaction's (`Heap::compact_with`), and machine
    /// code compiled against a moved base is freed after both
    /// (`Mutator::free_code_of_moved_bases`).
    pub(crate) fn apply_slide(&mut self, slide: &SiteSlide) {
        let old_end = self.sites.len();
        for range in &slide.ranges {
            if range.new == range.old {
                continue;
            }
            for offset in 0..range.size as usize {
                let source = range.old as usize + offset;
                let destination = range.new as usize + offset;
                self.sites[destination] =
                    std::mem::replace(&mut self.sites[source], CacheEntry::vacant());
                let cell = self.cells.site(source);
                self.cells.write_site(destination, cell);
            }
            self.sites_moved += u64::from(range.size);
        }
        self.sites.truncate(slide.end as usize);
        self.cells.vacate_sites(slide.end as usize..old_end);
        self.free_ranges = FreeRanges::default();
        self.site_slides += 1;
        for family in self.families.values_mut() {
            family.base = slide.remap_base(family.base);
        }
        self.loose_ranges = std::mem::take(&mut self.loose_ranges)
            .into_iter()
            .map(|(start, size)| (slide.remap(start), size))
            .collect();
        let remap_target = |target: &mut Target| {
            if let Target::Method { feedback_base, .. } = target {
                *feedback_base = slide.remap_base(*feedback_base);
            }
        };
        for entry in &mut self.sites {
            remap_target(&mut entry.target);
        }
        for slots in &mut self.pool {
            for slot in slots {
                remap_target(&mut slot.target);
            }
        }
        for entry in &mut self.global.entries {
            remap_target(&mut entry.target);
        }
    }

    /// Whether the last cold check that passed found `guard`'s receivers
    /// to have `level` (`verified_slot_level`); never for
    /// `DispatchId::NONE`.
    #[inline(always)]
    pub(crate) fn slot_level_verified(&self, guard: DispatchId, level: ApplicationIndex) -> bool {
        guard != DispatchId::NONE && self.verified_slot_level == Some((guard, level))
    }

    /// Records that a receiver with `guard` was found to have `level`;
    /// nothing for `DispatchId::NONE`, which names no one application.
    pub(crate) fn record_slot_level(&mut self, guard: DispatchId, level: ApplicationIndex) {
        if guard != DispatchId::NONE {
            self.verified_slot_level = Some((guard, level));
        }
    }

    /// How many sites the table holds -- `statistics`'s `site_entries`.
    pub(crate) fn table_len(&self) -> usize {
        self.sites.len()
    }

    /// The address of site 0's cell, which machine code finds a send's
    /// cell from (`jit::stubs::BlockWord::Cells`): fixed for the
    /// reservation's life.
    pub(crate) fn site_cells_address(&self) -> usize {
        self.cells.site_address(0)
    }

    /// The slot accessor whose site's own cell lies at `cell_address`, when
    /// the site's entry records one: the cell machine code's send hit,
    /// which the leaf stub hands its leaf. A cell copies its entry and the
    /// two are written together, so the hit on the site's own cell is a
    /// hit on this entry, for this key; a hit on a block's cell or a
    /// global row answers `None`, and the leaf decides the send as the
    /// interpreter's cached send does.
    pub(crate) fn site_slot_hit<'gc>(&self, cell_address: u64) -> Option<Resolved<'gc>> {
        let index = self.cells.site_of_cell(cell_address)?;
        match self.sites.get(index)?.target {
            Target::Slot {
                field,
                kind,
                chain,
                level,
                recorded_guard,
            } => Some(Resolved::Slot {
                kind,
                field: field as usize,
                chain,
                level,
                recorded_guard,
            }),
            _ => None,
        }
    }

    /// The address of polymorphic block 0's cells, which the polymorphic
    /// stub finds a site's block from (`jit::stubs::BlockWord::PolymorphicBlocks`).
    pub(crate) fn block_cells_address(&self) -> usize {
        self.cells.block_address(0)
    }

    /// The address of the global cache's mirror, which the megamorphic
    /// stub probes (`jit::stubs::BlockWord::GlobalRows`).
    pub(crate) fn global_rows_address(&self) -> usize {
        self.cells.global_address()
    }

    /// The global cache's group mask (`GlobalCache::group`), which the
    /// megamorphic stub hashes a key with (`jit::stubs::BlockWord::GlobalMask`).
    pub(crate) fn global_mask(&self) -> u64 {
        self.global.mask as u64
    }

    /// Writes afresh from their entries the cells through which machine
    /// code found the decision the miss leaf just made for `guard` at site
    /// `index` (`decide_at`): the site's own, the slot of its polymorphic
    /// block that holds `guard`, and, when `rule` names the key the global
    /// cache files it under, the mirror's row of that key. Each cell copies
    /// what its entry records, its entry address from the JIT's state,
    /// which the miss leaf found changed -- a body compiled since the
    /// entry was filled, or found never to be compiled -- so that the
    /// site's next send calls the body directly, or exits at once, from the
    /// site's cell, the polymorphic stub's scan or the megamorphic stub's
    /// probe alike.
    fn refresh_cells(
        &mut self,
        index: usize,
        guard: DispatchId,
        global_key: Option<(Symbol, CacheRule)>,
        jit: &JitState,
    ) {
        let Some(entry) = self.sites.get(index) else {
            return;
        };
        match entry.target {
            Target::Vacant | Target::Megamorphic { .. } => {}
            Target::Polymorphic { block, .. } => {
                let slot = self.pool.get(block as usize).and_then(|slots| {
                    slots
                        .iter()
                        .position(|slot| slot.guard == guard)
                        .map(|position| (position, jit::cell_entry(jit, &slots[position].target)))
                });
                if let Some((position, address)) = slot {
                    let cell = Cell {
                        entry: address,
                        ..self.cells.block_cell(block, position)
                    };
                    self.cells.write_block_cell(block, position, cell);
                }
            }
            _ if entry.guard == guard => {
                let cell = Cell {
                    entry: jit::cell_entry(jit, &entry.target),
                    ..self.cells.site(index)
                };
                self.cells.write_site(index, cell);
            }
            _ => {}
        }
        if let Some((selector, rule)) = global_key {
            self.global
                .refresh(&mut self.cells, guard, selector, rule, jit);
        }
    }

    /// Writes afresh from its entry every cell whose entry address is
    /// `address`, its entry address from the JIT's state: a site's own, a
    /// polymorphic block's, a global row. The JIT abandoned the body whose
    /// machine entry `address` is (`jit::abandon`), so that a cell of an
    /// entry for its method now names the stub that refuses the call
    /// instead; or `address` is the refusing stub's, and the JIT revived a
    /// body (`refresh_all_cells`).
    pub(crate) fn refresh_cells_entering(&mut self, address: u64, jit: &JitState) {
        self.refresh_cells_entering_any(&[address], jit);
    }

    /// `refresh_cells_entering` for every address of `addresses`, sorted:
    /// a slide freed the machine code of every body compiled against a
    /// base it moved (`Mutator::free_code_of_moved_bases`), whose entries
    /// `addresses` are, so that no cell names freed code. Only a cell that
    /// names one is written, from its entry, so that the walk raises no
    /// other body's usage.
    pub(crate) fn refresh_cells_entering_any(&mut self, addresses: &[u64], jit: &JitState) {
        debug_assert!(addresses.is_sorted(), "internal: the addresses are sorted");
        if addresses.is_empty() {
            return;
        }
        let named = |address: u64| addresses.binary_search(&address).is_ok();
        for (index, entry) in self.sites.iter().enumerate() {
            let cell = self.cells.site(index);
            if named(cell.entry) {
                let entry = jit::cell_entry(jit, &entry.target);
                self.cells.write_site(index, Cell { entry, ..cell });
            }
        }
        for (block, slots) in (0..).zip(&self.pool) {
            for (position, slot) in slots.iter().enumerate() {
                let cell = self.cells.block_cell(block, position);
                if named(cell.entry) {
                    let entry = jit::cell_entry(jit, &slot.target);
                    self.cells
                        .write_block_cell(block, position, Cell { entry, ..cell });
                }
            }
        }
        self.global.refresh_entering(&mut self.cells, &named, jit);
    }

    /// Writes afresh from its entry every cell -- a site's own, a
    /// polymorphic block's, a global row -- that names the refusing stub:
    /// the JIT revived an abandoned body (`JitState::decay_exits`,
    /// `JitState::note_abandoned_entry`), and a revival changes only such
    /// cells, a cell that refused the calls of the revived body's method
    /// naming the miss stub again. Every other cell is compared and left
    /// alone, so that the walk raises no body's usage.
    pub(crate) fn refresh_all_cells(&mut self, jit: &JitState) {
        let refusing = jit.refusing_stub_address();
        if refusing != 0 {
            self.refresh_cells_entering(refusing, jit);
        }
    }

    /// The entry address of every cell -- a site's own, a polymorphic
    /// block's, a global row -- as the cells hold them: what a debug
    /// build's check after a compaction or a slide holds to naming no twin
    /// (`Mutator::verify_code_zone_holders`).
    #[cfg(all(ns_jit, debug_assertions))]
    pub(crate) fn cell_entries(&self) -> Vec<u64> {
        let mut entries: Vec<u64> = (0..self.sites.len())
            .map(|index| self.cells.site(index).entry)
            .collect();
        for (block, slots) in (0..).zip(&self.pool) {
            for position in 0..slots.len() {
                entries.push(self.cells.block_cell(block, position).entry);
            }
        }
        entries.extend((0..self.global.entries.len()).map(|row| self.cells.global_row(row).entry));
        entries
    }

    /// Rewrites the entry address of every cell -- a site's own, a
    /// polymorphic block's, a global row -- through `plan`: a compaction of
    /// the code zone moved the bodies the cells name, and freed some, whose
    /// cells now name the miss stub (`jit::zone`). Not written from the
    /// entry, as every other write of a cell is: what the entry decided is
    /// unchanged, only the address of the code it decided moved, and a
    /// freed body's target is one `cell_entry` answers the miss stub for.
    pub(crate) fn remap_cell_entries(&mut self, plan: &crate::jit::zone::CompactionPlan) {
        for index in 0..self.sites.len() {
            let cell = self.cells.site(index);
            let entry = plan.remap(cell.entry);
            if entry != cell.entry {
                self.cells.write_site(index, Cell { entry, ..cell });
            }
        }
        for (block, slots) in (0..).zip(&self.pool) {
            for position in 0..slots.len() {
                let cell = self.cells.block_cell(block, position);
                let entry = plan.remap(cell.entry);
                if entry != cell.entry {
                    self.cells
                        .write_block_cell(block, position, Cell { entry, ..cell });
                }
            }
        }
        self.global.remap_entries(&mut self.cells, plan);
    }

    /// Moves site `index` on after a miss found `target` for `guard`
    /// (`docs/design/dispatch.md`, "A site's states"): vacant to
    /// monomorphic, monomorphic to a polymorphic block of four in the order
    /// first seen, a full block to megamorphic, which stays so -- Strongtalk's
    /// and V8's transitions. A site whose §5.9 decision differs from
    /// `private` is left alone: only a crafted image makes one.
    ///
    /// `cell` is the new entry's cell, which the caller computes from the
    /// send's key and `jit::cell_entry` (`cell_for`); it is written beside
    /// the entry wherever the entry lands, and the site's own cell follows
    /// the site's state, its entry address from `jit`.
    fn advance(
        &mut self,
        index: usize,
        guard: DispatchId,
        target: Target,
        private: bool,
        cell: Cell,
        jit: &JitState,
    ) {
        let Some(entry) = self.sites.get_mut(index) else {
            return;
        };
        if entry
            .target
            .site_privacy()
            .is_some_and(|site_private| site_private != private)
        {
            return;
        }
        match entry.target {
            Target::Vacant => {
                *entry = CacheEntry { guard, target };
                self.cells.write_site(index, cell);
            }
            Target::Megamorphic { .. } => {}
            Target::Polymorphic { block, .. } => {
                let Some(slots) = self.pool.get_mut(block as usize) else {
                    return;
                };
                // A guard already in the block is one whose entry could not
                // be materialized for this receiver; the fresh answer
                // replaces it rather than taking a second slot.
                if let Some(slot) = slots
                    .iter()
                    .position(|slot| slot.guard == guard || matches!(slot.target, Target::Vacant))
                {
                    slots[slot] = CacheEntry { guard, target };
                    self.cells.write_block_cell(block, slot, cell);
                    return;
                }
                *slots = CacheEntry::vacant_block();
                self.cells.vacate_blocks(block as usize..block as usize + 1);
                self.free_blocks.push(block);
                let megamorphic = Target::Megamorphic { private };
                self.cells
                    .write_site(index, site_state_cell(jit, &megamorphic, cell.start()));
                *entry = CacheEntry {
                    guard: DispatchId::VACANT,
                    target: megamorphic,
                };
            }
            _ if entry.guard == guard => {
                *entry = CacheEntry { guard, target };
                self.cells.write_site(index, cell);
            }
            _ => {
                let block = match self.free_blocks.pop() {
                    Some(block) => block,
                    None => match u32::try_from(self.pool.len()) {
                        Ok(block) if self.pool.len() < cells::MAXIMUM_BLOCKS => {
                            self.pool.push(CacheEntry::vacant_block());
                            self.cells.vacate_blocks(block as usize..block as usize + 1);
                            block
                        }
                        _ => return,
                    },
                };
                let polymorphic = Target::Polymorphic { block, private };
                let site_cell = site_state_cell(jit, &polymorphic, cell.start());
                let first = std::mem::replace(
                    entry,
                    CacheEntry {
                        guard: DispatchId::VACANT,
                        target: polymorphic,
                    },
                );
                // The first entry's cell as its target decides it now: its
                // entry address follows the JIT's state, which may have
                // changed since the site's cell was written.
                let first_cell = Cell {
                    entry: jit::cell_entry(jit, &first.target),
                    ..self.cells.site(index)
                };
                let slots = &mut self.pool[block as usize];
                slots[0] = first;
                slots[1] = CacheEntry { guard, target };
                self.cells.write_block_cell(block, 0, first_cell);
                self.cells.write_block_cell(block, 1, cell);
                self.cells.write_site(index, site_cell);
            }
        }
    }

    /// The target the polymorphic site `index` holds for `guard`.
    fn polymorphic_hit(&self, index: usize, guard: DispatchId) -> Option<Target> {
        let Target::Polymorphic { block, .. } = self.sites.get(index)?.target else {
            return None;
        };
        self.pool
            .get(block as usize)?
            .iter()
            .find(|slot| slot.guard == guard)
            .map(|slot| slot.target.clone())
    }

    /// Vacates every site, every polymorphic block and the global cache
    /// (`docs/design/dispatch.md`, "Invalidation"): a class redefinition
    /// invalidates every guard at once, rather than tracking which ones a
    /// change could affect. `families` stays -- an activation already
    /// running still names its family's range, so the next send through a
    /// vacated site fills the same index again instead of minting a new
    /// one; only a collection's `forget_families` drops a family, once
    /// nothing runs it. Clearing the pool rather than only the sites
    /// is what leaves every polymorphic block vacant too, not merely
    /// unreachable from a site that still names it.
    pub(crate) fn invalidate(&mut self) {
        for entry in &mut self.sites {
            *entry = CacheEntry::vacant();
        }
        self.cells.vacate_sites(0..self.sites.len());
        self.cells.vacate_blocks(0..self.pool.len());
        self.pool.clear();
        self.free_blocks.clear();
        self.global.clear(&mut self.cells);
        self.verified_slot_level = None;
    }

    /// Vacates exactly the entries that name an id a collection freed --
    /// a guard, as an entry's own or a lexical rule's N, and an
    /// application, as a slot target's level, with the guard it recorded
    /// -- and forgets the verified slot level if it names one. The one
    /// scrub a collection makes, run only when it freed something: a guard
    /// or an index minted again must never meet an entry filled for the
    /// application it named before, and every other entry stays warm
    /// (`docs/design/dispatch.md`, "Invalidation"). A polymorphic site
    /// stays polymorphic with its matching slots vacant; a megamorphic one
    /// holds no guard.
    pub(crate) fn forget_freed(&mut self, freed: &FreedIds) {
        let names_freed = |entry: &CacheEntry| {
            freed.guards.contains(&entry.guard) || target_names(&entry.target, freed)
        };
        for (index, entry) in self.sites.iter_mut().enumerate() {
            match entry.target {
                Target::Vacant | Target::Megamorphic { .. } => {}
                Target::Polymorphic { block, .. } => {
                    if let Some(slots) = self.pool.get_mut(block as usize) {
                        for (position, slot) in slots.iter_mut().enumerate() {
                            if names_freed(slot) {
                                *slot = CacheEntry::vacant();
                                self.cells.write_block_cell(block, position, Cell::VACANT);
                            }
                        }
                    }
                }
                _ => {
                    if names_freed(entry) {
                        *entry = CacheEntry::vacant();
                        self.cells.write_site(index, Cell::VACANT);
                    }
                }
            }
        }
        self.global.retain(&mut self.cells, |entry| {
            let lexical_freed =
                matches!(entry.rule, CacheRule::Lexical(guard) if freed.guards.contains(&guard));
            !(freed.guards.contains(&entry.guard)
                || lexical_freed
                || target_names(&entry.target, freed))
        });
        if self.verified_slot_level.is_some_and(|(guard, level)| {
            freed.guards.contains(&guard) || freed.applications.contains(&level)
        }) {
            self.verified_slot_level = None;
        }
    }

    /// The table's sizes: how much of what is allocated is actually filled,
    /// and how large one entry of each kind is, for
    /// `Interpreter::cache_statistics` and `--cache-statistics`.
    pub(crate) fn statistics(&self) -> CacheStatistics {
        let mut filled_sites = 0;
        let mut megamorphic_sites = 0;
        for entry in &self.sites {
            match entry.target {
                Target::Vacant => {}
                Target::Megamorphic { .. } => megamorphic_sites += 1,
                _ => filled_sites += 1,
            }
        }
        let filled_global_entries = self
            .global
            .entries
            .iter()
            .filter(|entry| !entry.is_vacant())
            .count();
        CacheStatistics {
            site_entries: self.table_len(),
            filled_sites,
            polymorphic_blocks: self.pool.len(),
            polymorphic_blocks_in_use: self.pool.len() - self.free_blocks.len(),
            megamorphic_sites,
            global_entries: self.global.entries.len(),
            filled_global_entries,
            entry_bytes: std::mem::size_of::<CacheEntry>(),
            global_entry_bytes: std::mem::size_of::<GlobalEntry>(),
            loose_ranges: self.loose_ranges.len(),
            site_slides: self.site_slides,
            sites_moved: self.sites_moved,
        }
    }

    /// Replaces the global cache with a fresh one sized for `entries`,
    /// vacating whatever it held: a test hook for measuring a program's
    /// answer under a cache small enough that every insert evicts
    /// (`NS_NEXT_GLOBAL_CACHE_ENTRIES`, `set_global_cache_size`), never a
    /// host's input path, so `entries` must be twice a power of two -- two
    /// per group -- and a size that is not is the host's mistake:
    /// `GlobalCache::with_groups` panics on it rather than rounding
    /// silently.
    pub(crate) fn set_global_cache_size(&mut self, entries: usize) {
        assert!(
            entries.is_multiple_of(2) && (entries / 2).is_power_of_two(),
            "the global cache must be sized to twice a power of two, not {entries}"
        );
        assert!(
            entries <= cells::MAXIMUM_GLOBAL_ROWS,
            "the global cache's mirror has room for {} entries, not {entries}",
            cells::MAXIMUM_GLOBAL_ROWS
        );
        let old_entries = self.global.entries.len();
        self.global = GlobalCache::with_groups(entries / 2);
        self.cells.vacate_global_rows(0..old_entries.max(entries));
    }

    /// The global cache's entry for the key, promoting it within its group
    /// (`GlobalCache::get`), its mirror row moving with it.
    fn global_get(
        &mut self,
        guard: DispatchId,
        selector: Symbol,
        rule: CacheRule,
    ) -> Option<&Target> {
        self.global.get(&mut self.cells, guard, selector, rule)
    }

    /// Records `target` for the key in the global cache, and `cell`, the
    /// target's cell for the send that found it, in its mirror row
    /// (`GlobalCache::insert`).
    fn global_insert(
        &mut self,
        guard: DispatchId,
        selector: Symbol,
        rule: CacheRule,
        target: Target,
        cell: Cell,
    ) {
        self.global
            .insert(&mut self.cells, guard, selector, rule, target, cell);
    }
}

/// The inline caches' sizes, read without disturbing them
/// (`InlineCaches::statistics`): how much of what is allocated is actually
/// filled -- a `filled_sites`/`megamorphic_sites` gap from `site_entries`
/// is every site still vacant -- and how large one entry of each kind is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheStatistics {
    pub site_entries: usize,
    pub filled_sites: usize,
    pub polymorphic_blocks: usize,
    pub polymorphic_blocks_in_use: usize,
    pub megamorphic_sites: usize,
    pub global_entries: usize,
    pub filled_global_entries: usize,
    pub entry_bytes: usize,
    pub global_entry_bytes: usize,
    /// The ranges given to closures an image decoded that are still
    /// recorded (`InlineCaches::fresh_loose_range`).
    pub loose_ranges: usize,
    /// How many full collections slid the site ranges down, and how many
    /// sites they moved in all (`InlineCaches::plan_slide`).
    pub site_slides: u64,
    pub sites_moved: u64,
}

/// One live range of a slide: where it starts, how many sites it has, and
/// where it starts once packed.
struct MovedRange {
    old: u32,
    size: u32,
    new: u32,
}

/// Where a full collection slides the site table's live ranges
/// (`InlineCaches::plan_slide`): each family's and each decoded closure's,
/// ascending, packed from 0. Read by every holder of a base the collection
/// rewrites -- the caches (`InlineCaches::apply_slide`), the heap's
/// closures and frames (`Heap::compact_with`) -- and by the JIT, which frees
/// the machine code compiled against a base that moved
/// (`JitState::free_bodies_with_moved_bases`).
pub(crate) struct SiteSlide {
    ranges: Vec<MovedRange>,
    /// The packed table's length.
    end: u32,
}

impl SiteSlide {
    /// The live range `base` lies in, if any: a binary search.
    fn range_of(&self, base: u32) -> Option<&MovedRange> {
        let following = self.ranges.partition_point(|range| range.old <= base);
        let range = &self.ranges[following.checked_sub(1)?];
        (base - range.old < range.size).then_some(range)
    }

    /// Where `base` lies once the ranges are packed: a base inside a live
    /// range moves with it. Any other base stays as it is --
    /// `NOT_ASSIGNED`, and 0, which a body without sends takes and which
    /// indexes no site -- since nothing that names one indexes the table.
    pub(crate) fn remap(&self, base: u32) -> u32 {
        self.range_of(base)
            .map_or(base, |range| range.new + (base - range.old))
    }

    /// `remap` for a `FeedbackBase`.
    pub(crate) fn remap_base(&self, base: FeedbackBase) -> FeedbackBase {
        FeedbackBase(self.remap(base.0))
    }

    /// Whether machine code compiled against `base` must be freed: the base
    /// moved, or it lies in no live range at all and so names sites the
    /// slide may give to another family. `NOT_ASSIGNED` and 0 never do.
    pub(crate) fn moves(&self, base: FeedbackBase) -> bool {
        if base == FeedbackBase::NOT_ASSIGNED || base.0 == 0 {
            return false;
        }
        self.range_of(base.0)
            .is_none_or(|range| range.new != range.old)
    }

    /// Whether `base` is one a live holder may name: 0, `NOT_ASSIGNED`, or
    /// inside a live range. A full collection frees no range a marked
    /// closure or an activation names (`InlineCaches::forget_families`),
    /// so a heap holder of any other base would name, once the ranges are
    /// packed, another family's sites: the heap's rewriters assert it in a
    /// debug build (`Heap::compact_with`).
    pub(crate) fn holds(&self, base: u32) -> bool {
        base == 0 || base == FeedbackBase::NOT_ASSIGNED.0 || self.range_of(base).is_some()
    }
}

/// Whether `target` names a freed application or guard: a slot target's
/// level, and the guard it recorded for a receiver with that level. Every
/// other target names no id but its entry's guard.
fn target_names(target: &Target, freed: &FreedIds) -> bool {
    match target {
        Target::Slot {
            level,
            recorded_guard,
            ..
        } => freed.applications.contains(level) || freed.guards.contains(recorded_guard),
        _ => false,
    }
}

/// The index of `site` in its activation's range, or `None` when the send
/// runs uncached: the reference lowering's sends, a site at or past its
/// code's family, and an activation whose range could not be assigned.
#[inline(always)]
pub(crate) fn site_index(
    feedback_base: FeedbackBase,
    family_size: u32,
    site: CallSite,
) -> Option<usize> {
    if site.number >= family_size || feedback_base == FeedbackBase::NOT_ASSIGNED {
        return None;
    }
    Some(feedback_base.index() + site.number as usize)
}

struct GlobalEntry {
    guard: DispatchId,
    selector: Symbol,
    /// Carries N's application for a `Lexical` entry -- see `CacheRule`.
    rule: CacheRule,
    target: Target,
}

impl GlobalEntry {
    fn vacant() -> GlobalEntry {
        GlobalEntry {
            guard: DispatchId::VACANT,
            selector: Symbol::NEW,
            rule: CacheRule::Public,
            target: Target::Vacant,
        }
    }

    fn holds(&self, guard: DispatchId, selector: Symbol, rule: CacheRule) -> bool {
        self.guard == guard && self.selector == selector && self.rule == rule
    }

    fn is_vacant(&self) -> bool {
        matches!(self.target, Target::Vacant)
    }
}

/// Every site's miss path, and the only cache a megamorphic site consults
/// (`docs/design/dispatch.md`, "The global cache"): a fixed number of
/// two-slot groups keyed by (dispatch id, selector, rule), whatever the
/// program's size -- Strongtalk's `lookupCache`
/// (`prior-art/strongtalk/vm/lookup/lookupCache.cpp`), one hash picking the
/// group rather than two hashes each picking a slot anywhere in the table.
pub(crate) struct GlobalCache {
    /// `groups` two-slot groups, flattened: group `index`'s slots are
    /// `entries[index * 2]` and `entries[index * 2 + 1]`.
    entries: Vec<GlobalEntry>,
    /// The number of groups, minus one.
    mask: usize,
}

impl GlobalCache {
    /// The first of the key's group's two adjacent slots, which the
    /// megamorphic stub computes as this does (`jit::targets`).
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the hash only picks a group, so folding it to the target's width keeps all the mask reads"
    )]
    pub(crate) fn group(&self, guard: DispatchId, selector: Symbol, rule: CacheRule) -> usize {
        let hash = crate::hash::IdentityBuildHasher::default().hash_one((guard, selector, rule));
        (hash as usize & self.mask) * 2
    }

    /// Strongtalk's `lookupCache`, `lookup_probe`/`lookup`
    /// (`prior-art/strongtalk/vm/lookup/lookupCache.cpp:188-205,231-242`):
    /// the read itself keeps the group least-recently-used. A hit at the
    /// first slot answers in place; a hit at the second slot promotes it --
    /// swaps the two -- before answering, so the key just used is the one a
    /// colliding key evicts last. A read costs at most the two comparisons
    /// plus, on a second-slot hit, one swap (`docs/design/dispatch.md`, "3b
    /// -- The caches").
    fn get(
        &mut self,
        cells: &mut Cells,
        guard: DispatchId,
        selector: Symbol,
        rule: CacheRule,
    ) -> Option<&Target> {
        let first = self.group(guard, selector, rule);
        if self.entries[first].holds(guard, selector, rule) {
            return Some(&self.entries[first].target);
        }
        if self.entries[first + 1].holds(guard, selector, rule) {
            self.entries.swap(first, first + 1);
            cells.swap_global_rows(first, first + 1);
            return Some(&self.entries[first].target);
        }
        None
    }

    /// The key already at the first slot is refreshed in place. Otherwise
    /// the first slot's entry moves to the second, discarding whatever the
    /// second held, and the key is written into the first. `get` is what
    /// keeps the group least-recently-used; this is what makes a genuine
    /// miss evict the slot `get`'s promotion left least recently used
    /// (`docs/design/dispatch.md`, "3b -- The caches"). This replaces 3b's
    /// rule, which evicted the first probe whatever its use.
    ///
    /// The second slot is never where the key is found: every caller runs a
    /// `get` for this same key first (`fill`'s two callers, `miss` and
    /// `send_lexical_first`, and nothing between the two writes the cache),
    /// and a `get` that hit the second slot promoted it. So the `else` arm
    /// always means "not in this group at all". It is written as one arm
    /// rather than three because it is also correct if the key *were* at the
    /// second slot -- the displaced first entry overwrites it, so no group
    /// can come to hold one key twice -- and a case that cannot arise is
    /// better handled than asserted against on the send path.
    ///
    /// `cell` is the target's cell for the send that found it (`cell_for`),
    /// whose `entry`, `level` and `chain_start` the mirror row records
    /// beside the key.
    fn insert(
        &mut self,
        cells: &mut Cells,
        guard: DispatchId,
        selector: Symbol,
        rule: CacheRule,
        target: Target,
        cell: Cell,
    ) {
        let first = self.group(guard, selector, rule);
        let second = first + 1;
        let entry = GlobalEntry {
            guard,
            selector,
            rule,
            target,
        };
        let row = GlobalRow {
            guard: u64::from(guard.bits()),
            selector: selector.index() as u64,
            rule: rule_word(rule),
            entry: cell.entry,
            level: cell.level,
            chain_start: cell.chain_start,
        };
        if self.entries[first].holds(guard, selector, rule) {
            self.entries[first] = entry;
        } else {
            self.entries[second] = std::mem::replace(&mut self.entries[first], entry);
            cells.write_global_row(second, cells.global_row(first));
        }
        cells.write_global_row(first, row);
    }

    /// Writes the mirror row of the key's entry afresh, its entry address
    /// from the JIT's state: `InlineCaches::refresh_cells`'s global part.
    fn refresh(
        &mut self,
        cells: &mut Cells,
        guard: DispatchId,
        selector: Symbol,
        rule: CacheRule,
        jit: &JitState,
    ) {
        let first = self.group(guard, selector, rule);
        for row in [first, first + 1] {
            if self.entries[row].holds(guard, selector, rule) && !self.entries[row].is_vacant() {
                let found = cells.global_row(row);
                cells.write_global_row(
                    row,
                    GlobalRow {
                        entry: jit::cell_entry(jit, &self.entries[row].target),
                        ..found
                    },
                );
            }
        }
    }

    /// Writes afresh every mirror row whose entry address `named` accepts:
    /// `InlineCaches::refresh_cells_entering_any`'s global part.
    fn refresh_entering(&self, cells: &mut Cells, named: &impl Fn(u64) -> bool, jit: &JitState) {
        for (row, entry) in self.entries.iter().enumerate() {
            let found = cells.global_row(row);
            if named(found.entry) {
                cells.write_global_row(
                    row,
                    GlobalRow {
                        entry: jit::cell_entry(jit, &entry.target),
                        ..found
                    },
                );
            }
        }
    }

    /// Rewrites every mirror row's entry address through `plan`:
    /// `InlineCaches::remap_cell_entries`'s global part.
    fn remap_entries(&self, cells: &mut Cells, plan: &crate::jit::zone::CompactionPlan) {
        for row in 0..self.entries.len() {
            let found = cells.global_row(row);
            let entry = plan.remap(found.entry);
            if entry != found.entry {
                cells.write_global_row(row, GlobalRow { entry, ..found });
            }
        }
    }

    /// Vacates every entry: `InlineCaches::invalidate`'s global half.
    fn clear(&mut self, cells: &mut Cells) {
        for entry in &mut self.entries {
            *entry = GlobalEntry::vacant();
        }
        cells.vacate_global_rows(0..self.entries.len());
    }

    /// Vacates every entry `keep` refuses: `InlineCaches::forget_freed`'s
    /// global half. A group whose first slot is vacated keeps its second,
    /// which `get` still finds there.
    fn retain(&mut self, cells: &mut Cells, keep: impl Fn(&GlobalEntry) -> bool) {
        for (index, entry) in self.entries.iter_mut().enumerate() {
            if !entry.is_vacant() && !keep(entry) {
                *entry = GlobalEntry::vacant();
                cells.write_global_row(index, GlobalRow::VACANT);
            }
        }
    }

    /// `groups` two-slot groups: `2 * groups` entries in all. `groups` must
    /// be a power of two so the group mask is a bitmask; a host that gets
    /// this wrong (`set_global_cache_size`) panics rather than the cache
    /// rounding it silently.
    pub(crate) fn with_groups(groups: usize) -> GlobalCache {
        assert!(
            groups.is_power_of_two(),
            "the global cache must have a power-of-two number of groups, not {groups}"
        );
        GlobalCache {
            entries: (0..groups * 2).map(|_| GlobalEntry::vacant()).collect(),
            mask: groups - 1,
        }
    }
}

/// What a cached send carries besides its site: the guard, the rule the
/// global cache files it under -- `Lexical`'s payload is N's application,
/// filled in only once a miss is certain (`docs/design/dispatch.md`, "3b --
/// The caches") -- and what `materialize` and `act` need: the receiver (`o`
/// for a lexical send, `self` for `super`), N's class for a private lexical
/// site, S for `super`.
///
/// The receiver is held by reference, to the send's own local, not copied
/// into the key. A `Value` field lets LLVM's SROA split the key's copy of it
/// into the pieces the hit path reads (the tag, a Boolean's byte, a handle's
/// word, the gap between them, the payload word) and store them one by one
/// wherever a whole `Value` is needed again -- the temporary a native takes,
/// `Step::Method`'s receiver, the key `miss` is handed -- so the next
/// 16-byte load of it fails store-to-load forwarding. That stall cost
/// `bench_send` 6% and `bench_megamorphic` 3.4% once a change elsewhere
/// tipped SROA over (`docs/design/execution.md`, "What `Behavior`, `Class`
/// and `Metaclass` cost"). Behind a reference the local stays one object
/// in memory and every whole copy is a single 16-byte move.
#[derive(Clone, Copy)]
struct SendKey<'send, 'gc> {
    guard: DispatchId,
    rule: CacheRule,
    receiver: &'send Value<'gc>,
    lexical_class: Option<ClassReference<'gc>>,
    superclass: Option<ClassReference<'gc>>,
    /// The running activation's `current_class`, for a `super` send only:
    /// a nested-class hit takes the cached hidden slot only for a receiver
    /// of exactly that application (`materialize`), whose chain contains
    /// S. `None` for every other send.
    current_class: Option<ClassReference<'gc>>,
}

impl SendKey<'_, '_> {
    fn private(&self) -> bool {
        self.rule == CacheRule::LexicalPrivate
    }
}

/// Where `chain`'s levels are counted from for this send. `None` when the
/// start does not exist -- only a crafted image makes that -- which makes
/// the send a miss.
#[inline(always)]
fn chain_start<'gc>(
    mutator: &mut Mutator<'gc>,
    chain: Chain,
    key: SendKey<'_, 'gc>,
) -> Option<ClassReference<'gc>> {
    match chain {
        Chain::Public | Chain::Protected | Chain::DoesNotUnderstand => {
            Some(mutator.class_of(*key.receiver))
        }
        Chain::Private => key.lexical_class,
        Chain::Super | Chain::SuperDoesNotUnderstand => key.superclass,
    }
}

/// How many superclass steps from `start` reach `level`, if at most
/// `u16::MAX`. Only the miss path counts, once per recorded target.
fn levels_between<'gc>(
    heap: &Heap<'gc>,
    start: ClassReference<'gc>,
    level: ClassReference<'gc>,
) -> Option<u16> {
    let mut current = start;
    for steps in 0..=u16::MAX {
        if current.same(level) {
            return Some(steps);
        }
        current = heap.class_superclass(current)?;
    }
    None
}

/// `level` superclass steps up from `start`: the defining class a hit
/// recomputes, since a class object is per enclosing instance and so is
/// never recorded (`docs/design/dispatch.md`, "An entry").
#[inline(always)]
fn level_above<'gc>(
    heap: &Heap<'gc>,
    start: ClassReference<'gc>,
    level: u16,
) -> Option<ClassReference<'gc>> {
    let mut current = start;
    for _ in 0..level {
        current = heap.class_superclass(current)?;
    }
    Some(current)
}

/// What an entry records for `resolved`, or `None` for what is never
/// recorded: `Unhandled`, a level too far up, a field past `u32`. It runs
/// nothing; `base_for` may assign the method's family its range.
fn record<'gc>(
    mutator: &mut Mutator<'gc>,
    resolved: &Resolved<'gc>,
    key: SendKey<'_, 'gc>,
) -> Option<Target> {
    let chain = resolved.chain()?;
    let needs_start = matches!(
        resolved,
        Resolved::Method { .. } | Resolved::NestedClass { .. } | Resolved::Primitive { .. }
    );
    let start = if needs_start {
        chain_start(mutator, chain, key)
    } else {
        None
    };
    Some(match resolved {
        Resolved::Method {
            method,
            defining_class,
            ..
        } => Target::Method {
            method: method.clone(),
            feedback_base: mutator.feedback_base_for(&method.code),
            level: levels_between(&mutator.heap, start?, *defining_class)?,
            chain,
        },
        // §5.10's slot handler is never recorded: its field is decided from
        // S's chain, so the receiver must be checked before it runs, and a
        // check in `act`'s handler arm -- even one more argument to the
        // handler's call -- slowed every program 4-9%, public sends
        // included. Uncached, `miss` and `decide_super_send` check it
        // before `act` (`Mutator::check_super_handler_slot`).
        Resolved::Slot {
            chain: Chain::SuperDoesNotUnderstand,
            ..
        } => return None,
        Resolved::Slot {
            kind, field, level, ..
        } => Target::Slot {
            field: u32::try_from(*field).ok()?,
            kind: *kind,
            chain,
            level: *level,
            recorded_guard: slot_guard(&mutator.heap, chain, *key.receiver, *level),
        },
        Resolved::NestedClass {
            mixin,
            declaring_class,
            ..
        } => Target::NestedClass {
            mixin: mixin.clone(),
            field: nested_field(mutator, chain, key, mixin),
            level: levels_between(&mutator.heap, start?, *declaring_class)?,
            chain,
        },
        Resolved::Primitive {
            primitive, level, ..
        } => Target::Primitive {
            primitive: *primitive,
            level: levels_between(&mutator.heap, start?, *level)?,
            chain,
        },
        Resolved::Factory { code, .. } => Target::Factory {
            code: code.clone(),
            chain,
        },
        Resolved::Unhandled => return None,
    })
}

/// The hidden slot a nested-class entry records: its index in the layout
/// of the application the entry's guard names, which is the one a hit can
/// rely on. A public or protected guard is the receiver's own dispatch id,
/// a private one N's application, and a `super` one S's
/// (`sends::super_start`). A hidden slot's index is the same in every
/// application over the one that declares it, since an application's
/// `nested_slot` extends its superclass application's, and the
/// declaration a `super` send finds lies in S's chain: so its index is S's,
/// which every receiver whose chain contains S shares (`materialize` checks
/// that the receiver has the running class's application).
fn nested_field<'gc>(
    mutator: &Mutator<'gc>,
    chain: Chain,
    key: SendKey<'_, 'gc>,
    mixin: &Rc<Mixin>,
) -> u32 {
    let class = match chain {
        Chain::Private => key.lexical_class,
        Chain::Super => key.superclass,
        _ => Some(mutator.class_of(*key.receiver)),
    };
    class
        .and_then(|class| super::nested::hidden_slot_index(mutator, class, mixin))
        .and_then(|index| u32::try_from(index).ok())
        .unwrap_or(super::nested::NO_CACHED_FIELD)
}

/// The guard a slot entry records: the receiver's own, for a private or
/// super site whose receiver has `level` -- so a hit on that receiver's
/// application passes `Mutator::slot_level_holds` with one compare -- and
/// `DispatchId::NONE` otherwise, so an entry never holds a guard for a
/// receiver without the level. A public or protected entry's guard is the
/// receiver's own dispatch id already, and `act` checks nothing for it.
fn slot_guard<'gc>(
    heap: &Heap<'gc>,
    chain: Chain,
    receiver: Value<'gc>,
    level: ApplicationIndex,
) -> DispatchId {
    match chain {
        Chain::Private | Chain::Super if heap.receiver_has_level(receiver, level) => {
            heap.dispatch_id(receiver)
        }
        _ => DispatchId::NONE,
    }
}

/// The `Resolved` an entry's target names for this send, with the base a
/// method's activation takes, or `None` when a step up does not exist,
/// which makes the send a miss. Takes the target by value, so that a
/// method's `Rc` moves into the step rather than being counted twice.
#[inline(always)]
fn materialize<'gc>(
    mutator: &mut Mutator<'gc>,
    target: Target,
    key: SendKey<'_, 'gc>,
) -> Option<(Resolved<'gc>, FeedbackBase)> {
    Some(match target {
        Target::Method {
            method,
            feedback_base,
            level,
            chain,
        } => {
            let start = chain_start(mutator, chain, key)?;
            let defining_class = level_above(&mutator.heap, start, level)?;
            (
                Resolved::Method {
                    method,
                    defining_class,
                    chain,
                },
                feedback_base,
            )
        }
        Target::Slot {
            field,
            kind,
            chain,
            level,
            recorded_guard,
        } => (
            Resolved::Slot {
                kind,
                field: field as usize,
                chain,
                level,
                recorded_guard,
            },
            FeedbackBase::NOT_ASSIGNED,
        ),
        Target::NestedClass {
            mixin,
            field,
            level,
            chain,
        } => {
            let start = chain_start(mutator, chain, key)?;
            let declaring_class = level_above(&mutator.heap, start, level)?;
            // The field is the guard's application's, which is the
            // receiver's on a public or protected site; on a private one
            // only a receiver of exactly that application may take it, and
            // on a `super` one only a receiver of the running class's
            // application, whose chain contains S -- a recorded guard as
            // the slot index's, decided without the receiver, needs. Any
            // other takes the accessor's lookup.
            let receiver_has = |guard: DispatchId| mutator.heap.dispatch_id(*key.receiver) == guard;
            let field = match chain {
                Chain::Private if !receiver_has(key.guard) => super::nested::NO_CACHED_FIELD,
                Chain::Super
                    if !key.current_class.is_some_and(|current_class| {
                        receiver_has(mutator.heap.class_dispatch_ids(current_class).instances)
                    }) =>
                {
                    super::nested::NO_CACHED_FIELD
                }
                _ => field,
            };
            (
                Resolved::NestedClass {
                    mixin,
                    declaring_class,
                    chain,
                    field,
                },
                FeedbackBase::NOT_ASSIGNED,
            )
        }
        Target::Primitive {
            primitive,
            level,
            chain,
        } => {
            let start = chain_start(mutator, chain, key)?;
            let level = level_above(&mutator.heap, start, level)?;
            (
                Resolved::Primitive {
                    primitive,
                    level,
                    chain,
                },
                FeedbackBase::NOT_ASSIGNED,
            )
        }
        // §6.3 instantiates the class object the factory message was sent
        // to, so the entry records no level: the class a hit instantiates is
        // the receiver, and the guard -- its metaclass's instances id --
        // names the application whose own method table held the entry
        // (`resolve::resolved_for` records nothing found anywhere else). A
        // receiver with that id is a class object of that metaclass's
        // declaration, so a hit reaches what a miss would -- the entry's
        // body too, which comes from the entry, for a hit and a miss alike.
        Target::Factory { code, chain } => match key.receiver.as_object() {
            Some(class_object) => (
                Resolved::Factory {
                    level: class_object,
                    code,
                    chain,
                },
                FeedbackBase::NOT_ASSIGNED,
            ),
            _ => return None,
        },
        // Site states are guarded by `VACANT`, which no receiver has, so a
        // guard that matched never names one.
        Target::Vacant | Target::Polymorphic { .. } | Target::Megamorphic { .. } => return None,
    })
}

/// The key of an explicit-receiver send to `receiver` (§5.5): its guard
/// the receiver's own dispatch id. The one place that builds it, for
/// `send_public` and the miss leaf's `decide_at`.
#[inline(always)]
fn public_key<'send, 'gc>(heap: &Heap<'gc>, receiver: &'send Value<'gc>) -> SendKey<'send, 'gc> {
    SendKey {
        guard: heap.dispatch_id(*receiver),
        rule: CacheRule::Public,
        receiver,
        lexical_class: None,
        superclass: None,
        current_class: None,
    }
}

/// An explicit-receiver send (§5.5), cached (`docs/design/dispatch.md`, "A
/// send, step by step").
#[inline(always)]
pub(crate) fn send_public<'gc>(
    mutator: &mut Mutator<'gc>,
    feedback_base: FeedbackBase,
    family_size: u32,
    site: CallSite,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: Arguments<'_, 'gc>,
) -> Result<Step<'gc>, Signal<'gc>> {
    let Some(index) = site_index(feedback_base, family_size, site) else {
        return mutator.decide_send(receiver, selector, arguments);
    };
    let key = public_key(&mutator.heap, &receiver);
    cached(
        mutator,
        index,
        key,
        selector,
        arguments,
        // `Public` carries no payload, so `miss` never calls this: a
        // zero-sized closure `cached`/`miss` never invoke, kept because
        // splitting them by send kind (an enum, a trait, or a duplicated
        // `miss`) would cost more than this one line does.
        |_mutator| None,
        |mutator, _| mutator.resolve_public(receiver, selector),
    )
}

/// A lexical send's cached site and its §5.9 privacy, `None` while the
/// site is vacant -- or `None` for an uncached send: the one place that
/// decides them, for `send_lexical` and the miss leaf's `decide_at`. A
/// body-less declaration's own code runs once per instantiation of its
/// class and starts its walk from the declaration's scope: it runs
/// uncached, so that every cached send starts at its class.
#[inline(always)]
fn lexical_site(
    caches: &InlineCaches,
    feedback_base: FeedbackBase,
    family_size: u32,
    site: CallSite,
    declaration_code: bool,
) -> Option<(usize, Option<bool>)> {
    site_index(feedback_base, family_size, site)
        .filter(|_| !declaration_code)
        .and_then(|index| {
            caches
                .sites
                .get(index)
                .map(|entry| (index, entry.target.site_privacy()))
        })
}

/// The enclosing object a cached lexical send of `activation` at lexical
/// `depth` is sent to, and where its walk started -- decided by
/// `sends::lexical_origin`, for the uncached path and this one.
#[inline(always)]
fn lexical_receiver<'gc>(
    heap: &Heap<'gc>,
    activation: ActivationReference,
    depth: u16,
) -> (Value<'gc>, LexicalOrigin<'gc>) {
    let (self_value, origin) = super::sends::lexical_origin(heap, activation, false);
    let enclosing = dispatch::kth_enclosing_object(heap, self_value, origin, depth);
    (enclosing, origin)
}

/// The key of a lexical send to `enclosing` at a site whose privacy is
/// decided: N's application for a private site, `o`'s otherwise, so a
/// non-private *hit* still never computes N -- `miss` only calls its
/// `lexical_of` closure on an actual miss, once N is needed for the
/// `Lexical` rule's payload (`docs/design/dispatch.md`, "3b -- The
/// caches").
#[inline(always)]
fn lexical_key<'send, 'gc>(
    heap: &Heap<'gc>,
    enclosing: &'send Value<'gc>,
    origin: LexicalOrigin<'gc>,
    depth: u16,
    private: bool,
) -> SendKey<'send, 'gc> {
    if private {
        let lexical_class = dispatch::kth_lexical_class(heap, origin, depth);
        SendKey {
            guard: lexical_dispatch_id(heap, lexical_class),
            rule: CacheRule::LexicalPrivate,
            receiver: enclosing,
            lexical_class,
            superclass: None,
            current_class: None,
        }
    } else {
        SendKey {
            guard: heap.dispatch_id(*enclosing),
            // A placeholder payload: `miss` fills it in only on a genuine
            // miss, so a monomorphic hit still never computes N.
            rule: CacheRule::Lexical(DispatchId::NONE),
            receiver: enclosing,
            lexical_class: None,
            superclass: None,
            current_class: None,
        }
    }
}

/// N for a lexical send's `Lexical` payload, which `miss` asks only on a
/// genuine miss. The closure borrows the send's locals, as one written in
/// place would, so that handing it to `miss` copies a pointer rather than
/// the walk's origin.
#[inline(always)]
fn lexical_class_of<'send, 'gc>(
    origin: &'send LexicalOrigin<'gc>,
    depth: &'send u16,
) -> impl FnOnce(&Mutator<'gc>) -> Option<ClassReference<'gc>> + 'send {
    move |mutator| dispatch::kth_lexical_class(&mutator.heap, *origin, *depth)
}

/// A lexical send's lookup, handed N where the miss already knows it;
/// borrowing as `lexical_class_of` does.
#[inline(always)]
fn resolve_lexical_send<'send, 'gc>(
    enclosing: &'send Value<'gc>,
    origin: &'send LexicalOrigin<'gc>,
    depth: &'send u16,
    selector: &'send Symbol,
) -> impl FnOnce(&mut Mutator<'gc>, LexicalClass<'gc>) -> Resolved<'gc> + 'send {
    move |mutator, known_lexical_class| {
        let lexical_class = match known_lexical_class {
            LexicalClass::Known(lexical_class) => lexical_class,
            LexicalClass::NotComputed => {
                dispatch::kth_lexical_class(&mutator.heap, *origin, *depth)
            }
        };
        mutator.resolve_lexical(*enclosing, lexical_class, *selector)
    }
}

/// A lexical send (§5.7-§5.9), cached. The site's privacy, decided when it
/// was first filled, chooses the guard (`lexical_key`).
#[expect(
    clippy::too_many_arguments,
    reason = "the site, its guard and the closures that decide a miss are passed separately so the hit path builds nothing"
)]
#[inline(always)]
pub(crate) fn send_lexical<'gc>(
    mutator: &mut Mutator<'gc>,
    activation: ActivationReference,
    feedback_base: FeedbackBase,
    family_size: u32,
    site: CallSite,
    depth: u16,
    declaration_code: bool,
    selector: Symbol,
    arguments: Arguments<'_, 'gc>,
) -> Result<Step<'gc>, Signal<'gc>> {
    let Some((index, privacy)) = lexical_site(
        &mutator.caches,
        feedback_base,
        family_size,
        site,
        declaration_code,
    ) else {
        return decide_lexical_send(
            mutator,
            activation,
            depth,
            declaration_code,
            selector,
            arguments,
        );
    };
    let (enclosing, origin) = lexical_receiver(&mutator.heap, activation, depth);
    let Some(private) = privacy else {
        return send_lexical_first(
            mutator, index, enclosing, origin, depth, selector, arguments,
        );
    };
    let key = lexical_key(&mutator.heap, &enclosing, origin, depth, private);
    cached(
        mutator,
        index,
        key,
        selector,
        arguments,
        lexical_class_of(&origin, &depth),
        resolve_lexical_send(&enclosing, &origin, &depth, &selector),
    )
}

/// N's application (`kth_lexical_class`'s class, by its `instances` dispatch
/// id) for the `Lexical` rule's payload, `DispatchId::NONE` where the
/// lexical chain does not reach that far.
#[inline(always)]
fn lexical_dispatch_id<'gc>(
    heap: &Heap<'gc>,
    lexical_class: Option<ClassReference<'gc>>,
) -> DispatchId {
    lexical_class.map_or(DispatchId::NONE, |class| {
        heap.class_dispatch_ids(class).instances
    })
}

/// A vacant lexical site's first send: `decided_lexical_first`'s
/// decision, acted on.
#[inline(never)]
fn send_lexical_first<'gc>(
    mutator: &mut Mutator<'gc>,
    index: usize,
    enclosing: Value<'gc>,
    origin: LexicalOrigin<'gc>,
    depth: u16,
    selector: Symbol,
    arguments: Arguments<'_, 'gc>,
) -> Result<Step<'gc>, Signal<'gc>> {
    let (resolved, base, _) =
        decided_lexical_first(mutator, index, enclosing, origin, depth, selector);
    mutator.act_with_base(resolved, base, enclosing, selector, arguments)
}

/// A vacant lexical site's first decision, for `send_lexical_first` and
/// the miss leaf's `decide_at`, with the guard the site's entry is found
/// under. Whether §5.9's private rule applies is not known until N is:
/// §5.9's private rule depends only on N and the selector, never on `o`,
/// so once N's application is the `Lexical` rule's payload, a match there
/// is right regardless of which site left it -- and the two rows this
/// checks, `LexicalPrivate` keyed by N alone and `Lexical` keyed by (`o`,
/// N), are the only ones N's decision could have filled. Only a genuine
/// miss on both falls to resolving here, recorded under the key its
/// privacy chooses.
fn decided_lexical_first<'gc>(
    mutator: &mut Mutator<'gc>,
    index: usize,
    enclosing: Value<'gc>,
    origin: LexicalOrigin<'gc>,
    depth: u16,
    selector: Symbol,
) -> (Resolved<'gc>, FeedbackBase, DispatchId) {
    let lexical_class = dispatch::kth_lexical_class(&mutator.heap, origin, depth);
    let lexical_id = lexical_dispatch_id(&mutator.heap, lexical_class);
    if lexical_id != DispatchId::NONE {
        let private_key = SendKey {
            guard: lexical_id,
            rule: CacheRule::LexicalPrivate,
            receiver: &enclosing,
            lexical_class,
            superclass: None,
            current_class: None,
        };
        if let Some((resolved, base)) = global_found(mutator, index, private_key, selector) {
            return (resolved, base, lexical_id);
        }
    }
    let guard = mutator.heap.dispatch_id(enclosing);
    if guard != DispatchId::NONE {
        let public_key = SendKey {
            guard,
            rule: CacheRule::Lexical(lexical_id),
            receiver: &enclosing,
            lexical_class: None,
            superclass: None,
            current_class: None,
        };
        if let Some((resolved, base)) = global_found(mutator, index, public_key, selector) {
            return (resolved, base, guard);
        }
    }
    let resolved = mutator.resolve_lexical(enclosing, lexical_class, selector);
    let private = resolved.chain() == Some(Chain::Private);
    let (guard, rule) = if private {
        (lexical_id, CacheRule::LexicalPrivate)
    } else {
        (guard, CacheRule::Lexical(lexical_id))
    };
    let key = SendKey {
        guard,
        rule,
        receiver: &enclosing,
        lexical_class,
        superclass: None,
        current_class: None,
    };
    fill(mutator, index, key, selector, &resolved);
    (resolved, FeedbackBase::NOT_ASSIGNED, guard)
}

/// `key`'s global-cache row materialized, the still-vacant site `index`
/// advanced to it; `None` on a miss, which leaves the site untouched for
/// the caller to resolve instead.
fn global_found<'gc>(
    mutator: &mut Mutator<'gc>,
    index: usize,
    key: SendKey<'_, 'gc>,
    selector: Symbol,
) -> Option<(Resolved<'gc>, FeedbackBase)> {
    let target = mutator
        .caches
        .global_get(key.guard, selector, key.rule)
        .cloned()?;
    let found = materialize(mutator, target.clone(), key)?;
    let cell = cell_for(mutator, key, &target);
    mutator
        .caches
        .advance(index, key.guard, target, key.private(), cell, &mutator.jit);
    Some(found)
}

/// A `super` send's cached site, its activation's current class and S
/// (`sends::super_start`): the one place that decides them, for
/// `send_super` and the miss leaf's `decide_at`. `None` for an uncached
/// site, and where there is no current class or no superclass, whose
/// errors the uncached path raises.
#[inline(always)]
fn super_site<'gc>(
    mutator: &Mutator<'gc>,
    activation: ActivationReference,
    feedback_base: FeedbackBase,
    family_size: u32,
    site: CallSite,
) -> Option<(usize, ClassReference<'gc>, ClassReference<'gc>)> {
    site_index(feedback_base, family_size, site).and_then(|index| {
        let current_class = mutator.heap.activation_current_class(activation)?;
        let superclass = super::sends::super_start(&mutator.heap, current_class)?;
        Some((index, current_class, superclass))
    })
}

/// The key of a `super` send of `self_value`, keyed by S's application:
/// everything §5.10 finds, and the step `level_above` takes from S on a
/// hit, is a function of S alone.
#[inline(always)]
fn super_key<'send, 'gc>(
    heap: &Heap<'gc>,
    self_value: &'send Value<'gc>,
    current_class: ClassReference<'gc>,
    superclass: ClassReference<'gc>,
) -> SendKey<'send, 'gc> {
    SendKey {
        guard: heap.class_dispatch_ids(superclass).instances,
        rule: CacheRule::Super,
        receiver: self_value,
        lexical_class: None,
        superclass: Some(superclass),
        current_class: Some(current_class),
    }
}

/// A `super` send (§5.10), cached, keyed by S's application
/// (`super_key`). A body-less declaration's own code is cached like any
/// other: its S is its class object's superclass, as every code's is. Its
/// two errors are raised by the uncached path, which is where a send
/// without a `current_class` or a superclass goes.
#[inline(always)]
pub(crate) fn send_super<'gc>(
    mutator: &mut Mutator<'gc>,
    activation: ActivationReference,
    feedback_base: FeedbackBase,
    family_size: u32,
    site: CallSite,
    selector: Symbol,
    arguments: Arguments<'_, 'gc>,
) -> Result<Step<'gc>, Signal<'gc>> {
    let Some((index, current_class, superclass)) =
        super_site(mutator, activation, feedback_base, family_size, site)
    else {
        return decide_super_send(mutator, activation, selector, arguments);
    };
    let self_value = mutator.heap.activation_receiver(activation);
    let key = super_key(&mutator.heap, &self_value, current_class, superclass);
    cached(
        mutator,
        index,
        key,
        selector,
        arguments,
        // `Super` carries no payload either -- see `send_public`'s copy of
        // this closure for why it stays rather than being split out.
        |_mutator| None,
        |mutator, _| mutator.resolve_super(superclass, selector),
    )
}

/// What a miss already knows of a lexical send's N when it falls to the
/// lookup, so that N is computed once.
#[derive(Clone, Copy)]
enum LexicalClass<'gc> {
    /// Nothing needed N yet -- every `Public` and `Super` send, and a
    /// `Lexical` one whose guard has no id.
    NotComputed,
    /// N as computed: the class `depth` levels out, or `None` where the
    /// lexical chain does not reach that far.
    Known(Option<ClassReference<'gc>>),
}

/// The target of the site's own entry for `key`'s guard: a monomorphic
/// hit once `materialize` finds its step up, or `None`. It answers the
/// target rather than its `Resolved`, so that the caller materializes
/// and acts where the `Resolved` is consumed: a `Resolved` answered out
/// of here is handed back through memory and tested again before
/// `act_with_base`, on the hit path of every send.
#[inline(always)]
fn site_target(mutator: &Mutator<'_>, index: usize, guard: DispatchId) -> Option<Target> {
    let entry = mutator.caches.sites.get(index)?;
    (entry.guard == guard).then(|| entry.target.clone())
}

/// A monomorphic hit, or everything else out of line. `lexical_of` answers
/// N, for the `Lexical` rule's payload; it is never called here, only handed
/// to `miss`, so a hit costs nothing for it.
#[inline(always)]
fn cached<'gc>(
    mutator: &mut Mutator<'gc>,
    index: usize,
    key: SendKey<'_, 'gc>,
    selector: Symbol,
    arguments: Arguments<'_, 'gc>,
    lexical_of: impl FnOnce(&Mutator<'gc>) -> Option<ClassReference<'gc>>,
    resolve: impl FnOnce(&mut Mutator<'gc>, LexicalClass<'gc>) -> Resolved<'gc>,
) -> Result<Step<'gc>, Signal<'gc>> {
    if let Some(target) = site_target(mutator, index, key.guard) {
        if let Some((resolved, base)) = materialize(mutator, target, key) {
            return mutator.act_with_base(resolved, base, *key.receiver, selector, arguments);
        }
    }
    miss(
        mutator,
        index,
        key.guard,
        key.rule,
        *key.receiver,
        key.lexical_class,
        key.superclass,
        key.current_class,
        selector,
        arguments,
        lexical_of,
        resolve,
    )
}

/// `decided_miss`'s decision, acted on. Out of line so that a monomorphic
/// hit keeps its registers; not `#[cold]`, since a polymorphic hit comes
/// through here. It takes its key's parts rather than the key: a key
/// handed over whole is handed over through memory, and the sender then
/// builds it in memory where it is made -- before the guard's compare,
/// on the hit path of every send, the receiver's word spilled for the
/// key's reference to it -- where its parts, in registers until this
/// call, are stored only on the miss (`cargo xtask gates codegen`).
#[expect(
    clippy::too_many_arguments,
    reason = "the key's parts are passed one by one so that the hit path builds no key in memory"
)]
#[inline(never)]
fn miss<'gc>(
    mutator: &mut Mutator<'gc>,
    index: usize,
    guard: DispatchId,
    rule: CacheRule,
    receiver: Value<'gc>,
    lexical_class: Option<ClassReference<'gc>>,
    superclass: Option<ClassReference<'gc>>,
    current_class: Option<ClassReference<'gc>>,
    selector: Symbol,
    arguments: Arguments<'_, 'gc>,
    lexical_of: impl FnOnce(&Mutator<'gc>) -> Option<ClassReference<'gc>>,
    resolve: impl FnOnce(&mut Mutator<'gc>, LexicalClass<'gc>) -> Resolved<'gc>,
) -> Result<Step<'gc>, Signal<'gc>> {
    let key = SendKey {
        guard,
        rule,
        receiver: &receiver,
        lexical_class,
        superclass,
        current_class,
    };
    decided_miss(
        mutator,
        index,
        key,
        selector,
        lexical_of,
        resolve,
        #[inline(always)]
        |mutator, resolved, base| {
            mutator.act_with_base(resolved, base, receiver, selector, arguments)
        },
        |mutator, resolved| {
            mutator.check_super_handler_slot(&resolved, receiver)?;
            mutator.act(resolved, receiver, selector, arguments)
        },
    )
}

/// A send's decision past its site's own entry, for `miss` and the miss
/// leaf's `decided`: the site's polymorphic block, then the global cache,
/// then `resolve`, whose answer is recorded in both. `lexical_of` is
/// called at most once, only once a miss is certain and the `Lexical`
/// rule's payload is actually needed -- and not at all for a `Public` or
/// `Super` key, whose rule has no payload to fill. `resolve` is handed N
/// where it is already known -- a private key's, or the payload's -- so
/// that a lexical miss computes it once. Only a resolution can be a
/// `super` `doesNotUnderstand:` slot handler, which is never recorded, so
/// that `miss`'s check of it after any of the three is the uncached
/// send's.
///
/// What is decided goes on where it is found rather than back as a
/// value: a hit in the block or the global cache to `hit`, a lookup's
/// resolution to `resolved`, so that `miss` acts at each, as the
/// interpreter's send did before the miss leaf shared the decision. A
/// resolution carried back through one join was copied through memory on
/// every polymorphic and megamorphic hit, read back wider than it was
/// written, a store-forwarding stall that cost DeltaBlue 5% with the JIT
/// off.
#[expect(
    clippy::too_many_arguments,
    reason = "the key, the two lookups it may need and the two places a decision goes on, each a closure the caller's send inlines"
)]
#[inline(always)]
fn decided_miss<'gc, Answer>(
    mutator: &mut Mutator<'gc>,
    index: usize,
    mut key: SendKey<'_, 'gc>,
    selector: Symbol,
    lexical_of: impl FnOnce(&Mutator<'gc>) -> Option<ClassReference<'gc>>,
    resolve: impl FnOnce(&mut Mutator<'gc>, LexicalClass<'gc>) -> Resolved<'gc>,
    hit: impl FnOnce(&mut Mutator<'gc>, Resolved<'gc>, FeedbackBase) -> Answer,
    resolved: impl FnOnce(&mut Mutator<'gc>, Resolved<'gc>) -> Answer,
) -> Answer {
    let mut known_lexical_class = if key.private() {
        LexicalClass::Known(key.lexical_class)
    } else {
        LexicalClass::NotComputed
    };
    if key.guard != DispatchId::NONE {
        if let Some(target) = mutator.caches.polymorphic_hit(index, key.guard) {
            if let Some((found, base)) = materialize(mutator, target, key) {
                return hit(mutator, found, base);
            }
        }
        if let CacheRule::Lexical(ref mut lexical) = key.rule {
            let lexical_class = lexical_of(mutator);
            *lexical = lexical_dispatch_id(&mutator.heap, lexical_class);
            known_lexical_class = LexicalClass::Known(lexical_class);
        }
        let global = mutator
            .caches
            .global_get(key.guard, selector, key.rule)
            .cloned();
        if let Some(target) = global {
            if let Some((found, base)) = materialize(mutator, target.clone(), key) {
                let cell = cell_for(mutator, key, &target);
                mutator
                    .caches
                    .advance(index, key.guard, target, key.private(), cell, &mutator.jit);
                return hit(mutator, found, base);
            }
        }
    }
    let found = resolve(mutator, known_lexical_class);
    fill(mutator, index, key, selector, &found);
    resolved(mutator, found)
}

/// What the miss leaf answers machine code for a send its cell did not
/// decide (`decide_at`).
pub(crate) enum MissAnswer {
    /// Jump to this address, the callee's machine entry, its receiver and
    /// current class handed over in the machine words
    /// (`Heap::hand_machine_callee`).
    Enter(u64),
    /// Refuse the call: the interpreter runs the send.
    Refuse,
}

/// A send's decision as `decision_at` makes it for machine code's leaves:
/// what it resolved to and the activation's base, its receiver, the guard
/// and site it was decided at, the global key its cells are refreshed
/// under (`InlineCaches::refresh_cells`), and its selector and argument
/// slots.
struct Decision<'gc> {
    resolved: Resolved<'gc>,
    base: FeedbackBase,
    receiver: Value<'gc>,
    guard: DispatchId,
    index: usize,
    global_key: Option<(Symbol, CacheRule)>,
    selector: Symbol,
    argument_base: u16,
    argument_count: u16,
}

/// The decision of the send at `program_counter` of `activation` -- a
/// `SendPublic`, `SendLexical` or `SendSuper` -- for machine code's
/// leaves: the decision the interpreter's cached send makes
/// (`send_public`, `send_lexical`, `send_super`), through the same
/// functions -- its site and key (`public_key`, `lexical_site`,
/// `lexical_receiver` and `lexical_key`, `super_site` and `super_key`),
/// the site's entry (`site_target`, materialized), the rest
/// (`decided_miss`, or a vacant lexical site's `decided_lexical_first`),
/// filling the caches as they fill them -- but no `act`. `None` for an
/// uncached site, which only the interpreter decides, and for a `super`
/// send without a current class or a superclass, whose error it raises.
fn decision_at<'gc>(
    mutator: &mut Mutator<'gc>,
    activation: ActivationReference,
    program_counter: u32,
) -> Option<Decision<'gc>> {
    let code = mutator.heap.activation_code(activation);
    let feedback_base = mutator.heap.activation_feedback_base(activation);
    let family_size = code.family_size;
    let Some(CachedSend {
        kind,
        selector,
        call_site,
        argument_base,
        argument_count,
    }) = code.cached_send(program_counter as usize)
    else {
        unreachable!("internal: only a cached send calls through its cell");
    };
    let (resolved, base, receiver, guard, index, global_key) = match kind {
        CachedSendKind::Public {
            receiver: receiver_register,
        } => {
            let index = site_index(feedback_base, family_size, call_site)?;
            let register_base = mutator.heap.activation_register_base(activation);
            let receiver = mutator.heap.register(register_base, receiver_register);
            let key = public_key(&mutator.heap, &receiver);
            let (resolved, base) = decided(
                mutator,
                index,
                key,
                selector,
                |_mutator| None,
                |mutator, _| mutator.resolve_public(receiver, selector),
            );
            let global_key = Some((selector, CacheRule::Public));
            (resolved, base, receiver, key.guard, index, global_key)
        }
        CachedSendKind::Lexical { depth } => {
            let (index, privacy) = lexical_site(
                &mutator.caches,
                feedback_base,
                family_size,
                call_site,
                code.is_declaration_code(),
            )?;
            let (enclosing, origin) = lexical_receiver(&mutator.heap, activation, depth);
            match privacy {
                None => {
                    // The site's own cell is the only one its first send
                    // could have reached.
                    let (resolved, base, guard) =
                        decided_lexical_first(mutator, index, enclosing, origin, depth, selector);
                    (resolved, base, enclosing, guard, index, None)
                }
                Some(private) => {
                    let key = lexical_key(&mutator.heap, &enclosing, origin, depth, private);
                    let (resolved, base) = decided(
                        mutator,
                        index,
                        key,
                        selector,
                        lexical_class_of(&origin, &depth),
                        resolve_lexical_send(&enclosing, &origin, &depth, &selector),
                    );
                    // The rule as `decided_miss` filled its payload in.
                    let rule = if private {
                        CacheRule::LexicalPrivate
                    } else {
                        let lexical_class =
                            dispatch::kth_lexical_class(&mutator.heap, origin, depth);
                        CacheRule::Lexical(lexical_dispatch_id(&mutator.heap, lexical_class))
                    };
                    (
                        resolved,
                        base,
                        enclosing,
                        key.guard,
                        index,
                        Some((selector, rule)),
                    )
                }
            }
        }
        CachedSendKind::Super => {
            let (index, current_class, superclass) =
                super_site(mutator, activation, feedback_base, family_size, call_site)?;
            let self_value = mutator.heap.activation_receiver(activation);
            let key = super_key(&mutator.heap, &self_value, current_class, superclass);
            let (resolved, base) = decided(
                mutator,
                index,
                key,
                selector,
                |_mutator| None,
                |mutator, _| mutator.resolve_super(superclass, selector),
            );
            let global_key = Some((selector, CacheRule::Super));
            (resolved, base, self_value, key.guard, index, global_key)
        }
    };
    Some(Decision {
        resolved,
        base,
        receiver,
        guard,
        index,
        global_key,
        selector,
        argument_base,
        argument_count: crate::heap::register_number(argument_count),
    })
}

/// The miss leaf's decision (`jit::leaves`'s `miss`) of the send at
/// `program_counter` of `activation`: `decision_at`'s, and so an uncached
/// site, which only the interpreter decides, refuses. A method machine
/// code runs (`jit::runs_in_machine_code`) is its callee: its body is
/// compiled when this entry is the one that compiles it
/// (`jit::note_entry_by_call`), and the cells the send reached its
/// decision through -- the site's, its polymorphic block's slot, the
/// global cache's row -- are written afresh
/// (`InlineCaches::refresh_cells`): once the body has machine code they
/// name its machine entry, so that its next send calls the body straight,
/// and the answer is that entry, the callee's receiver and current class
/// handed over for it to store; once the body never will -- refused, or
/// exiting at once -- they name the refusing stub, so that the next send
/// exits at once, and the answer refuses the call. A target that is no method with code is entered at the stub
/// its cell names (`jit::stub_entry`), which runs it as `act` does, the
/// receiver handed over. Every other decision refuses the call, and the interpreter,
/// running the send again, finds the entry the fill made, or raises the
/// error a `super` send without a current class or a superclass raises.
pub(crate) fn decide_at<'gc>(
    mutator: &mut Mutator<'gc>,
    activation: ActivationReference,
    program_counter: u32,
) -> MissAnswer {
    let Some(Decision {
        resolved,
        base,
        receiver,
        guard,
        index,
        global_key,
        argument_count,
        ..
    }) = decision_at(mutator, activation, program_counter)
    else {
        return MissAnswer::Refuse;
    };
    // A factory's body runs on the class object the send instantiates, as
    // its receiver and its current class: compiled and entered as a
    // method's body is, below. One that stands for too many arguments
    // takes another count than the send places; only the push lays its
    // frame out. Every miss counts the entry: the interpreter's push of a
    // factory runs its first two instructions itself
    // (`Mutator::enter_factory_body`) and never enters its body at its
    // start, where a push counts, so a count left to it would never
    // compile the body.
    if let Resolved::Factory { level, code, .. } = &resolved {
        if code.argument_count != argument_count {
            return MissAnswer::Refuse;
        }
        let base = mutator.feedback_base_for(code);
        jit::note_entry(&mut mutator.jit, code, base, None);
        jit::after_compile_trigger(mutator);
        let address = jit::machine_entry_address(&mutator.jit, code, base);
        mutator
            .caches
            .refresh_cells(index, guard, global_key, &mutator.jit);
        let Some(address) = address else {
            return MissAnswer::Refuse;
        };
        mutator.heap.hand_machine_callee(receiver, *level);
        return MissAnswer::Enter(address);
    }
    let stubbed = match &resolved {
        Resolved::Method { method, chain, .. } if !jit::runs_in_machine_code(method, *chain) => {
            Some(jit::Stubbed::Method {
                method,
                chain: *chain,
            })
        }
        Resolved::Method { .. } => None,
        Resolved::Slot {
            kind, field, chain, ..
        } => Some(jit::Stubbed::Slot {
            kind: *kind,
            chain: *chain,
            field: *field,
        }),
        Resolved::Primitive {
            primitive, chain, ..
        } => Some(jit::Stubbed::Primitive {
            primitive: *primitive,
            chain: *chain,
        }),
        Resolved::Factory { .. } => {
            unreachable!("internal: a factory was answered above")
        }
        Resolved::NestedClass { chain, field, .. } => Some(jit::Stubbed::NestedClass {
            chain: *chain,
            field: *field,
        }),
        Resolved::Unhandled => {
            return MissAnswer::Refuse;
        }
    };
    if let Some(stubbed) = stubbed {
        return match jit::stub_entry(&mutator.jit, &stubbed) {
            Some(entry) => {
                mutator.heap.hand_machine_receiver(receiver);
                MissAnswer::Enter(entry)
            }
            None => MissAnswer::Refuse,
        };
    }
    let Resolved::Method {
        method,
        defining_class,
        chain: _,
    } = resolved
    else {
        unreachable!("internal: every other resolution was answered above");
    };
    let base = if base == FeedbackBase::NOT_ASSIGNED {
        mutator.feedback_base_for(&method.code)
    } else {
        base
    };
    jit::note_entry_by_call(&mut mutator.jit, &method.code, base, Some(&method));
    jit::after_compile_trigger(mutator);
    let address = jit::machine_entry_address(&mutator.jit, &method.code, base);
    // The cells the decision went through follow the JIT's state either
    // way: the callee's machine entry once it has one, the refusing stub
    // once it never will -- refused or exiting at once -- so that the
    // site's next send exits at once rather than calling the miss leaf
    // again; the miss stub while it may still be compiled.
    mutator
        .caches
        .refresh_cells(index, guard, global_key, &mutator.jit);
    let Some(address) = address else {
        return MissAnswer::Refuse;
    };
    mutator.heap.hand_machine_callee(receiver, defining_class);
    MissAnswer::Enter(address)
}

/// The leaf stub's run (`jit::leaves`'s `act`) of the send at
/// `program_counter` of `activation`, whose cell named the leaf stub for
/// the target it records: `decision_at`'s decision -- which finds that
/// entry, since a cell and its entry are written together -- acted on by
/// `Mutator::act_for_machine_code`, on the send's arguments in its slots,
/// where the call placed them. The value the send answers, or `None`
/// where the interpreter must run the send, nothing having changed.
pub(crate) fn act_at<'gc>(
    mutator: &mut Mutator<'gc>,
    activation: ActivationReference,
    program_counter: u32,
) -> Option<Value<'gc>> {
    let decision = decision_at(mutator, activation, program_counter)?;
    let register_base = mutator.heap.activation_register_base(activation);
    let arguments = Arguments::Slots(crate::heap::ArgumentSlots::new(
        register_base,
        decision.argument_base,
        decision.argument_count,
    ));
    mutator.act_for_machine_code(
        decision.resolved,
        decision.receiver,
        decision.selector,
        arguments,
    )
}

/// `cached`'s decision, without `act`: the site's own entry
/// (`site_target`, materialized), else `decided_miss`.
fn decided<'gc>(
    mutator: &mut Mutator<'gc>,
    index: usize,
    key: SendKey<'_, 'gc>,
    selector: Symbol,
    lexical_of: impl FnOnce(&Mutator<'gc>) -> Option<ClassReference<'gc>>,
    resolve: impl FnOnce(&mut Mutator<'gc>, LexicalClass<'gc>) -> Resolved<'gc>,
) -> (Resolved<'gc>, FeedbackBase) {
    if let Some(target) = site_target(mutator, index, key.guard) {
        if let Some(found) = materialize(mutator, target, key) {
            return found;
        }
    }
    decided_miss(
        mutator,
        index,
        key,
        selector,
        lexical_of,
        resolve,
        |_mutator, found, base| (found, base),
        |_mutator, found| (found, FeedbackBase::NOT_ASSIGNED),
    )
}

/// Records `resolved` in the global cache and at the site -- unless the key
/// has no id, or the resolution's privacy is not the site's (only a crafted
/// image makes a site's N change; refinement 1). Before `act`, which may run
/// code. `key.rule`'s `Lexical` payload, if any, must already be N's
/// application -- `decided_miss` fills it in before falling through
/// here; `decided_lexical_first` builds it directly, since it never goes
/// through `decided_miss`.
fn fill<'gc>(
    mutator: &mut Mutator<'gc>,
    index: usize,
    key: SendKey<'_, 'gc>,
    selector: Symbol,
    resolved: &Resolved<'gc>,
) {
    if key.guard == DispatchId::NONE {
        return;
    }
    if (resolved.chain() == Some(Chain::Private)) != key.private() {
        return;
    }
    if let Some(target) = record(mutator, resolved, key) {
        let cell = cell_for(mutator, key, &target);
        mutator
            .caches
            .global_insert(key.guard, selector, key.rule, target.clone(), cell);
        mutator
            .caches
            .advance(index, key.guard, target, key.private(), cell, &mutator.jit);
        // The sender's feedback changed: its optimization waits for it to
        // settle -- but under the optimize stress mode, whose budgets run
        // out at once. The sender is the innermost activation, the
        // interpreter's and the miss leaf's alike
        // (`materialize::leaf_frame`).
        #[cfg(ns_jit)]
        if !mutator.jit.optimize_stress() {
            mutator
                .heap
                .raise_running_optimization_budget(jit::BUDGET_AFTER_CACHE_CHANGE);
        }
    }
}

/// The cell an entry for `target` takes at a site with `key`: its guard
/// (the key's dispatch id, or for a `super` or private lexical site the
/// key class's application word, as `Cell` says), its entry address from
/// the JIT (`jit::cell_entry`), and where its `current_class` is found.
fn cell_for<'gc>(mutator: &Mutator<'gc>, key: SendKey<'_, 'gc>, target: &Target) -> Cell {
    let application_word = |class: Option<ClassReference<'gc>>| {
        class
            .and_then(|class| mutator.heap.class_application_word(class))
            .unwrap_or(VACANT_GUARD)
    };
    let chain_start = rule_chain_start(key.rule);
    let guard = match key.rule {
        CacheRule::Public | CacheRule::Lexical(_) => u64::from(key.guard.bits()),
        CacheRule::LexicalPrivate => application_word(key.lexical_class),
        CacheRule::Super => application_word(key.superclass),
    };
    Cell {
        guard,
        entry: jit::cell_entry(&mutator.jit, target),
        level: cell_level(target),
        chain_start: chain_start_word(chain_start, target),
    }
}

/// A cell's `chain_start` word for `target`, counted from `chain_start`:
/// the chain start, and above it a private or `super` slot accessor's
/// recorded guard, which its stub compares a receiver's with
/// (`Cell::chain_start`) -- a copy of what the entry records, so that the
/// cell stays a function of its entry's target.
fn chain_start_word(chain_start: ChainStart, target: &Target) -> u64 {
    let recorded = match target {
        Target::Slot {
            chain: Chain::Private | Chain::Super,
            recorded_guard,
            ..
        } => u64::from(recorded_guard.bits()),
        _ => 0,
    };
    chain_start as u64 | (recorded << CHAIN_START_BITS)
}

/// The cell of a site that holds no entry of its own -- polymorphic or
/// megamorphic -- whose guard no receiver matches, so that machine code
/// takes the stub its entry names; `chain_start` is the site's, kept so
/// that a lexical site's cell still says which guard its sends compare.
fn site_state_cell(jit: &JitState, target: &Target, chain_start: u64) -> Cell {
    Cell {
        guard: VACANT_GUARD,
        entry: jit::cell_entry(jit, target),
        level: cell_level(target),
        chain_start,
    }
}

/// A cell's `level` word for `target`: its superclass steps from the
/// chain's start, 0 for a target that counts none -- a slot, which runs
/// no body, and a factory, which instantiates the receiver -- and a
/// polymorphic site's block number.
fn cell_level(target: &Target) -> u64 {
    match target {
        Target::Method { level, .. }
        | Target::NestedClass { level, .. }
        | Target::Primitive { level, .. } => u64::from(*level),
        Target::Polymorphic { block, .. } => u64::from(*block),
        Target::Vacant
        | Target::Megamorphic { .. }
        | Target::Slot { .. }
        | Target::Factory { .. } => 0,
    }
}

/// Where a target found on `chain` counts its levels from, for its cell.
fn chain_start_of(chain: Chain) -> ChainStart {
    match chain {
        Chain::Public | Chain::Protected | Chain::DoesNotUnderstand => ChainStart::ReceiverClass,
        Chain::Private => ChainStart::LexicalClass,
        Chain::Super | Chain::SuperDoesNotUnderstand => ChainStart::Superclass,
    }
}

/// A global row's `rule` word: the rule's tag in the low
/// `RULE_TAG_BITS` bits, and a `Lexical` rule's N -- its application's
/// dispatch id -- above them.
pub(crate) fn rule_word(rule: CacheRule) -> u64 {
    match rule {
        CacheRule::Lexical(lexical) => {
            rule_tag(rule) | (u64::from(lexical.bits()) << RULE_TAG_BITS)
        }
        CacheRule::Public | CacheRule::LexicalPrivate | CacheRule::Super => rule_tag(rule),
    }
}

/// How many low bits of a rule word its tag takes.
pub(crate) const RULE_TAG_BITS: u8 = 2;

/// The mask of a rule word's tag.
pub(crate) const RULE_TAG_MASK: i32 = (1 << RULE_TAG_BITS) - 1;

/// A `Lexical` rule's tag, beneath its N.
pub(crate) const RULE_TAG_LEXICAL: i32 = 1;

/// The tag of `rule`, which a send in machine code hands the stubs
/// (`jit::targets`): its discriminant, in `CacheRule`'s order, which is
/// also what the rule's derived `Hash` writes first -- the megamorphic
/// stub hashes the tag where `GlobalCache::group` hashes the rule.
pub(crate) const fn rule_tag(rule: CacheRule) -> u64 {
    match rule {
        CacheRule::Public => 0,
        CacheRule::Lexical(_) => RULE_TAG_LEXICAL as u64,
        CacheRule::LexicalPrivate => 2,
        CacheRule::Super => 3,
    }
}

/// A call site's state as a test sees it (`Interpreter::call_site_states`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallSiteState {
    Vacant,
    Monomorphic(CachedTarget),
    /// The block's filled entries, in the order first seen.
    Polymorphic(Vec<CachedTarget>),
    Megamorphic,
}

/// One entry's target as a test sees it: what it runs, and whether §5.9's
/// private rule found it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedTarget {
    pub kind: CachedTargetKind,
    pub private: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CachedTargetKind {
    Method(Symbol),
    Slot {
        field: u32,
    },
    /// The nested class's name, and the absolute index of the hidden slot
    /// a hit reads, `None` where a hit takes the accessor's lookup.
    NestedClass {
        name: Symbol,
        field: Option<u32>,
    },
    Primitive,
    Factory,
    /// A `doesNotUnderstand:` handler, whatever it is.
    DoesNotUnderstand,
}

/// Which side of a class a method is looked up on for
/// `Interpreter::call_site_states`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MethodSide {
    Instance,
    Class,
}

/// A call site's cell as a test reads it (`Interpreter::call_site_cells`):
/// the four words machine code reads (`cells::Cell`).
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallSiteCell {
    pub guard: u64,
    pub entry: u64,
    pub level: u64,
    pub chain_start: u64,
}

impl CallSiteCell {
    /// Whether the cell reads as a vacant site's: no receiver matches it.
    pub fn is_vacant(&self) -> bool {
        let vacant = Cell::VACANT;
        (self.guard, self.entry, self.level, self.chain_start)
            == (vacant.guard, vacant.entry, vacant.level, vacant.chain_start)
    }
}

impl InlineCaches {
    /// The states of the sites in `code`'s family that send `sent`, in
    /// emission order, or all vacant when the family has not run. For tests
    /// (`Interpreter::call_site_states`).
    pub(crate) fn states_of(&self, code: &Rc<CompiledCode>, sent: Symbol) -> Vec<CallSiteState> {
        self.site_indices_of(code, sent)
            .into_iter()
            .map(
                |index| match index.and_then(|index| self.sites.get(index)) {
                    None => CallSiteState::Vacant,
                    Some(entry) => self.state_of(entry),
                },
            )
            .collect()
    }

    /// The cells of the sites in `code`'s family that send `sent`, in
    /// emission order, each vacant while the family has not run. For tests
    /// (`Interpreter::call_site_cells`).
    pub(crate) fn cells_of(&self, code: &Rc<CompiledCode>, sent: Symbol) -> Vec<CallSiteCell> {
        self.site_indices_of(code, sent)
            .into_iter()
            .map(|index| {
                let cell = index.map_or(Cell::VACANT, |index| self.cells.site(index));
                CallSiteCell {
                    guard: cell.guard,
                    entry: cell.entry,
                    level: cell.level,
                    chain_start: cell.chain_start,
                }
            })
            .collect()
    }

    /// The table index of each site in `code`'s family that sends `sent`,
    /// in emission order; `None` for each while the family has not run.
    fn site_indices_of(&self, code: &Rc<CompiledCode>, sent: Symbol) -> Vec<Option<usize>> {
        // Call-site numbers are dense in emission order across the family,
        // so sorting by number is emission order.
        let mut numbers = Vec::new();
        let mut pending: Vec<Rc<CompiledCode>> = vec![Rc::clone(code)];
        while let Some(body) = pending.pop() {
            for instruction in &body.instructions() {
                if let Instruction::MakeClosure { code: block, .. } = instruction {
                    pending.push(Rc::clone(block));
                }
                if let Some(site) = instruction.call_site() {
                    if site != CallSite::UNCACHED && sent_selector(instruction) == Some(sent) {
                        numbers.push(site.number);
                    }
                }
            }
        }
        numbers.sort_unstable();
        // `families` has no entry at all while the family has never run --
        // treated the same as `NOT_ASSIGNED`, since `site_index` already
        // answers `None` for that and every site is vacant either way.
        let base = self
            .families
            .get(&(Rc::as_ptr(code) as usize))
            .map_or(FeedbackBase::NOT_ASSIGNED, |family| family.base);
        numbers
            .into_iter()
            .map(|number| site_index(base, code.family_size, CallSite { number }))
            .collect()
    }

    /// Whether every cell agrees with its entry -- each site's, each
    /// polymorphic block slot's, each global row's -- as the writes beside
    /// the entries are to keep it: the first disagreement found, described,
    /// otherwise. Run by `Interpreter::verify_cells` and, in a debug build,
    /// before every scavenge of the scavenge stress mode.
    pub(crate) fn verify_cells(&self, heap: &Heap<'_>, jit: &JitState) -> Result<(), String> {
        self.verify_cells_against(&|word| heap.application_dispatch_id_for_word(word), jit)
    }

    /// `verify_cells`, with `application_of` answering the instances id
    /// of the application whose class objects hold a word in their
    /// application field (`Heap::application_dispatch_id_for_word`).
    fn verify_cells_against(
        &self,
        application_of: &dyn Fn(u64) -> Option<DispatchId>,
        jit: &JitState,
    ) -> Result<(), String> {
        let site_words = self.cells.site_words(self.sites.len());
        let block_words = self.cells.block_words(self.pool.len());
        let global_words = self.cells.global_words(self.global.entries.len());
        // Every table's cells were written vacant as it grew, so each slice
        // holds one cell or row per entry.
        if site_words.len() != self.sites.len() * cells::CELL_WORDS
            || block_words.len() != self.pool.len() * POLYMORPHIC_WIDTH * cells::CELL_WORDS
            || global_words.len() != self.global.entries.len() * cells::ROW_WORDS
        {
            return Err("the cells of a table were never written".to_owned());
        }
        // Most entries are vacant, and a vacant entry's cell must read
        // `Cell::VACANT` whole: the guard no receiver matches, and the entry
        // word 0, since the polymorphic stub, entered on every mismatch,
        // jumps to the entry of a cell whose guard is vacant.
        let check_cell = |entry: &CacheEntry, words: &[u64], place: &dyn Fn() -> String| {
            if matches!(entry.target, Target::Vacant) {
                if words != Cell::VACANT.words() {
                    return Err(format!(
                        "{}: a vacant entry's cell reads {words:?}",
                        place()
                    ));
                }
                return Ok(None);
            }
            let found = Cell::from_words(words);
            match expected_cell(entry.guard, &entry.target, found, application_of, jit) {
                Ok(Some(expected)) if !cell_may_be(found, expected, jit) => Err(format!(
                    "{}: the cell reads {found:?}, its entry's is {expected:?}",
                    place()
                )),
                Ok(Some(_)) => Ok(None),
                Ok(None) => Ok(Some(found)),
                Err(problem) => Err(format!("{}: {problem}", place())),
            }
        };
        for (index, (entry, words)) in self
            .sites
            .iter()
            .zip(site_words.as_chunks::<{ cells::CELL_WORDS }>().0)
            .enumerate()
        {
            if let Some(found) = check_cell(entry, words, &|| format!("site {index}"))? {
                verify_site_state_cell(entry, found, jit)
                    .map_err(|problem| format!("site {index}: {problem}"))?;
            }
        }
        for (block, (slots, words)) in self
            .pool
            .iter()
            .zip(
                block_words
                    .as_chunks::<{ POLYMORPHIC_WIDTH * cells::CELL_WORDS }>()
                    .0,
            )
            .enumerate()
        {
            for (position, (slot, words)) in slots
                .iter()
                .zip(words.as_chunks::<{ cells::CELL_WORDS }>().0)
                .enumerate()
            {
                let place = || format!("block {block} slot {position}");
                if check_cell(slot, words, &place)?.is_some() {
                    return Err(format!("{}: a block holds a site state", place()));
                }
            }
        }
        for (index, entry) in self.global.entries.iter().enumerate() {
            let words = &global_words[index * cells::ROW_WORDS..][..cells::ROW_WORDS];
            if entry.is_vacant() {
                let row = GlobalRow::from_words(words);
                if row.guard != VACANT_GUARD || row.entry != GlobalRow::VACANT.entry {
                    return Err(format!(
                        "global row {index} of a vacant entry reads {words:?}"
                    ));
                }
                continue;
            }
            let found = GlobalRow::from_words(words);
            let found_cell = Cell {
                guard: found.guard,
                entry: found.entry,
                level: found.level,
                chain_start: found.chain_start,
            };
            // A row's guard is the key's dispatch id whatever the rule,
            // compared below, so the cell's guard is not asked about.
            let any_guard = |_: u64| Some(entry.guard);
            let cell = expected_cell(entry.guard, &entry.target, found_cell, &any_guard, jit)
                .map_err(|problem| format!("global entry {index}: {problem}"))?
                .ok_or_else(|| format!("global entry {index} holds a site state"))?;
            if cell.start() != rule_chain_start(entry.rule) as u64 {
                return Err(format!("global entry {index}'s rule is not its target's"));
            }
            let expected = GlobalRow {
                guard: u64::from(entry.guard.bits()),
                selector: entry.selector.index() as u64,
                rule: rule_word(entry.rule),
                entry: cell.entry,
                level: cell.level,
                chain_start: cell.chain_start,
            };
            let entry_holds = jit.cell_entry_may_be(found.entry, expected.entry);
            let others_hold = GlobalRow {
                entry: expected.entry,
                ..found
            } == expected;
            if !others_hold || !entry_holds || words[cells::ROW_WORDS - 2..] != [0, 0] {
                return Err(format!(
                    "global row {index} reads {words:?}, its entry's is {expected:?}"
                ));
            }
        }
        Ok(())
    }

    fn state_of(&self, entry: &CacheEntry) -> CallSiteState {
        match &entry.target {
            Target::Vacant => CallSiteState::Vacant,
            Target::Megamorphic { .. } => CallSiteState::Megamorphic,
            Target::Polymorphic { block, .. } => CallSiteState::Polymorphic(
                self.pool
                    .get(*block as usize)
                    .into_iter()
                    .flatten()
                    .filter_map(|slot| cached_target(&slot.target))
                    .collect(),
            ),
            target => match cached_target(target) {
                Some(cached) => CallSiteState::Monomorphic(cached),
                None => CallSiteState::Vacant,
            },
        }
    }
}

/// The cell an entry with `guard` and `target` is to have, `found` the
/// one it has, or `None` for a site state, whose cell
/// `verify_site_state_cell` checks: `Cell::VACANT` for a vacant entry,
/// and for a filled one its guard -- the entry's own, or for a target
/// counted from N or S a word of the application whose id the entry's
/// guard is, as `application_of` finds -- with the entry address, level
/// and chain start its target decides. A global row's guard is the
/// entry's own whatever the rule, and its caller compares it so.
fn expected_cell(
    guard: DispatchId,
    target: &Target,
    found: Cell,
    application_of: &dyn Fn(u64) -> Option<DispatchId>,
    jit: &JitState,
) -> Result<Option<Cell>, String> {
    let chain = match target {
        Target::Vacant => return Ok(Some(Cell::VACANT)),
        Target::Polymorphic { .. } | Target::Megamorphic { .. } => return Ok(None),
        Target::Method { chain, .. }
        | Target::Slot { chain, .. }
        | Target::NestedClass { chain, .. }
        | Target::Primitive { chain, .. }
        | Target::Factory { chain, .. } => *chain,
    };
    let chain_start = chain_start_of(chain);
    let guard = match chain_start {
        ChainStart::ReceiverClass => u64::from(guard.bits()),
        ChainStart::LexicalClass | ChainStart::Superclass => {
            if application_of(found.guard) != Some(guard) {
                return Err(format!(
                    "the guard {} is no word of the application with id {}",
                    found.guard,
                    guard.bits()
                ));
            }
            found.guard
        }
    };
    Ok(Some(Cell {
        guard,
        entry: jit::expected_cell_entry(jit, target),
        level: cell_level(target),
        chain_start: chain_start_word(chain_start, target),
    }))
}

/// Whether `found` is a cell an entry whose cell is `expected` may hold:
/// the same, but for an entry address the JIT's state has moved on from
/// since -- the miss stub of a body since compiled or refused, which the
/// site's next miss rewrites (`decide_at`).
fn cell_may_be(found: Cell, expected: Cell, jit: &JitState) -> bool {
    Cell {
        entry: expected.entry,
        ..found
    } == expected
        && jit.cell_entry_may_be(found.entry, expected.entry)
}

/// Whether a polymorphic or megamorphic site's cell is the one its state
/// is to have (`site_state_cell`): no receiver matches its guard, its
/// entry and level are its state's, and it counts from N exactly when the
/// site is private -- the one thing its chain start must say, since
/// whether the site is a `super` one its instruction says.
fn verify_site_state_cell(entry: &CacheEntry, found: Cell, jit: &JitState) -> Result<(), String> {
    let private = entry.target.site_privacy() == Some(true);
    let lexical = found.chain_start == ChainStart::LexicalClass as u64;
    let expected = Cell {
        chain_start: found.chain_start,
        ..site_state_cell(jit, &entry.target, 0)
    };
    if found != expected || private != lexical {
        return Err(format!(
            "the site state's cell reads {found:?}, its state's is {expected:?} (private: {private})"
        ));
    }
    Ok(())
}

/// Where a target recorded under `rule` counts its levels from: the
/// `chain_start_of` its chain, which `fill` keeps consistent with the rule.
fn rule_chain_start(rule: CacheRule) -> ChainStart {
    match rule {
        CacheRule::Public | CacheRule::Lexical(_) => ChainStart::ReceiverClass,
        CacheRule::LexicalPrivate => ChainStart::LexicalClass,
        CacheRule::Super => ChainStart::Superclass,
    }
}

/// The selector a send instruction sends.
fn sent_selector(instruction: &Instruction) -> Option<Symbol> {
    match instruction {
        Instruction::SendPublic { selector, .. }
        | Instruction::SendInlinable { selector, .. }
        | Instruction::SendLexical { selector, .. }
        | Instruction::SendSuper { selector, .. } => Some(*selector),
        _ => None,
    }
}

/// `target` in the test view; `None` for a site state.
fn cached_target(target: &Target) -> Option<CachedTarget> {
    let (kind, chain) = match target {
        Target::Vacant | Target::Polymorphic { .. } | Target::Megamorphic { .. } => return None,
        Target::Factory { chain, .. } => (CachedTargetKind::Factory, *chain),
        Target::Method { method, chain, .. } => (CachedTargetKind::Method(method.selector), *chain),
        Target::Slot { field, chain, .. } => (CachedTargetKind::Slot { field: *field }, *chain),
        Target::NestedClass {
            mixin,
            field,
            chain,
            ..
        } => (
            CachedTargetKind::NestedClass {
                name: mixin.name,
                field: (*field != super::nested::NO_CACHED_FIELD).then_some(*field),
            },
            *chain,
        ),
        Target::Primitive { chain, .. } => (CachedTargetKind::Primitive, *chain),
    };
    let kind = match (kind, chain) {
        (CachedTargetKind::Factory, _) => CachedTargetKind::Factory,
        (_, Chain::DoesNotUnderstand | Chain::SuperDoesNotUnderstand) => {
            CachedTargetKind::DoesNotUnderstand
        }
        (kind, _) => kind,
    };
    Some(CachedTarget {
        kind,
        private: chain == Chain::Private,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bytecode::{CodeKind, CompiledCode, Instruction};
    use ns_syntax::Span;

    /// The cell a public site's entry for `target` takes with `guard`.
    pub(super) fn public_cell(guard: DispatchId, target: &Target, jit: &JitState) -> Cell {
        Cell {
            guard: u64::from(guard.bits()),
            entry: jit::cell_entry(jit, target),
            level: cell_level(target),
            chain_start: ChainStart::ReceiverClass as u64,
        }
    }

    impl InlineCaches {
        /// `advance` with the cell a public site's entry takes, for the
        /// tests of the caches themselves.
        fn advance_public(
            &mut self,
            index: usize,
            guard: DispatchId,
            target: Target,
            private: bool,
            jit: &JitState,
        ) {
            let cell = public_cell(guard, &target, jit);
            self.advance(index, guard, target, private, cell, jit);
        }
    }

    /// A global cache with its cells, for the tests of the global cache
    /// alone, whose entries are filed as a public site's.
    struct TestGlobal {
        global: GlobalCache,
        cells: Cells,
        jit: JitState,
    }

    impl TestGlobal {
        fn with_groups(groups: usize) -> TestGlobal {
            let mut cells = Cells::new();
            cells.vacate_global_rows(0..groups * 2);
            TestGlobal {
                global: GlobalCache::with_groups(groups),
                cells,
                jit: JitState::new(),
            }
        }

        fn get(&mut self, guard: DispatchId, selector: Symbol, rule: CacheRule) -> Option<&Target> {
            self.global.get(&mut self.cells, guard, selector, rule)
        }

        fn insert(&mut self, guard: DispatchId, selector: Symbol, rule: CacheRule, target: Target) {
            let cell = public_cell(guard, &target, &self.jit);
            self.global
                .insert(&mut self.cells, guard, selector, rule, target, cell);
        }

        fn clear(&mut self) {
            self.global.clear(&mut self.cells);
        }
    }

    /// A factory target on `chain`, for the tests of the caches themselves,
    /// which read only its chain.
    fn factory_target(chain: Chain) -> Target {
        Target::Factory {
            code: Rc::new(crate::compiler::factory_body(0)),
            chain,
        }
    }

    #[test]
    fn free_ranges_coalesce_and_fit_first() {
        let mut free = FreeRanges::default();
        free.free(10, 4);
        free.free(20, 3);
        free.free(14, 6); // joins 10..14 and 20..23 into 10..23
        assert_eq!(free.take_first_fit(5), Some(10));
        assert_eq!(free.take_first_fit(8), Some(15));
        assert_eq!(free.take_first_fit(1), None);
    }

    #[test]
    fn the_free_range_ending_at_the_table_is_taken_for_truncation() {
        let mut free = FreeRanges::default();
        free.free(4, 2);
        free.free(30, 10);
        assert_eq!(free.take_ending_at(40), Some((30, 10)));
        assert_eq!(free.take_ending_at(40), None);
        assert_eq!(free.take_ending_at(6), Some((4, 2)));
    }

    /// A slide packs the live ranges from 0: a base inside one moves with
    /// it, by its offset in the range, and every other base stays; machine
    /// code compiled against a base that moved, or that lies in no live
    /// range, is freed, and against `NOT_ASSIGNED` or 0 never.
    #[test]
    fn a_slide_packs_the_live_ranges_and_remaps_each_base_inside_one() {
        let slide = SiteSlide {
            ranges: vec![
                MovedRange {
                    old: 0,
                    size: 3,
                    new: 0,
                },
                MovedRange {
                    old: 10,
                    size: 4,
                    new: 3,
                },
                MovedRange {
                    old: 20,
                    size: 2,
                    new: 7,
                },
            ],
            end: 9,
        };
        assert_eq!(slide.remap(0), 0);
        assert_eq!(slide.remap(2), 2);
        assert_eq!(slide.remap(10), 3);
        assert_eq!(slide.remap(12), 5);
        assert_eq!(slide.remap(21), 8);
        assert_eq!(slide.remap(5), 5, "a free site's base stays");
        assert_eq!(slide.remap(30), 30, "a base past the table stays");
        let not_assigned = FeedbackBase::NOT_ASSIGNED;
        assert_eq!(slide.remap_base(not_assigned), not_assigned);
        assert!(!slide.moves(FeedbackBase(0)));
        assert!(!slide.moves(FeedbackBase(1)), "a range at 0 never moves");
        assert!(slide.moves(FeedbackBase(10)));
        assert!(slide.moves(FeedbackBase(5)), "a base in no live range");
        assert!(!slide.moves(not_assigned));
    }

    fn body(family_size: u32) -> Rc<CompiledCode> {
        let mut code = CompiledCode::new(
            CodeKind::Method,
            vec![Instruction::Return { register: 0 }],
            0,
            1,
            0,
            vec![],
            Span::new(0, 0),
        );
        // Only the cache table's arithmetic is under test, so the family
        // size is set rather than derived from sends.
        code.family_size = family_size;
        Rc::new(code)
    }

    /// A family gets one contiguous range the first time it is asked for,
    /// and the same one every time after; ranges never overlap.
    #[test]
    fn a_family_keeps_the_range_it_was_first_given() {
        let mut caches = InlineCaches::new();
        let (first, second) = (body(3), body(2));
        let first_base = caches.base_for(&first);
        let second_base = caches.base_for(&second);
        assert_eq!(caches.base_for(&first), first_base);
        assert_eq!(first_base.index(), 0);
        assert_eq!(second_base.index(), 3);
        assert_eq!(caches.table_len(), 5);
    }

    /// A family whose body only the table holds gives its range back unless
    /// a live closure or activation names its base; a range inside the
    /// table is taken by the next family, one at its end is cut off.
    #[test]
    fn a_family_nothing_runs_gives_its_range_back() {
        let mut caches = InlineCaches::new();
        let (named, unnamed, held, last) = (body(2), body(3), body(1), body(4));
        let named_base = caches.base_for(&named);
        let unnamed_base = caches.base_for(&unnamed);
        caches.base_for(&held);
        let last_base = caches.base_for(&last);
        assert_eq!(caches.table_len(), 10);
        drop((named, unnamed, last));
        let live_bases = BTreeSet::from([named_base.raw()]);
        caches.forget_families(&live_bases);
        // `last`'s range was the table's end; `unnamed`'s stays free inside.
        assert_eq!(last_base.index(), 6);
        assert_eq!(caches.table_len(), 6);
        assert_eq!(caches.base_for(&body(3)), unnamed_base);
        drop(held);
    }

    /// A body with no sends needs no range and never allocates one.
    #[test]
    fn a_family_without_sends_takes_no_room() {
        let mut caches = InlineCaches::new();
        let base = caches.base_for(&body(0));
        assert_ne!(base, FeedbackBase::NOT_ASSIGNED);
        assert_eq!(caches.table_len(), 0);
    }

    /// A decoded closure's range is fresh each time it is asked for: any
    /// range of the family's size is a correct one.
    #[test]
    fn a_fresh_range_is_appended() {
        let mut caches = InlineCaches::new();
        caches.base_for(&body(2));
        assert_eq!(caches.fresh_range(4).index(), 2);
        assert_eq!(caches.table_len(), 6);
    }

    /// A site index past its code's family, or from an unassigned base, is
    /// never used: the send runs uncached.
    #[test]
    fn a_site_outside_its_family_is_uncached() {
        assert_eq!(site_index(FeedbackBase(4), 3, CallSite { number: 3 }), None);
        assert_eq!(
            site_index(FeedbackBase::NOT_ASSIGNED, 3, CallSite { number: 0 }),
            None
        );
        assert_eq!(site_index(FeedbackBase(4), 3, CallSite::UNCACHED), None);
        assert_eq!(
            site_index(FeedbackBase(4), 3, CallSite { number: 2 }),
            Some(6)
        );
    }

    /// A group of one -- every key hashes into the same two slots -- keeps
    /// three distinct keys apart by rule, evicts the least recently used
    /// when a third arrives, and `clear` vacates every key at once.
    #[test]
    fn a_global_cache_keeps_colliding_keys_apart() {
        let mut global = TestGlobal::with_groups(1);
        let selector = Symbol::NEW;
        let guards: Vec<DispatchId> = (3..6).map(DispatchId::from_counter).collect();
        let chain_of = |global: &mut TestGlobal, guard: DispatchId, rule: CacheRule| match global
            .get(guard, selector, rule)
        {
            Some(Target::Factory { chain, .. }) => Some(*chain),
            _ => None,
        };
        let chains = [Chain::Public, Chain::Super, Chain::SuperDoesNotUnderstand];
        global.insert(
            guards[0],
            selector,
            CacheRule::Public,
            factory_target(chains[0]),
        );
        global.insert(
            guards[1],
            selector,
            CacheRule::Public,
            factory_target(chains[1]),
        );
        assert_eq!(
            chain_of(&mut global, guards[0], CacheRule::Public),
            Some(chains[0])
        );
        assert_eq!(
            chain_of(&mut global, guards[1], CacheRule::Public),
            Some(chains[1])
        );
        assert_eq!(
            chain_of(&mut global, guards[0], CacheRule::Super),
            None,
            "the rule is part of the key"
        );
        global.insert(
            guards[2],
            selector,
            CacheRule::Public,
            factory_target(chains[2]),
        );
        assert_eq!(
            chain_of(&mut global, guards[0], CacheRule::Public),
            None,
            "evicted"
        );
        assert_eq!(
            chain_of(&mut global, guards[1], CacheRule::Public),
            Some(chains[1])
        );
        assert_eq!(
            chain_of(&mut global, guards[2], CacheRule::Public),
            Some(chains[2])
        );
        global.clear();
        assert!(guards
            .iter()
            .all(|guard| global.get(*guard, selector, CacheRule::Public).is_none()));
    }

    /// Two entries that agree on guard and selector but carry a different
    /// `Lexical` payload -- N's application -- answer independently: a read
    /// under one N never sees what a different N left behind, which is what
    /// lets a vacant lexical site trust a hit without having resolved its
    /// own N yet (`docs/design/dispatch.md`, "3b -- The caches").
    #[test]
    fn a_global_cache_keys_a_lexical_entry_by_its_lexical_application_too() {
        let mut global = TestGlobal::with_groups(2);
        let selector = Symbol::NEW;
        let guard = DispatchId::from_counter(3);
        let (first_lexical, second_lexical) =
            (DispatchId::from_counter(4), DispatchId::from_counter(5));
        global.insert(
            guard,
            selector,
            CacheRule::Lexical(first_lexical),
            factory_target(Chain::Protected),
        );
        assert!(matches!(
            global.get(guard, selector, CacheRule::Lexical(first_lexical)),
            Some(Target::Factory {
                chain: Chain::Protected,
                ..
            })
        ));
        assert!(
            global
                .get(guard, selector, CacheRule::Lexical(second_lexical))
                .is_none(),
            "a different N never answers from another N's entry"
        );
    }

    /// Strongtalk's `lookupCache`, `lookup_probe`/`lookup`
    /// (`prior-art/strongtalk/vm/lookup/lookupCache.cpp:188-205,231-242`): a
    /// hit at the second slot promotes it to the first, so a colliding key a
    /// later send reads survives a third key's insert even though it was
    /// inserted first, while a key nothing reads again is the one that
    /// insert evicts -- the group keeps the more recently *used* of two
    /// colliding keys, not merely the more recently inserted (which is all
    /// `insert` alone, without a promoting `get`, would give: 3b's insert
    /// evicted the first probe whatever its use, and even a non-evicting
    /// insert-only policy would still evict by insertion order, never by
    /// use).
    #[test]
    fn a_group_keeps_the_more_recently_used_of_two_keys() {
        let mut cache = TestGlobal::with_groups(1);
        let selector = Symbol::NEW;
        let (first, second, third) = (
            DispatchId::from_counter(3),
            DispatchId::from_counter(4),
            DispatchId::from_counter(5),
        );
        let target_for = factory_target;
        cache.insert(
            first,
            selector,
            CacheRule::Public,
            target_for(Chain::Public),
        );
        cache.insert(
            second,
            selector,
            CacheRule::Public,
            target_for(Chain::Protected),
        );
        // `first` sits at the group's second (least recently used) slot
        // after `second`'s insert shifted it there; reading it here promotes
        // it back to the first slot.
        assert!(
            cache.get(first, selector, CacheRule::Public).is_some(),
            "the older key is still found"
        );
        cache.insert(third, selector, CacheRule::Public, target_for(Chain::Super));
        assert!(
            cache.get(second, selector, CacheRule::Public).is_none(),
            "the key nothing read again is the one a third key evicts"
        );
        assert!(
            cache.get(first, selector, CacheRule::Public).is_some(),
            "the key a read promoted survives the third key's insert"
        );
        assert!(cache.get(third, selector, CacheRule::Public).is_some());
    }

    /// `DispatchId::NONE` names no one application, so it is never
    /// recorded as verified nor answered as one.
    #[test]
    fn no_slot_level_verification_is_recorded_or_answered_for_no_guard() {
        let mut caches = InlineCaches::new();
        let level = ApplicationIndex::from_index(1).expect("a small index");
        caches.record_slot_level(DispatchId::NONE, level);
        assert!(!caches.slot_level_verified(DispatchId::NONE, level));
        caches.verified_slot_level = Some((DispatchId::NONE, level));
        assert!(!caches.slot_level_verified(DispatchId::NONE, level));
    }

    /// A slot level verification answers only for the guard and level it
    /// was recorded for, the last one recorded, and is gone after an
    /// invalidation.
    #[test]
    fn a_slot_level_verification_answers_its_own_pair_until_invalidated() {
        let mut caches = InlineCaches::new();
        let guard = |number: u32| DispatchId::from_counter(DispatchId::FIRST_MINTED + number);
        let level = |index: usize| ApplicationIndex::from_index(index).expect("a small index");
        assert!(!caches.slot_level_verified(guard(0), level(1)));
        caches.record_slot_level(guard(0), level(1));
        assert!(caches.slot_level_verified(guard(0), level(1)));
        assert!(!caches.slot_level_verified(guard(1), level(1)));
        assert!(!caches.slot_level_verified(guard(0), level(2)));
        caches.record_slot_level(guard(1), level(1));
        assert!(!caches.slot_level_verified(guard(0), level(1)));
        caches.invalidate();
        assert!(!caches.slot_level_verified(guard(1), level(1)));
    }

    /// A collection's scrub vacates exactly what names a freed id -- a
    /// monomorphic entry's guard, a slot target's level and the guard it
    /// recorded, a polymorphic block's matching slot, a global entry's
    /// guard and a lexical rule's N, the verified slot level -- and keeps
    /// every other entry warm.
    #[test]
    fn forgetting_freed_ids_vacates_exactly_the_entries_naming_them() {
        let mut caches = InlineCaches::new();
        let guard = |number: u32| DispatchId::from_counter(DispatchId::FIRST_MINTED + number);
        let level = |index: usize| ApplicationIndex::from_index(index).expect("a small index");
        let slot = |level, recorded_guard| Target::Slot {
            field: 0,
            kind: AccessorKind::Getter,
            chain: Chain::Public,
            level,
            recorded_guard,
        };
        let kept = level(9);
        let jit = JitState::new();
        let base = caches.fresh_range(5).index();
        caches.advance_public(base, guard(0), slot(kept, DispatchId::NONE), false, &jit);
        caches.advance_public(
            base + 1,
            guard(1),
            slot(level(1), DispatchId::NONE),
            false,
            &jit,
        );
        caches.advance_public(base + 2, guard(1), slot(kept, guard(2)), false, &jit);
        caches.advance_public(
            base + 3,
            guard(1),
            slot(kept, DispatchId::NONE),
            false,
            &jit,
        );
        caches.advance_public(
            base + 4,
            guard(0),
            slot(kept, DispatchId::NONE),
            false,
            &jit,
        );
        caches.advance_public(
            base + 4,
            guard(1),
            slot(kept, DispatchId::NONE),
            false,
            &jit,
        );
        let selector = Symbol::intern("value");
        let lexical = CacheRule::Lexical(guard(0));
        for (key, rule) in [
            (guard(0), CacheRule::Public),
            (guard(1), lexical),
            (guard(1), CacheRule::Public),
        ] {
            let target = slot(kept, DispatchId::NONE);
            let cell = public_cell(key, &target, &jit);
            caches.global_insert(key, selector, rule, target, cell);
        }
        caches.record_slot_level(guard(1), level(1));
        let mut freed = FreedIds::default();
        freed.guards.extend([guard(0), guard(2)]);
        freed.applications.insert(level(1));
        caches.forget_freed(&freed);
        for index in base..base + 3 {
            assert!(matches!(caches.sites[index].target, Target::Vacant));
        }
        assert!(!matches!(caches.sites[base + 3].target, Target::Vacant));
        assert!(matches!(
            caches.sites[base + 4].target,
            Target::Polymorphic { .. }
        ));
        assert!(caches.polymorphic_hit(base + 4, guard(0)).is_none());
        assert!(caches.polymorphic_hit(base + 4, guard(1)).is_some());
        assert!(caches
            .global_get(guard(0), selector, CacheRule::Public)
            .is_none());
        assert!(caches.global_get(guard(1), selector, lexical).is_none());
        assert!(caches
            .global_get(guard(1), selector, CacheRule::Public)
            .is_some());
        assert!(!caches.slot_level_verified(guard(1), level(1)));
        assert_eq!(caches.verify_cells_against(&|_| None, &jit), Ok(()));
    }

    /// A site moves vacant, monomorphic, polymorphic in the order first
    /// seen, megamorphic on a fifth guard, and gives its block back.
    #[test]
    fn a_site_moves_through_its_states() {
        let mut caches = InlineCaches::new();
        let base = caches.fresh_range(1);
        let index = base.index();
        let slot = |field| Target::Slot {
            field,
            kind: AccessorKind::Getter,
            chain: Chain::Public,
            level: ApplicationIndex::UNREGISTERED,
            recorded_guard: DispatchId::NONE,
        };
        let guard = |number: u32| DispatchId::from_counter(DispatchId::FIRST_MINTED + number);
        let jit = JitState::new();
        caches.advance_public(index, guard(0), slot(0), false, &jit);
        assert_eq!(
            caches.state_of(&caches.sites[index]),
            CallSiteState::Monomorphic(CachedTarget {
                kind: CachedTargetKind::Slot { field: 0 },
                private: false
            })
        );
        for number in 1..4 {
            caches.advance_public(index, guard(number), slot(number), false, &jit);
        }
        let fields = match caches.state_of(&caches.sites[index]) {
            CallSiteState::Polymorphic(targets) => targets
                .into_iter()
                .map(|target| target.kind)
                .collect::<Vec<_>>(),
            _ => panic!("four guards make a polymorphic site"),
        };
        assert_eq!(
            fields,
            (0..4)
                .map(|field| CachedTargetKind::Slot { field })
                .collect::<Vec<_>>()
        );
        assert!(matches!(
            caches.polymorphic_hit(index, guard(2)),
            Some(Target::Slot { field: 2, .. })
        ));
        assert_eq!(caches.verify_cells_against(&|_| None, &jit), Ok(()));
        caches.advance_public(index, guard(4), slot(4), false, &jit);
        assert_eq!(
            caches.state_of(&caches.sites[index]),
            CallSiteState::Megamorphic
        );
        assert_eq!(caches.free_blocks, vec![0]);
        assert!(caches.polymorphic_hit(index, guard(2)).is_none());
        // A private target never enters a site decided non-private.
        caches.advance_public(index, guard(5), slot(5), true, &jit);
        assert_eq!(
            caches.state_of(&caches.sites[index]),
            CallSiteState::Megamorphic
        );
        // Out of range is a no-op, not a panic.
        caches.advance_public(usize::MAX, guard(0), slot(0), false, &jit);
        assert_eq!(caches.verify_cells_against(&|_| None, &jit), Ok(()));
    }

    /// An entry holds no handle and is the size the pool's blocks are laid
    /// out for.
    #[cfg(all(target_pointer_width = "64", not(debug_assertions)))]
    #[test]
    fn an_entry_stays_the_size_the_caches_were_measured_at() {
        assert_eq!(std::mem::size_of::<Target>(), 16);
        assert_eq!(std::mem::size_of::<CacheEntry>(), 24);
        // Pins the global cache's own entry: `CacheRule`'s `Lexical` payload
        // is a `DispatchId`, which padding happens to absorb today -- add a
        // second payload to any rule and this catches it silently doubling
        // the cache's footprint instead of a passing test hiding it.
        assert_eq!(std::mem::size_of::<GlobalEntry>(), 32);
    }

    /// A factory entry records the body the lookup found, and a hit hands
    /// that very body to the factory's activation: the guard -- the
    /// metaclass's instances id -- fixes the entry, so the hit reads nothing
    /// of the method table again.
    #[test]
    fn a_factory_hit_pushes_the_body_the_fill_recorded() {
        let source = "class Plain = Object new () () : ()\n";
        let module = ns_syntax::parse(source).expect("parse");
        let resolved = crate::resolve(
            module,
            Rc::from(source),
            crate::types::SourceOrigin::File(std::path::PathBuf::from("t.ns")),
        )
        .expect("resolve");
        let mut interpreter = crate::Interpreter::bootstrap();
        let class = interpreter.load_module(&resolved);
        interpreter.enter(|mutator| {
            let receiver = mutator.fetch(&class);
            let found = mutator.resolve_public(receiver, Symbol::NEW);
            let Resolved::Factory {
                code: found_code, ..
            } = &found
            else {
                panic!("`new` resolves to the factory entry");
            };
            let found_code = found_code.clone();
            let key = SendKey {
                guard: mutator.heap.dispatch_id(receiver),
                rule: CacheRule::Public,
                receiver: &receiver,
                lexical_class: None,
                superclass: None,
                current_class: None,
            };
            let target = record(mutator, &found, key).expect("a factory is recorded");
            let Target::Factory { code: recorded, .. } = &target else {
                panic!("a factory is recorded as one");
            };
            assert!(Rc::ptr_eq(recorded, &found_code));
            let (hit, _) = materialize(mutator, target, key).expect("a hit");
            match mutator.act(hit, receiver, Symbol::NEW, Arguments::NONE) {
                Ok(crate::interpreter::Step::Factory { code, .. }) => {
                    assert!(
                        Rc::ptr_eq(&code, &found_code),
                        "the hit pushes the recorded body"
                    );
                }
                _ => panic!("a factory hit decides the factory's activation"),
            }
        });
    }

    fn expect_ok(result: crate::HostResult, what: &str) -> crate::Root {
        match result {
            Ok(root) => root,
            Err(crate::HostSignal::Return) => panic!("{what}: a non-local return escaped"),
            Err(crate::HostSignal::Error(error)) => {
                panic!("{what}: unexpected Newspeak error: {}", error.message)
            }
        }
    }

    /// The one site `narrow Inner` fills for `Narrow`, whose hidden slot is
    /// field 0; a forward then replaces the enclosing object with a `Wide`,
    /// whose hidden slot is field 3. The next sends miss `Narrow`'s guard
    /// and answer the `Wide`'s own class -- never what field 0 of the `Wide`
    /// holds -- and the entry they leave records field 3.
    #[test]
    fn a_nested_class_hit_after_the_enclosing_object_was_reshaped_takes_the_slow_path() {
        let source = indoc::indoc! {"
            class Subject = Object new () (
              class Narrow = Object new () (
                public class Inner = Object new () ()
              )
              class Wide = Object new (
                | first ::= 1. second ::= 2. third ::= 3. |
              ) (
                public class Inner = Object new () ()
              )
              public run: mirror = (
                | narrow wide classes |
                narrow:: Narrow new.
                wide:: Wide new.
                classes:: Array new: 3.
                1 to: 3 do: [:index |
                  classes at: index put: narrow Inner.
                  index = 1 ifTrue: [
                    mirror elementsOf: {narrow} forwardIdentityToElementsOf: {wide} ifFail: [^4].
                  ].
                ].
                (classes at: 1) == (classes at: 2) ifTrue: [^1].
                (classes at: 2) == (classes at: 3) ifFalse: [^2].
                (classes at: 3) == wide Inner ifFalse: [^3].
                ^0
              )
            ) : ()
        "};
        let module = ns_syntax::parse(source).expect("parse");
        let resolved = crate::resolve(
            module,
            Rc::from(source),
            crate::types::SourceOrigin::File(std::path::PathBuf::from("t.ns")),
        )
        .expect("resolve");
        let mut interpreter = crate::Interpreter::bootstrap();
        #[cfg(debug_assertions)]
        interpreter.set_stress_caches(false);
        let class = interpreter.load_module(&resolved);
        let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        let mirror = interpreter.enter(|mutator| {
            let mirror = mutator.vm_mirror();
            mutator.stash(mirror)
        });
        let answer = expect_ok(
            interpreter.send(&instance, Symbol::intern("run:"), &[mirror]),
            "run:",
        );
        let answer = interpreter.enter(|mutator| mutator.fetch(&answer).as_small_integer());
        assert_eq!(answer, Some(0));
        assert_eq!(
            interpreter.call_site_states(
                &class,
                MethodSide::Instance,
                Symbol::intern("run:"),
                Symbol::intern("Inner"),
            ),
            vec![
                CallSiteState::Monomorphic(CachedTarget {
                    kind: CachedTargetKind::NestedClass {
                        name: Symbol::intern("Inner"),
                        field: Some(3),
                    },
                    private: false,
                }),
                CallSiteState::Monomorphic(CachedTarget {
                    kind: CachedTargetKind::NestedClass {
                        name: Symbol::intern("Inner"),
                        field: Some(3),
                    },
                    private: false,
                }),
            ]
        );
    }
}
