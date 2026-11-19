//! The scavenger (`docs/design/memory.md`, "Generations"): a Cheney copy
//! of the nursery's survivors, as Spur's `doScavenge:`
//! (`prior-art/opensmalltalk-vm/src/spur64.cog/cointerp.c:48477`) and
//! Strongtalk's `Universe::scavenge`
//! (`prior-art/strongtalk/vm/memory/universe_more.cpp:81-137`) are. A
//! survivor goes to the future survivor space, or to the old space once
//! its age reaches the threshold, the survivor space is full, or the
//! scavenge tenures everything. Its old place becomes a forwarder: the
//! forwarded format, its class word naming the copy.
//!
//! Only strong words keep a young object alive
//! (`object::strong_pointer_ranges`): the words of a weak object and the
//! fields of an ephemeron whose key has not survived wait for `finish`,
//! which forwards or clears them once the copying is done
//! (`docs/design/core-library.md`, "The heap and the collectors").
//!
//! A child of `heap`, so it may name a handle's private offset. It holds
//! the space and nothing else of the heap, so that the roots it traces --
//! the activation stack, the host roots, the step state -- can be borrowed
//! beside it.

use std::num::NonZeroU64;

use crate::heap::header::{Format, Header, MAX_AGE};
use crate::heap::object::{self, CLASS, FIELDS, pointer_ranges_of, strong_pointer_ranges};
use crate::heap::pending::PendingEphemerons;
use crate::heap::space::{RegionName, Space, allocation_range, block_range, is_young, object_at};
use crate::heap::{
    FrameReference, ObjectReference, ResolvedFutures, Word, header_index, offset_of_header,
};
use crate::trace::Tracer;

pub(crate) struct Scavenger<'space> {
    pub(in crate::heap) space: &'space mut Space,
    tenure_all: bool,
    threshold: u8,
    /// The test for a resolved future, whose value decides for it in a
    /// weak word or an ephemeron's key (`ResolvedFutures`); `None` when
    /// the program has made no future.
    futures: Option<ResolvedFutures>,
    /// The words copied into the survivor space, by the age they arrive
    /// with: Strongtalk's age table (`ageTable.cpp`).
    pub(in crate::heap) ages: [usize; MAX_AGE as usize + 1],
    /// Where the Cheney scan stands in the survivor space and in the old
    /// space; everything below is scanned, everything from here to the
    /// region's top was copied and not yet scanned.
    survivor_scan: usize,
    promoted_scan: usize,
    /// Promoted objects that still name a young one: the scavenge's
    /// additions to the remembered set.
    pub(in crate::heap) remembered: Vec<NonZeroU64>,
    /// Every copied or remembered `WeakPointers` object, its words settled
    /// by `finish`; and every `Ephemeron` whose key had not survived when
    /// it was scanned. Neither list names an object twice, because only a
    /// scan lists one and no object is scanned twice but a listed
    /// ephemeron, rescanned once its key survived and so not listed again:
    /// the remembered set holds each old object once, the Cheney loop
    /// scans each copy once, and the remembered objects lie below where
    /// its scan of the old space starts.
    weak_objects: Vec<NonZeroU64>,
    ephemerons: Vec<NonZeroU64>,
    /// Whether `copy` lists what it copies on `copied`: only during
    /// `finish`'s fixpoint, while its index of the pending ephemerons, by
    /// the young objects that would resolve them, holds an unresolved one.
    recording: bool,
    /// Every object `finish`'s fixpoint has copied and not yet looked up,
    /// by the offset it was copied from -- where a pending key word, or
    /// the forwarders it starts, ends.
    copied: Vec<NonZeroU64>,
    /// How many objects were copied: the nursery's new count, and how many
    /// of them went old.
    pub(in crate::heap) survived: usize,
    pub(in crate::heap) promoted: usize,
    #[cfg(debug_assertions)]
    heap_id: u32,
    #[cfg(debug_assertions)]
    from_epoch: u32,
    #[cfg(debug_assertions)]
    to_epoch: u32,
}

impl<'space> Scavenger<'space> {
    pub(in crate::heap) fn new(
        space: &'space mut Space,
        tenure_all: bool,
        threshold: u8,
        futures: Option<ResolvedFutures>,
        #[cfg(debug_assertions)] heap_id: u32,
        #[cfg(debug_assertions)] from_epoch: u32,
        #[cfg(debug_assertions)] to_epoch: u32,
    ) -> Scavenger<'space> {
        let survivor_scan = space.future_survivor().top;
        let promoted_scan = space.region(RegionName::Old).top;
        Scavenger {
            space,
            tenure_all,
            threshold,
            futures,
            ages: [0; MAX_AGE as usize + 1],
            survivor_scan,
            promoted_scan,
            remembered: Vec::new(),
            weak_objects: Vec::new(),
            ephemerons: Vec::new(),
            recording: false,
            copied: Vec::new(),
            survived: 0,
            promoted: 0,
            #[cfg(debug_assertions)]
            heap_id,
            #[cfg(debug_assertions)]
            from_epoch,
            #[cfg(debug_assertions)]
            to_epoch,
        }
    }

    /// Where the object `offset` names lives once the scavenge is done:
    /// followed along every forwarder, whatever made each link (the chains
    /// are the scavenger's own: a bulk forward leaves none behind), and copied
    /// if it is young and not already in the survivor space being filled,
    /// as Spur's `scavengeReferentsOf:` decides (`cointerp.c:43316-43322`).
    pub(in crate::heap) fn evacuate(&mut self, offset: NonZeroU64) -> NonZeroU64 {
        let offset = self.follow(offset);
        if self.stays(offset) {
            return offset;
        }
        self.copy(offset)
    }

    /// The end of the chain of forwarders that starts at `offset`. A young
    /// object is copied once per scavenge, so a chain is one link long; the
    /// loop costs nothing for that and never meets a bulk forward's
    /// forwarder, which no handle names.
    fn follow(&self, offset: NonZeroU64) -> NonZeroU64 {
        let words = self.space.words();
        let mut offset = offset;
        loop {
            let header = header_index(offset);
            if Header(words[header]).format() != Format::Forwarded {
                return offset;
            }
            offset = match Word::from_bits(words[header + CLASS]).object_offset() {
                Some(target) => target,
                None => unreachable!("internal: a forwarder names an object"),
            };
        }
    }

    /// Whether the object at `offset`, the end of its forwarders, stays
    /// where it is: old, or already copied into the survivor space being
    /// filled.
    fn stays(&self, offset: NonZeroU64) -> bool {
        let future = self.space.future_survivor();
        !is_young(offset)
            || (future.base..future.end).contains(&usize::try_from(offset.get()).expect(
                "internal: an offset lies inside the object space's reservation, DEFAULT_RESERVATION",
            ))
    }

    /// Where the object `offset` names lives once the scavenge is done, if
    /// a strong word has reached it so far; `None` for a young object
    /// nothing copied.
    fn survivor(&self, offset: NonZeroU64) -> Option<NonZeroU64> {
        let offset = self.follow(offset);
        self.stays(offset).then_some(offset)
    }

    /// Copies the young object at `offset` into the survivor space or the
    /// old space and leaves a forwarder in its place; answers the copy.
    fn copy(&mut self, offset: NonZeroU64) -> NonZeroU64 {
        let words = self.space.words();
        let header = header_index(offset);
        let source = allocation_range(words, header);
        let header_within = header - source.start;
        let age = Header(words[header]).age();
        let length = source.len();
        let survivor = if self.tenure_all || age >= self.threshold {
            None
        } else {
            self.space.allocate_survivor(length)
        };
        let (start, promoted) = match survivor {
            Some(start) => (start, false),
            // The old space was committed for the worst case before the
            // scavenge began, so this never fails short of the reservation.
            None => match self.space.allocate_old(length) {
                Some(start) => (start, true),
                None => unreachable!(
                    "internal: a scavenge promotes into memory committed before it began"
                ),
            },
        };
        let destination = start / 8;
        let words = self.space.words_mut();
        words.copy_within(source, destination);
        let moved = destination + header_within;
        let copied = Header(words[moved]);
        // A ByteArray's data sit at its own address's alignment; left at the
        // old block's offset, every read would answer wrong bytes.
        if copied.format() == Format::AlignedBytes {
            object::realign(words, header, moved);
        }
        words[moved] = if promoted {
            copied.with_age(0)
        } else {
            let new_age = (age + 1).min(MAX_AGE);
            self.ages[usize::from(new_age)] += length;
            copied.with_age(new_age)
        }
        .0;
        let moved_offset = match NonZeroU64::new(8 * moved as u64) {
            Some(moved_offset) => moved_offset,
            None => unreachable!("internal: a copy lies past the first object"),
        };
        words[header] = Header(words[header]).with_format(Format::Forwarded).0;
        words[header + CLASS] = Word::with_object_offset(moved_offset).bits();
        self.survived += 1;
        self.promoted += usize::from(promoted);
        if self.recording {
            #[cfg(test)]
            crate::heap::pending::work::count_recorded();
            self.copied.push(offset);
        }
        moved_offset
    }

    /// Rewrites every strong pointer word of the object at word `header`;
    /// answers whether one names a young object afterwards. A weak object
    /// is listed for `finish`, and so is an ephemeron whose key -- a young
    /// object nothing has copied yet -- has not survived so far. An
    /// ephemeron whose key survived only as a resolved future's value
    /// (`key_survives`) is keyed by that value from here on.
    pub(in crate::heap) fn scan_object(&mut self, header: usize) -> bool {
        let words = self.space.words();
        let block = block_range(words, header);
        let format = Header(words[header]).format();
        let ranges = match format {
            Format::WeakPointers | Format::Ephemeron => {
                let mut key_died = false;
                let ranges = strong_pointer_ranges(format, &words[block.clone()], |key| {
                    let survives = self.key_survives(key);
                    key_died = !survives;
                    survives
                });
                if format == Format::WeakPointers {
                    self.weak_objects.push(offset_of_header(header));
                } else if key_died {
                    self.ephemerons.push(offset_of_header(header));
                } else if let Some(key) = self.field_range(header).next() {
                    self.see_through_resolved_future(key);
                }
                ranges
            }
            _ => pointer_ranges_of(format, &words[block.clone()]),
        };
        let mut names_young = false;
        for range in ranges {
            for position in range.start + block.start..range.end + block.start {
                let word = Word::from_bits(self.space.words()[position]);
                if let Some(target) = word.object_offset().filter(|target| is_young(*target)) {
                    let moved = self.evacuate(target);
                    self.space.words_mut()[position] = Word::with_object_offset(moved).bits();
                    names_young |= is_young(moved);
                }
            }
        }
        names_young
    }

    /// Whether an object was copied and not yet scanned: what `drain`
    /// would scan.
    fn has_unscanned(&self) -> bool {
        self.survivor_scan < self.space.future_survivor().top
            || self.promoted_scan < self.space.region(RegionName::Old).top
    }

    /// The Cheney loop: scans what was copied until nothing new was.
    pub(in crate::heap) fn drain(&mut self) {
        loop {
            let mut progressed = false;
            while self.survivor_scan < self.space.future_survivor().top {
                let (header, next) = object_at(self.space.words(), self.survivor_scan / 8);
                self.scan_object(header);
                self.survivor_scan = 8 * next;
                progressed = true;
            }
            while self.promoted_scan < self.space.region(RegionName::Old).top {
                let (header, next) = object_at(self.space.words(), self.promoted_scan / 8);
                if self.scan_object(header) {
                    self.remember(header);
                }
                self.promoted_scan = 8 * next;
                progressed = true;
            }
            if !progressed {
                return;
            }
        }
    }

    /// Ends a scavenge whose Cheney loop `drain` has run dry, as
    /// `Heap::finish_marking` ends a marking: scans each listed ephemeron
    /// whose key has since survived, drains, and checks again, until
    /// nothing copied waits to be scanned; mourns every ephemeron left
    /// unresolved -- its key reached by nothing but ephemerons -- by
    /// setting its fields to nil; and rewrites every word of a listed weak
    /// object that names a young object to the copy, or to nil where
    /// nothing copied it -- a resolved future nothing copied first to its
    /// value (`see_through_resolved_future`).
    ///
    /// The first `pending::scanned_checks` checks scan every unresolved
    /// ephemeron; the ones still unresolved after them are indexed by the
    /// young objects whose copy would resolve them (`pending_keys`), and
    /// from then on copies are recorded while the index holds one, and
    /// each check looks them up and scans the ephemerons they resolve.
    /// Each indexed ephemeron is scanned at most once more and each object
    /// the fixpoint copies looked up once, so a chain of ephemerons each
    /// keyed by the previous one's value costs its length, not its square,
    /// and a fixpoint that ends within the scans -- a weak map's --
    /// indexes nothing.
    ///
    /// Must run before the nursery is vacated: a weak word left naming it
    /// would name whatever is allocated there next. An old object either
    /// pass leaves naming a young one is remembered, so that the next
    /// scavenge settles its words again.
    ///
    /// Mourning and clearing write nil, which names no young object, so
    /// they are the collector's own rewrites and need no write barrier.
    pub(in crate::heap) fn finish(&mut self) {
        if !self.ephemerons.is_empty() {
            let scanned_checks = crate::heap::pending::scanned_checks();
            let mut checks = 0;
            // The unresolved ephemerons while the checks scan them, and
            // the index of them afterwards.
            let mut waiting = Vec::new();
            let mut pending = PendingEphemerons::new();
            // Spare buffers the scavenger's lists are swapped with each
            // round, so a long chain allocates nothing per round.
            let mut listed = Vec::new();
            let mut copied = Vec::new();
            loop {
                std::mem::swap(&mut self.ephemerons, &mut listed);
                if checks < scanned_checks {
                    waiting.append(&mut listed);
                    let mut position = 0;
                    while position < waiting.len() {
                        #[cfg(test)]
                        crate::heap::pending::work::count_examined();
                        let ephemeron = waiting[position];
                        if self.ephemeron_key_survives(ephemeron) {
                            self.resolve_ephemeron(ephemeron);
                            waiting.swap_remove(position);
                        } else {
                            position += 1;
                        }
                    }
                    checks += 1;
                } else {
                    // Ephemerons the last drain scanned before their key
                    // survived join the index; one whose key that drain
                    // copied meanwhile was copied before it was indexed,
                    // so it is scanned here.
                    for ephemeron in listed.drain(..) {
                        #[cfg(test)]
                        crate::heap::pending::work::count_examined();
                        if self.ephemeron_key_survives(ephemeron) {
                            self.resolve_ephemeron(ephemeron);
                        } else {
                            let keys = self.pending_keys(ephemeron);
                            pending.add(ephemeron, keys.into_iter().flatten());
                            // From here on a copy may resolve it -- even one
                            // this check makes resolving a later ephemeron.
                            self.recording = true;
                        }
                    }
                    std::mem::swap(&mut self.copied, &mut copied);
                    for offset in copied.drain(..) {
                        if pending.is_empty() {
                            break;
                        }
                        pending.take(offset, |ephemeron| self.resolve_ephemeron(ephemeron));
                    }
                }
                // Every copy waits to be scanned, so nothing waiting means
                // nothing new survived since the last drain.
                if !self.has_unscanned() {
                    // Only a drain lists ephemerons.
                    debug_assert!(self.ephemerons.is_empty());
                    break;
                }
                // The scans are over and the fixpoint goes on: what they
                // left unresolved is indexed before the drain that may
                // resolve it records what it copies. A scan copies as it
                // resolves, so a key may have survived after its
                // ephemeron was passed over: each is checked again, and
                // from the first one indexed on, what a resolution copies
                // is recorded.
                if checks == scanned_checks {
                    for ephemeron in std::mem::take(&mut waiting) {
                        #[cfg(test)]
                        crate::heap::pending::work::count_examined();
                        if self.ephemeron_key_survives(ephemeron) {
                            self.resolve_ephemeron(ephemeron);
                        } else {
                            let keys = self.pending_keys(ephemeron);
                            pending.add(ephemeron, keys.into_iter().flatten());
                            self.recording = true;
                        }
                    }
                }
                // Only the index needs what a drain copies, and an empty
                // one can resolve nothing with it: an ephemeron the drain
                // lists meanwhile meets the check above next round.
                self.recording = !pending.is_empty();
                if pending.is_empty() {
                    self.copied.clear();
                }
                self.drain();
            }
            self.recording = false;
            for ephemeron in waiting.into_iter().chain(pending.unresolved()) {
                let fields = self.field_range(header_index(ephemeron));
                self.space.words_mut()[fields].fill(Word::NIL.bits());
            }
        }
        for weak_object in std::mem::take(&mut self.weak_objects) {
            let header = header_index(weak_object);
            let mut names_young = false;
            for position in self.field_range(header) {
                self.see_through_resolved_future(position);
                let word = Word::from_bits(self.space.words()[position]);
                if let Some(target) = word.object_offset().filter(|target| is_young(*target)) {
                    let settled = match self.survivor(target) {
                        Some(moved) => {
                            names_young |= is_young(moved);
                            Word::with_object_offset(moved)
                        }
                        None => Word::NIL,
                    };
                    self.space.words_mut()[position] = settled.bits();
                }
            }
            if names_young && !is_young(weak_object) {
                self.remember(header);
            }
        }
    }

    /// Scans the listed ephemeron at `offset`, whose key has survived: its
    /// guarded fields are copied, and an old one that names a young object
    /// afterwards is remembered.
    fn resolve_ephemeron(&mut self, offset: NonZeroU64) {
        debug_assert!(
            self.ephemeron_key_survives(offset),
            "internal: an ephemeron is resolved only once its key survives"
        );
        let header = header_index(offset);
        if self.scan_object(header) && !is_young(offset) {
            self.remember(header);
        }
    }

    /// The young objects whose copy resolves the listed ephemeron at
    /// `offset`, whose key has not survived, as `key_survives` judges it:
    /// the key, at the end of its forwarders, where `copy` records it, and
    /// when the key is a resolved future, its value too.
    fn pending_keys(&self, offset: NonZeroU64) -> [Option<NonZeroU64>; 2] {
        let Some(key) = self.space.words()[self.field_range(header_index(offset))]
            .first()
            .and_then(|key| Word::from_bits(*key).object_offset())
        else {
            return [None, None];
        };
        let value = self
            .resolved_future_value(key)
            .and_then(|value| value.object_offset())
            .map(|value| self.follow(value));
        [Some(self.follow(key)), value]
    }

    /// Whether the key of the listed ephemeron at `offset` has survived. A
    /// listed ephemeron's key is an object: one with no field or another
    /// key is scanned whole and never listed.
    fn ephemeron_key_survives(&self, offset: NonZeroU64) -> bool {
        self.space.words()[self.field_range(header_index(offset))]
            .first()
            .is_some_and(|key| self.key_survives(*key))
    }

    /// Whether an ephemeron whose key word is `key` keeps its value so far:
    /// an immediate key always, an object once a strong word has reached
    /// it, and a resolved future nothing has reached once its value
    /// survives -- the future stands for its value, which `==` sees, as a
    /// full collection judges it once `replace_resolved_futures` has
    /// rewritten the key. An unresolved future is judged as itself.
    fn key_survives(&self, key: u64) -> bool {
        let Some(target) = Word::from_bits(key).object_offset() else {
            return true;
        };
        if self.survivor(target).is_some() {
            return true;
        }
        self.resolved_future_value(target).is_some_and(|value| {
            value
                .object_offset()
                .is_none_or(|value| self.survivor(value).is_some())
        })
    }

    /// The value word of the object at `offset` when it is a resolved
    /// future (`ResolvedFutures`), read where the object lives now.
    fn resolved_future_value(&self, offset: NonZeroU64) -> Option<Word> {
        self.futures?
            .value_of(self.space.words(), offset, |found| self.follow(found))
    }

    /// Rewrites the word at `position`, a weak word or a surviving
    /// ephemeron's key, to the value of the resolved future it names when
    /// nothing has copied that future, as `replace_resolved_futures` would
    /// have rewritten it before a full collection: otherwise a scavenge
    /// would forget, or keep only through a copy of the proxy, an entry a
    /// collection keeps for its value. The caller then settles the word as
    /// any other; the value's word names the young object as it was, which
    /// the caller follows to its copy.
    fn see_through_resolved_future(&mut self, position: usize) {
        let Some(target) = Word::from_bits(self.space.words()[position]).object_offset() else {
            return;
        };
        if self.survivor(target).is_some() {
            return;
        }
        if let Some(value) = self.resolved_future_value(target) {
            self.space.words_mut()[position] = value.bits();
        }
    }

    /// The words of the fields of the object at word `header`.
    fn field_range(&self, header: usize) -> std::ops::Range<usize> {
        let block = block_range(self.space.words(), header);
        (block.start + FIELDS).min(block.end)..block.end
    }

    /// Adds the old object at word `header` to the scavenge's remembered
    /// set unless its bit says it is in already, so that no object is in
    /// the set twice: an old weak object or ephemeron whose strong words
    /// name a young object was remembered when they were scanned.
    fn remember(&mut self, header: usize) {
        let words = self.space.words_mut();
        let found = Header(words[header]);
        if !found.remembered() {
            words[header] = found.with_remembered(true).0;
            self.remembered.push(offset_of_header(header));
        }
    }

    /// Refuses a handle from another heap or another epoch, and answers the
    /// epoch the scavenge brands every handle it keeps with.
    #[cfg(debug_assertions)]
    pub(in crate::heap) fn check_and_rebrand(&self, origin: u32, epoch: u32) -> u32 {
        assert_eq!(
            origin, self.heap_id,
            "a handle from another heap is stored in this one"
        );
        assert_eq!(
            epoch, self.from_epoch,
            "a stale handle is stored in the heap: minted in move epoch {}, found during move {}",
            epoch, self.to_epoch
        );
        self.to_epoch
    }
}

impl Tracer for Scavenger<'_> {
    fn object(&mut self, handle: &mut ObjectReference<'_>) {
        #[cfg(debug_assertions)]
        {
            handle.epoch = self.check_and_rebrand(handle.origin, handle.epoch);
        }
        if is_young(handle.offset) {
            handle.offset = self.evacuate(handle.offset);
        }
    }

    fn frame(&mut self, handle: &mut FrameReference<'_>) {
        #[cfg(debug_assertions)]
        {
            handle.epoch = self.check_and_rebrand(handle.origin, handle.epoch);
        }
        if is_young(handle.offset) {
            handle.offset = self.evacuate(handle.offset);
        }
    }

    fn word(&mut self, word: &mut Word) {
        if let Some(target) = word.object_offset().filter(|target| is_young(*target)) {
            *word = Word::with_object_offset(self.evacuate(target));
        }
    }
}

/// The age at which the next scavenge tenures: the lowest at which the
/// survivors of that age and younger exceed `desired_words`, so that those
/// younger than it fit; never below 1 and never above `MAX_AGE`. HotSpot's
/// `ageTable::compute_tenuring_threshold` with `TargetSurvivorRatio=50` and
/// `MaxTenuringThreshold=15`, after Strongtalk's `ageTable::tenuring_threshold`
/// (`prior-art/strongtalk/vm/memory/ageTable.cpp:37-46`), which answers one
/// age less and so tenures the age that would just have fitted as well.
pub(in crate::heap) fn tenuring_threshold(
    ages: &[usize; MAX_AGE as usize + 1],
    desired_words: usize,
) -> u8 {
    let mut total = 0;
    for age in 1..=MAX_AGE {
        total += ages[usize::from(age)];
        if total > desired_words {
            return age;
        }
    }
    MAX_AGE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_threshold_is_the_age_past_which_the_survivors_overflow_half_the_space() {
        let mut ages = [0; MAX_AGE as usize + 1];
        ages[1] = 300;
        ages[2] = 300;
        ages[3] = 300;
        assert_eq!(
            tenuring_threshold(&ages, 500),
            2,
            "ages 1 and 2 already exceed 500 words"
        );
        assert_eq!(
            tenuring_threshold(&ages, 1_000),
            MAX_AGE,
            "everything fits: tenure nothing by age"
        );
        ages[1] = 600;
        assert_eq!(tenuring_threshold(&ages, 500), 1, "never below 1");
    }
}
