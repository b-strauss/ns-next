//! Inline caches (`docs/design/dispatch.md`, "3b -- The caches"): the table of
//! every call site's entry, per heap, reached from an activation's
//! `feedback_base` -- V8's split of shared code and per-context feedback,
//! reached the way HotSpot's interpreter reaches `MethodData` -- with a
//! polymorphic block pool and a global cache behind every site.
//!
//! A cache records what `resolve` decided and never decides: a hit is `act`
//! on a `Resolved` rebuilt from an entry, so it can do nothing a miss would
//! not (`resolve.rs`).

use std::hash::BuildHasher;
use std::rc::Rc;

use crate::builtins::primitive::Primitive;
use crate::bytecode::{CallSite, CompiledCode, Instruction};
use crate::dispatch;
use crate::hash::IdentityMap;
use crate::heap::{ActivationReference, Heap};
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
    /// target: §6.3 instantiates the *receiver*, which the guard already
    /// names.
    Factory {
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
            | Target::Factory { chain } => Some(*chain == Chain::Private),
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
/// resolve differently under §5.5, §5.9 and §5.10.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum CacheRule {
    Public,
    Lexical,
    LexicalPrivate,
    Super,
}

pub(crate) const POLYMORPHIC_WIDTH: usize = 4;
pub(crate) const GLOBAL_CACHE_ENTRIES: usize = 1024;

pub(crate) struct InlineCaches {
    /// Every call site of every family that has run, `VACANT` until filled.
    sites: Vec<CacheEntry>,
    /// Polymorphic blocks, four entries each in the order first seen --
    /// V8's width, Strongtalk's shape.
    pool: Vec<[CacheEntry; POLYMORPHIC_WIDTH]>,
    /// Blocks a megamorphic transition or an invalidation gave back.
    free_blocks: Vec<u32>,
    global: GlobalCache,
    /// Code identity -> the start of its family's range.
    families: IdentityMap<usize, FeedbackBase>,
    /// Every body in `families`, held so that its address -- the key -- is
    /// never reused by another body while the key is in the map.
    family_codes: Vec<Rc<CompiledCode>>,
}

impl InlineCaches {
    pub(crate) fn new() -> InlineCaches {
        InlineCaches {
            sites: Vec::new(),
            pool: Vec::new(),
            free_blocks: Vec::new(),
            global: GlobalCache::with_entries(GLOBAL_CACHE_ENTRIES),
            families: IdentityMap::default(),
            family_codes: Vec::new(),
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
        if let Some(base) = self.families.get(&key) {
            return *base;
        }
        let base = self.fresh_range(code.family_size);
        self.families.insert(key, base);
        self.family_codes.push(code.clone());
        base
    }

    /// A new range of `size` sites at the end of the table. `NOT_ASSIGNED`
    /// if the table cannot grow that far, which leaves every site of the
    /// family uncached.
    pub(crate) fn fresh_range(&mut self, size: u32) -> FeedbackBase {
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
    /// change could affect. `families` and `family_codes` stay -- an
    /// activation already running still names its family's range, so the
    /// next send through a vacated site fills the same index again instead
    /// of minting a new one. Clearing the pool rather than only the sites
    /// is what leaves every polymorphic block vacant too, not merely
    /// unreachable from a site that still names it.
    pub(crate) fn invalidate(&mut self) {
        for entry in &mut self.sites {
            *entry = CacheEntry::vacant();
        }
        self.pool.clear();
        self.free_blocks.clear();
        self.global.clear();
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

    /// Replaces the global cache with a fresh one of `entries`, vacating
    /// whatever it held: a test hook for measuring a program's answer under
    /// a cache small enough that every insert evicts
    /// (`NS_NEXT_GLOBAL_CACHE_ENTRIES`, `set_global_cache_size`), never a
    /// host's input path, so a size that is not a power of two is the
    /// host's mistake -- `GlobalCache::with_entries` panics on it rather
    /// than rounding silently.
    pub(crate) fn set_global_cache_size(&mut self, entries: usize) {
        self.global = GlobalCache::with_entries(entries);
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
/// entries keyed by (dispatch id, selector, rule), whatever the program's
/// size.
pub(crate) struct GlobalCache {
    entries: Vec<GlobalEntry>,
    mask: usize,
}

impl GlobalCache {
    /// Two probes at two hashes of the key, as Primordial Soup's
    /// `LookupCache` probes (`prior-art/primordialsoup/vm/lookup_cache.h`).
    fn probes(&self, guard: DispatchId, selector: Symbol, rule: CacheRule) -> [usize; 2] {
        let hash = crate::hash::IdentityBuildHasher::default().hash_one((guard, selector, rule));
        [hash as usize & self.mask, (hash >> 32) as usize & self.mask]
    }

    fn get(&self, guard: DispatchId, selector: Symbol, rule: CacheRule) -> Option<&Target> {
        self.probes(guard, selector, rule)
            .into_iter()
            .map(|probe| &self.entries[probe])
            .find(|entry| entry.holds(guard, selector, rule))
            .map(|entry| &entry.target)
    }

    /// The first probe if it is vacant or already holds this key, else the
    /// second if vacant, else the first, evicting it. Primordial Soup's
    /// `InsertOrdinary` (`prior-art/primordialsoup/vm/lookup_cache.cc`)
    /// probes the same two places but writes the key into both; writing one
    /// keeps whatever the other held, so two keys whose first probes collide
    /// both stay cached while the second probe of one is free.
    fn insert(&mut self, guard: DispatchId, selector: Symbol, rule: CacheRule, target: Target) {
        let [first, second] = self.probes(guard, selector, rule);
        let slot = if self.entries[first].is_vacant()
            || self.entries[first].holds(guard, selector, rule)
        {
            first
        } else if self.entries[second].is_vacant()
            || self.entries[second].holds(guard, selector, rule)
        {
            second
        } else {
            first
        };
        self.entries[slot] = GlobalEntry {
            guard,
            selector,
            rule,
            target,
        };
    }

    /// Vacates every entry: `InlineCaches::invalidate`'s global half.
    fn clear(&mut self) {
        for entry in &mut self.entries {
            *entry = GlobalEntry::vacant();
        }
    }

    pub(crate) fn with_entries(entries: usize) -> GlobalCache {
        assert!(
            entries.is_power_of_two(),
            "the global cache must be sized to a power of two, not {entries}"
        );
        GlobalCache {
            entries: (0..entries).map(|_| GlobalEntry::vacant()).collect(),
            mask: entries - 1,
        }
    }
}

/// What a cached send carries besides its site: the guard, the rule the
/// global cache files it under, and what `materialize` and `act` need --
/// the receiver (`o` for a lexical send, `self` for `super`), N for a
/// private lexical site, S for `super`.
#[derive(Clone, Copy)]
struct SendKey<'gc> {
    guard: DispatchId,
    rule: CacheRule,
    receiver: Value<'gc>,
    lexical_class: Option<ClassReference<'gc>>,
    superclass: Option<ClassReference<'gc>>,
}

impl SendKey<'_> {
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
    key: SendKey<'gc>,
) -> Option<ClassReference<'gc>> {
    match chain {
        Chain::Public | Chain::Protected | Chain::DoesNotUnderstand => {
            Some(mutator.class_of(&key.receiver))
        }
        Chain::PublicClassSide | Chain::ProtectedClassSide => match key.receiver {
            Value::Object(object) => mutator.metaclass_of(object),
            _ => None,
        },
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
    key: SendKey<'gc>,
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
        Resolved::Slot { kind, field, .. } => Target::Slot {
            field: u32::try_from(*field).ok()?,
            kind: *kind,
            chain,
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
        Resolved::Factory { .. } => Target::Factory { chain },
        Resolved::Unhandled => return None,
    })
}

/// The `Resolved` an entry's target names for this send, with the base a
/// method's activation takes, or `None` when a step up does not exist,
/// which makes the send a miss. Takes the target by value, so that a
/// method's `Rc` moves into the step rather than being counted twice.
#[inline(always)]
fn materialize<'gc>(
    mutator: &mut Mutator<'gc>,
    target: Target,
    key: SendKey<'gc>,
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
        Target::Slot { field, kind, chain } => (
            Resolved::Slot {
                kind,
                field: field as usize,
                chain,
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
        // the receiver, which the guard -- that class object's class-side
        // dispatch id -- already determines. A factory entry is reachable
        // only on the receiver's own metaclass (§3.2), so no chain walk can
        // have reached one from another class object's.
        Target::Factory { chain } => match key.receiver {
            Value::Object(class_object) => (
                Resolved::Factory {
                    level: class_object,
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
        guard: mutator.heap.dispatch_id(&receiver),
        rule: CacheRule::Public,
        receiver,
        lexical_class: None,
        superclass: None,
    };
    cached(mutator, index, key, selector, arguments, |mutator| {
        mutator.resolve_public(receiver, selector)
    })
}

/// A lexical send (§5.7-§5.9), cached. The site's privacy, decided when it
/// was first filled, chooses the guard: N's application for a private site,
/// `o`'s otherwise, so a non-private hit never computes N.
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
        dispatch::kth_enclosing_object(&mutator.heap, &self_value, defining_class.as_ref(), depth);
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
            guard: lexical_class.map_or(DispatchId::NONE, |class| {
                mutator.heap.class_dispatch_ids(class).instances
            }),
            rule: CacheRule::LexicalPrivate,
            receiver: enclosing,
            lexical_class,
            superclass: None,
        }
    } else {
        SendKey {
            guard: mutator.heap.dispatch_id(&enclosing),
            rule: CacheRule::Lexical,
            receiver: enclosing,
            lexical_class: None,
            superclass: None,
        }
    };
    cached(mutator, index, key, selector, arguments, |mutator| {
        let lexical_class = dispatch::kth_lexical_class(&mutator.heap, defining_class, depth);
        mutator.resolve_lexical(enclosing, lexical_class, selector)
    })
}

/// A vacant lexical site's first send: whether §5.9's private rule applies
/// is not known until it is resolved, so it skips the global cache, whose
/// entry for `o` may come from a site whose N declares nothing private
/// (refinement 2 of the plan that built this).
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
    let resolved = mutator.resolve_lexical(enclosing, lexical_class, selector);
    let private = resolved.chain() == Some(Chain::Private);
    let (guard, rule) = if private {
        let guard = lexical_class.map_or(DispatchId::NONE, |class| {
            mutator.heap.class_dispatch_ids(class).instances
        });
        (guard, CacheRule::LexicalPrivate)
    } else {
        (mutator.heap.dispatch_id(&enclosing), CacheRule::Lexical)
    };
    let key = SendKey {
        guard,
        rule,
        receiver: enclosing,
        lexical_class,
        superclass: None,
    };
    fill(mutator, index, key, selector, &resolved);
    mutator.act(resolved, enclosing, None, selector, arguments)
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
    let current_class = mutator.heap.activation_current_class(activation);
    let superclass = current_class.and_then(|class| mutator.heap.class_superclass(class));
    let (Some(index), Some(current_class), Some(superclass)) = (
        site_index(feedback_base, family_size, site),
        current_class,
        superclass,
    ) else {
        return decide_super_send(mutator, activation, selector, arguments);
    };
    let self_value = mutator.heap.activation_receiver(activation);
    let key = SendKey {
        guard: mutator.heap.class_dispatch_ids(current_class).instances,
        rule: CacheRule::Super,
        receiver: self_value,
        lexical_class: None,
        superclass: Some(superclass),
    };
    cached(mutator, index, key, selector, arguments, |mutator| {
        mutator.resolve_super(superclass, selector)
    })
}

/// A monomorphic hit, or everything else out of line.
#[inline(always)]
fn cached<'gc>(
    mutator: &mut Mutator<'gc>,
    index: usize,
    key: SendKey<'gc>,
    selector: Symbol,
    arguments: &[Value<'gc>],
    resolve: impl FnOnce(&mut Mutator<'gc>) -> Resolved<'gc>,
) -> Result<Step<'gc>, Signal<'gc>> {
    if let Some(entry) = mutator.caches.sites.get(index) {
        if entry.guard == key.guard {
            let target = entry.target.clone();
            if let Some((resolved, base)) = materialize(mutator, target, key) {
                return mutator.act_with_base(
                    resolved,
                    base,
                    key.receiver,
                    key.superclass,
                    selector,
                    arguments,
                );
            }
        }
    }
    miss(mutator, index, key, selector, arguments, resolve)
}

/// The site's polymorphic block, then the global cache, then `resolve`,
/// whose answer is recorded in both before it is acted on. Out of line so
/// that a monomorphic hit keeps its registers; not `#[cold]`, since a
/// polymorphic hit comes through here.
#[inline(never)]
fn miss<'gc>(
    mutator: &mut Mutator<'gc>,
    index: usize,
    key: SendKey<'gc>,
    selector: Symbol,
    arguments: &[Value<'gc>],
    resolve: impl FnOnce(&mut Mutator<'gc>) -> Resolved<'gc>,
) -> Result<Step<'gc>, Signal<'gc>> {
    if key.guard != DispatchId::NONE {
        if let Some(target) = mutator.caches.polymorphic_hit(index, key.guard) {
            if let Some((resolved, base)) = materialize(mutator, target, key) {
                return mutator.act_with_base(
                    resolved,
                    base,
                    key.receiver,
                    key.superclass,
                    selector,
                    arguments,
                );
            }
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
                return mutator.act_with_base(
                    resolved,
                    base,
                    key.receiver,
                    key.superclass,
                    selector,
                    arguments,
                );
            }
        }
    }
    let resolved = resolve(mutator);
    fill(mutator, index, key, selector, &resolved);
    mutator.act(resolved, key.receiver, key.superclass, selector, arguments)
}

/// Records `resolved` in the global cache and at the site -- unless the key
/// has no id, or the resolution's privacy is not the site's (only a crafted
/// image makes a site's N change; refinement 1). Before `act`, which may run
/// code.
fn fill<'gc>(
    mutator: &mut Mutator<'gc>,
    index: usize,
    key: SendKey<'gc>,
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
        let mut pending: Vec<&CompiledCode> = vec![code];
        while let Some(body) = pending.pop() {
            for instruction in &body.instructions {
                if let Instruction::MakeClosure { code: block, .. } = instruction {
                    pending.push(block);
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
            .copied()
            .unwrap_or(FeedbackBase::NOT_ASSIGNED);
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
        Target::Factory { chain } => (CachedTargetKind::Factory, *chain),
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

    fn body(family_size: u32) -> Rc<CompiledCode> {
        Rc::new(CompiledCode {
            kind: CodeKind::Method,
            instructions: vec![Instruction::Return { register: 0 }],
            argument_count: 0,
            register_count: 1,
            statement_program_counter: vec![],
            family_size,
            source: Span::new(0, 0),
        })
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

    /// Three keys whose two probes are the same two entries: the second
    /// takes the free probe, the third evicts the first probe's key, and
    /// each key answers only its own target.
    #[test]
    fn a_global_cache_keeps_colliding_keys_apart() {
        let mut global = GlobalCache::with_entries(2);
        let selector = Symbol::NEW;
        let guards: Vec<DispatchId> = (0..1000)
            .map(DispatchId::from_counter)
            .filter(|guard| global.probes(*guard, selector, CacheRule::Public) == [0, 1])
            .take(3)
            .collect();
        assert_eq!(guards.len(), 3, "the hash spreads keys over both orders");
        let chain_of = |global: &GlobalCache, guard: DispatchId, rule: CacheRule| match global
            .get(guard, selector, rule)
        {
            Some(Target::Factory { chain }) => Some(*chain),
            _ => None,
        };
        let chains = [Chain::Public, Chain::Super, Chain::SuperDoesNotUnderstand];
        global.insert(
            guards[0],
            selector,
            CacheRule::Public,
            Target::Factory { chain: chains[0] },
        );
        global.insert(
            guards[1],
            selector,
            CacheRule::Public,
            Target::Factory { chain: chains[1] },
        );
        assert_eq!(
            chain_of(&global, guards[0], CacheRule::Public),
            Some(chains[0])
        );
        assert_eq!(
            chain_of(&global, guards[1], CacheRule::Public),
            Some(chains[1])
        );
        assert_eq!(
            chain_of(&global, guards[0], CacheRule::Super),
            None,
            "the rule is part of the key"
        );
        global.insert(
            guards[2],
            selector,
            CacheRule::Public,
            Target::Factory { chain: chains[2] },
        );
        assert_eq!(
            chain_of(&global, guards[0], CacheRule::Public),
            None,
            "evicted"
        );
        assert_eq!(
            chain_of(&global, guards[1], CacheRule::Public),
            Some(chains[1])
        );
        assert_eq!(
            chain_of(&global, guards[2], CacheRule::Public),
            Some(chains[2])
        );
        global.clear();
        assert!(guards
            .iter()
            .all(|guard| global.get(*guard, selector, CacheRule::Public).is_none()));
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
        };
        let guard = DispatchId::from_counter;
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
    }
}
