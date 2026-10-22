//! Inline caches (`docs/design/dispatch.md`, "3b -- The caches"): the table of
//! every call site's entry, per heap, reached from an activation's
//! `feedback_base` -- V8's split of shared code and per-context feedback,
//! reached the way HotSpot's interpreter reaches `MethodData` -- with a
//! polymorphic block pool and a global cache behind every site.
//!
//! A cache records what `resolve` decided and never decides: a hit is `act`
//! on a `Resolved` rebuilt from an entry, so it can do nothing a miss would
//! not (`resolve.rs`).

use std::collections::BTreeSet;
use std::hash::BuildHasher;
use std::rc::Rc;

use crate::builtins::primitive::Primitive;
use crate::bytecode::{CallSite, CompiledCode, Instruction};
use crate::dispatch;
use crate::hash::IdentityMap;
use crate::heap::reclaim::FreedIds;
use crate::heap::{ActivationReference, ApplicationIndex, Heap};
use crate::mixin::{AccessorKind, DispatchId, Mixin, MixinMethod};
use crate::types::*;
use crate::Symbol;

use super::resolve::{Chain, Resolved};
use super::sends::{decide_lexical_send, decide_super_send};
use super::{Mutator, Signal, Step};

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
    ranges: std::collections::BTreeMap<u32, u32>,
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
        Some(start)
    }

    /// Makes `start..start + size` free, merged with its neighbours, so that
    /// two adjacent freed families serve one larger one.
    fn free(&mut self, start: u32, size: u32) {
        // An empty range would sit at another one's start or end and break
        // the rule that no two are adjacent.
        debug_assert!(size > 0, "internal: an empty range is never freed");
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
        InlineCaches {
            sites: Vec::new(),
            pool: Vec::new(),
            free_blocks: Vec::new(),
            free_ranges: FreeRanges::default(),
            global: GlobalCache::with_groups(GLOBAL_CACHE_ENTRIES / 2),
            verified_slot_level: None,
            families: IdentityMap::default(),
        }
    }

    /// `code`'s range, assigned the first time its family's root is
    /// activated, so that a family that never runs takes no room -- as V8
    /// allocates a feedback vector lazily. A body with no sends is given
    /// base 0 and no room: no site of it is ever indexed.
    pub(crate) fn base_for(&mut self, code: &Rc<CompiledCode>) -> FeedbackBase {
        if code.family_size == 0 {
            return FeedbackBase(0);
        }
        let key = Rc::as_ptr(code) as usize;
        if let Some(family) = self.families.get(&key) {
            return family.base;
        }
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

    /// Drops every family whose root body only the family holds and whose
    /// range no live closure or activation names (`live_bases`), vacates
    /// its sites and frees its range for the next family, then gives back
    /// the table's free tail. A body nothing else holds can never be
    /// activated again, so no new activation asks for the range; a closure
    /// or activation of the family still names it by base, which is what
    /// `live_bases` keeps -- a range freed under one would be filled by
    /// another family's sends, and its hits would act on them. Run by a
    /// full collection (`Mutator::reclaim_ids`).
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
        let InlineCaches {
            sites,
            pool,
            free_blocks,
            free_ranges,
            ..
        } = self;
        for (base, size) in freed {
            // `base_for` records no family without sends, and an empty range
            // inside another free one would break the map's coalescing. A
            // family the table had no room for holds no range to free.
            if size == 0 || base == FeedbackBase::NOT_ASSIGNED {
                continue;
            }
            for entry in &mut sites[base.index()..][..size as usize] {
                if let Target::Polymorphic { block, .. } = entry.target {
                    if let Some(slots) = pool.get_mut(block as usize) {
                        *slots = CacheEntry::vacant_block();
                        free_blocks.push(block);
                    }
                }
                *entry = CacheEntry::vacant();
            }
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
    /// far, which leaves every site of the family uncached.
    pub(crate) fn fresh_range(&mut self, size: u32) -> FeedbackBase {
        if let Some(start) = self.free_ranges.take_first_fit(size) {
            return FeedbackBase(start);
        }
        let start = self.sites.len();
        match start
            .checked_add(size as usize)
            .and_then(|end| u32::try_from(end).ok())
        {
            Some(end) if end < FeedbackBase::NOT_ASSIGNED.0 => {
                self.sites.resize(end as usize, CacheEntry::vacant());
                FeedbackBase(start as u32)
            }
            _ => FeedbackBase::NOT_ASSIGNED,
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

    /// Moves site `index` on after a miss found `target` for `guard`
    /// (`docs/design/dispatch.md`, "A site's states"): vacant to
    /// monomorphic, monomorphic to a polymorphic block of four in the order
    /// first seen, a full block to megamorphic, which stays so -- Strongtalk's
    /// and V8's transitions. A site whose §5.9 decision differs from
    /// `private` is left alone: only a crafted image makes one.
    fn advance(&mut self, index: usize, guard: DispatchId, target: Target, private: bool) {
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
            Target::Vacant => *entry = CacheEntry { guard, target },
            Target::Megamorphic { .. } => {}
            Target::Polymorphic { block, .. } => {
                let Some(slots) = self.pool.get_mut(block as usize) else {
                    return;
                };
                // A guard already in the block is one whose entry could not
                // be materialized for this receiver; the fresh answer
                // replaces it rather than taking a second slot.
                if let Some(slot) = slots
                    .iter_mut()
                    .find(|slot| slot.guard == guard || matches!(slot.target, Target::Vacant))
                {
                    *slot = CacheEntry { guard, target };
                    return;
                }
                *slots = CacheEntry::vacant_block();
                self.free_blocks.push(block);
                *entry = CacheEntry {
                    guard: DispatchId::VACANT,
                    target: Target::Megamorphic { private },
                };
            }
            _ if entry.guard == guard => *entry = CacheEntry { guard, target },
            _ => {
                let block = match self.free_blocks.pop() {
                    Some(block) => block,
                    None => match u32::try_from(self.pool.len()) {
                        Ok(block) => {
                            self.pool.push(CacheEntry::vacant_block());
                            block
                        }
                        Err(_) => return,
                    },
                };
                let first = std::mem::replace(
                    entry,
                    CacheEntry {
                        guard: DispatchId::VACANT,
                        target: Target::Polymorphic { block, private },
                    },
                );
                let slots = &mut self.pool[block as usize];
                slots[0] = first;
                slots[1] = CacheEntry { guard, target };
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
        self.pool.clear();
        self.free_blocks.clear();
        self.global.clear();
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
        for entry in &mut self.sites {
            match entry.target {
                Target::Vacant | Target::Megamorphic { .. } => {}
                Target::Polymorphic { block, .. } => {
                    if let Some(slots) = self.pool.get_mut(block as usize) {
                        for slot in slots.iter_mut().filter(|slot| names_freed(slot)) {
                            *slot = CacheEntry::vacant();
                        }
                    }
                }
                _ => {
                    if names_freed(entry) {
                        *entry = CacheEntry::vacant();
                    }
                }
            }
        }
        self.global.retain(|entry| {
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
        self.global = GlobalCache::with_groups(entries / 2);
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
    /// The first of the key's group's two adjacent slots.
    fn group(&self, guard: DispatchId, selector: Symbol, rule: CacheRule) -> usize {
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
    fn get(&mut self, guard: DispatchId, selector: Symbol, rule: CacheRule) -> Option<&Target> {
        let first = self.group(guard, selector, rule);
        if self.entries[first].holds(guard, selector, rule) {
            return Some(&self.entries[first].target);
        }
        if self.entries[first + 1].holds(guard, selector, rule) {
            self.entries.swap(first, first + 1);
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
    fn insert(&mut self, guard: DispatchId, selector: Symbol, rule: CacheRule, target: Target) {
        let first = self.group(guard, selector, rule);
        let second = first + 1;
        let entry = GlobalEntry {
            guard,
            selector,
            rule,
            target,
        };
        if self.entries[first].holds(guard, selector, rule) {
            self.entries[first] = entry;
        } else {
            self.entries[second] = std::mem::replace(&mut self.entries[first], entry);
        }
    }

    /// Vacates every entry: `InlineCaches::invalidate`'s global half.
    fn clear(&mut self) {
        for entry in &mut self.entries {
            *entry = GlobalEntry::vacant();
        }
    }

    /// Vacates every entry `keep` refuses: `InlineCaches::forget_freed`'s
    /// global half. A group whose first slot is vacated keeps its second,
    /// which `get` still finds there.
    fn retain(&mut self, keep: impl Fn(&GlobalEntry) -> bool) {
        for entry in &mut self.entries {
            if !entry.is_vacant() && !keep(entry) {
                *entry = GlobalEntry::vacant();
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
            feedback_base: mutator.caches.base_for(&method.code),
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
            level,
            chain,
        } => {
            let start = chain_start(mutator, chain, key)?;
            let declaring_class = level_above(&mutator.heap, start, level)?;
            (
                Resolved::NestedClass {
                    mixin,
                    declaring_class,
                    chain,
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
    arguments: &[Value<'gc>],
) -> Result<Step<'gc>, Signal<'gc>> {
    let Some(index) = site_index(feedback_base, family_size, site) else {
        return mutator.decide_send(receiver, selector, arguments);
    };
    let key = SendKey {
        guard: mutator.heap.dispatch_id(receiver),
        rule: CacheRule::Public,
        receiver: &receiver,
        lexical_class: None,
        superclass: None,
    };
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

/// A lexical send (§5.7-§5.9), cached. The site's privacy, decided when it
/// was first filled, chooses the guard: N's application for a private site,
/// `o`'s otherwise, so a non-private *hit* still never computes N -- `miss`
/// only calls its `lexical_of` closure on an actual miss, once N is needed
/// for the `Lexical` rule's payload (`docs/design/dispatch.md`, "3b -- The
/// caches").
#[allow(clippy::too_many_arguments)]
#[inline(always)]
pub(crate) fn send_lexical<'gc>(
    mutator: &mut Mutator<'gc>,
    activation: ActivationReference,
    feedback_base: FeedbackBase,
    family_size: u32,
    site: CallSite,
    depth: u16,
    selector: Symbol,
    arguments: &[Value<'gc>],
) -> Result<Step<'gc>, Signal<'gc>> {
    let privacy = site_index(feedback_base, family_size, site).and_then(|index| {
        mutator
            .caches
            .sites
            .get(index)
            .map(|entry| (index, entry.target.site_privacy()))
    });
    let Some((index, privacy)) = privacy else {
        return decide_lexical_send(mutator, activation, depth, selector, arguments);
    };
    // As in `decide_lexical_send`: the depth counts out from the class that
    // defines the running method, not from the receiver's class.
    let (self_value, defining_class) = mutator
        .heap
        .activation_receiver_and_current_class(activation);
    let enclosing =
        dispatch::kth_enclosing_object(&mutator.heap, self_value, defining_class.as_ref(), depth);
    let Some(private) = privacy else {
        return send_lexical_first(
            mutator,
            index,
            enclosing,
            defining_class,
            depth,
            selector,
            arguments,
        );
    };
    let key = if private {
        let lexical_class = dispatch::kth_lexical_class(&mutator.heap, defining_class, depth);
        SendKey {
            guard: lexical_dispatch_id(&mutator.heap, lexical_class),
            rule: CacheRule::LexicalPrivate,
            receiver: &enclosing,
            lexical_class,
            superclass: None,
        }
    } else {
        SendKey {
            guard: mutator.heap.dispatch_id(enclosing),
            // A placeholder payload: `miss` fills it in only on a genuine
            // miss, so a monomorphic hit still never computes N.
            rule: CacheRule::Lexical(DispatchId::NONE),
            receiver: &enclosing,
            lexical_class: None,
            superclass: None,
        }
    };
    cached(
        mutator,
        index,
        key,
        selector,
        arguments,
        |mutator| dispatch::kth_lexical_class(&mutator.heap, defining_class, depth),
        |mutator, known_lexical_class| {
            let lexical_class = match known_lexical_class {
                LexicalClass::Known(lexical_class) => lexical_class,
                LexicalClass::NotComputed => {
                    dispatch::kth_lexical_class(&mutator.heap, defining_class, depth)
                }
            };
            mutator.resolve_lexical(enclosing, lexical_class, selector)
        },
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

/// A vacant lexical site's first send. Whether §5.9's private rule applies
/// is not known until N is: §5.9's private rule depends only on N and the
/// selector, never on `o`, so once N's application is the `Lexical` rule's
/// payload, a match there is right regardless of which site left it -- and
/// the two rows this checks, `LexicalPrivate` keyed by N alone and
/// `Lexical` keyed by (`o`, N), are the only ones N's decision could have
/// filled. Only a genuine miss on both falls to resolving here, exactly as
/// before the entries existed.
#[inline(never)]
fn send_lexical_first<'gc>(
    mutator: &mut Mutator<'gc>,
    index: usize,
    enclosing: Value<'gc>,
    defining_class: Option<ClassReference<'gc>>,
    depth: u16,
    selector: Symbol,
    arguments: &[Value<'gc>],
) -> Result<Step<'gc>, Signal<'gc>> {
    let lexical_class = dispatch::kth_lexical_class(&mutator.heap, defining_class, depth);
    let lexical_id = lexical_dispatch_id(&mutator.heap, lexical_class);
    if lexical_id != DispatchId::NONE {
        let private_key = SendKey {
            guard: lexical_id,
            rule: CacheRule::LexicalPrivate,
            receiver: &enclosing,
            lexical_class,
            superclass: None,
        };
        if let Some(step) = global_hit(mutator, index, private_key, selector, arguments) {
            return step;
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
        };
        if let Some(step) = global_hit(mutator, index, public_key, selector, arguments) {
            return step;
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
    };
    fill(mutator, index, key, selector, &resolved);
    mutator.act(resolved, enclosing, selector, arguments)
}

/// `key`'s global-cache row, materialized and acted on, if it already holds
/// an answer -- advancing the still-vacant site `index` to it. `None` on a
/// miss, which leaves the site untouched for the caller to resolve instead.
fn global_hit<'gc>(
    mutator: &mut Mutator<'gc>,
    index: usize,
    key: SendKey<'_, 'gc>,
    selector: Symbol,
    arguments: &[Value<'gc>],
) -> Option<Result<Step<'gc>, Signal<'gc>>> {
    let target = mutator
        .caches
        .global
        .get(key.guard, selector, key.rule)
        .cloned()?;
    let (resolved, base) = materialize(mutator, target.clone(), key)?;
    mutator
        .caches
        .advance(index, key.guard, target, key.private());
    Some(mutator.act_with_base(resolved, base, *key.receiver, selector, arguments))
}

/// A `super` send (§5.10), cached, keyed by the running method's
/// application: S, and so everything §5.10 finds from it, is a function of
/// that application. Its two errors are raised by the uncached path, which
/// is where a send without a `current_class` or a superclass goes.
#[inline(always)]
pub(crate) fn send_super<'gc>(
    mutator: &mut Mutator<'gc>,
    activation: ActivationReference,
    feedback_base: FeedbackBase,
    family_size: u32,
    site: CallSite,
    selector: Symbol,
    arguments: &[Value<'gc>],
) -> Result<Step<'gc>, Signal<'gc>> {
    // The classes are read only for a cached site; the uncached path reads
    // them itself.
    let cached_site = site_index(feedback_base, family_size, site).and_then(|index| {
        let current_class = mutator.heap.activation_current_class(activation)?;
        let superclass = mutator.heap.class_superclass(current_class)?;
        Some((index, current_class, superclass))
    });
    let Some((index, current_class, superclass)) = cached_site else {
        return decide_super_send(mutator, activation, selector, arguments);
    };
    let self_value = mutator.heap.activation_receiver(activation);
    let key = SendKey {
        guard: mutator.heap.class_dispatch_ids(current_class).instances,
        rule: CacheRule::Super,
        receiver: &self_value,
        lexical_class: None,
        superclass: Some(superclass),
    };
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

/// A monomorphic hit, or everything else out of line. `lexical_of` answers
/// N, for the `Lexical` rule's payload; it is never called here, only handed
/// to `miss`, so a hit costs nothing for it.
#[inline(always)]
fn cached<'gc>(
    mutator: &mut Mutator<'gc>,
    index: usize,
    key: SendKey<'_, 'gc>,
    selector: Symbol,
    arguments: &[Value<'gc>],
    lexical_of: impl FnOnce(&Mutator<'gc>) -> Option<ClassReference<'gc>>,
    resolve: impl FnOnce(&mut Mutator<'gc>, LexicalClass<'gc>) -> Resolved<'gc>,
) -> Result<Step<'gc>, Signal<'gc>> {
    if let Some(entry) = mutator.caches.sites.get(index) {
        if entry.guard == key.guard {
            let target = entry.target.clone();
            if let Some((resolved, base)) = materialize(mutator, target, key) {
                return mutator.act_with_base(resolved, base, *key.receiver, selector, arguments);
            }
        }
    }
    miss(
        mutator, index, key, selector, arguments, lexical_of, resolve,
    )
}

/// The site's polymorphic block, then the global cache, then `resolve`,
/// whose answer is recorded in both before it is acted on. Out of line so
/// that a monomorphic hit keeps its registers; not `#[cold]`, since a
/// polymorphic hit comes through here. `lexical_of` is called at most once,
/// only once a miss is certain and the `Lexical` rule's payload is actually
/// needed -- and not at all for a `Public` or `Super` key, whose rule has no
/// payload to fill. `resolve` is handed N where it is already known -- a
/// private key's, or the payload's -- so that a lexical miss computes it
/// once.
#[inline(never)]
fn miss<'gc>(
    mutator: &mut Mutator<'gc>,
    index: usize,
    mut key: SendKey<'_, 'gc>,
    selector: Symbol,
    arguments: &[Value<'gc>],
    lexical_of: impl FnOnce(&Mutator<'gc>) -> Option<ClassReference<'gc>>,
    resolve: impl FnOnce(&mut Mutator<'gc>, LexicalClass<'gc>) -> Resolved<'gc>,
) -> Result<Step<'gc>, Signal<'gc>> {
    let mut known_lexical_class = if key.private() {
        LexicalClass::Known(key.lexical_class)
    } else {
        LexicalClass::NotComputed
    };
    if key.guard != DispatchId::NONE {
        if let Some(target) = mutator.caches.polymorphic_hit(index, key.guard) {
            if let Some((resolved, base)) = materialize(mutator, target, key) {
                return mutator.act_with_base(resolved, base, *key.receiver, selector, arguments);
            }
        }
        if let CacheRule::Lexical(ref mut lexical) = key.rule {
            let lexical_class = lexical_of(mutator);
            *lexical = lexical_dispatch_id(&mutator.heap, lexical_class);
            known_lexical_class = LexicalClass::Known(lexical_class);
        }
        let global = mutator
            .caches
            .global
            .get(key.guard, selector, key.rule)
            .cloned();
        if let Some(target) = global {
            if let Some((resolved, base)) = materialize(mutator, target.clone(), key) {
                mutator
                    .caches
                    .advance(index, key.guard, target, key.private());
                return mutator.act_with_base(resolved, base, *key.receiver, selector, arguments);
            }
        }
    }
    let resolved = resolve(mutator, known_lexical_class);
    fill(mutator, index, key, selector, &resolved);
    mutator.check_super_handler_slot(&resolved, *key.receiver)?;
    mutator.act(resolved, *key.receiver, selector, arguments)
}

/// Records `resolved` in the global cache and at the site -- unless the key
/// has no id, or the resolution's privacy is not the site's (only a crafted
/// image makes a site's N change; refinement 1). Before `act`, which may run
/// code. `key.rule`'s `Lexical` payload, if any, must already be N's
/// application -- `miss` fills it in before falling through here;
/// `send_lexical_first` builds it directly, since it never goes through
/// `miss`.
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
        mutator
            .caches
            .global
            .insert(key.guard, selector, key.rule, target.clone());
        mutator
            .caches
            .advance(index, key.guard, target, key.private());
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
    /// The nested class's name.
    NestedClass(Symbol),
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

impl InlineCaches {
    /// The states of the sites in `code`'s family that send `sent`, in
    /// emission order, or all vacant when the family has not run. For tests
    /// (`Interpreter::call_site_states`).
    pub(crate) fn states_of(&self, code: &Rc<CompiledCode>, sent: Symbol) -> Vec<CallSiteState> {
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
            .map(|number| {
                match site_index(base, code.family_size, CallSite { number })
                    .and_then(|index| self.sites.get(index))
                {
                    None => CallSiteState::Vacant,
                    Some(entry) => self.state_of(entry),
                }
            })
            .collect()
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
        Target::NestedClass { mixin, chain, .. } => {
            (CachedTargetKind::NestedClass(mixin.name), *chain)
        }
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
        let mut global = GlobalCache::with_groups(1);
        let selector = Symbol::NEW;
        let guards: Vec<DispatchId> = (3..6).map(DispatchId::from_counter).collect();
        let chain_of = |global: &mut GlobalCache, guard: DispatchId, rule: CacheRule| match global
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
        let mut global = GlobalCache::with_groups(2);
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
        let mut cache = GlobalCache::with_groups(1);
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
        let base = caches.fresh_range(5).index();
        caches.advance(base, guard(0), slot(kept, DispatchId::NONE), false);
        caches.advance(base + 1, guard(1), slot(level(1), DispatchId::NONE), false);
        caches.advance(base + 2, guard(1), slot(kept, guard(2)), false);
        caches.advance(base + 3, guard(1), slot(kept, DispatchId::NONE), false);
        caches.advance(base + 4, guard(0), slot(kept, DispatchId::NONE), false);
        caches.advance(base + 4, guard(1), slot(kept, DispatchId::NONE), false);
        let selector = Symbol::intern("value");
        let lexical = CacheRule::Lexical(guard(0));
        for (key, rule) in [
            (guard(0), CacheRule::Public),
            (guard(1), lexical),
            (guard(1), CacheRule::Public),
        ] {
            caches
                .global
                .insert(key, selector, rule, slot(kept, DispatchId::NONE));
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
            .global
            .get(guard(0), selector, CacheRule::Public)
            .is_none());
        assert!(caches.global.get(guard(1), selector, lexical).is_none());
        assert!(caches
            .global
            .get(guard(1), selector, CacheRule::Public)
            .is_some());
        assert!(!caches.slot_level_verified(guard(1), level(1)));
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
        caches.advance(index, guard(0), slot(0), false);
        assert_eq!(
            caches.state_of(&caches.sites[index]),
            CallSiteState::Monomorphic(CachedTarget {
                kind: CachedTargetKind::Slot { field: 0 },
                private: false
            })
        );
        for number in 1..4 {
            caches.advance(index, guard(number), slot(number), false);
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
        caches.advance(index, guard(4), slot(4), false);
        assert_eq!(
            caches.state_of(&caches.sites[index]),
            CallSiteState::Megamorphic
        );
        assert_eq!(caches.free_blocks, vec![0]);
        assert!(caches.polymorphic_hit(index, guard(2)).is_none());
        // A private target never enters a site decided non-private.
        caches.advance(index, guard(5), slot(5), true);
        assert_eq!(
            caches.state_of(&caches.sites[index]),
            CallSiteState::Megamorphic
        );
        // Out of range is a no-op, not a panic.
        caches.advance(usize::MAX, guard(0), slot(0), false);
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
            };
            let target = record(mutator, &found, key).expect("a factory is recorded");
            let Target::Factory { code: recorded, .. } = &target else {
                panic!("a factory is recorded as one");
            };
            assert!(Rc::ptr_eq(recorded, &found_code));
            let (hit, _) = materialize(mutator, target, key).expect("a hit");
            match mutator.act(hit, receiver, Symbol::NEW, &[]) {
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
}
