//! The ephemerons a collector's fixpoint still waits on, indexed by the
//! object whose survival decides each (`docs/design/core-library.md`, "The
//! heap and the collectors"). While the index holds an ephemeron, a
//! collector records every object its fixpoint newly reaches and looks
//! each up here once, so an ephemeron is resolved
//! when its key is reached rather than found by rescanning every pending
//! one per round, which costs the square of a chain's length.
//!
//! The index costs a hash insertion per ephemeron, which a fixpoint of
//! one or two rounds -- a weak map's, most of its ephemerons listed only
//! because the main tracing reached them before their keys, the rest
//! keyed by dead objects -- never earns back. So the first
//! `scanned_checks` checks scan the unresolved ephemerons instead, and
//! only those still unresolved after them are indexed: a fixpoint that
//! ends within them indexes nothing, and a chain costs at most that many
//! scans beyond its length -- in a scavenge one check more, since the
//! switch checks each ephemeron again.
//!
//! A child of `heap`; it holds offsets only, as the collector sees them,
//! and lives no longer than one collection.

use crate::hash::IdentityMap;
use std::num::NonZeroU64;

/// How many of a fixpoint's checks scan the unresolved ephemerons before
/// the ones still unresolved are indexed.
pub(in crate::heap) const SCANNED_CHECKS: usize = 2;

/// `SCANNED_CHECKS`, which a test may lower to reach the index with a
/// short fixpoint (`work::set_scanned_checks`).
pub(in crate::heap) fn scanned_checks() -> usize {
    #[cfg(test)]
    {
        work::SCANNED_CHECKS_OVERRIDE.with(std::cell::Cell::get)
    }
    #[cfg(not(test))]
    {
        SCANNED_CHECKS
    }
}

/// The pending ephemerons, each reachable from every object whose survival
/// resolves it: one for a key, two for a resolved future's key in a
/// scavenge (the proxy and its value). Each is resolved at most once.
pub(in crate::heap) struct PendingEphemerons {
    /// The ephemerons, in the order they were added.
    ephemerons: Vec<NonZeroU64>,
    /// Whether each ephemeron has been resolved, by its position above:
    /// one indexed under two objects must not be resolved twice.
    resolved: Vec<bool>,
    /// The first link of each indexed object's list.
    first_link: IdentityMap<u64, usize>,
    /// The links: an ephemeron's position, and the next link of the same
    /// object's list. Several ephemerons may share a key.
    links: Vec<(usize, Option<usize>)>,
    /// How many ephemerons are not yet resolved: none, and no object the
    /// fixpoint reaches can resolve anything, so it need not be recorded.
    unresolved_count: usize,
}

impl PendingEphemerons {
    pub(in crate::heap) fn new() -> PendingEphemerons {
        PendingEphemerons {
            ephemerons: Vec::new(),
            resolved: Vec::new(),
            first_link: IdentityMap::default(),
            links: Vec::new(),
            unresolved_count: 0,
        }
    }

    /// Whether every ephemeron added has been resolved, so that looking
    /// an object up would find nothing.
    pub(in crate::heap) fn is_empty(&self) -> bool {
        self.unresolved_count == 0
    }

    /// Indexes the ephemeron at `ephemeron` under each object of `keys`,
    /// whose survival resolves it.
    pub(in crate::heap) fn add(
        &mut self,
        ephemeron: NonZeroU64,
        keys: impl IntoIterator<Item = NonZeroU64>,
    ) {
        let position = self.ephemerons.len();
        self.ephemerons.push(ephemeron);
        self.resolved.push(false);
        self.unresolved_count += 1;
        for key in keys {
            let link = self.links.len();
            let next = self.first_link.insert(key.get(), link);
            self.links.push((position, next));
        }
    }

    /// Hands every unresolved ephemeron indexed under `key` to `resolve`,
    /// once, and forgets the key: the object has survived, and is reached
    /// only once.
    pub(in crate::heap) fn take(&mut self, key: NonZeroU64, mut resolve: impl FnMut(NonZeroU64)) {
        #[cfg(test)]
        work::count_lookup();
        let mut link = self.first_link.remove(&key.get());
        while let Some(current) = link {
            let (position, next) = self.links[current];
            if !self.resolved[position] {
                self.resolved[position] = true;
                self.unresolved_count -= 1;
                #[cfg(test)]
                work::count_resolution();
                resolve(self.ephemerons[position]);
            }
            link = next;
        }
    }

    /// Every ephemeron never resolved: its key reached by nothing but
    /// ephemerons.
    pub(in crate::heap) fn unresolved(&self) -> impl Iterator<Item = NonZeroU64> + '_ {
        self.ephemerons
            .iter()
            .zip(&self.resolved)
            .filter(|(_, resolved)| !**resolved)
            .map(|(ephemeron, _)| *ephemeron)
    }
}

/// How much work the fixpoints did on this thread, for the tests that hold
/// it linear in what the fixpoint reaches.
#[cfg(test)]
pub(in crate::heap) mod work {
    use std::cell::Cell;

    thread_local! {
        static LOOKUPS: Cell<usize> = const { Cell::new(0) };
        static RESOLUTIONS: Cell<usize> = const { Cell::new(0) };
        static EXAMINED: Cell<usize> = const { Cell::new(0) };
        static RECORDED: Cell<usize> = const { Cell::new(0) };
        pub(in crate::heap) static SCANNED_CHECKS_OVERRIDE: Cell<usize> =
            const { Cell::new(super::SCANNED_CHECKS) };
    }

    /// Has the fixpoints on this thread scan `checks` checks before they
    /// index: 0 indexes from the first check on, so a test reaches the
    /// index without a fixpoint of three rounds.
    pub(in crate::heap) fn set_scanned_checks(checks: usize) {
        SCANNED_CHECKS_OVERRIDE.with(|scanned_checks| scanned_checks.set(checks));
    }

    /// The work counted since the last `take`.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(in crate::heap) struct Work {
        /// Objects looked up in the index.
        pub(in crate::heap) lookups: usize,
        /// Ephemerons the index resolved.
        pub(in crate::heap) resolutions: usize,
        /// Unresolved ephemerons a check examined, resolving, keeping or
        /// indexing each, and in a scavenge each the switch to the index
        /// checked again; indexing at the marking's switch reads no key
        /// and is not counted.
        pub(in crate::heap) examined: usize,
        /// Objects a drain recorded for lookup.
        pub(in crate::heap) recorded: usize,
    }

    pub(in crate::heap) fn count_lookup() {
        LOOKUPS.with(|lookups| lookups.set(lookups.get() + 1));
    }

    pub(in crate::heap) fn count_resolution() {
        RESOLUTIONS.with(|resolutions| resolutions.set(resolutions.get() + 1));
    }

    pub(in crate::heap) fn count_examined() {
        EXAMINED.with(|examined| examined.set(examined.get() + 1));
    }

    pub(in crate::heap) fn count_recorded() {
        RECORDED.with(|recorded| recorded.set(recorded.get() + 1));
    }

    /// The work counted since the last call, which starts the count
    /// afresh.
    pub(in crate::heap) fn take() -> Work {
        Work {
            lookups: LOOKUPS.with(|lookups| lookups.replace(0)),
            resolutions: RESOLUTIONS.with(|resolutions| resolutions.replace(0)),
            examined: EXAMINED.with(|examined| examined.replace(0)),
            recorded: RECORDED.with(|recorded| recorded.replace(0)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offset(value: u64) -> NonZeroU64 {
        NonZeroU64::new(value).expect("a test offset is not zero")
    }

    #[test]
    fn an_ephemeron_under_two_keys_is_resolved_once_and_the_rest_stay_unresolved() {
        let mut pending = PendingEphemerons::new();
        pending.add(offset(8), [offset(100), offset(200)]);
        pending.add(offset(16), [offset(100)]);
        pending.add(offset(24), [offset(300)]);
        assert!(!pending.is_empty());
        let mut resolved = Vec::new();
        pending.take(offset(200), |ephemeron| resolved.push(ephemeron));
        pending.take(offset(100), |ephemeron| resolved.push(ephemeron));
        pending.take(offset(100), |ephemeron| resolved.push(ephemeron));
        assert_eq!(resolved, [offset(8), offset(16)]);
        assert_eq!(pending.unresolved().collect::<Vec<_>>(), [offset(24)]);
        assert!(!pending.is_empty());
        pending.take(offset(300), |ephemeron| resolved.push(ephemeron));
        assert!(pending.is_empty(), "every ephemeron is resolved");
    }
}
